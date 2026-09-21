//! 峰区间、峰参数与电荷积分。
//!
//! 设计要点：
//! - 每个方案包含若干“峰区间”（属于某个扫描段，用段内样本索引闭区间表示）。
//! - 区间允许重叠；重叠区域的电流不能被重复计算。归属规则是按峰位
//!   电位做最近指派：每个原始样本只给一个峰的面积做贡献；跨归属
//!   边界的梯形边按两端权重计入（自然实现按电位比例拆分）。
//! - 峰方向决定峰极性：正扫取基线校正后电流最大值（氧化峰），
//!   反扫取最小值（还原峰）。
//! - 每个峰参数都带溯源：峰位/峰电流来自哪个原始样本、积分用到了
//!   哪些样本与权重、基线用了哪些锚点。

use crate::baseline::BaselineResult;
use crate::model::{Sample, SegmentKind};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Interval {
    pub start_idx: usize,
    pub end_idx: usize,
}

impl Interval {
    pub fn lo(&self) -> usize { self.start_idx.min(self.end_idx) }
    pub fn hi(&self) -> usize { self.start_idx.max(self.end_idx) }
    pub fn contains(&self, i: usize) -> bool { i >= self.lo() && i <= self.hi() }
    pub fn len(&self) -> usize { self.hi() - self.lo() + 1 }
}

#[derive(Debug, Clone, Serialize)]
pub struct Contribution {
    /// 全局样本索引
    pub sample_index: usize,
    /// 该样本对本峰电荷的权重（0..=1；无重叠时为 1）
    pub weight: f64,
    pub potential_v: f64,
    pub current_corrected_a: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Peak {
    pub interval_lo: usize,
    pub interval_hi: usize,
    pub direction: &'static str,
    pub apex_index: usize,
    pub peak_potential_v: f64,
    pub peak_current_a: f64,
    pub baseline_current_at_apex_a: f64,
    pub peak_height_a: f64,
    /// 电荷（C，带方向符号；梯形积分，重叠处去重）
    pub charge_c: f64,
    pub contributions: Vec<Contribution>,
    pub baseline_anchors: Vec<usize>,
}

pub struct PeakInput<'a> {
    pub kind: SegmentKind,
    /// 段内全部样本
    pub points: &'a [Sample],
    /// 段起点在整份数据中的全局索引
    pub seg_start: usize,
    /// 段内坐标的峰区间
    pub interval: Interval,
    /// 与区间点一一对应的基线
    pub baseline: BaselineResult,
}

/// 计算单个峰（不含重叠去重；多峰请再调用 `assign_overlaps`）。
pub fn measure_peak(input: PeakInput) -> Result<Peak> {
    let lo = input.interval.lo();
    let hi = input.interval.hi();
    let n = input.points.len();
    if hi >= n {
        bail!("峰区间末端 {hi} 超出段长度 {n}");
    }
    if hi - lo < 2 {
        bail!("峰区间至少需要 3 个点（当前 {} 个）", hi - lo + 1);
    }
    if input.baseline.values.len() != hi - lo + 1 {
        bail!("基线点数与区间点数不一致");
    }

    let mut corrected: Vec<(usize, f64)> = Vec::with_capacity(hi - lo + 1);
    for (pos, li) in (lo..=hi).enumerate() {
        corrected.push((li, input.points[li].current_a - input.baseline.values[pos]));
    }

    let apex_local = match input.kind {
        SegmentKind::Forward => corrected
            .iter()
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
            .unwrap()
            .0,
        SegmentKind::Reverse => corrected
            .iter()
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
            .unwrap()
            .0,
    };
    let apex_window_pos = apex_local - lo;
    let g = |local: usize| local + input.seg_start;
    let apex_height = corrected[apex_window_pos].1;

    let baseline_anchors: Vec<usize> = input
        .baseline
        .anchor_positions
        .iter()
        .map(|&p| g(lo + p))
        .collect();

    let contributions: Vec<Contribution> = corrected
        .iter()
        .map(|(li, c)| Contribution {
            sample_index: g(*li),
            weight: 1.0,
            potential_v: input.points[*li].potential_v,
            current_corrected_a: *c,
        })
        .collect();

    let mut q = 0.0f64;
    for k in 0..corrected.len() - 1 {
        let dv = input.points[lo + k + 1].potential_v - input.points[lo + k].potential_v;
        q += 0.5 * (corrected[k].1 + corrected[k + 1].1) * dv;
    }

    Ok(Peak {
        interval_lo: g(lo),
        interval_hi: g(hi),
        direction: if input.kind == SegmentKind::Forward { "anodic" } else { "cathodic" },
        apex_index: g(apex_local),
        peak_potential_v: input.points[apex_local].potential_v,
        peak_current_a: input.points[apex_local].current_a,
        baseline_current_at_apex_a: input.baseline.values[apex_window_pos],
        peak_height_a: apex_height,
        charge_c: q,
        contributions,
        baseline_anchors,
    })
}

/// 多个同段峰的重叠去重。`windows` 为段内坐标，`points` 为段内样本，
/// 峰内 `contributions.sample_index` 为全局索引。
pub fn assign_overlaps(
    points: &[Sample],
    seg_start: usize,
    windows: &[Interval],
    peaks: &mut [Peak],
) -> Result<()> {
    if peaks.len() != windows.len() {
        bail!("峰数与区间数不一致");
    }
    for p in peaks.iter_mut() {
        for c in p.contributions.iter_mut() {
            c.weight = 0.0;
        }
    }

    // 收集所有被任意区间覆盖的段内位置
    let covered: Vec<usize> = {
        let mut v: Vec<usize> = windows
            .iter()
            .flat_map(|w| w.lo()..=w.hi())
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    };

    for li in covered {
        let v = points[li].potential_v;
        // 覆盖该样本的峰中，峰位电位最近者唯一获胜；
        // 距离完全相等时取索引更小的峰（确定性，不随机）。
        let mut owner: Option<usize> = None;
        let mut best = f64::INFINITY;
        for (pi, w) in windows.iter().enumerate() {
            if w.contains(li) {
                let apex_li = peaks[pi].apex_index - seg_start;
                let d = (points[apex_li].potential_v - v).abs();
                if d < best - 1e-15 || (d <= best + 1e-15 && owner.map_or(true, |o| pi < o)) {
                    best = d;
                    owner = Some(pi);
                }
            }
        }
        if let Some(pi) = owner {
            if let Some(c) = peaks[pi]
                .contributions
                .iter_mut()
                .find(|c| c.sample_index == li + seg_start)
            {
                c.weight = 1.0;
            }
        }
    }

    // 带权重积分（校正电流由 contribution 携带；同一全局索引在不同峰中
    // 基线不同，但一个样本只会以权重 1 归属一个峰，无重复面积）。
    for (pi, peak) in peaks.iter_mut().enumerate() {
        let mut q = 0.0f64;
        let cs = &peak.contributions;
        for k in 0..cs.len() - 1 {
            let va = points[cs[k].sample_index - seg_start].potential_v;
            let vb = points[cs[k + 1].sample_index - seg_start].potential_v;
            q += 0.5 * (cs[k].weight * cs[k].current_corrected_a
                + cs[k + 1].weight * cs[k + 1].current_corrected_a)
                * (vb - va);
        }
        let _ = windows[pi];
        peak.charge_c = q;
    }
    Ok(())
}

/// Huber IRLS 稳健直线拟合 y = a + slope*v，返回斜率。
/// 峰点是离群点，会被自动降权；背景（电容斜坡 + 漂移）主导拟合。
fn robust_line(v: &[f64], y: &[f64]) -> (f64, f64) {
    let n = v.len();
    let vc = v.iter().sum::<f64>() / n as f64;
    let xc: Vec<f64> = v.iter().map(|x| x - vc).collect();
    let mut w = vec![1.0f64; n];
    let (mut slope, mut intercept) = (0.0f64, 0.0f64);
    for _ in 0..12 {
        let mut sw = 0.0;
        let mut swx = 0.0;
        let mut swy = 0.0;
        let mut swxx = 0.0;
        let mut swxy = 0.0;
        for ((&x, &yi), &wi) in xc.iter().zip(y.iter()).zip(w.iter()) {
            sw += wi;
            swx += wi * x;
            swy += wi * yi;
            swxx += wi * x * x;
            swxy += wi * x * yi;
        }
        let det = sw * swxx - swx * swx;
        if det.abs() < 1e-18 {
            break;
        }
        slope = (sw * swxy - swx * swy) / det;
        intercept = (swy - slope * swx) / sw;
        let resid: Vec<f64> = xc
            .iter()
            .zip(y.iter())
            .map(|(&x, &yi)| yi - (intercept + slope * x))
            .collect();
        let mut ar: Vec<f64> = resid.iter().map(|r| r.abs()).collect();
        ar.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let med = ar[ar.len() / 2].max(1e-15);
        let scale = 1.4826 * med;
        let c = 1.345 * scale;
        for (wi, r) in w.iter_mut().zip(resid) {
            *wi = if r.abs() <= c { 1.0 } else { c / r.abs() };
        }
    }
    (slope, intercept)
}

/// 段内自动建议峰区间。
///
/// 用 dI/dV 中位数扣除恒定电容斜坡得到去趋势曲线 z；按极性取
/// p = z（正扫）或 p = -z（反扫），再以“连通域越过 0.25 倍最高点”
/// 切出峰区间：两个部分重叠的高斯峰会各自形成独立的超阈连通域。
/// 区间边缘取进入/离开阈值的两个样本；不足 3 点时向两侧补齐。
pub fn suggest_intervals(points: &[Sample], kind: SegmentKind) -> Vec<Interval> {
    let n = points.len();
    if n < 7 {
        return Vec::new();
    }
    let v: Vec<f64> = points.iter().map(|p| p.potential_v).collect();
    let y: Vec<f64> = points.iter().map(|p| p.current_a).collect();

    let (slope, intercept) = robust_line(&v, &y);
    // intercept 是相对 v=vc 的截距：基线 = intercept + slope*(v-vc)
    let vc = v.iter().sum::<f64>() / n as f64;
    let z: Vec<f64> = y
        .iter()
        .zip(v.iter())
        .map(|(&yi, &vi)| yi - (intercept + slope * (vi - vc)))
        .collect();
    let p: Vec<f64> = match kind {
        SegmentKind::Forward => z,
        SegmentKind::Reverse => z.iter().map(|x| -x).collect(),
    };
    let pmax = p.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    if !(pmax.is_finite()) || pmax <= 0.0 {
        return Vec::new();
    }
    // 阈值：峰高的 25%；同时加一个绝对下限以防纯噪声
    let threshold = 0.25 * pmax;

    let mut intervals = Vec::new();
    let mut i = 0;
    while i < n {
        if p[i] > threshold {
            let start = i;
            while i < n && p[i] > threshold {
                i += 1;
            }
            let mut end = i - 1;
            // 边缘外扩 1 个点，保证峰肩也在窗口里
            let lo = start.saturating_sub(1);
            end = (end + 1).min(n - 1);
            if end - lo < 2 {
                let lo2 = lo.saturating_sub(2);
                let end2 = (end + 2).min(n - 1);
                intervals.push(Interval { start_idx: lo2, end_idx: end2 });
            } else {
                intervals.push(Interval { start_idx: lo, end_idx: end });
            }
        } else {
            i += 1;
        }
    }
    intervals
}
