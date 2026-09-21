use crate::model::{BaselineKind, DatasetMeta, Point};
use rusqlite::{params, Connection};
use std::sync::Mutex;

pub struct Db(pub Mutex<Connection>);

pub fn open(path: &str) -> rusqlite::Result<Db> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    init(&conn)?;
    Ok(Db(Mutex::new(conn)))
}

pub fn open_memory() -> rusqlite::Result<Db> {
    let conn = Connection::open_in_memory()?;
    init(&conn)?;
    Ok(Db(Mutex::new(conn)))
}

fn init(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS datasets (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            reference_electrode TEXT NOT NULL,
            potential_unit TEXT NOT NULL,
            current_unit TEXT NOT NULL,
            time_unit TEXT NOT NULL,
            scan_rate REAL NOT NULL,
            scan_rate_unit TEXT NOT NULL,
            electrode_area REAL NOT NULL,
            electrode_area_unit TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE TABLE IF NOT EXISTS points (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            dataset_id INTEGER NOT NULL REFERENCES datasets(id) ON DELETE CASCADE,
            idx INTEGER NOT NULL,
            time REAL NOT NULL,
            potential REAL NOT NULL,
            current REAL NOT NULL,
            UNIQUE(dataset_id, idx)
        );
        CREATE INDEX IF NOT EXISTS idx_points_ds ON points(dataset_id, idx);
        CREATE TABLE IF NOT EXISTS proposals (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            dataset_id INTEGER NOT NULL REFERENCES datasets(id) ON DELETE CASCADE,
            name TEXT NOT NULL,
            baseline TEXT NOT NULL,
            poly_degree INTEGER NOT NULL DEFAULT 2,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE TABLE IF NOT EXISTS intervals (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            proposal_id INTEGER NOT NULL REFERENCES proposals(id) ON DELETE CASCADE,
            segment_index INTEGER NOT NULL,
            start_idx INTEGER NOT NULL,
            end_idx INTEGER NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE TABLE IF NOT EXISTS runs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            ts TEXT NOT NULL DEFAULT (datetime('now')),
            action TEXT NOT NULL,
            detail TEXT NOT NULL
        );
        "#,
    )
}

pub fn insert_dataset(conn: &Connection, meta: &DatasetMeta, points: &[Point]) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO datasets
         (name, reference_electrode, potential_unit, current_unit, time_unit,
          scan_rate, scan_rate_unit, electrode_area, electrode_area_unit)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![
            meta.name,
            meta.reference_electrode,
            meta.potential_unit,
            meta.current_unit,
            meta.time_unit,
            meta.scan_rate,
            meta.scan_rate_unit,
            meta.electrode_area,
            meta.electrode_area_unit
        ],
    )?;
    let id = conn.last_insert_rowid();
    {
        let mut stmt = conn.prepare(
            "INSERT INTO points (dataset_id, idx, time, potential, current)
             VALUES (?1,?2,?3,?4,?5)",
        )?;
        for p in points {
            stmt.execute(params![id, p.idx as i64, p.time, p.potential, p.current])?;
        }
    }
    Ok(id)
}

pub fn list_datasets(conn: &Connection) -> rusqlite::Result<Vec<(i64, DatasetMeta, usize)>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, reference_electrode, potential_unit, current_unit, time_unit,
                scan_rate, scan_rate_unit, electrode_area, electrode_area_unit,
                (SELECT COUNT(*) FROM points p WHERE p.dataset_id = d.id)
         FROM datasets d ORDER BY id",
    )?;
    let rows = stmt.query_map([], row_to_dataset)?;
    rows.collect()
}

fn row_to_dataset(row: &rusqlite::Row<'_>) -> rusqlite::Result<(i64, DatasetMeta, usize)> {
    Ok((
        row.get(0)?,
        DatasetMeta {
            name: row.get(1)?,
            reference_electrode: row.get(2)?,
            potential_unit: row.get(3)?,
            current_unit: row.get(4)?,
            time_unit: row.get(5)?,
            scan_rate: row.get(6)?,
            scan_rate_unit: row.get(7)?,
            electrode_area: row.get(8)?,
            electrode_area_unit: row.get(9)?,
        },
        row.get::<_, i64>(10)? as usize,
    ))
}

pub fn get_dataset(conn: &Connection, id: i64) -> rusqlite::Result<Option<(DatasetMeta, Vec<Point>)>> {
    let meta = conn
        .query_row(
            "SELECT 1,name,reference_electrode,potential_unit,current_unit,time_unit,
                    scan_rate,scan_rate_unit,electrode_area,electrode_area_unit
             FROM datasets WHERE id = ?1",
            params![id],
            |row| {
                Ok(DatasetMeta {
                    name: row.get(1)?,
                    reference_electrode: row.get(2)?,
                    potential_unit: row.get(3)?,
                    current_unit: row.get(4)?,
                    time_unit: row.get(5)?,
                    scan_rate: row.get(6)?,
                    scan_rate_unit: row.get(7)?,
                    electrode_area: row.get(8)?,
                    electrode_area_unit: row.get(9)?,
                })
            },
        )
        .optional()?;
    let Some(meta) = meta else { return Ok(None) };
    let mut stmt = conn.prepare(
        "SELECT idx, time, potential, current FROM points
         WHERE dataset_id = ?1 ORDER BY idx",
    )?;
    let points = stmt
        .query_map(params![id], |row| {
            Ok(Point {
                idx: row.get::<_, i64>(0)? as usize,
                time: row.get(1)?,
                potential: row.get(2)?,
                current: row.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Some((meta, points)))
}

pub fn delete_dataset(conn: &Connection, id: i64) -> rusqlite::Result<bool> {
    Ok(conn.execute("DELETE FROM datasets WHERE id = ?1", params![id])? > 0)
}

pub fn create_proposal(
    conn: &Connection,
    dataset_id: i64,
    name: &str,
    baseline: BaselineKind,
    poly_degree: i64,
) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO proposals (dataset_id, name, baseline, poly_degree) VALUES (?1,?2,?3,?4)",
        params![dataset_id, name, format!("{baseline:?}").to_lowercase(), poly_degree],
    )?;
    Ok(conn.last_insert_rowid())
}

#[derive(Debug, Clone)]
pub struct ProposalRow {
    pub id: i64,
    pub dataset_id: i64,
    pub name: String,
    pub baseline: BaselineKind,
    pub poly_degree: i64,
}

pub fn list_proposals(conn: &Connection, dataset_id: Option<i64>) -> rusqlite::Result<Vec<ProposalRow>> {
    let sql = "SELECT id, dataset_id, name, baseline, poly_degree FROM proposals
               WHERE (?1 IS NULL OR dataset_id = ?1) ORDER BY id";
    let mut stmt = conn.prepare(sql)?;
    stmt.query_map(params![dataset_id], |row| {
        let b: String = row.get(3)?;
        let baseline = match b.as_str() {
            "localpoly" => BaselineKind::LocalPoly,
            "derivative" => BaselineKind::Derivative,
            _ => BaselineKind::EndpointLinear,
        };
        Ok(ProposalRow {
            id: row.get(0)?,
            dataset_id: row.get(1)?,
            name: row.get(2)?,
            baseline,
            poly_degree: row.get(4)?,
        })
    })?
    .collect()
}

pub fn get_proposal(conn: &Connection, id: i64) -> rusqlite::Result<Option<ProposalRow>> {
    list_proposals(conn, None).map(|v| v.into_iter().find(|p| p.id == id))
}

pub fn add_interval(
    conn: &Connection,
    proposal_id: i64,
    segment_index: i64,
    start: i64,
    end: i64,
) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO intervals (proposal_id, segment_index, start_idx, end_idx)
         VALUES (?1,?2,?3,?4)",
        params![proposal_id, segment_index, start, end],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn update_interval(conn: &Connection, id: i64, start: i64, end: i64) -> rusqlite::Result<bool> {
    Ok(
        conn.execute("UPDATE intervals SET start_idx=?2, end_idx=?3 WHERE id=?1", params![id, start, end])?
            > 0,
    )
}

pub fn delete_interval(conn: &Connection, id: i64) -> rusqlite::Result<bool> {
    Ok(conn.execute("DELETE FROM intervals WHERE id=?1", params![id])? > 0)
}

#[derive(Debug, Clone)]
pub struct IntervalRow {
    pub id: i64,
    pub segment_index: usize,
    pub start: usize,
    pub end: usize,
}

pub fn list_intervals(conn: &Connection, proposal_id: i64) -> rusqlite::Result<Vec<IntervalRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, segment_index, start_idx, end_idx FROM intervals
         WHERE proposal_id=?1 ORDER BY id",
    )?;
    stmt.query_map(params![proposal_id], |row| {
        Ok(IntervalRow {
            id: row.get(0)?,
            segment_index: row.get::<_, i64>(1)? as usize,
            start: row.get::<_, i64>(2)? as usize,
            end: row.get::<_, i64>(3)? as usize,
        })
    })?
    .collect()
}

pub fn log_run(conn: &Connection, action: &str, detail: &str) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO runs (action, detail) VALUES (?1,?2)",
        params![action, detail],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn list_runs(conn: &Connection) -> rusqlite::Result<Vec<RunRow>> {
    let mut stmt = conn.prepare("SELECT id, ts, action, detail FROM runs ORDER BY id")?;
    stmt.query_map([], |row| {
        Ok(RunRow {
            id: row.get(0)?,
            ts: row.get(1)?,
            action: row.get(2)?,
            detail: row.get(3)?,
        })
    })?
    .collect()
}

#[derive(Debug, serde::Serialize)]
pub struct RunRow {
    pub id: i64,
    pub ts: String,
    pub action: String,
    pub detail: String,
}

pub fn reset_all(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "DELETE FROM intervals;
         DELETE FROM proposals;
         DELETE FROM points;
         DELETE FROM datasets;
         DELETE FROM runs;",
    )
}

// rusqlite OptionalExtension
trait OptionalExt<T> {
    fn optional(self) -> rusqlite::Result<Option<T>>;
}
impl<T> OptionalExt<T> for rusqlite::Result<T> {
    fn optional(self) -> rusqlite::Result<Option<T>> {
        match self {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}
