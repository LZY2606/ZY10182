//! 重新生成 fixtures/*.csv（确定性输出，用于复核与重放）。
use std::fs;
use std::path::PathBuf;

use fengyitai::fixtures::generate_csv;

fn main() -> anyhow::Result<()> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures");
    fs::create_dir_all(&dir)?;
    let files = [
        ("cycle_50mVs.csv", 0.05, "Ag/AgCl(3M KCl)", 0.071, "A"),
        ("cycle_100mVs.csv", 0.10, "Ag/AgCl(3M KCl)", 0.071, "A"),
        ("cycle_200mVs.csv", 0.20, "Ag/AgCl(3M KCl)", 0.071, "A"),
        ("cycle_100mVs_sce.csv", 0.10, "SCE(sat. KCl)", 0.125, "A"),
    ];
    for (name, rate, refer, area, unit) in files {
        let csv = generate_csv(rate, refer, area, unit);
        let path = dir.join(name);
        fs::write(&path, &csv)?;
        println!("wrote {} ({} bytes)", path.display(), csv.len());
    }
    Ok(())
}
