//! SQLite 持久化层。
//!
//! 原始样本与全部分析方案都入库；分析结果同时保存“溯源快照” JSON，
//! 即使后续方案被修改，历史峰参数依赖的原始点仍可追溯。

use crate::model::{Metadata, RunData, Segment, SegmentKind};
use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::sync::Mutex;

pub struct Db {
    pub conn: Mutex<Connection>,
}

pub const SCHEMA: &str = r#"
PRAGMA journal_mode=WAL;

CREATE TABLE IF NOT EXISTS datasets (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    file_name TEXT NOT NULL,
    scan_rate_v_s REAL NOT NULL,
    scan_rate_unit TEXT NOT NULL,
    potential_unit TEXT NOT NULL,
    current_unit TEXT NOT NULL,
    time_unit TEXT NOT NULL,
    area_cm2 REAL NOT NULL,
    area_unit TEXT NOT NULL,
    reference_electrode TEXT NOT NULL,
    tolerance_v REAL NOT NULL,
    imported_at_ms INTEGER NOT NULL,
    content_sha256 TEXT NOT NULL,
    raw_csv TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS samples (
    dataset_id INTEGER NOT NULL REFERENCES datasets(id) ON DELETE CASCADE,
    sample_pos INTEGER NOT NULL,
    instrument_index INTEGER NOT NULL,
    potential_v REAL NOT NULL,
    current_a REAL NOT NULL,
    time_s REAL NOT NULL,
    label INTEGER NOT NULL,
    PRIMARY KEY (dataset_id, sample_pos)
);

CREATE TABLE IF NOT EXISTS segments (
    dataset_id INTEGER NOT NULL REFERENCES datasets(id) ON DELETE CASCADE,
    seg_order INTEGER NOT NULL,
    kind INTEGER NOT NULL,
    cycle INTEGER NOT NULL,
    start_pos INTEGER NOT NULL,
    end_pos INTEGER NOT NULL,
    PRIMARY KEY (dataset_id, seg_order)
);

-- 分析方案（基线/区间方案归属于“数据集 + 扫描段”）
CREATE TABLE IF NOT EXISTS proposals (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    dataset_id INTEGER NOT NULL REFERENCES datasets(id) ON DELETE CASCADE,
    seg_order INTEGER NOT NULL,
    name TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    UNIQUE(dataset_id, seg_order, name)
);

CREATE TABLE IF NOT EXISTS baselines (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    proposal_id INTEGER NOT NULL REFERENCES proposals(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    params_json TEXT NOT NULL,
    values_json TEXT NOT NULL,
    anchor_positions_json TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    UNIQUE(proposal_id)
);

CREATE TABLE IF NOT EXISTS intervals (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    proposal_id INTEGER NOT NULL REFERENCES proposals(id) ON DELETE CASCADE,
    peak_no INTEGER NOT NULL,
    start_pos INTEGER NOT NULL,
    end_pos INTEGER NOT NULL,
    UNIQUE(proposal_id, peak_no)
);

CREATE TABLE IF NOT EXISTS peaks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    proposal_id INTEGER NOT NULL REFERENCES proposals(id) ON DELETE CASCADE,
    peak_no INTEGER NOT NULL,
    apex_pos INTEGER NOT NULL,
    peak_potential_v REAL NOT NULL,
    peak_current_a REAL NOT NULL,
    baseline_current_at_apex_a REAL NOT NULL,
    peak_height_a REAL NOT NULL,
    charge_c REAL NOT NULL,
    provenance_json TEXT NOT NULL,
    UNIQUE(proposal_id, peak_no)
);

-- 峰对方案（作用于同一数据集的一对正反扫 proposal）
CREATE TABLE IF NOT EXISTS pair_proposals (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    dataset_id INTEGER NOT NULL REFERENCES datasets(id) ON DELETE CASCADE,
    forward_proposal_id INTEGER NOT NULL REFERENCES proposals(id) ON DELETE CASCADE,
    reverse_proposal_id INTEGER NOT NULL REFERENCES proposals(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    result_json TEXT NOT NULL,
    UNIQUE(dataset_id, name)
);

-- 跨数据集的扫速标度拟合尝试（含被拒绝的尝试，便于复核）
CREATE TABLE IF NOT EXISTS scaling_runs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at_ms INTEGER NOT NULL,
    name TEXT NOT NULL,
    dataset_ids_json TEXT NOT NULL,
    status TEXT NOT NULL,           -- ok | rejected
    incompatibilities_json TEXT NOT NULL,
    result_json TEXT NOT NULL
);

-- 运行记录（审计日志）
CREATE TABLE IF NOT EXISTS events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    at_ms INTEGER NOT NULL,
    actor TEXT NOT NULL,
    action TEXT NOT NULL,
    detail TEXT NOT NULL
);
"#;

pub fn now_ms_pub() -> i64 {
    now_ms()
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

pub fn sha256_hex(data: &[u8]) -> String {
    // 不引入 sha2 依赖：使用 rusqlite 内置 SQLite 无 sha 函数；
    // 这里用一个简单的 FNV-1a 64 + 长度的双哈希作为内容指纹（仅用于复核比对）
    let mut h1: u64 = 0xcbf29ce484222325;
    let mut h2: u64 = 0x6c62272e07bb0142;
    for &b in data {
        h1 ^= b as u64;
        h1 = h1.wrapping_mul(0x100000001b3);
        h2 = h2.rotate_left(5) ^ b as u64;
        h2 = h2.wrapping_mul(0x9e3779b97f4a7c15);
    }
    format!("fnv-{:016x}-{:016x}-{}", h1, h2, data.len())
}

impl Db {
    pub fn open(path: &str) -> Result<Db> {
        let conn = Connection::open(path).with_context(|| format!("打开数据库失败: {path}"))?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Db { conn: Mutex::new(conn) })
    }

    /// 内存数据库（测试用）
    pub fn open_in_memory() -> Result<Db> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Db { conn: Mutex::new(conn) })
    }

    pub fn log_event(&self, action: &str, detail: &str) -> Result<()> {
        self.log_event_as("user", action, detail)
    }

    pub fn log_event_as(&self, actor: &str, action: &str, detail: &str) -> Result<()> {
        let conn = self.conn.lock().expect("db lock");
        conn.execute(
            "INSERT INTO events(at_ms, actor, action, detail) VALUES (?1,?2,?3,?4)",
            params![now_ms(), actor, action, detail],
        )?;
        Ok(())
    }

    pub fn insert_dataset(
        &self,
        file_name: &str,
        raw_csv: &str,
        run: &RunData,
    ) -> Result<i64> {
        let mut conn = self.conn.lock().expect("db lock");
        let tx = conn.transaction()?;
        let m: &Metadata = &run.meta;
        tx.execute(
            "INSERT INTO datasets(name,file_name,scan_rate_v_s,scan_rate_unit,
                potential_unit,current_unit,time_unit,area_cm2,area_unit,
                reference_electrode,tolerance_v,imported_at_ms,content_sha256,raw_csv)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
            params![
                m.name,
                file_name,
                m.scan_rate,
                m.scan_rate_unit,
                m.potential_unit,
                m.current_unit,
                m.time_unit,
                m.area,
                m.area_unit,
                m.reference_electrode,
                m.tolerance_v,
                now_ms(),
                sha256_hex(raw_csv.as_bytes()),
                raw_csv,
            ],
        )?;
        let id = tx.last_insert_rowid();
        for (pos, s) in run.samples.iter().enumerate() {
            tx.execute(
                "INSERT INTO samples(dataset_id,sample_pos,instrument_index,potential_v,current_a,time_s,label)
                 VALUES (?1,?2,?3,?4,?5,?6,?7)",
                params![id, pos as i64, s.index as i64, s.potential_v, s.current_a, s.time_s, run.labels[pos] as i64],
            )?;
        }
        for (order, seg) in run.segments.iter().enumerate() {
            tx.execute(
                "INSERT INTO segments(dataset_id,seg_order,kind,cycle,start_pos,end_pos)
                 VALUES (?1,?2,?3,?4,?5,?6)",
                params![
                    id,
                    order as i64,
                    seg.kind as i64,
                    seg.cycle,
                    seg.start_idx as i64,
                    seg.end_idx as i64
                ],
            )?;
        }
        tx.commit()?;
        Ok(id)
    }

    pub fn load_run(&self, dataset_id: i64) -> Result<Option<RunData>> {
        let conn = self.conn.lock().expect("db lock");
        let meta = conn
            .query_row(
                "SELECT name,scan_rate_v_s,scan_rate_unit,potential_unit,current_unit,time_unit,
                        area_cm2,area_unit,reference_electrode,tolerance_v
                 FROM datasets WHERE id=?1",
                params![dataset_id],
                |row| {
                    Ok(Metadata {
                        name: row.get(0)?,
                        scan_rate: row.get(1)?,
                        scan_rate_unit: row.get(2)?,
                        potential_unit: row.get(3)?,
                        current_unit: row.get(4)?,
                        time_unit: row.get(5)?,
                        area: row.get(6)?,
                        area_unit: row.get(7)?,
                        reference_electrode: row.get(8)?,
                        tolerance_v: row.get(9)?,
                    })
                },
            )
            .ok();
        let Some(meta) = meta else { return Ok(None) };
        let mut stmt = conn.prepare(
            "SELECT instrument_index,potential_v,current_a,time_s,label
             FROM samples WHERE dataset_id=?1 ORDER BY sample_pos",
        )?;
        let mut samples = Vec::new();
        let mut labels = Vec::new();
        let rows = stmt.query_map(params![dataset_id], |row| {
            Ok((
                crate::model::Sample {
                    index: row.get::<_, i64>(0)? as usize,
                    potential_v: row.get(1)?,
                    current_a: row.get(2)?,
                    time_s: row.get(3)?,
                },
                row.get::<_, i64>(4)? as i8,
            ))
        })?;
        for r in rows {
            let (s, l) = r?;
            samples.push(s);
            labels.push(l);
        }
        let mut stmt2 = conn.prepare(
            "SELECT kind,cycle,start_pos,end_pos FROM segments
             WHERE dataset_id=?1 ORDER BY seg_order",
        )?;
        let segments = stmt2
            .query_map(params![dataset_id], |row| {
                Ok(Segment {
                    kind: SegmentKind::from_i64(row.get(0)?),
                    cycle: row.get(1)?,
                    start_idx: row.get::<_, i64>(2)? as usize,
                    end_idx: row.get::<_, i64>(3)? as usize,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(Some(RunData { meta, samples, labels, segments }))
    }
}
