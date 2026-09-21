//! Axum HTTP 服务：JSON API + 内嵌静态页面。

use crate::analysis::ProposalSpec;
use crate::db::Db;
use crate::import::import_run;
use crate::studio::{PairRequest, ScalingRequest};
use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;
use std::sync::Arc;
use tower_http::services::ServeDir;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Db>,
    pub static_dir: Option<std::path::PathBuf>,
}

pub fn router(state: AppState) -> Router {
    let r = Router::new()
        .route("/", get(index))
        .route("/api/health", get(health))
        .route("/api/datasets", get(list_datasets).post(import_dataset))
        .route("/api/datasets/{id}", get(get_dataset).delete(delete_dataset))
        .route("/api/datasets/{id}/raw", get(get_raw))
        .route("/api/datasets/{id}/proposals", get(list_proposals).post(create_proposal))
        .route("/api/proposals/{pid}", get(get_proposal).delete(delete_proposal))
        .route("/api/datasets/{id}/pair-proposals", get(list_pairs).post(create_pair))
        .route("/api/scaling", post(run_scaling).get(list_scaling))
        .route("/api/events", get(list_events))
        .route("/api/admin/reset", post(reset))
        .route("/api/fixtures", get(list_fixtures))
        .route("/api/fixtures/{name}/import", post(import_fixture));
    let r = if let Some(dir) = &state.static_dir {
        r.nest_service("/static", ServeDir::new(dir.clone()))
    } else {
        r
    };
    r.with_state(state)
}

async fn index(State(st): State<AppState>) -> Response {
    let html = if let Some(dir) = &st.static_dir {
        tokio::fs::read_to_string(dir.join("index.html")).await.ok()
    } else {
        None
    };
    let html = html.unwrap_or_else(|| include_str!("../static/index.html").to_string());
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html,
    )
        .into_response()
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "ok": true, "service": "伏安峰议台" }))
}

#[derive(Serialize)]
struct DatasetSummary {
    id: i64,
    name: String,
    file_name: String,
    scan_rate_v_s: f64,
    scan_rate_unit: String,
    potential_unit: String,
    current_unit: String,
    time_unit: String,
    area_cm2: f64,
    area_unit: String,
    reference_electrode: String,
    tolerance_v: f64,
    n_samples: usize,
    n_segments: usize,
    imported_at_ms: i64,
    content_sha256: String,
}

async fn list_datasets(State(st): State<AppState>) -> ApiResp<Json<Vec<DatasetSummary>>> {
    let db = st.db.clone();
    let data = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<DatasetSummary>> {
        let conn = db.conn.lock().expect("db lock");
        let mut stmt = conn.prepare(
            "SELECT id,name,file_name,scan_rate_v_s,scan_rate_unit,potential_unit,current_unit,
                    time_unit,area_cm2,area_unit,reference_electrode,tolerance_v,imported_at_ms,content_sha256
             FROM datasets ORDER BY id",
        )?;
        let base = stmt
            .query_map([], |row| {
                Ok(DatasetSummary {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    file_name: row.get(2)?,
                    scan_rate_v_s: row.get(3)?,
                    scan_rate_unit: row.get(4)?,
                    potential_unit: row.get(5)?,
                    current_unit: row.get(6)?,
                    time_unit: row.get(7)?,
                    area_cm2: row.get(8)?,
                    area_unit: row.get(9)?,
                    reference_electrode: row.get(10)?,
                    tolerance_v: row.get(11)?,
                    n_samples: 0,
                    n_segments: 0,
                    imported_at_ms: row.get(12)?,
                    content_sha256: row.get(13)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);
        let mut out = Vec::new();
        for mut d in base {
            d.n_samples = conn.query_row(
                "SELECT COUNT(*) FROM samples WHERE dataset_id=?1",
                rusqlite::params![d.id],
                |r| r.get::<_, i64>(0),
            )? as usize;
            d.n_segments = conn.query_row(
                "SELECT COUNT(*) FROM segments WHERE dataset_id=?1",
                rusqlite::params![d.id],
                |r| r.get::<_, i64>(0),
            )? as usize;
            out.push(d);
        }
        Ok(out)
    })
    .await.map_err(join_err)??;
    Ok(Json(data))
}

type ApiResp<T> = Result<T, ApiError>;

struct ApiError {
    status: StatusCode,
    message: String,
}


fn join_err(e: tokio::task::JoinError) -> ApiError {
    ApiError { status: StatusCode::INTERNAL_SERVER_ERROR, message: format!("后台任务失败: {e}") }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        ApiError { status: StatusCode::BAD_REQUEST, message: format!("{e:#}") }
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(e: serde_json::Error) -> Self {
        ApiError { status: StatusCode::INTERNAL_SERVER_ERROR, message: format!("JSON 错误: {e}") }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = serde_json::json!({ "error": self.message });
        (self.status, Json(body)).into_response()
    }
}

/// multipart 上传或 raw CSV 文本导入。
async fn import_dataset(
    State(st): State<AppState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> ApiResp<Json<serde_json::Value>> {
    let ct = headers
        .get(header::CONTENT_TYPE)
        .map(|v| v.to_str().unwrap_or("").to_string())
        .unwrap_or_default();

    let (filename, text) = if ct.starts_with("multipart/form-data") {
        let boundary = ct
            .split("boundary=")
            .nth(1)
            .ok_or_else(|| bad("multipart 缺少 boundary"))?
            .to_string();
        let (name, data) = parse_multipart(&boundary, &body)?;
        let text = String::from_utf8(data).map_err(|e| anyhow::anyhow!("CSV 不是 UTF-8: {e}"))?;
        (name, text)
    } else {
        let text = String::from_utf8(body.to_vec()).map_err(|e| anyhow::anyhow!("CSV 不是 UTF-8: {e}"))?;
        ("upload.csv".to_string(), text)
    };

    let default_name = filename.trim_end_matches(".csv").to_string();
    let db = st.db.clone();
    let file_for_resp = filename.clone();
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<(i64, String)> {
        let run = import_run(&text, &default_name).map_err(|e| anyhow::anyhow!("导入失败：{e:#}"))?;
        let id = db.insert_dataset(&filename, &text, &run)?;
        db.log_event("import", &format!("dataset_id={id} file={filename} n={}", run.samples.len()))?;
        Ok((id, run.meta.name))
    })
    .await.map_err(join_err)??;
    Ok(Json(serde_json::json!({
        "id": result.0, "name": result.1, "file_name": file_for_resp
    })))
}

fn bad(msg: &str) -> ApiError {
    ApiError { status: StatusCode::BAD_REQUEST, message: msg.to_string() }
}

/// 极简 multipart/form-data 解析：只取第一个带 filename 的文件部件。
fn parse_multipart(boundary: &str, body: &[u8]) -> anyhow::Result<(String, Vec<u8>)> {
    let delim = format!("--{boundary}");
    let text = std::str::from_utf8(body).map_err(|e| anyhow::anyhow!("multipart 非 UTF-8: {e}"))?;
    let mut filename = "upload.csv".to_string();
    let mut data: Option<Vec<u8>> = None;
    for part in text.split(&delim) {
        let part = part.trim_start_matches("\r\n");
        if part.is_empty() || part.starts_with("--") || part.trim().is_empty() {
            continue;
        }
        let split_at = part.find("\r\n\r\n").or_else(|| part.find("\n\n"));
        let Some(at) = split_at else { continue };
        let header = &part[..at];
        let sep_len = if part[at..].starts_with("\r\n\r\n") { 4 } else { 2 };
        let content = part[at + sep_len..].trim_end_matches("\r\n").trim_end_matches('\n');
        if let Some(line) = header.lines().find(|l| l.to_ascii_lowercase().contains("filename=")) {
            if let Some(f) = line.split("filename=\"").nth(1).and_then(|s| s.split('"').next()) {
                if !f.is_empty() {
                    filename = f.to_string();
                }
            }
            data = Some(content.as_bytes().to_vec());
            break;
        }
    }
    data.map(|d| (filename, d))
        .ok_or_else(|| anyhow::anyhow!("multipart 中未找到 CSV 文件部件（字段文件名请使用 file）"))
}

async fn get_dataset(State(st): State<AppState>, Path(id): Path<i64>) -> ApiResp<Json<serde_json::Value>> {
    let db = st.db.clone();
    let v = tokio::task::spawn_blocking(move || -> anyhow::Result<serde_json::Value> {
        let Some(run) = db.load_run(id)? else {
            return Err(anyhow::anyhow!("数据集 {id} 不存在"));
        };
        Ok(serde_json::json!({
            "id": id,
            "metadata": run.meta,
            "segments": run.segments.iter().enumerate().map(|(i, s)| serde_json::json!({
                "seg_order": i,
                "kind": s.kind.as_str(),
                "cycle": s.cycle,
                "start_pos": s.start_idx,
                "end_pos": s.end_idx,
            })).collect::<Vec<_>>(),
            "samples": run.samples.iter().enumerate().map(|(pos, sm)| serde_json::json!({
                "pos": pos,
                "index": sm.index,
                "potential_v": sm.potential_v,
                "current_a": sm.current_a,
                "time_s": sm.time_s,
                "label": run.labels[pos],
            })).collect::<Vec<_>>(),
        }))
    })
    .await.map_err(join_err)??;
    Ok(Json(v))
}

async fn delete_dataset(State(st): State<AppState>, Path(id): Path<i64>) -> ApiResp<Json<serde_json::Value>> {
    let db = st.db.clone();
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let n = {
            let conn = db.conn.lock().expect("db lock");
            conn.execute("DELETE FROM datasets WHERE id=?1", rusqlite::params![id])?
        };
        if n == 0 {
            return Err(anyhow::anyhow!("数据集 {id} 不存在"));
        }
        db.log_event("delete", &format!("dataset_id={id}"))?;
        Ok(())
    })
    .await.map_err(join_err)??;
    Ok(Json(serde_json::json!({"deleted": id})))
}

async fn get_raw(State(st): State<AppState>, Path(id): Path<i64>) -> ApiResp<Response> {
    let db = st.db.clone();
    let (csv, name) = tokio::task::spawn_blocking(move || -> anyhow::Result<(String, String)> {
        let conn = db.conn.lock().expect("db lock");
        conn.query_row(
            "SELECT raw_csv,file_name FROM datasets WHERE id=?1",
            rusqlite::params![id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )
        .map_err(|_| anyhow::anyhow!("数据集 {id} 不存在"))
    })
    .await.map_err(join_err)??;
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8"),
            (header::CONTENT_DISPOSITION, &format!("attachment; filename=\"{name}\"")),
        ],
        csv,
    )
        .into_response())
}

async fn create_proposal(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    Json(spec): Json<ProposalSpec>,
) -> ApiResp<Json<serde_json::Value>> {
    let db = st.db.clone();
    let detail = tokio::task::spawn_blocking(move || -> anyhow::Result<serde_json::Value> {
        let (pid, d) = db.upsert_proposal(id, &spec)?;
        db.log_event("proposal", &format!("dataset={id} proposal={pid} name={}", d.name))?;
        Ok(serde_json::to_value(d)?)
    })
    .await.map_err(join_err)??;
    Ok(Json(detail))
}

async fn list_proposals(State(st): State<AppState>, Path(id): Path<i64>) -> ApiResp<Json<Vec<serde_json::Value>>> {
    let db = st.db.clone();
    let v = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<serde_json::Value>> {
        let ds = db.list_proposals(id)?;
        Ok(ds.into_iter().map(|d| serde_json::to_value(d).unwrap()).collect())
    })
    .await.map_err(join_err)??;
    Ok(Json(v))
}

async fn get_proposal(State(st): State<AppState>, Path(pid): Path<i64>) -> ApiResp<Json<serde_json::Value>> {
    let db = st.db.clone();
    let v = tokio::task::spawn_blocking(move || -> anyhow::Result<serde_json::Value> {
        match db.load_proposal(pid)? {
            Some(d) => Ok(serde_json::to_value(d)?),
            None => Err(anyhow::anyhow!("方案 {pid} 不存在")),
        }
    })
    .await.map_err(join_err)??;
    Ok(Json(v))
}

async fn delete_proposal(State(st): State<AppState>, Path(pid): Path<i64>) -> ApiResp<Json<serde_json::Value>> {
    let db = st.db.clone();
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let n = {
            let conn = db.conn.lock().expect("db lock");
            conn.execute("DELETE FROM proposals WHERE id=?1", rusqlite::params![pid])?
        };
        if n == 0 {
            return Err(anyhow::anyhow!("方案 {pid} 不存在"));
        }
        db.log_event("delete_proposal", &format!("proposal_id={pid}"))?;
        Ok(())
    })
    .await.map_err(join_err)??;
    Ok(Json(serde_json::json!({"deleted": pid})))
}

async fn create_pair(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<PairRequest>,
) -> ApiResp<Json<serde_json::Value>> {
    let db = st.db.clone();
    let v = tokio::task::spawn_blocking(move || -> anyhow::Result<serde_json::Value> {
        let d = db.create_pair_proposal(id, &req)?;
        db.log_event("pair", &format!("dataset={id} pair={} name={}", d.id, d.name))?;
        Ok(serde_json::to_value(d)?)
    })
    .await.map_err(join_err)??;
    Ok(Json(v))
}

async fn list_pairs(State(st): State<AppState>, Path(id): Path<i64>) -> ApiResp<Json<Vec<serde_json::Value>>> {
    let db = st.db.clone();
    let v = tokio::task::spawn_blocking(move || db.list_pair_proposals(id)).await.map_err(join_err)??;
    Ok(Json(v))
}

async fn run_scaling(
    State(st): State<AppState>,
    Json(req): Json<ScalingRequest>,
) -> ApiResp<(StatusCode, Json<serde_json::Value>)> {
    let db = st.db.clone();
    let req_name = req.name.clone();
    let resp = tokio::task::spawn_blocking(move || db.run_scaling(&req)).await.map_err(join_err)??;
    let code = match &resp {
        crate::studio::ScalingResponse::Ok { .. } => StatusCode::OK,
        crate::studio::ScalingResponse::Rejected { .. } => StatusCode::UNPROCESSABLE_ENTITY,
    };
    let status_label = if code == StatusCode::OK { "ok" } else { "rejected" };
    let _ = st.db.log_event("scaling", &format!("name={req_name} status={status_label}"));
    Ok((code, Json(serde_json::to_value(resp)?)))
}

async fn list_scaling(State(st): State<AppState>) -> ApiResp<Json<Vec<serde_json::Value>>> {
    let db = st.db.clone();
    let v = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<serde_json::Value>> {
        let conn = db.conn.lock().expect("db lock");
        let mut stmt = conn.prepare(
            "SELECT id,created_at_ms,name,dataset_ids_json,status,incompatibilities_json,result_json
             FROM scaling_runs ORDER BY id",
        )?;
        let mut out = Vec::new();
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
            ))
        })?;
        for r in rows {
            let (id, ts, name, ids, status, incomp, result) = r?;
            out.push(serde_json::json!({
                "id": id, "created_at_ms": ts, "name": name,
                "dataset_ids": serde_json::from_str::<serde_json::Value>(&ids)?,
                "status": status,
                "incompatibilities": serde_json::from_str::<serde_json::Value>(&incomp)?,
                "result": serde_json::from_str::<serde_json::Value>(&result)?,
            }));
        }
        Ok(out)
    })
    .await.map_err(join_err)??;
    Ok(Json(v))
}

async fn list_events(State(st): State<AppState>) -> ApiResp<Json<Vec<serde_json::Value>>> {
    let db = st.db.clone();
    let v = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<serde_json::Value>> {
        let conn = db.conn.lock().expect("db lock");
        let mut stmt = conn.prepare(
            "SELECT id,at_ms,actor,action,detail FROM events ORDER BY id",
        )?;
        let mut out = Vec::new();
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?;
        for r in rows {
            let (id, ts, actor, action, detail) = r?;
            out.push(serde_json::json!({
                "id": id, "at_ms": ts, "actor": actor, "action": action, "detail": detail
            }));
        }
        Ok(out)
    })
    .await.map_err(join_err)??;
    Ok(Json(v))
}

async fn reset(State(st): State<AppState>) -> ApiResp<Json<serde_json::Value>> {
    let db = st.db.clone();
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        db.wipe_all()?;
        db.log_event("reset", "清空全部数据集与方案（保留事件日志）")?;
        Ok(())
    })
    .await.map_err(join_err)??;
    Ok(Json(serde_json::json!({"ok": true})))
}

async fn list_fixtures() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "fixtures": crate::fixtures::file_names() }))
}

async fn import_fixture(
    State(st): State<AppState>,
    Path(name): Path<String>,
) -> ApiResp<Json<serde_json::Value>> {
    let content = crate::fixtures::by_name(&name)
        .ok_or_else(|| bad(&format!("未知 fixture: {name}")))?;
    let default_name = name.trim_end_matches(".csv").to_string();
    let db = st.db.clone();
    let fname = name.clone();
    let id = tokio::task::spawn_blocking(move || -> anyhow::Result<(i64, String)> {
        let run = import_run(content, &default_name)?;
        let id = db.insert_dataset(&fname, content, &run)?;
        db.log_event("import_fixture", &format!("dataset_id={id} file={fname}"))?;
        Ok((id, run.meta.name))
    })
    .await.map_err(join_err)??;
    Ok(Json(serde_json::json!({"id": id.0, "name": id.1, "file_name": name})))
}

/// 导出运行记录（JSON Lines）。
pub fn events_as_jsonl(db: &Db) -> anyhow::Result<String> {
    let conn = db.conn.lock().expect("db lock");
    let mut stmt = conn.prepare("SELECT id,at_ms,actor,action,detail FROM events ORDER BY id")?;
    let mut out = String::new();
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
        ))
    })?;
    for r in rows {
        let (id, ts, actor, action, detail) = r?;
        out.push_str(&serde_json::json!({
            "id": id, "at_ms": ts, "actor": actor, "action": action, "detail": detail
        }).to_string());
        out.push('\n');
    }
    Ok(out)
}
