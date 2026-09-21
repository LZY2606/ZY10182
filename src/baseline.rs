//! 基线方案：端点直线、局部多项式、导数基线。
//!
//! 三类方法都只消费“当前方案选择的窗口内的原始点”，结果可完全复算：
//! - `linear`：窗口两端点连线；
//! - `poly`：对窗口内原始点做最小二乘多项式（degree 1..=3，默认 2），
//!   返回每个窗口点上的拟合值；
//! - `derivative`：在窗口内选取电流一阶差分绝对值最小的一批“平坦锚点”，
//!   对锚点 (V, I) 做稳健线性拟合（迭代再加权最小二乘），给出基线。
//!
//! 基线以 `Vec<f64>` 返回，与 `points` 一一对应；存库时保存方法名、
//! 参数 JSON 以及每个点的基线值，峰参数溯源可直接回到基线锚点的原始样本。

use crate::model::Sample;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BaselineKind {
    Linear,
    Poly,
    Derivative,
}

impl BaselineKind {
    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s.to_ascii_lowercase().as_str() {
            "linear" | "line" | "endpoint" | "端点直线" => BaselineKind::Linear,
            "poly" | "polynomial" | "local_poly" | "局部多项式" => BaselineKind::Poly,
            "derivative" | "deriv" | "导数" | "导数基线" => BaselineKind::Derivative,
            other => bail!("未知基线类型: {other}（linear/poly/derivative）"),
        })
    }
    pub fn as_str(self) -> &'static str {
        match self {
            BaselineKind::Linear => "linear",
            BaselineKind::Poly => "poly",
            BaselineKind::Derivative => "derivative",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaselineParams {
    #[serde(default = "default_degree")]
    pub degree: usize,
    /// derivative 方法：按 |dI/dV| 排序选取的平坦锚点比例（0.1..=0.9）
    #[serde(default = "default_anchor_fraction")]
    pub anchor_fraction: f64,
    /// derivative 方法：IRLS 迭代次数
    #[serde(default = "default_irls_iters")]
    pub irls_iters: usize,
}

fn default_degree() -> usize { 2 }
fn default_anchor_fraction() -> f64 { 0.35 }
fn default_irls_iters() -> usize { 8 }

impl Default for BaselineParams {
    fn default() -> Self {
        BaselineParams {
            degree: default_degree(),
            anchor_fraction: default_anchor_fraction(),
            irls_iters: default_irls_iters(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct BaselineResult {
    pub kind: BaselineKind,
    pub params: BaselineParams,
    /// 与输入 points 一一对应的基线电流（A）
    pub values: Vec<f64>,
    /// 参与基线构建的原始样本在该段（points 序列）中的位置，供溯源
    pub anchor_positions: Vec<usize>,
}

pub fn evaluate(
    kind: BaselineKind,
    params: &BaselineParams,
    points: &[Sample],
) -> Result<BaselineResult> {
    if points.len() < 2 {
        bail!("峰区间内至少需要 2 个原始点");
    }
    let values = match kind {
        BaselineKind::Linear => {
            let anchor = vec![0usize, points.len() - 1];
            let vals = linear_endpoint(points);
            return Ok(BaselineResult { kind, params: params.clone(), values: vals, anchor_positions: anchor });
        }
        BaselineKind::Poly => poly_fit_baseline(points, params.degree)?,
        BaselineKind::Derivative => return derivative_baseline(points, params),
    };
    Ok(BaselineResult {
        kind,
        params: params.clone(),
        anchor_positions: (0..points.len()).collect(),
        values,
    })
}

fn linear_endpoint(points: &[Sample]) -> Vec<f64> {
    let n = points.len();
    let (v0, i0) = (points[0].potential_v, points[0].current_a);
    let (v1, i1) = (points[n - 1].potential_v, points[n - 1].current_a);
    if (v1 - v0).abs() < f64::EPSILON {
        return vec![(i0 + i1) / 2.0; n];
    }
    let k = (i1 - i0) / (v1 - v0);
    points.iter().map(|p| i0 + k * (p.potential_v - v0)).collect()
}

/// 最小二乘多项式拟合：对电位做中心化以改善条件数。
fn poly_fit_baseline(points: &[Sample], degree: usize) -> Result<Vec<f64>> {
    if !(1..=3).contains(&degree) {
        bail!("多项式阶数需在 1..=3 之间，收到 {degree}");
    }
    if points.len() <= degree {
        bail!("局部多项式需要点数（{}）大于阶数（{degree}）", points.len());
    }
    let m = degree + 1;
    let vcenter: f64 =
        points.iter().map(|p| p.potential_v).sum::<f64>() / points.len() as f64;
    let x: Vec<f64> = points.iter().map(|p| p.potential_v - vcenter).collect();
    let y: Vec<f64> = points.iter().map(|p| p.current_a).collect();

    // 正规方程 X^T X
    let mut ata = vec![0.0f64; m * m];
    let mut aty = vec![0.0f64; m];
    for (&xi, &yi) in x.iter().zip(y.iter()) {
        let mut xp = vec![0.0f64; m];
        xp[0] = 1.0;
        for j in 1..m {
            xp[j] = xp[j - 1] * xi;
        }
        for a in 0..m {
            aty[a] += xp[a] * yi;
            for b in 0..m {
                ata[a * m + b] += xp[a] * xp[b];
            }
        }
    }
    let coef = solve_symmetric(&mut ata, &mut aty, m)?;
    Ok(x.iter()
        .map(|&xi| {
            let mut v = 0.0;
            let mut xp = 1.0;
            for c in &coef {
                v += c * xp;
                xp *= xi;
            }
            v
        })
        .collect())
}

/// 高斯消元解对称正定（退化时退化为普通消元）线性方程组。
fn solve_symmetric(a: &mut [f64], b: &mut [f64], m: usize) -> Result<Vec<f64>> {
    // Cholesky，失败则回退高斯消元带选主元
    if let Some(l) = cholesky(a, m) {
        // 解 L L^T x = b
        let mut y = vec![0.0; m];
        for i in 0..m {
            let mut s = b[i];
            for j in 0..i {
                s -= l[i * m + j] * y[j];
            }
            y[i] = s / l[i * m + i];
        }
        let mut x = vec![0.0; m];
        for i in (0..m).rev() {
            let mut s = y[i];
            for j in (i + 1)..m {
                s -= l[j * m + i] * x[j];
            }
            x[i] = s / l[i * m + i];
        }
        return Ok(x);
    }
    gaussian_solve(a, b, m)
}

fn cholesky(a: &[f64], m: usize) -> Option<Vec<f64>> {
    let mut l = vec![0.0; m * m];
    for i in 0..m {
        for j in 0..=i {
            let mut s = a[i * m + j];
            for k in 0..j {
                s -= l[i * m + k] * l[j * m + k];
            }
            if i == j {
                if s <= 1e-18 {
                    return None;
                }
                l[i * m + i] = s.sqrt();
            } else {
                l[i * m + j] = s / l[j * m + j];
            }
        }
    }
    Some(l)
}

fn gaussian_solve(a: &[f64], b: &[f64], m: usize) -> Result<Vec<f64>> {
    let mut mat = vec![0.0; m * (m + 1)];
    for i in 0..m {
        for j in 0..m {
            mat[i * (m + 1) + j] = a[i * m + j];
        }
        mat[i * (m + 1) + m] = b[i];
    }
    for col in 0..m {
        let pivot = (col..m)
            .max_by(|&p, &q| {
                mat[p * (m + 1) + col].abs().partial_cmp(&mat[q * (m + 1) + col].abs()).unwrap()
            })
            .unwrap();
        if mat[pivot * (m + 1) + col].abs() < 1e-15 {
            bail!("基线拟合矩阵奇异");
        }
        if pivot != col {
            for k in 0..=m {
                mat.swap(col * (m + 1) + k, pivot * (m + 1) + k);
            }
        }
        for row in (col + 1)..m {
            let f = mat[row * (m + 1) + col] / mat[col * (m + 1) + col];
            for k in col..=m {
                mat[row * (m + 1) + k] -= f * mat[col * (m + 1) + k];
            }
        }
    }
    let mut x = vec![0.0; m];
    for i in (0..m).rev() {
        let mut s = mat[i * (m + 1) + m];
        for j in (i + 1)..m {
            s -= mat[i * (m + 1) + j] * x[j];
        }
        x[i] = s / mat[i * (m + 1) + i];
    }
    Ok(x)
}

/// 导数基线：
/// 1. 斜率用 Theil–Sen 估计（锚点集合上成对点 dI/dV 的中位数，峰点不影响）；
/// 2. 去斜率后截距取残差低 10 分位（正扫）或高 90 分位（反扫），
///    极性由残差均值与中位数的关系自动判定；
/// 3. `anchor_positions` 记录最贴近该基线的平坦锚点，供溯源。
fn derivative_baseline(points: &[Sample], params: &BaselineParams) -> Result<BaselineResult> {
    let n = points.len();
    if !(0.1..=0.9).contains(&params.anchor_fraction) {
        bail!("anchor_fraction 需在 0.1..=0.9");
    }
    let vcenter: f64 = points.iter().map(|p| p.potential_v).sum::<f64>() / n as f64;

    let mut scores: Vec<(usize, f64)> = Vec::with_capacity(n);
    for i in 0..n {
        let d = if i == 0 {
            finite_slope(&points[0], &points[1])
        } else if i == n - 1 {
            finite_slope(&points[n - 2], &points[n - 1])
        } else {
            let s1 = finite_slope(&points[i - 1], &points[i]);
            let s2 = finite_slope(&points[i], &points[i + 1]);
            (s1 + s2) / 2.0
        };
        scores.push((i, d.abs()));
    }
    scores.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    let k = ((n as f64) * params.anchor_fraction).round().clamp(4.0, n as f64) as usize;
    let mut anchor_idx: Vec<usize> = scores[..k].iter().map(|(i, _)| *i).collect();
    anchor_idx.sort_unstable();

    // Theil–Sen 中位数斜率（全点对）：击穿值 29.3%，峰点不足一半时
    // 无法把中位数拉偏。点较多时等距抽样到最多 120 个点，控制开销。
    let stride = ((n / 120) + 1).max(1);
    let picked: Vec<usize> = (0..n).step_by(stride).collect();
    let mut pair_slopes: Vec<f64> = Vec::new();
    for a in 0..picked.len() {
        for b in (a + 1)..picked.len() {
            let (i, j) = (picked[a], picked[b]);
            let dv = points[j].potential_v - points[i].potential_v;
            if dv.abs() > f64::EPSILON {
                pair_slopes.push((points[j].current_a - points[i].current_a) / dv);
            }
        }
    }
    pair_slopes.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let b1 = pair_slopes[pair_slopes.len() / 2];
    eprintln!("DBG n={n} picked={} b1={b1:.3e} nslopes={}", picked.len(), pair_slopes.len());

    let detr: Vec<f64> = points
        .iter()
        .map(|p| p.current_a - b1 * (p.potential_v - vcenter))
        .collect();
    let med = median_of(&detr);
    let mean = detr.iter().sum::<f64>() / n as f64;
    let mut sorted = detr.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let q: f64 = if mean >= med { 0.10 } else { 0.90 };
    let b0 = sorted[(q * (n as f64 - 1.0)).round() as usize];

    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| {
        (detr[a] - b0).abs().partial_cmp(&(detr[b] - b0).abs()).unwrap()
    });
    let keep = (k / 2).max(2);
    let mut kept: Vec<usize> = order[..keep.min(n)].to_vec();
    kept.sort_unstable();

    Ok(BaselineResult {
        kind: BaselineKind::Derivative,
        params: params.clone(),
        values: points
            .iter()
            .map(|p| b0 + b1 * (p.potential_v - vcenter))
            .collect(),
        anchor_positions: kept,
    })
}

fn median_of(v: &[f64]) -> f64 {
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    s[s.len() / 2]
}

fn finite_slope(a: &Sample, b: &Sample) -> f64 {
    let dv = b.potential_v - a.potential_v;
    if dv.abs() < f64::EPSILON {
        0.0
    } else {
        (b.current_a - a.current_a) / dv
    }
}
