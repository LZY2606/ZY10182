//! 扫速标度：ip ~ v^b（Randles–Sevcik 扩散控制 b≈0.5，表面控制 b≈1）。
//!
//! 单位 / 电极面积 / 参考电极口径不一致的数据禁止共同拟合：
//! 系统明确列出不兼容字段，而不是静默换算。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScalingPoint {
    pub dataset_id: i64,
    pub dataset_name: String,
    pub scan_rate_v_s: f64,
    pub peak_current_a: f64,
    pub charge_c: f64,
}

#[derive(Debug, Clone)]
pub struct DataCalibre {
    pub dataset_id: i64,
    pub dataset_name: String,
    pub potential_unit: String,
    pub current_unit: String,
    pub time_unit: String,
    pub scan_rate_unit: String,
    pub area: f64,
    pub area_unit: String,
    pub reference_electrode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Incompatibility {
    pub field: String,
    pub values: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScalingFit {
    pub n_points: usize,
    pub exponent_b: f64,
    pub intercept_log10: f64,
    pub r_squared: f64,
    pub points: Vec<ScalingPoint>,
}

fn norm_token(s: &str) -> String {
    s.trim().replace([' ', '\t'], "").to_ascii_lowercase()
}

/// 校验口径是否一致；返回不兼容字段列表（空表示可以共同拟合）。
pub fn check_calibre(items: &[DataCalibre]) -> Vec<Incompatibility> {
    let mut bad = Vec::new();
    if items.len() < 2 {
        return bad;
    }
    let first = &items[0];
    macro_rules! check_str {
        ($field:expr, $get:expr) => {{
            let vals: Vec<String> = items.iter().map(|i| norm_token(&$get(i))).collect();
            let fv = norm_token(&$get(first));
            if vals.iter().any(|v| v != &fv) {
                bad.push(Incompatibility {
                    field: $field.to_string(),
                    values: items
                        .iter()
                        .map(|i| format!("{}: {}", i.dataset_name, $get(i)))
                        .collect(),
                });
            }
        }};
    }
    check_str!("reference_electrode", |i: &DataCalibre| i.reference_electrode.clone());
    check_str!("potential_unit", |i: &DataCalibre| i.potential_unit.clone());
    check_str!("current_unit", |i: &DataCalibre| i.current_unit.clone());
    check_str!("time_unit", |i: &DataCalibre| i.time_unit.clone());
    check_str!("scan_rate_unit", |i: &DataCalibre| i.scan_rate_unit.clone());
    check_str!("area_unit", |i: &DataCalibre| i.area_unit.clone());

    // 面积数值必须一致（系统不擅自做面积归一）
    let areas: Vec<f64> = items.iter().map(|i| i.area).collect();
    if areas.iter().any(|a| (*a - first.area).abs() > 1e-12) {
        bad.push(Incompatibility {
            field: "area".to_string(),
            values: items
                .iter()
                .map(|i| format!("{}: {} {}", i.dataset_name, i.area, i.area_unit))
                .collect(),
        });
    }
    bad
}

/// log10(ip) = b * log10(v) + c 最小二乘。
pub fn fit_power_law(points: &[ScalingPoint]) -> Result<ScalingFit, String> {
    if points.len() < 2 {
        return Err("共同标度拟合至少需要 2 个不同扫速的数据点".to_string());
    }
    let xs: Vec<f64> = points.iter().map(|p| p.scan_rate_v_s.log10()).collect();
    let ys: Vec<f64> = points.iter().map(|p| p.peak_current_a.abs().log10()).collect();
    if xs.iter().chain(ys.iter()).any(|v| !v.is_finite()) {
        return Err("存在非正扫速或非正峰电流，无法取对数".to_string());
    }
    let n = points.len() as f64;
    let mx = xs.iter().sum::<f64>() / n;
    let my = ys.iter().sum::<f64>() / n;
    let mut sxx = 0.0;
    let mut sxy = 0.0;
    let mut syy = 0.0;
    for (&x, &y) in xs.iter().zip(ys.iter()) {
        sxx += (x - mx).powi(2);
        sxy += (x - mx) * (y - my);
        syy += (y - my).powi(2);
    }
    if sxx <= f64::EPSILON {
        return Err("扫速值全部相同，无法拟合标度".to_string());
    }
    let b = sxy / sxx;
    let c = my - b * mx;
    let r2 = if syy > f64::EPSILON { (sxy * sxy / (sxx * syy)).clamp(0.0, 1.0) } else { 1.0 };
    Ok(ScalingFit {
        n_points: points.len(),
        exponent_b: b,
        intercept_log10: c,
        r_squared: r2,
        points: points.to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cal(id: i64, name: &str, area: f64, refer: &str) -> DataCalibre {
        DataCalibre {
            dataset_id: id,
            dataset_name: name.to_string(),
            potential_unit: "V".into(),
            current_unit: "A".into(),
            time_unit: "s".into(),
            scan_rate_unit: "V/s".into(),
            area,
            area_unit: "cm2".into(),
            reference_electrode: refer.into(),
        }
    }

    #[test]
    fn same_calibre_is_compatible() {
        let items = vec![cal(1, "a", 0.07, "Ag/AgCl"), cal(2, "b", 0.07, "Ag/AgCl")];
        assert!(check_calibre(&items).is_empty());
    }

    #[test]
    fn reference_change_is_rejected_with_field_name() {
        let items = vec![
            cal(1, "s1", 0.07, "Ag/AgCl(3M KCl)"),
            cal(2, "s2", 0.07, "SCE"),
        ];
        let bad = check_calibre(&items);
        assert!(bad.iter().any(|x| x.field == "reference_electrode"));
        assert!(bad.iter().flat_map(|x| x.values.iter()).any(|v| v.contains("SCE")));
    }

    #[test]
    fn area_mismatch_is_rejected() {
        let items = vec![cal(1, "a", 0.07, "R"), cal(2, "b", 0.20, "R")];
        assert!(check_calibre(&items).iter().any(|x| x.field == "area"));
    }

    #[test]
    fn unit_mismatch_is_rejected() {
        let mut a = cal(1, "a", 0.07, "R");
        let mut b = cal(2, "b", 0.07, "R");
        b.current_unit = "mA".into();
        a.scan_rate_unit = "V/s".into();
        let bad = check_calibre(&[a, b]);
        assert!(bad.iter().any(|x| x.field == "current_unit"));
    }

    #[test]
    fn power_law_recovers_half() {
        let pts: Vec<ScalingPoint> = [0.05, 0.1, 0.2, 0.5]
            .iter()
            .enumerate()
            .map(|(i, &v)| ScalingPoint {
                dataset_id: i as i64 + 1,
                dataset_name: format!("d{i}"),
                scan_rate_v_s: v,
                peak_current_a: 2.0 * v.sqrt(),
                charge_c: 0.0,
            })
            .collect();
        let fit = fit_power_law(&pts).unwrap();
        assert!((fit.exponent_b - 0.5).abs() < 1e-9);
        assert!(fit.r_squared > 0.999999);
    }
}
