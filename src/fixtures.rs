//! 固定 fixture 生成器：三份循环伏安记录。
//!
//! `cycle_50mVs.csv` / `cycle_100mVs.csv` / `cycle_200mVs.csv`
//! 同口径（Ag/AgCl(3M KCl)、V/A/s、0.071 cm²），仅扫速不同，可共同拟合
//! 峰电流 ~ v^0.5 标度；每份记录都包含：
//! - 起始 0 V 保持平台（3 个点）与两个回转电位处的等电位平台（各 3 个点，
//!   验收时可再手动延长一个样本，分段不得抖动）；
//! - 正扫两个部分重叠的氧化峰（约 0.30 V 与 0.42 V）；
//! - 反扫两个还原峰（约 0.24 V 与 0.36 V）；
//! - 电容背景 i_cap = Cdl * v 与线性漂移。
//!
//! `cycle_100mVs_sce.csv` 与 100 mV/s 文件数据相同但参考电极口径变为 SCE、
//! 电极面积不同，用于验证共同标度被拒绝。

pub const FILES: &[(&str, &str)] = &[
    ("cycle_50mVs.csv", include_str!("../fixtures/cycle_50mVs.csv")),
    ("cycle_100mVs.csv", include_str!("../fixtures/cycle_100mVs.csv")),
    ("cycle_200mVs.csv", include_str!("../fixtures/cycle_200mVs.csv")),
    ("cycle_100mVs_sce.csv", include_str!("../fixtures/cycle_100mVs_sce.csv")),
];

pub fn file_names() -> Vec<&'static str> {
    FILES.iter().map(|(n, _)| *n).collect()
}

pub fn by_name(name: &str) -> Option<&'static str> {
    FILES.iter().find(|(n, _)| *n == name).map(|(_, c)| *c)
}

/// 程序化生成一份 fixture（确定性，供生成脚本与测试复核使用）。
#[allow(clippy::too_many_arguments)]
pub fn generate_csv(
    scan_rate_v_s: f64,
    reference: &str,
    area_cm2: f64,
    current_unit: &str,
) -> String {
    let v_lo = 0.0f64;
    let v_hi = 0.6f64;
    let dt = 0.02; // 采样间隔 (s)
    let dv = scan_rate_v_s * dt;
    let n_ramp_up = ((v_hi - v_lo) / dv).round() as usize;

    let mut pot: Vec<f64> = Vec::new();
    // 起始平台
    for _ in 0..3 {
        pot.push(v_lo);
    }
    // 正扫
    for k in 0..=n_ramp_up {
        pot.push(v_lo + (k as f64) * dv);
        let _ = &mut pot;
    }
    if (pot.last().copied().unwrap_or(0.0) - v_hi).abs() > 1e-9 {
        *pot.last_mut().unwrap() = v_hi;
    }
    // 上回转平台（3 个相同电位点，含末尾样本；验收可再延长一个）
    for _ in 0..2 {
        pot.push(v_hi);
    }
    // 反扫
    for k in 1..n_ramp_up {
        pot.push(v_hi - (k as f64) * dv);
    }
    pot.push(v_lo);
    // 下回转平台
    for _ in 0..2 {
        pot.push(v_lo);
    }

    // 电流模型（A）：两个部分重叠高斯峰 + 电容背景 + 漂移
    let cdl = 120e-6; // F/cm2 等效（数值仅用于产生形状）
    let base_scale = (scan_rate_v_s / 0.1).sqrt();
    let i_an1 = 5.0e-6 * base_scale;
    let i_an2 = 3.2e-6 * base_scale;
    let i_ca1 = -4.6e-6 * base_scale;
    let i_ca2 = -3.0e-6 * base_scale;
    let gauss = |v: f64, c: f64, w: f64, a: f64| -> f64 {
        a * (-(v - c).powi(2) / (2.0 * w * w)).exp()
    };

    let mut lines = Vec::new();
    let cur_factor = if current_unit == "uA" { 1e6 } else { 1.0 };
    lines.push(format!("#name=循环伏安 {:.0} mV/s ({reference})", scan_rate_v_s * 1e3));
    lines.push(format!("#scan_rate={} V/s", fmt_num(scan_rate_v_s)));
    lines.push("#potential_unit=V".to_string());
    lines.push(format!("#current_unit={current_unit}"));
    lines.push("#time_unit=s".to_string());
    lines.push(format!("#area={} cm2", fmt_num(area_cm2)));
    lines.push(format!("#reference_electrode={reference}"));
    lines.push("#tolerance_v=1e-9".to_string());
    lines.push("potential,current,time".to_string());

    for (i, &v) in pot.iter().enumerate() {
        // 方向：按采样顺序判断（平台继承由分析端负责，这里仅生成形状）
        let t = i as f64 * dt;
        let mut i_sig = gauss(v, 0.30, 0.030, i_an1) + gauss(v, 0.42, 0.035, i_an2)
            + gauss(v, 0.24, 0.030, i_ca1)
            + gauss(v, 0.36, 0.035, i_ca2);
        // 反扫时阳极峰电流应只在正扫出现——用时间方向判定：
        // 根据电位序列方向给峰加符号门（用 v 相对前一点）
        let direction = if i == 0 {
            1.0
        } else if v > pot[i - 1] + 1e-12 {
            1.0
        } else if v < pot[i - 1] - 1e-12 {
            -1.0
        } else if i < 3 {
            1.0
        } else {
            // 平台：继承最近的明确方向
            let mut d = 0.0;
            for j in (0..i).rev() {
                if (v - pot[j]).abs() > 1e-12 {
                    d = if pot[j] < v { 1.0 } else { -1.0 };
                    break;
                }
            }
            d
        };
        if direction <= 0.0 {
            // 反扫：保留阴极两峰，去掉阳极两峰贡献
            i_sig = gauss(v, 0.24, 0.030, i_ca1) + gauss(v, 0.36, 0.035, i_ca2);
        } else {
            i_sig = gauss(v, 0.30, 0.030, i_an1) + gauss(v, 0.42, 0.035, i_an2);
        }
        // 电容电流 i = C_eff * dE/dt：线性斜坡（无净偏置）
        let cap = cdl * scan_rate_v_s * if direction >= 0.0 { 1.0 } else { -1.0 };
        let drift = 0.10e-6 * (v - 0.3);
        let current = i_sig + cap + drift;
        lines.push(format!(
            "{},{},{}",
            fmt_num(v),
            fmt_num(current * cur_factor),
            fmt_num(t)
        ));
    }
    lines.join("\n") + "\n"
}

fn fmt_num(x: f64) -> String {
    if x == 0.0 {
        "0".to_string()
    } else if x.abs() >= 0.01 {
        format!("{:.6}", x).trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        format!("{:.6e}", x)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::import_run;

    #[test]
    fn fixtures_import_and_segment_deterministically() {
        for (name, content) in FILES {
            let run = import_run(content, name).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(run.segments.len(), 2, "{name} 应切成正反两段");
            assert_eq!(run.segments[0].kind, crate::model::SegmentKind::Forward);
            assert_eq!(run.segments[1].kind, crate::model::SegmentKind::Reverse);
            // 上回转平台 3 个点全部留在正扫段
            let f = &run.segments[0];
            let hi = run.samples[f.end_idx].potential_v;
            assert!((hi - 0.6).abs() < 1e-9);
        }
    }
}
