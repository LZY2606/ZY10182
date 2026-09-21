use crate::baseline::baseline_for;
use crate::model::{BaselineKind, Direction, PeakMetrics, Point, Segment};

#[derive(Debug, Clone)]
pub struct PeakIntervalInput {
    pub id: i64,
    pub segment_index: usize,
    pub start: usize,
    pub end: usize,
}

fn trapezoid_charge(time: &[f64], signal: &[f64], lo: usize, hi: usize) -> f64 {
    let mut q = 0.0;
    for i in lo..hi {
        let dt = time[i + 1] - time[i];
        q += 0.5 * (signal[i] + signal[i + 1]) * dt;
    }
    q
}

/// Compute peaks for all intervals of one proposal. Overlapping intervals on the
/// same segment are split at the inter-peak valley so no current area is counted
/// twice: effective integration ranges are disjoint.
pub fn compute_peaks(
    points: &[Point],
    segments: &[Segment],
    intervals: &[PeakIntervalInput],
    kind: BaselineKind,
    poly_degree: usize,
) -> Vec<PeakMetrics> {
    let pot: Vec<f64> = points.iter().map(|p| p.potential).collect();
    let cur: Vec<f64> = points.iter().map(|p| p.current).collect();
    let time: Vec<f64> = points.iter().map(|p| p.time).collect();

    struct Raw {
        input: PeakIntervalInput,
        seg: Segment,
        base: Vec<f64>,
        signal: Vec<f64>,
        apex: usize,
        warnings: Vec<String>,
    }

    let mut raws: Vec<Raw> = Vec::new();
    for iv in intervals {
        let Some(seg) = segments.get(iv.segment_index.clamp(0, segments.len())) else {
            continue;
        };
        let lo = iv.start.max(seg.start).min(seg.end);
        let hi = iv.end.min(seg.end).max(lo);
        if hi <= lo {
            continue;
        }
        let (base, mut warnings) =
            baseline_for(kind, poly_degree, &pot, &cur, seg.start, seg.end, lo, hi);
        let signal: Vec<f64> = (lo..=hi).map(|i| cur[i] - base[i - lo]).collect();

        // apex: extremum expected from the scan direction
        let (apex_off, _) = match seg.direction {
            Direction::Anodic => signal
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.partial_cmp(b.1).unwrap()),
            Direction::Cathodic => signal
                .iter()
                .enumerate()
                .min_by(|a, b| a.1.partial_cmp(b.1).unwrap()),
        };
        let apex = lo + apex_off;
        let expect = seg.direction.sign() as f64;
        if signal[apex_off] * expect <= 0.0 {
            warnings.push("peak extrema has opposite sign to scan direction".into());
        }
        raws.push(Raw {
            input: PeakIntervalInput {
                id: iv.id,
                segment_index: iv.segment_index,
                start: lo,
                end: hi,
            },
            seg: seg.clone(),
            base,
            signal,
            apex,
            warnings,
        });
    }

    // Disjoint effective ranges, resolved independently per segment.
    let mut eff: Vec<(usize, usize, bool)> = raws
        .iter()
        .map(|r| (r.input.start, r.input.end, false))
        .collect();
    for seg_index in 0..segments.len() {
        let idx: Vec<usize> = raws
            .iter()
            .enumerate()
            .filter(|(_, r)| r.input.segment_index == seg_index)
            .map(|(i, _)| i)
            .collect();
        if idx.len() < 2 {
            continue;
        }
        let mut order = idx.clone();
        order.sort_by_key(|&i| raws[i].input.start);
        for w in order.windows(2) {
            let (li, ri) = (w[0], w[1]);
            let l = &raws[li];
            let r = &raws[ri];
            if r.input.start > l.input.end {
                continue; // disjoint
            }
            // valley cut between the two apices: smallest |current - mean baseline|
            let (a0, a1) = (l.apex.min(r.apex), l.apex.max(r.apex));
            let cut = (a0..a1)
                .min_by(|&i, &j| {
                    let bi = if i >= l.input.start && i <= l.input.end {
                        cur[i] - l.base[i - l.input.start]
                    } else {
                        cur[i]
                    };
                    let bj = if j >= l.input.start && j <= l.input.end {
                        cur[j] - l.base[j - l.input.start]
                    } else {
                        cur[j]
                    };
                    bi.abs()
                        .partial_cmp(&bj.abs())
                        .unwrap()
                })
                .unwrap_or(a0);
            let cut = cut.max(l.input.start).min(r.input.end);
            eff[li].1 = eff[li].1.min(cut);
            eff[ri].0 = eff[ri].0.max(cut + 1);
            eff[li].2 = true;
            eff[ri].2 = true;
        }
    }

    let mut out = Vec::new();
    for (r, (elo, ehi, overlap)) in raws.iter().zip(eff) {
        let (elo, ehi) = if elo <= ehi { (elo, ehi) } else { (r.apex, r.apex) };
        let apex_off = r.apex - r.input.start;
        let charge = trapezoid_charge(&time, &r.signal, elo, ehi);
        out.push(PeakMetrics {
            interval_id: r.input.id,
            segment_index: r.input.segment_index,
            direction: r.seg.direction,
            peak_idx: r.apex,
            peak_potential: pot[r.apex],
            peak_current: r.signal[apex_off],
            peak_current_raw: cur[r.apex],
            charge,
            interval_start: r.input.start,
            interval_end: r.input.end,
            eff_start: elo,
            eff_end: ehi,
            overlap_adjusted: overlap,
            baseline: kind,
            warnings: r.warnings.clone(),
        });
    }
    out.sort_by_key(|p| p.interval_id);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp_anodic(n: usize, peaks: &[(f64, f64, f64)]) -> (Vec<Point>, Segment) {
        let mut pts = Vec::new();
        for i in 0..n {
            let e = i as f64 / (n - 1) as f64;
            let mut j = 0.0;
            for (mu, amp, w) in peaks {
                j += amp * (-0.5 * ((e - mu) / w).powi(2)).exp();
            }
            pts.push(Point {
                idx: i,
                time: i as f64 * 0.1,
                potential: e,
                current: j,
            });
        }
        (
            pts,
            Segment {
                index: 0,
                direction: Direction::Anodic,
                cycle: 1,
                start: 0,
                end: n - 1,
            },
        )
    }

    #[test]
    fn finds_single_gaussian_peak() {
        let n = 201;
        let (pts, seg) = ramp_anodic(n, &[(0.3, 8e-6, 0.02)]);
        let ivs = vec![PeakIntervalInput {
            id: 1,
            segment_index: 0,
            start: 0,
            end: n - 1,
        }];
        let peaks = compute_peaks(&pts, &[seg], &ivs, BaselineKind::EndpointLinear, 2);
        assert_eq!(peaks.len(), 1);
        assert!((peaks[0].peak_potential - 0.3).abs() < 0.01);
        assert!((peaks[0].peak_current - 8e-6).abs() < 0.5e-6);
        assert!(peaks[0].charge > 0.0);
    }

    #[test]
    fn overlapping_intervals_do_not_double_count() {
        let n = 401;
        let (pts, seg) = ramp_anodic(n, &[(0.3, 8e-6, 0.02), (0.42, 6e-6, 0.025)]);
        let ivs = vec![
            PeakIntervalInput { id: 1, segment_index: 0, start: 20, end: 180 },
            PeakIntervalInput { id: 2, segment_index: 0, start: 100, end: 300 },
        ];
        let peaks = compute_peaks(&pts, &[seg], &ivs, BaselineKind::EndpointLinear, 2);
        assert_eq!(peaks.len(), 2);
        assert!(peaks[0].overlap_adjusted);
        assert!(peaks[1].overlap_adjusted);
        assert!(peaks[0].eff_end < peaks[1].eff_start);
        assert!(peaks[0].eff_start <= peaks[0].peak_idx && peaks[0].peak_idx <= peaks[0].eff_end);
        assert!(peaks[1].eff_start <= peaks[1].peak_idx && peaks[1].peak_idx <= peaks[1].eff_end);
        let union: std::collections::HashSet<usize> = (peaks[0].eff_start..=peaks[0].eff_end)
            .chain(peaks[1].eff_start..=peaks[1].eff_end)
            .collect();
        let total = (peaks[0].eff_end - peaks[0].eff_start + 1)
            + (peaks[1].eff_end - peaks[1].eff_start + 1);
        assert_eq!(union.len(), total);
    }
}
