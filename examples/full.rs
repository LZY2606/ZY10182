use fengyitai::analysis::analyze_segment;
use fengyitai::baseline::{BaselineKind, BaselineParams};
use fengyitai::db::Db;
use fengyitai::import::import_run;

fn main() -> anyhow::Result<()> {
    let db = Db::open_in_memory()?;
    for (name, csv) in fengyitai::fixtures::FILES {
        let run = import_run(csv, name)?;
        let id = db.insert_dataset(name, csv, &run)?;
        println!("== {name} id={id} rate={} V/s", run.meta.scan_rate);
        let fwd = analyze_segment(&run, 0, BaselineKind::Derivative, &BaselineParams::default(), &[])?;
        let rev = analyze_segment(&run, 1, BaselineKind::Derivative, &BaselineParams::default(), &[])?;
        for p in &fwd.peaks {
            println!(
                "  AN apex#{} V={:.4} height={:.3e}A charge={:.3e}C npts={}",
                p.apex_index, p.peak_potential_v, p.peak_height_a, p.charge_c, p.contributions.len()
            );
        }
        for p in &rev.peaks {
            println!(
                "  CA apex#{} V={:.4} height={:.3e}A charge={:.3e}C npts={}",
                p.apex_index, p.peak_potential_v, p.peak_height_a, p.charge_c, p.contributions.len()
            );
        }
    }
    Ok(())
}
