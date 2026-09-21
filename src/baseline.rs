use crate::model::BaselineKind;

/// Least-squares polynomial y = sum c_j x^j of given degree.
pub fn polyfit(x: &[f64], y: &[f64], degree: usize) -> Option<Vec<f64>> {
    if x.len() != y.len() || x.is_empty() || degree >= x.len() {
        return None;
    }
    let m = degree + 1;
    let mut a = vec![0.0; m * m];
    let mut b = vec![0.0; m];
    for (xi, yi) in x.iter().zip(y) {
        let mut xp = 1.0;
        let mut powers = Vec::with_capacity(2 * m - 1);
        for _ in 0..(2 * m - 1) {
            powers.push(xp);
            xp *= xi;
        }
        for r in 0..m {
            for c in 0..m {
                a[r * m + c] += powers[r + c];
            }
            b[r] += powers[r] * yi;
        }
    }
    solve_matrix(&mut a, &mut b, m).then_some(b)
}

fn solve_matrix(a: &mut [f64], b: &mut [f64], m: usize) -> bool {
    for col in 0..m {
        let pivot = (col..m)
            .max_by(|i, j| {
                a[i * m + col]
                    .abs()
                    .partial_cmp(&a[j * m + col].abs())
                    .unwrap()
            })
            .unwrap_or(col);
        if a[pivot * m + col].abs() < 1e-14 {
            return false;
        }
        if pivot != col {
            for c in 0..m {
                a.swap(col * m + c, pivot * m + c);
            }
            b.swap(col, pivot);
        }
        let piv = a[col * m + col];
        for r in (col + 1)..m {
            let factor = a[r * m + col] / piv;
            if factor != 0.0 {
                for c in col..m {
                    a[r * m + c] -= factor * a[col * m + c];
                }
                b[r] -= factor * b[col];
            }
        }
    }
    for r in (0..m).rev() {
        let mut v = b[r];
        for c in (r + 1)..m {
            v -= a[r * m + c] * b[c];
        }
        let piv = a[r * m + r];
        if piv.abs() < 1e-14 {
            return false;
        }
        b[r] = v / piv;
    }
    true
}

#[allow(clippy::ptr_arg)]
pub fn eval_poly(c: &Vec<f64>, x: f64) -> f64 {
    let mut acc = 0.0;
    for v in c.iter().rev() {
        acc = acc * x + v;
    }
    acc
}

fn endpoint_line(pot: &[f64], cur: &[f64], lo: usize, hi: usize, out: &mut [f64], out_off: usize) {
    let span = pot[hi] - pot[lo];
    for i in lo..=hi {
        let f = if span.abs() > 1e-15 {
            (pot[i] - pot[lo]) / span
        } else {
            (i - lo) as f64 / (hi - lo).max(1) as f64
        };
        out[i - out_off] = cur[lo] + f * (cur[hi] - cur[lo]);
    }
}

/// Baseline current for every point in [lo, hi]. Returns baseline slice values
/// indexed from lo (`out[i - lo]`) plus warnings.
pub fn baseline_for(
    kind: BaselineKind,
    poly_degree: usize,
    pot: &[f64],
    cur: &[f64],
    seg_lo: usize,
    seg_hi: usize,
    lo: usize,
    hi: usize,
) -> (Vec<f64>, Vec<String>) {
    let mut warnings = Vec::new();
    let mut base = vec![0.0; hi - lo + 1];
    match kind {
        BaselineKind::EndpointLinear => {
            endpoint_line(pot, cur, lo, hi, &mut base, lo);
        }
        BaselineKind::LocalPoly => {
            let wing: Vec<(f64, f64)> = (seg_lo..=seg_hi)
                .filter(|&i| i < lo || i > hi)
                .map(|i| (pot[i], cur[i]))
                .collect();
            let degree = poly_degree.clamp(0, 3);
            let xs: Vec<f64> = wing.iter().map(|p| p.0).collect();
            let ys: Vec<f64> = wing.iter().map(|p| p.1).collect();
            match polyfit(&xs, &ys, degree.min(xs.len().saturating_sub(1))) {
                Some(c) => {
                    for i in lo..=hi {
                        base[i - lo] = eval_poly(&c, pot[i]);
                    }
                }
                None => {
                    warnings.push(format!(
                        "local-poly needs >= {} wing points; fell back to endpoint linear",
                        degree + 2
                    ));
                    endpoint_line(pot, cur, lo, hi, &mut base, lo);
                }
            }
        }
        BaselineKind::Derivative => {
            // Anchors: flattest points inside the interval (lowest |dI/dE|).
            // A line through the first / last such anchor approximates the
            // derivative-selected baseline.
            let mut slopes: Vec<(usize, f64)> = (lo..hi)
                .map(|i| {
                    let de = pot[i + 1] - pot[i];
                    let s = if de.abs() > 1e-15 {
                        (cur[i + 1] - cur[i]) / de
                    } else {
                        0.0
                    };
                    (i, s.abs())
                })
                .collect();
            if slopes.len() < 2 {
                warnings.push("derivative baseline: interval too small; endpoint linear used".into());
                endpoint_line(pot, cur, lo, hi, &mut base, lo);
            } else {
                slopes.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
                let q = slopes.len() / 4 + 1;
                let mut anchors: Vec<usize> = slopes.iter().take(q).map(|p| p.0).collect();
                anchors.sort_unstable();
                let a0 = *anchors.first().unwrap();
                let a1 = (*anchors.last().unwrap() + 1).min(hi);
                if a1 <= a0 {
                    endpoint_line(pot, cur, lo, hi, &mut base, lo);
                } else {
                    endpoint_line(pot, cur, a0, a1, &mut base, lo);
                    for i in lo..a0 {
                        base[i - lo] = base[a0 - lo];
                    }
                    for i in (a1 + 1)..=hi {
                        base[i - lo] = base[a1 - lo];
                    }
                }
            }
        }
    }
    (base, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polyfit_recovers_quadratic() {
        let xs: Vec<f64> = (0..10).map(|i| i as f64).collect();
        let ys: Vec<f64> = xs.iter().map(|x| 2.0 * x * x - 3.0 * x + 5.0).collect();
        let c = polyfit(&xs, &ys, 2).unwrap();
        assert!((c[0] - 5.0).abs() < 1e-6);
        assert!((c[1] + 3.0).abs() < 1e-6);
        assert!((c[2] - 2.0).abs() < 1e-6);
    }

    #[test]
    fn endpoint_line_on_linear_background() {
        let pot: Vec<f64> = (0..11).map(|i| i as f64 * 0.01).collect();
        let cur: Vec<f64> = pot.iter().map(|e| 1e-6 + 10e-6 * e).collect();
        let (b, w) = baseline_for(
            BaselineKind::EndpointLinear,
            2,
            &pot,
            &cur,
            0,
            10,
            2,
            8,
        );
        assert!(w.is_empty());
        for i in 2..=8 {
            assert!((b[i - 2] - cur[i]).abs() < 1e-12);
        }
    }

    #[test]
    fn local_poly_uses_wings() {
        let pot: Vec<f64> = (0..21).map(|i| i as f64 * 0.01).collect();
        let cur: Vec<f64> = pot
            .iter()
            .enumerate()
            .map(|(i, e)| 2.0 * e * e + if (8..=12).contains(&i) { 1.0 } else { 0.0 })
            .collect();
        let (b, w) = baseline_for(BaselineKind::LocalPoly, 2, &pot, &cur, 0, 20, 8, 12);
        assert!(w.is_empty());
        for i in 8..=12 {
            assert!((b[i - 8] - 2.0 * pot[i] * pot[i]).abs() < 1e-6);
        }
    }

    #[test]
    fn local_poly_fallback_warns() {
        let pot = [0.0, 0.01, 0.02, 0.03];
        let cur = [0.0, 1e-6, 2e-6, 3e-6];
        let (_, w) = baseline_for(BaselineKind::LocalPoly, 2, &pot, &cur, 0, 3, 1, 2);
        assert!(w.iter().any(|m| m.contains("fell back")));
    }
}
