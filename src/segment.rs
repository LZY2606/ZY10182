use crate::model::{Direction, Segment};

/// Segment a potential trace into monotonic scan segments by the sign of dE.
///
/// Rules:
/// * Constant-potential plateaus (|dE| <= eps) never create spurious segments:
///   a plateau inherits the direction of the scan that reaches it.
/// * A plateau sitting at a turning point is taken by the segment ending there,
///   so the reversal point belongs to exactly one segment. Segments never share
///   point indices.
///
/// Segment point ranges are inclusive and disjoint. Extending a reversal plateau
/// by one sample therefore only shifts the boundary by one point; the segment
/// count, directions and ordering do not jitter.
pub fn segment_potential(potential: &[f64], eps: f64) -> Vec<Segment> {
    let n = potential.len();
    if n < 2 {
        return Vec::new();
    }

    let mut signs = vec![0i8; n - 1];
    for (i, w) in potential.windows(2).enumerate() {
        let d = w[1] - w[0];
        signs[i] = if d > eps {
            1
        } else if d < -eps {
            -1
        } else {
            0
        };
    }

    // Back-fill a leading plateau with the first real motion.
    if let Some(first) = signs.iter().position(|s| *s != 0) {
        for s in signs.iter_mut().take(first) {
            *s = signs[first];
        }
    }
    // Forward-fill every other plateau: the plateau continues the current scan.
    let mut prev = 0i8;
    for s in signs.iter_mut() {
        if *s == 0 {
            *s = prev;
        } else {
            prev = *s;
        }
    }

    // Maximal runs of equal sign over steps. A run of steps s..=e covers
    // raw points s..=e+1.
    let mut runs: Vec<(i8, usize, usize)> = Vec::new();
    let mut start = 0usize;
    let mut cur = signs[0];
    for (i, &s) in signs.iter().enumerate().skip(1) {
        if s != cur {
            runs.push((cur, start, i - 1));
            start = i;
            cur = s;
        }
    }
    runs.push((cur, start, signs.len() - 1));

    let mut out = Vec::new();
    let mut anodic_count = 0usize;
    for (k, (sign, step_start, step_end)) in runs.into_iter().enumerate() {
        if sign == 0 {
            continue; // fully flat trace
        }
        let direction = if sign > 0 {
            Direction::Anodic
        } else {
            Direction::Cathodic
        };
        if direction == Direction::Anodic {
            anodic_count += 1;
        }
        // First segment owns its first point; later segments start one point
        // after the previous end, so the reversal point is owned by one segment.
        let point_start = if k == 0 { step_start } else { step_start + 1 };
        let point_end = step_end + 1;
        if point_start <= point_end {
            out.push(Segment {
                index: out.len(),
                direction,
                cycle: anodic_count.max(1),
                start: point_start,
                end: point_end,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs(segs: &[Segment]) -> Vec<Direction> {
        segs.iter().map(|s| s.direction).collect()
    }

    #[test]
    fn splits_at_reversal() {
        let p = [0.0, 1.0, 2.0, 2.0, 1.0, 0.0];
        let s = segment_potential(&p, 1e-12);
        assert_eq!(s.len(), 2);
        assert_eq!(dirs(&s), vec![Direction::Anodic, Direction::Cathodic]);
        // reversal point (index 3) belongs to the anodic segment only
        assert_eq!(s[0].start, 0);
        assert_eq!(s[0].end, 3);
        assert_eq!(s[1].start, 4);
        assert_eq!(s[1].end, 5);
    }

    #[test]
    fn plateau_does_not_split_scan() {
        let mut p = vec![0.0, 1.0, 2.0];
        p.extend(std::iter::repeat(2.0).take(5));
        p.push(3.0);
        let s = segment_potential(&p, 1e-12);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].start, 0);
        assert_eq!(s[0].end, p.len() - 1);
    }

    #[test]
    fn leading_and_trailing_plateaus() {
        let p = [1.0, 1.0, 1.0, 2.0, 3.0, 3.0, 3.0];
        let s = segment_potential(&p, 1e-12);
        assert_eq!(s.len(), 1);
        assert_eq!((s[0].start, s[0].end), (0, 6));
    }

    #[test]
    fn extending_reversal_plateau_does_not_jitter() {
        let short = [
            0.0, 1.0, 2.0, 3.0, 3.0, 3.0, 2.0, 1.0, 0.0,
        ];
        let mut long = short.to_vec();
        long.insert(5, 3.0); // one more plateau sample before turning
        let a = segment_potential(&short, 1e-12);
        let b = segment_potential(&long, 1e-12);
        assert_eq!(a.len(), b.len());
        assert_eq!(dirs(&a), dirs(&b));
        // potential extent of each segment is unchanged
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(short[x.start], long[y.start]);
            assert_eq!(short[x.end], long[y.end]);
            assert_eq!(x.direction, y.direction);
        }
    }

    #[test]
    fn full_cycle_with_plateaus() {
        let mut p: Vec<f64> = Vec::new();
        p.extend(std::iter::repeat(0.0).take(3));
        (1..=10).for_each(|i| p.push(i as f64));
        p.extend(std::iter::repeat(10.0).take(3));
        (0..10).rev().for_each(|i| p.push(i as f64));
        p.extend(std::iter::repeat(0.0).take(3));
        let s = segment_potential(&p, 1e-12);
        assert_eq!(s.len(), 2);
        assert_eq!(dirs(&s), vec![Direction::Anodic, Direction::Cathodic]);
        // disjoint ownership covers every point
        assert_eq!(s[0].start, 0);
        assert_eq!(s[1].end, p.len() - 1);
        assert_eq!(s[0].end + 1, s[1].start);
    }
}
