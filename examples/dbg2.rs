use fengyitai::baseline::{evaluate, BaselineKind, BaselineParams};
use fengyitai::import::import_run;
use fengyitai::peaks::suggest_intervals;

fn main() -> anyhow::Result<()> {
    let csv = fengyitai::fixtures::by_name("cycle_100mVs.csv").unwrap();
    let run = import_run(csv, "d")?;
    let seg = &run.segments[0];
    let pts = &run.samples[seg.start_idx..=seg.end_idx];
    let wins = suggest_intervals(pts, fengyitai::model::SegmentKind::Forward);
    for (wi, w) in wins.iter().enumerate() {
        let wp = &pts[w.lo()..=w.hi()];
        let bl = evaluate(BaselineKind::Derivative, &BaselineParams::default(), wp)?;
        let apex = wp.iter().enumerate().max_by(|a,b| (a.1.current_a-bl.values[a.0]).partial_cmp(&(b.1.current_a-bl.values[b.0])).unwrap()).unwrap().0;
        println!("win{wi} [lo..hi]={}..{} V {:.3}..{:.3}", w.lo(), w.hi(), wp.first().unwrap().potential_v, wp.last().unwrap().potential_v);
        println!("  edge currents: {:.3e} {:.3e}; baseline edge: {:.3e} {:.3e}",
            wp.first().unwrap().current_a, wp.last().unwrap().current_a, bl.values.first().unwrap(), bl.values.last().unwrap());
        println!("  apex V={:.3} Iraw={:.3e} Ibl={:.3e} corrected={:.3e}",
            wp[apex].potential_v, wp[apex].current_a, bl.values[apex], wp[apex].current_a-bl.values[apex]);
        // min raw current in window
        let imin = wp.iter().map(|p| p.current_a).fold(f64::INFINITY, f64::min);
        println!("  Imin(window)={imin:.3e}");
    }
    Ok(())
}
