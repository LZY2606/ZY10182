//! 候选峰对：把正扫氧化峰与反扫还原峰组成峰对，枚举双射配对方案，
//! 分数相同的最优方案全部保留（并列候选），绝不随机丢弃。

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct PeakRef {
    /// 峰在其扫描段方案内的序号（稳定标识由调用方拼接 segment/proposal）
    pub id: String,
    pub peak_potential_v: f64,
    pub peak_height_a: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct PairingEdge {
    pub oxidation_id: String,
    pub reduction_id: String,
    pub peak_separation_v: f64,
    pub height_abs_ratio: f64,
    pub edge_cost: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Pairing {
    pub rank: usize,
    pub tied: bool,
    pub total_cost: f64,
    pub edges: Vec<PairingEdge>,
}

/// 配对成本：归一化电位错配 + 归一化峰高错配。
/// `v_scale` 为典型峰间距尺度，`i_scale` 为典型峰高尺度（由全部峰自动估计）。
fn edge_cost(o: &PeakRef, r: &PeakRef, v_scale: f64, i_scale: f64) -> f64 {
    let dv = (o.peak_potential_v - r.peak_potential_v).abs();
    let ho = o.peak_height_a.abs().max(f64::MIN_POSITIVE);
    let hr = r.peak_height_a.abs().max(f64::MIN_POSITIVE);
    let hratio = ho / hr;
    // log 比在 1 附近对称：|ln(ratio)|
    let hmiss = hratio.ln().abs();
    let dv_n = if v_scale > 0.0 { dv / v_scale } else { dv };
    let _ = i_scale;
    dv_n + hmiss
}

fn scales(ox: &[PeakRef], red: &[PeakRef]) -> (f64, f64) {
    let mut vs: Vec<f64> = Vec::new();
    let mut hs: Vec<f64> = Vec::new();
    for p in ox.iter().chain(red.iter()) {
        vs.push(p.peak_potential_v);
        hs.push(p.peak_height_a.abs());
    }
    let v_scale = if vs.len() >= 2 {
        let lo = vs.iter().cloned().fold(f64::INFINITY, f64::min);
        let hi = vs.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        (hi - lo).max(1e-9)
    } else {
        1.0
    };
    let i_scale = {
        let med = median(&hs);
        med.max(1e-12)
    };
    (v_scale, i_scale)
}

fn median(v: &[f64]) -> f64 {
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if s.is_empty() {
        return 0.0;
    }
    s[s.len() / 2]
}

/// 枚举所有双射配对（n<=4 时 24 种以内，可接受）；多余一侧的峰留为未配对。
/// 返回按成本升序的全部方案；调用方可只保留与最优并列的方案。
pub fn pair_candidates(ox: &[PeakRef], red: &[PeakRef]) -> Vec<Pairing> {
    if ox.is_empty() || red.is_empty() {
        return Vec::new();
    }
    let (v_scale, i_scale) = scales(ox, red);

    // 以较少的一侧为“匹配方”，枚举到较多一侧的不重复选择
    let (small, large, small_is_ox) = if ox.len() <= red.len() {
        (ox, red, true)
    } else {
        (red, ox, false)
    };

    let mut assignments: Vec<Vec<usize>> = Vec::new();
    let mut used = vec![false; large.len()];
    let mut cur = Vec::new();
    fn dfs2(
        depth: usize,
        total: usize,
        large_len: usize,
        used: &mut [bool],
        cur: &mut Vec<usize>,
        out: &mut Vec<Vec<usize>>,
    ) {
        if depth == total {
            out.push(cur.clone());
            return;
        }
        for j in 0..large_len {
            if !used[j] {
                used[j] = true;
                cur.push(j);
                dfs2(depth + 1, total, large_len, used, cur, out);
                cur.pop();
                used[j] = false;
            }
        }
    }
    dfs2(0, small.len(), large.len(), &mut used, &mut cur, &mut assignments);

    let mut scored: Vec<(f64, Vec<PairingEdge>)> = assignments
        .into_iter()
        .map(|chosen| {
            let mut edges = Vec::with_capacity(chosen.len());
            let mut total = 0.0;
            for (si, lj) in chosen.into_iter().enumerate() {
                let (o, r) = if small_is_ox {
                    (&small[si], &large[lj])
                } else {
                    (&large[lj], &small[si])
                };
                let cost = edge_cost(o, r, v_scale, i_scale);
                total += cost;
                edges.push(PairingEdge {
                    oxidation_id: o.id.clone(),
                    reduction_id: r.id.clone(),
                    peak_separation_v: (o.peak_potential_v - r.peak_potential_v).abs(),
                    height_abs_ratio: o.peak_height_a.abs() / r.peak_height_a.abs().max(f64::MIN_POSITIVE),
                    edge_cost: cost,
                });
            }
            (total, edges)
        })
        .collect();

    scored.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let best = scored.first().map(|(c, _)| *c).unwrap_or(0.0);
    const TIE_EPS: f64 = 1e-9;
    let mut winners: Vec<(f64, Vec<PairingEdge>)> = scored
        .into_iter()
        .take_while(|(c, _)| (*c - best).abs() <= TIE_EPS)
        .collect();
    let tied = winners.len() > 1;
    winners
        .drain(..)
        .enumerate()
        .map(|(i, (cost, edges))| Pairing {
            rank: i + 1,
            tied,
            total_cost: cost,
            edges,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(id: &str, v: f64, h: f64) -> PeakRef {
        PeakRef { id: id.to_string(), peak_potential_v: v, peak_height_a: h }
    }

    #[test]
    fn unique_when_separations_differ() {
        let ox = vec![p("o1", 0.25, 1.0), p("o2", 0.45, 1.0)];
        let red = vec![p("r1", 0.20, 1.0), p("r2", 0.40, 1.0)];
        let wins = pair_candidates(&ox, &red);
        assert!(!wins.is_empty());
        assert_eq!(wins[0].tied, false);
        // 唯一最优：o1-r1, o2-r2
        let e0 = &wins[0].edges[0];
        assert_eq!(e0.oxidation_id, "o1");
        assert_eq!(e0.reduction_id, "r1");
    }

    #[test]
    fn ties_are_all_retained() {
        // 几何对称：ox={c-a,c+a}, red={c-b,c+b}，
        // 恒等与交叉两种配法的 |ΔE| 总和同为 2|b-a|，必须并列保留
        let ox = vec![p("o1", 0.30, 1.0), p("o2", 0.40, 1.0)];
        let red = vec![p("r1", 0.60, 1.0), p("r2", 0.70, 1.0)];
        let wins = pair_candidates(&ox, &red);
        assert_eq!(wins.len(), 2, "对称情形应有两个并列最优配法");
        assert!(wins.iter().all(|w| w.tied));
        let sets: Vec<std::collections::HashSet<(String, String)>> = wins
            .iter()
            .map(|w| {
                w.edges
                    .iter()
                    .map(|e| (e.oxidation_id.clone(), e.reduction_id.clone()))
                    .collect()
            })
            .collect();
        assert_ne!(sets[0], sets[1]);
    }

    #[test]
    fn unmatched_extra_peak_is_dropped_not_forced() {
        let ox = vec![p("o1", 0.3, 1.0)];
        let red = vec![p("r1", 0.3, 1.0), p("r2", 0.9, 1.0)];
        let wins = pair_candidates(&ox, &red);
        assert_eq!(wins[0].edges.len(), 1);
    }
}
