fn main() {
    for (name, csv) in fengyitai::fixtures::FILES {
        let run = fengyitai::import::import_run(csv, name).unwrap();
        println!("== {name}: {} samples, {} segs", run.samples.len(), run.segments.len());
        for (i, seg) in run.segments.iter().enumerate() {
            let pts = &run.samples[seg.start_idx..=seg.end_idx];
            let auto = fengyitai::peaks::suggest_intervals(pts, seg.kind);
            println!("  seg{i} {:?} [{},{}] auto windows: {:?}", seg.kind, seg.start_idx, seg.end_idx,
                auto.iter().map(|w| (w.start_idx+seg.start_idx, w.end_idx+seg.start_idx)).collect::<Vec<_>>());
        }
    }
}
