//! 伏安峰议台 —— 本地服务入口。

use std::sync::Arc;

use clap::Parser;
use fengyitai::{
    db::Db,
    server::{router, AppState},
};

/// 循环伏安峰分析本地服务
#[derive(Parser, Debug)]
#[command(name = "fengyitai", version, about = "伏安峰议台：循环伏安基线 / 峰区间 / 峰对多方案分析")]
struct Args {
    /// 监听地址，如 127.0.0.1:5522
    #[arg(long, default_value = "127.0.0.1:5522")]
    listen: String,

    /// SQLite 数据库文件（可用 FENGYITAI_DB 覆盖默认 ./data/fengyitai.sqlite）
    #[arg(long)]
    db: Option<String>,

    /// 静态资源目录（默认使用编译期内嵌页面，一般无需指定）
    #[arg(long)]
    static_dir: Option<std::path::PathBuf>,

    /// 导出全部运行记录（JSON Lines）到指定文件后退出，用于复核归档
    #[arg(long)]
    export_events: Option<std::path::PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let db_path = args
        .db
        .or_else(|| std::env::var("FENGYITAI_DB").ok())
        .unwrap_or_else(|| "data/fengyitai.sqlite".to_string());

    if let Some(parent) = std::path::Path::new(&db_path).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let db = Arc::new(Db::open(&db_path)?);
    db.log_event_as("system", "start", &format!("db={db_path}"))?;

    if let Some(path) = args.export_events {
        let jsonl = fengyitai::server::events_as_jsonl(&db)?;
        std::fs::write(&path, jsonl)?;
        println!("运行记录已导出到 {}", path.display());
        return Ok(());
    }

    let state = AppState { db: db.clone(), static_dir: args.static_dir };
    let listener = tokio::net::TcpListener::bind(&args.listen).await?;
    println!("伏安峰议台已启动: http://{}/", args.listen);
    println!("数据文件: {db_path}（删除该文件或调用 POST /api/admin/reset 可清空后重新导入复核）");
    axum::serve(listener, router(state)).await?;
    Ok(())
}
