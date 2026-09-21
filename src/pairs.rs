use crate::model::{PeakMetrics, PeakPair};

/// Score: smaller is better. Combines peak separation and the deviation of
/// current/charge ratios from 1. Deterministic formula so identical candidates
/// produce bit-identical scores and stay tied.
pub fn pair_score(a: &PeakMetrics, c: &PeakMetrics) -> f64 {
    let delta = (a.peak_potential - c.peak_potential).abs();
    let ir = a.peak_current.abs() / c.peak_current.abs().max(1e-30);
    let qr = a.charge.abs() / c.charge.abs().max(1e-30);
    delta + 0.05 * (ir.ln().abs() + qr.ln().abs())
}

/// Build every anodic x cathodic candidate, sorted by score. Equal scores share
/// a rank and are all retained (`tied = true`); no tied candidate is dropped.
pub fn build_pairs(peaks: &[PeakMetrics]) -> Vec<PeakPair> {
    use crate::model::Direction;
    let anodic: Vec<&PeakMetrics> = peaks.iter().filter(|p| p.direction == Direction::Anodic).collect();
    let cathodic: Vec<&PeakMetrics> = peaks
        .iter()
        .filter(|p| p.direction == Direction::Cathodic)
        .collect();

    let mut raw: Vec<PeakPair> = anodic
        .iter()
        .flat_map(|a| cathodic.iter().map(move |c| (**a, **c)))
        .map(|(a, c)| {
            let score = pair_score(&a, &c);
            PeakPair {
                delta_ep: a.peak_potential - c.peak_potential,
                current_ratio: a.peak_current / c.peak_current,
                charge_ratio: a.charge / c.charge,
                score,
                rank: 0,
                tied: false,
                anodic: a,
                cathodic: c,
            }
        })
        .collect();
    raw.sort_by(|x, y| {
        x.score
            .partial_cmp(&y.score)
            .unwrap()
            .then_with(|| x.anodic.interval_id.cmp(&y.anodic.interval_id))
            .then_with(|| x.cathodic.interval_id.cmp(&y.cathodic.interval_id))
    });

    // score multiplicity decides the tied flag
    let mut rank = 0usize;
    let mut i = 0;
    while i < raw.len() {
        let mut j = i + 1;
        while j < raw.len() && same_score(raw[j].score, raw[i].score) {
            j += 1;
        }
        rank += 1;
        let group_tied = j - i > 1;
        for p in raw[i..j].iter_mut() {
            p.rank = rank;
            p.tied = group_tied;
        }
        i = j;
    }
    raw
}

fn same_score(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-12_f64.max(1e-9 * a.abs())
}

/// Keep the top `limit` rank groups; ties at the cutoff are all retained.
pub fn top_groups(pairs: Vec<PeakPair>, limit: usize) -> Vec<PeakPair> {
    if limit == 0 {
        return pairs;
    }
    pairs.into_iter().filter(|p| p.rank <= limit).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{BaselineKind, Direction, PeakMetrics};

    fn mk(id: i64, dir: Direction, ep: f64, ip: f64, q: f64) -> PeakMetrics {
        PeakMetrics {
            interval_id: id,
            segment_index: 0,
            direction: dir,
            peak_idx: 0,
            peak_potential: ep,
            peak_current: ip,
            peak_current_raw: ip,
            charge: q,
            interval_start: 0,
            interval_end: 1,
            eff_start: 0,
            eff_end: 1,
            overlap_adjusted: false,
            baseline: BaselineKind::EndpointLinear,
            warnings: vec![],
        }
    }

    #[test]
    fn equal_scores_are_tied_and_retained() {
        let peaks = vec![
            mk(1, Direction::Anodic, 0.3, 8e-6, 1e-6),
            mk(2, Direction::Cathodic, 0.28, -8e-6, -1e-6),
            mk(3, Direction::Cathodic, 0.28, -8e-6, -1e-6),
        ];
        let pairs = build_pairs(&peaks);
        assert_eq!(pairs.len(), 2);
        assert!(pairs.iter().all(|p| p.tied));
        assert!(pairs.iter().all(|p| p.rank == 1));
        let kept = top_groups(pairs.clone(), 1);
        assert_eq!(kept.len(), 2, "both tied candidates must survive cutoff");
    }

    #[test]
    fn distinct_scores_get_distinct_ranks() {
        let peaks = vec![
            mk(1, Direction::Anodic, 0.3, 8e-6, 1e-6),
            mk(2, Direction::Cathodic, 0.29, -8e-6, -1e-6),
            mk(3, Direction::Cathodic, 0.10, -8e-6, -1e-6),
        ];
        let pairs = build_pairs(&peaks);
        assert_eq!(pairs.len(), 2);
        assert!(!pairs[0].tied && !pairs[1].tied);
        assert_eq!(pairs[0].rank, 1);
        assert_eq!(pairs[1].rank, 2);
    }
}
