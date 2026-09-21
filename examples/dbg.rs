fn main() {
    let csv = fengyitai::fixtures::by_name("cycle_100mVs.csv").unwrap();
    let run = fengyitai::import::import_run(csv, "d").unwrap();
    for (si, seg) in run.segments.iter().enumerate() {
        let pts = &run.samples[seg.start_idx..=seg.end_idx];
        let n = pts.len();
        let v: Vec<f64> = pts.iter().map(|p| p.potential_v).collect();
        let y: Vec<f64> = pts.iter().map(|p| p.current_a).collect();
        let k = n/20;
        let med = |a:&[f64]| { let mut z=a.to_vec(); z.sort_by(|x,y| x.partial_cmp(y).unwrap()); z[z.len()/2] };
        let slope = (med(&y[n-k..])-med(&y[..k]))/(med(&v[n-k..])-med(&v[..k]));
        let z: Vec<f64> = y.iter().zip(v.iter()).map(|(yi,vi)| yi-slope*(vi-v[0])).collect();
        let dz: Vec<f64> = (1..n).map(|i| z[i]-z[i-1]).collect();
        // print z extrema positions
        let mut zmax: (isize, f64) = (-1, f64::NEG_INFINITY); let mut zmin: (isize, f64) = (-1, f64::INFINITY);
        for (i,&zv) in z.iter().enumerate() { if zv>zmax.1 {zmax=(i as isize,zv);} if zv<zmin.1 {zmin=(i as isize,zv);} }
        let mut a: Vec<f64> = dz.iter().map(|d| d.abs()).collect();
        a.sort_by(|x,y| x.partial_cmp(y).unwrap());
        println!("seg{si} n={n} slope={slope:.3e} med|dz|={:.3e} max|dz|={:.3e} zmax=({},{:.3e}) zmin=({},{:.3e}) z0={:.3e} zlast={:.3e}",
            a[a.len()/2], a[a.len()-1], zmax.0, zmax.1, zmin.0, zmin.1, z[0], z[n-1]);
        // count sign changes of dz
        let mut sign_changes = 0;
        for w in dz.windows(2) { if w[0]*w[1] < 0.0 {sign_changes+=1;} }
        println!("  dz sign changes: {sign_changes}");
    }

    // 阈值法诊断
    for (si, seg) in run.segments.iter().enumerate() {
        let pts = &run.samples[seg.start_idx..=seg.end_idx];
        let n=pts.len();
        let vv:Vec<f64>=pts.iter().map(|p|p.potential_v).collect();
        let yy:Vec<f64>=pts.iter().map(|p|p.current_a).collect();
        let mut sl:Vec<f64>=(1..n).filter_map(|i|{let dv=vv[i]-vv[i-1]; if dv.abs()>1e-18 {Some((yy[i]-yy[i-1])/dv)} else {None}}).collect();
        sl.sort_by(|a,b|a.partial_cmp(b).unwrap());
        let slope=sl[sl.len()/2];
        let zz:Vec<f64>=yy.iter().zip(vv.iter()).map(|(yi,vi)| yi-slope*(vi-vv[0])).collect();
        let pp:Vec<f64>=if si==0 {zz.clone()} else {zz.iter().map(|x|-x).collect()};
        let pmax=pp.iter().cloned().fold(f64::NEG_INFINITY,f64::max);
        let pmin=pp.iter().cloned().fold(f64::INFINITY,f64::min);
        let above=pp.iter().filter(|x| **x > 0.25*pmax).count();
        println!("DIAG seg{si} slope={slope:.3e} pmin={pmin:.3e} pmax={pmax:.3e} thr={:.3e} above={above}/{n}",0.25*pmax);
    }
}
