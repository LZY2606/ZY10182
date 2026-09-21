//! 分析编排：给定数据集、扫描段、基线方案与峰区间（全局样本坐标），
//! 产出峰参数与逐点溯源；并负责把方案持久化。

use crate::baseline::{evaluate, BaselineKind, BaselineParams, BaselineResult};
use crate::db::Db;
use crate::model::{RunData, SegmentKind};
use crate::peaks::{assign_overlaps, measure_peak, Interval, Peak};
use anyhow::{bail, Context, Result};
use rusqlite::params;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize)]
pub struct ProposalSpec {
    pub seg_order: i64,
    pub name: String,
    pub baseline: String,
    #[serde(default)]
    pub params: BaselineParams,
    /// 全局样本坐标的闭区间；为空表示按当前段自动建议
    #[serde(default)]
    pub intervals: Vec<IntervalGlobal>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
pub struct IntervalGlobal {
    pub start_idx: usize,
    pub end_idx: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PeakProvenance {
    pub peak_no: usize,
    pub direction: String,
    pub interval_sample_pos: [usize; 2],
    pub apex_sample_pos: usize,
    pub apex_instrument_index: usize,
    pub baseline_kind: String,
    pub baseline_params: BaselineParams,
    pub baseline_anchor_sample_pos: Vec<usize>,
    pub contributions: Vec<ContributionTrace>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ContributionTrace {
    pub sample_pos: usize,
    pub instrument_index: usize,
    pub time_s: f64,
    pub potential_v: f64,
    pub raw_current_a: f64,
    pub baseline_current_a: f64,
    pub corrected_current_a: f64,
    pub weight: f64,
}

#[derive(Debug, Serialize)]
pub struct AnalyzedPeak {
    pub peak_no: usize,
    #[serde(flatten)]
    pub peak: Peak,
}

#[derive(Debug, Serialize)]
pub struct ProposalDetail {
    pub id: i64,
    pub dataset_id: i64,
    pub seg_order: i64,
    pub name: String,
    pub direction: String,
    pub baseline_kind: String,
    pub params: BaselineParams,
    pub intervals_global: Vec<IntervalGlobal>,
    pub peaks: Vec<PeakDetailJson>,
}

#[derive(Debug, Serialize)]
pub struct PeakDetailJson {
    pub peak_no: usize,
    pub interval_lo: usize,
    pub interval_hi: usize,
    pub apex_index: usize,
    pub peak_potential_v: f64,
    pub peak_current_a: f64,
    pub baseline_current_at_apex_a: f64,
    pub peak_height_a: f64,
    pub charge_c: f64,
    pub direction: String,
    pub provenance: PeakProvenance,
}

pub struct SegmentAnalysis {
    pub peaks: Vec<Peak>,
    pub windows_local: Vec<Interval>,
    pub baselines: Vec<BaselineResult>,
    pub intervals_global: Vec<IntervalGlobal>,
}

/// 对一个扫描段执行分析（纯函数）。
pub fn analyze_segment(
    run: &RunData,
    seg_order: usize,
    kind: BaselineKind,
    bparams: &BaselineParams,
    intervals_global: &[IntervalGlobal],
) -> Result<SegmentAnalysis> {
    let seg = run
        .segments
        .get(seg_order)
        .with_context(|| format!("扫描段 {seg_order} 不存在（共 {} 段）", run.segments.len()))?;
    let seg_points = &run.samples[seg.start_idx..=seg.end_idx];

    let windows_local: Vec<Interval> = if intervals_global.is_empty() {
        let auto = crate::peaks::suggest_intervals(seg_points, seg.kind);
        if auto.is_empty() {
            bail!("自动检测未在该扫描段发现峰区间，请手动给出峰区间");
        }
        auto
    } else {
        let mut out = Vec::new();
        for g in intervals_global {
            if g.start_idx < seg.start_idx
                || g.end_idx > seg.end_idx
                || g.end_idx < g.start_idx
            {
                bail!(
                    "峰区间 [{}, {}] 越出扫描段 {} 的范围 [{}, {}]",
                    g.start_idx, g.end_idx, seg_order, seg.start_idx, seg.end_idx
                );
            }
            out.push(Interval {
                start_idx: g.start_idx - seg.start_idx,
                end_idx: g.end_idx - seg.start_idx,
            });
        }
        out
    };

    let intervals_global_used: Vec<IntervalGlobal> = windows_local
        .iter()
        .map(|w| IntervalGlobal {
            start_idx: w.lo() + seg.start_idx,
            end_idx: w.hi() + seg.start_idx,
        })
        .collect();

    let mut peaks = Vec::with_capacity(windows_local.len());
    let mut baselines = Vec::with_capacity(windows_local.len());
    for w in &windows_local {
        let window_points = &seg_points[w.lo()..=w.hi()];
        let bl = evaluate(kind, bparams, window_points)?;
        let peak = measure_peak(crate::peaks::PeakInput {
            kind: seg.kind,
            points: seg_points,
            seg_start: seg.start_idx,
            interval: *w,
            baseline: bl.clone(),
        })?;
        peaks.push(peak);
        baselines.push(bl);
    }

    assign_overlaps(seg_points, seg.start_idx, &windows_local, &mut peaks)?;

    Ok(SegmentAnalysis { peaks, windows_local, baselines, intervals_global: intervals_global_used })
}

impl Db {
    /// 创建或整体替换一个分析方案。
    pub fn upsert_proposal(
        &self,
        dataset_id: i64,
        spec: &ProposalSpec,
    ) -> Result<(i64, ProposalDetail)> {
        let run = self
            .load_run(dataset_id)?
            .with_context(|| format!("数据集 {dataset_id} 不存在"))?;
        let kind = BaselineKind::parse(&spec.baseline)?;
        let analysis = analyze_segment(
            &run,
            spec.seg_order as usize,
            kind,
            &spec.params,
            &spec.intervals,
        )?;

        let mut conn = self.conn.lock().expect("db lock");
        let tx = conn.transaction()?;
        // 同名整体替换
        tx.execute(
            "DELETE FROM proposals WHERE dataset_id=?1 AND seg_order=?2 AND name=?3",
            params![dataset_id, spec.seg_order, spec.name],
        )?;
        tx.execute(
            "INSERT INTO proposals(dataset_id,seg_order,name,created_at_ms)
             VALUES (?1,?2,?3,?4)",
            params![dataset_id, spec.seg_order, spec.name, crate::db::now_ms_pub()],
        )?;
        let pid = tx.last_insert_rowid();

        let seg = &run.segments[spec.seg_order as usize];
        let mut values_map = serde_json::Map::new();
        let mut anchors_map = serde_json::Map::new();
        for (i, bl) in analysis.baselines.iter().enumerate() {
            values_map.insert(format!("{}", i + 1), serde_json::to_value(&bl.values)?);
            let global_anchors: Vec<usize> = bl
                .anchor_positions
                .iter()
                .map(|&p| analysis.intervals_global[i].start_idx + p)
                .collect();
            anchors_map.insert(format!("{}", i + 1), serde_json::to_value(global_anchors)?);
        }
        tx.execute(
            "INSERT INTO baselines(proposal_id,kind,params_json,values_json,anchor_positions_json,created_at_ms)
             VALUES (?1,?2,?3,?4,?5,?6)",
            params![
                pid,
                kind.as_str(),
                serde_json::to_string(&spec.params)?,
                serde_json::to_string(&values_map)?,
                serde_json::to_string(&anchors_map)?,
                crate::db::now_ms_pub()
            ],
        )?;

        let mut peak_jsons = Vec::new();
        for (i, ((peak, w), bl)) in analysis
            .peaks
            .iter()
            .zip(analysis.windows_local.iter())
            .zip(analysis.baselines.iter())
            .enumerate()
        {
            let peak_no = i + 1;
            tx.execute(
                "INSERT INTO intervals(proposal_id,peak_no,start_pos,end_pos)
                 VALUES (?1,?2,?3,?4)",
                params![
                    pid,
                    peak_no as i64,
                    (w.lo() + seg.start_idx) as i64,
                    (w.hi() + seg.start_idx) as i64
                ],
            )?;

            // 溯源：每个权重非零的贡献点都可定位到原始样本
            let mut traces = Vec::new();
            for c in &peak.contributions {
                let pos = c.sample_index;
                let s = &run.samples[pos];
                let base_i = s.current_a - c.current_corrected_a;
                traces.push(ContributionTrace {
                    sample_pos: pos,
                    instrument_index: s.index,
                    time_s: s.time_s,
                    potential_v: s.potential_v,
                    raw_current_a: s.current_a,
                    baseline_current_a: base_i,
                    corrected_current_a: c.current_corrected_a,
                    weight: c.weight,
                });
            }
            let global_anchors: Vec<usize> = bl
                .anchor_positions
                .iter()
                .map(|&p| analysis.intervals_global[i].start_idx + p)
                .collect();
            let prov = PeakProvenance {
                peak_no,
                direction: peak.direction.to_string(),
                interval_sample_pos: [
                    w.lo() + seg.start_idx,
                    w.hi() + seg.start_idx,
                ],
                apex_sample_pos: peak.apex_index,
                apex_instrument_index: run.samples[peak.apex_index].index,
                baseline_kind: kind.as_str().to_string(),
                baseline_params: spec.params.clone(),
                baseline_anchor_sample_pos: global_anchors,
                contributions: traces,
            };
            let prov_json = serde_json::to_string(&prov)?;
            tx.execute(
                "INSERT INTO peaks(proposal_id,peak_no,apex_pos,peak_potential_v,peak_current_a,
                    baseline_current_at_apex_a,peak_height_a,charge_c,provenance_json)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![
                    pid,
                    peak_no as i64,
                    peak.apex_index as i64,
                    peak.peak_potential_v,
                    peak.peak_current_a,
                    peak.baseline_current_at_apex_a,
                    peak.peak_height_a,
                    peak.charge_c,
                    prov_json
                ],
            )?;
            peak_jsons.push(prov);
        }
        tx.commit()?;

        drop(run);
        let detail = self.load_proposal(pid)?.context("新建方案读取失败")?;
        Ok((pid, detail))
    }

    #[allow(clippy::too_many_arguments)]
    /// 从库读回完整方案（含峰与溯源）。
    pub fn load_proposal(&self, proposal_id: i64) -> Result<Option<ProposalDetail>> {
        let conn = self.conn.lock().expect("db lock");
        let head = conn.query_row(
            "SELECT p.dataset_id,p.seg_order,p.name,b.kind,b.params_json
             FROM proposals p JOIN baselines b ON b.proposal_id=p.id
             WHERE p.id=?1",
            params![proposal_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        );
        let Ok((dataset_id, seg_order, name, kind_s, params_s)) = head else {
            return Ok(None);
        };
        let Some(run) = self.load_run(dataset_id)? else { return Ok(None) };
        let kind = BaselineKind::parse(&kind_s)?;
        let params: BaselineParams = serde_json::from_str(&params_s)?;
        let direction = match run.segments[seg_order as usize].kind {
            SegmentKind::Forward => "forward".to_string(),
            SegmentKind::Reverse => "reverse".to_string(),
        };

        let mut intervals_global = Vec::new();
        let mut peaks_out = Vec::new();
        let mut stmt = conn.prepare(
            "SELECT i.peak_no,i.start_pos,i.end_pos,
                    p.apex_pos,p.peak_potential_v,p.peak_current_a,
                    p.baseline_current_at_apex_a,p.peak_height_a,p.charge_c,p.provenance_json
             FROM intervals i JOIN peaks p ON p.proposal_id=i.proposal_id AND p.peak_no=i.peak_no
             WHERE i.proposal_id=?1 ORDER BY i.peak_no",
        )?;
        let rows = stmt.query_map(params![proposal_id], |row| {
            Ok((
                row.get::<_, i64>(0)? as usize,
                row.get::<_, i64>(1)? as usize,
                row.get::<_, i64>(2)? as usize,
                row.get::<_, i64>(3)? as usize,
                row.get::<_, f64>(4)?,
                row.get::<_, f64>(5)?,
                row.get::<_, f64>(6)?,
                row.get::<_, f64>(7)?,
                row.get::<_, f64>(8)?,
                row.get::<_, String>(9)?,
            ))
        })?;
        for r in rows {
            let (no, lo, hi, apex, ep, ic, ib, h, q, prov_json) = r?;
            intervals_global.push(IntervalGlobal { start_idx: lo, end_idx: hi });
            let provenance: PeakProvenance = serde_json::from_str(&prov_json)?;
            peaks_out.push(PeakDetailJson {
                peak_no: no,
                interval_lo: lo,
                interval_hi: hi,
                apex_index: apex,
                peak_potential_v: ep,
                peak_current_a: ic,
                baseline_current_at_apex_a: ib,
                peak_height_a: h,
                charge_c: q,
                direction: provenance.direction.clone(),
                provenance,
            });
        }
        Ok(Some(ProposalDetail {
            id: proposal_id,
            dataset_id,
            seg_order,
            name,
            direction,
            baseline_kind: kind.as_str().to_string(),
            params,
            intervals_global,
            peaks: peaks_out,
        }))
    }

    pub fn list_proposals(&self, dataset_id: i64) -> Result<Vec<ProposalDetail>> {
        let ids: Vec<i64> = {
            let conn = self.conn.lock().expect("db lock");
            let mut stmt = conn.prepare("SELECT id FROM proposals WHERE dataset_id=?1 ORDER BY id")?;
            let mut v = Vec::new();
            for row in stmt.query_map(params![dataset_id], |r| r.get::<_, i64>(0))? {
                v.push(row?);
            }
            v
        };
        let mut out = Vec::new();
        for id in ids {
            if let Some(d) = self.load_proposal(id)? {
                out.push(d);
            }
        }
        Ok(out)
    }
}
