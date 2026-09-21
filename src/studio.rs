//! 峰对方案与扫速标度的应用服务。

use crate::analysis::ProposalDetail;
use crate::db::Db;
use crate::pairs::{pair_candidates, PeakRef};
use crate::scaling::{self, DataCalibre, ScalingPoint};
use anyhow::{bail, Result};
use rusqlite::params;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct PairRequest {
    pub name: String,
    pub forward_proposal_id: i64,
    pub reverse_proposal_id: i64,
}

#[derive(Debug, Serialize)]
pub struct PairProposalDetail {
    pub id: i64,
    pub dataset_id: i64,
    pub name: String,
    pub forward_proposal_id: i64,
    pub reverse_proposal_id: i64,
    pub pairings: serde_json::Value,
}

fn peak_refs(detail: &ProposalDetail, tag: &str) -> Vec<PeakRef> {
    detail
        .peaks
        .iter()
        .map(|p| PeakRef {
            id: format!("{}#{}", tag, p.peak_no),
            peak_potential_v: p.peak_potential_v,
            peak_height_a: p.peak_height_a,
        })
        .collect()
}

impl Db {
    pub fn create_pair_proposal(&self, dataset_id: i64, req: &PairRequest) -> Result<PairProposalDetail> {
        let fwd = self
            .load_proposal(req.forward_proposal_id)?
            .ok_or_else(|| anyhow::anyhow!("正扫方案 {} 不存在", req.forward_proposal_id))?;
        let rev = self
            .load_proposal(req.reverse_proposal_id)?
            .ok_or_else(|| anyhow::anyhow!("反扫方案 {} 不存在", req.reverse_proposal_id))?;
        if fwd.dataset_id != dataset_id || rev.dataset_id != dataset_id {
            bail!("两个方案必须属于同一数据集");
        }
        if fwd.direction != "forward" || rev.direction != "reverse" {
            bail!("请按顺序选择正扫方案与反扫方案（当前：{} / {}）", fwd.direction, rev.direction);
        }
        if fwd.peaks.is_empty() || rev.peaks.is_empty() {
            bail!("两个方案都至少需要一个峰才能配对");
        }

        let winners = pair_candidates(&peak_refs(&fwd, "an"), &peak_refs(&rev, "ca"));
        let result_json = serde_json::json!({
            "winners": winners,
            "forward": fwd,
            "reverse": rev,
        })
        .to_string();

        let mut conn = self.conn.lock().expect("db lock");
        conn.execute(
            "INSERT INTO pair_proposals(dataset_id,forward_proposal_id,reverse_proposal_id,name,created_at_ms,result_json)
             VALUES (?1,?2,?3,?4,?5,?6)
             ON CONFLICT(dataset_id,name) DO UPDATE SET
               forward_proposal_id=excluded.forward_proposal_id,
               reverse_proposal_id=excluded.reverse_proposal_id,
               created_at_ms=excluded.created_at_ms,
               result_json=excluded.result_json",
            params![
                dataset_id,
                req.forward_proposal_id,
                req.reverse_proposal_id,
                req.name,
                crate::db::now_ms_pub(),
                result_json
            ],
        )?;
        let id = conn.last_insert_rowid();
        drop(conn);
        Ok(PairProposalDetail {
            id,
            dataset_id,
            name: req.name.clone(),
            forward_proposal_id: req.forward_proposal_id,
            reverse_proposal_id: req.reverse_proposal_id,
            pairings: serde_json::json!({ "winners": winners }),
        })
    }

    pub fn list_pair_proposals(&self, dataset_id: i64) -> Result<Vec<serde_json::Value>> {
        let conn = self.conn.lock().expect("db lock");
        let mut stmt = conn.prepare(
            "SELECT id,name,forward_proposal_id,reverse_proposal_id,result_json
             FROM pair_proposals WHERE dataset_id=?1 ORDER BY id",
        )?;
        let mut out = Vec::new();
        let rows = stmt.query_map(params![dataset_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?;
        for r in rows {
            let (id, name, fp, rp, result_json) = r?;
            let mut v: serde_json::Value = serde_json::from_str(&result_json)?;
            if let Some(obj) = v.as_object_mut() {
                obj.insert(
                    "id".into(),
                    serde_json::json!({"id":id,"name":name,"forward_proposal_id":fp,"reverse_proposal_id":rp}),
                );
            }
            out.push(v);
        }
        Ok(out)
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum ScalingResponse {
    Rejected { name: String, incompatibilities: Vec<scaling::Incompatibility> },
    Ok { name: String, fit: scaling::ScalingFit },
}

#[derive(Debug, Deserialize)]
pub struct ScalingRequest {
    pub name: String,
    pub dataset_ids: Vec<i64>,
    /// 每个数据集取峰时使用的分析方案 id；缺省时取该数据集最近的正扫方案
    #[serde(default)]
    pub proposal_ids: std::collections::HashMap<String, i64>,
    /// 取每个方案的第几号峰，默认 1
    #[serde(default = "default_peak_no")]
    pub peak_no: usize,
}

fn default_peak_no() -> usize { 1 }

impl Db {
    pub fn run_scaling(&self, req: &ScalingRequest) -> Result<ScalingResponse> {
        if req.dataset_ids.len() < 2 {
            bail!("共同标度至少选择 2 个数据集");
        }
        let mut calibres = Vec::new();
        let mut points = Vec::new();
        for &did in &req.dataset_ids {
            let run = self
                .load_run(did)?
                .ok_or_else(|| anyhow::anyhow!("数据集 {did} 不存在"))?;
            calibres.push(DataCalibre {
                dataset_id: did,
                dataset_name: run.meta.name.clone(),
                potential_unit: run.meta.potential_unit.clone(),
                current_unit: run.meta.current_unit.clone(),
                time_unit: run.meta.time_unit.clone(),
                scan_rate_unit: run.meta.scan_rate_unit.clone(),
                area: run.meta.area,
                area_unit: run.meta.area_unit.clone(),
                reference_electrode: run.meta.reference_electrode.clone(),
            });

            let proposal_id = match req.proposal_ids.get(&did.to_string()) {
                Some(id) => *id,
                None => {
                    let all = self.list_proposals(did)?;
                    all.iter()
                        .find(|p| p.direction == "forward")
                        .or_else(|| all.first())
                        .map(|p| p.id)
                        .ok_or_else(|| anyhow::anyhow!("数据集 {} 还没有分析方案", run.meta.name))?
                }
            };
            let detail = self
                .load_proposal(proposal_id)?
                .ok_or_else(|| anyhow::anyhow!("方案 {proposal_id} 不存在"))?;
            if detail.dataset_id != did {
                bail!("方案 {proposal_id} 不属于数据集 {did}");
            }
            let peak = detail
                .peaks
                .iter()
                .find(|p| p.peak_no == req.peak_no)
                .ok_or_else(|| anyhow::anyhow!("方案 {proposal_id} 中没有 {} 号峰", req.peak_no))?;
            points.push(ScalingPoint {
                dataset_id: did,
                dataset_name: run.meta.name.clone(),
                scan_rate_v_s: run.meta.scan_rate,
                peak_current_a: peak.peak_height_a,
                charge_c: peak.charge_c,
            });
        }

        let bad = scaling::check_calibre(&calibres);
        let (status, incomp_json, result_json) = if bad.is_empty() {
            match scaling::fit_power_law(&points) {
                Ok(fit) => {
                    let v = serde_json::to_value(&fit)?;
                    ("ok", "[]".to_string(), v.to_string())
                }
                Err(msg) => {
                    let incomp = vec![scaling::Incompatibility {
                        field: "fit".to_string(),
                        values: vec![msg],
                    }];
                    ("rejected", serde_json::to_string(&incomp)?, "null".to_string())
                }
            }
        } else {
            ("rejected", serde_json::to_string(&bad)?, "null".to_string())
        };

        {
            let conn = self.conn.lock().expect("db lock");
            conn.execute(
                "INSERT INTO scaling_runs(created_at_ms,name,dataset_ids_json,status,incompatibilities_json,result_json)
                 VALUES (?1,?2,?3,?4,?5,?6)",
                params![
                    crate::db::now_ms_pub(),
                    req.name,
                    serde_json::to_string(&req.dataset_ids)?,
                    status,
                    incomp_json,
                    result_json
                ],
            )?;
        }

        if status == "ok" {
            let fit: scaling::ScalingFit = serde_json::from_str(&result_json)?;
            Ok(ScalingResponse::Ok { name: req.name.clone(), fit })
        } else {
            let incompatibilities: Vec<scaling::Incompatibility> = serde_json::from_str(&incomp_json)?;
            Ok(ScalingResponse::Rejected { name: req.name.clone(), incompatibilities })
        }
    }
}

/// 清空除事件日志以外的全部数据（复核重放用）。
impl Db {
    pub fn wipe_all(&self) -> Result<()> {
        let conn = self.conn.lock().expect("db lock");
        conn.execute_batch(
            "DELETE FROM scaling_runs;
             DELETE FROM pair_proposals;
             DELETE FROM peaks;
             DELETE FROM intervals;
             DELETE FROM baselines;
             DELETE FROM proposals;
             DELETE FROM segments;
             DELETE FROM samples;
             DELETE FROM datasets;",
        )?;
        Ok(())
    }
}
