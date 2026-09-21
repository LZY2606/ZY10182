use crate::model::{DatasetMeta, PeakMetrics};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Incompatible {
    pub fields: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScalingFit {
    pub x: Vec<f64>,
    pub y: Vec<f64>,
    pub slope: f64,
    pub intercept: f64,
    pub r2: f64,
    pub points: Vec<ScalingPoint>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScalingPoint {
    pub dataset_id: i64,
    pub dataset_name: String,
    pub scan_rate: f64,
    pub sqrt_scan_rate: f64,
    pub peak_current: f64,
}

/// Datasets may only share a scan-rate scaling when units, reference electrode
/// convention and electrode area agree. Returns the list of incompatible fields.
pub fn compatibility_error(items: &[(&DatasetMeta, &PeakMetrics)]) -> Option<Incompatible> {
    let mut fields = Vec::new();
    if items.len() < 2 {
        return None;
    }
    let base = items[0].0;
    let mut push_if = |cond: bool, name: &str, fields: &mut Vec<String>| {
        if cond {
            fields.push(name.to_string());
        }
    };
    for (m, _) in &items[1..] {
        push_if(m.potential_unit != base.potential_unit, "potential_unit", &mut fields);
        push_if(m.current_unit != base.current_unit, "current_unit", &mut fields);
        push_if(m.time_unit != base.time_unit, "time_unit", &mut fields);
        push_if(
            m.scan_rate_unit != base.scan_rate_unit,
            "scan_rate_unit",
            &mut fields,
        );
        push_if(
            m.reference_electrode != base.reference_electrode,
            "reference_electrode",
            &mut fields,
        );
        push_if(
            (m.electrode_area - base.electrode_area).abs() > 1e-12
                || m.electrode_area_unit != base.electrode_area_unit,
            "electrode_area",
            &mut fields,
        );
    }
    if fields.is_empty() {
        None
    } else {
        fields.sort();
        fields.dedup();
        Some(Incompatible { fields })
    }
}

/// Linear fit of peak current against sqrt(scan rate) (Randles-Sevcik scale).
pub fn fit_scaling(items: Vec<(i64, String, &DatasetMeta, PeakMetrics)>) -> Result<ScalingFit, Incompatible> {
    let refs: Vec<(&DatasetMeta, &PeakMetrics)> = items.iter().map(|(_, _, m, p)| (*m, &*p)).collect();
    if let Some(err) = compatibility_error(&refs) {
        return Err(err);
    }
    let mut points = Vec::new();
    for (id, name, m, p) in items {
        points.push(ScalingPoint {
            dataset_id: id,
            dataset_name: name,
            scan_rate: m.scan_rate,
            sqrt_scan_rate: m.scan_rate.max(0.0).sqrt(),
            peak_current: p.peak_current,
        });
    }
    let xs: Vec<f64> = points.iter().map(|p| p.sqrt_scan_rate).collect();
    let ys: Vec<f64> = points.iter().map(|p| p.peak_current).collect();
    let n = xs.len() as f64;
    let sx: f64 = xs.iter().sum();
    let sy: f64 = ys.iter().sum();
    let sxx: f64 = xs.iter().map(|x| x * x).sum();
    let sxy: f64 = xs.iter().zip(&ys).map(|(x, y)| x * y).sum();
    let (slope, intercept) = if xs.len() == 1 {
        (0.0, ys[0])
    } else {
        let denom = n * sxx - sx * sx;
        if denom.abs() < 1e-15 {
            (0.0, sy / n)
        } else {
            let slope = (n * sxy - sx * sy) / denom;
            (slope, (sy - slope * sx) / n)
        }
    };
    let mean = sy / n;
    let sst: f64 = ys.iter().map(|y| (y - mean).powi(2)).sum();
    let sse: f64 = ys
        .iter()
        .zip(&xs)
        .map(|(y, x)| (y - (slope * x + intercept)).powi(2))
        .sum();
    let r2 = if sst.abs() < 1e-18 { 1.0 } else { 1.0 - sse / sst };
    Ok(ScalingFit {
        x: xs,
        y: ys,
        slope,
        intercept,
        r2,
        points,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{BaselineKind, Direction, PeakMetrics};

    fn meta(ref_el: &str, area: f64, v: f64) -> DatasetMeta {
        DatasetMeta {
            name: "d".into(),
            reference_electrode: ref_el.into(),
            potential_unit: "V".into(),
            current_unit: "A".into(),
            time_unit: "s".into(),
            scan_rate: v,
            scan_rate_unit: "V/s".into(),
            electrode_area: area,
            electrode_area_unit: "cm^2".into(),
        }
    }

    fn peak(ip: f64) -> PeakMetrics {
        PeakMetrics {
            interval_id: 1,
            segment_index: 0,
            direction: Direction::Anodic,
            peak_idx: 0,
            peak_potential: 0.3,
            peak_current: ip,
            peak_current_raw: ip,
            charge: 1e-6,
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
    fn rejects_reference_change() {
        let m1 = meta("Ag/AgCl", 0.07, 0.1);
        let m2 = meta("SCE", 0.07, 0.4);
        let p1 = peak(8e-6);
        let p2 = peak(16e-6);
        let err = fit_scaling(vec![(1, "a".into(), &m1, p1), (2, "b".into(), &m2, p2)]).unwrap_err();
        assert_eq!(err.fields, vec!["reference_electrode"]);
    }

    #[test]
    fn rejects_area_and_units() {
        let mut m1 = meta("Ag/AgCl", 0.07, 0.1);
        let mut m2 = meta("Ag/AgCl", 0.10, 0.4);
        m2.current_unit = "mA".into();
        let p1 = peak(8e-6);
        let p2 = peak(16e-6);
        let err = fit_scaling(vec![(1, "a".into(), &m1, p1), (2, "b".into(), &m2, p2)]).unwrap_err();
        assert!(err.fields.contains(&"current_unit".to_string()));
        assert!(err.fields.contains(&"electrode_area".to_string()));
    }

    #[test]
    fn fits_compatible_sqrt_scan_rate() {
        let m1 = meta("Ag/AgCl", 0.07, 0.1);
        let m2 = meta("Ag/AgCl", 0.07, 0.4);
        let p1 = peak(8e-6);
        let p2 = peak(16e-6);
        let fit = fit_scaling(vec![(1, "a".into(), &m1, p1), (2, "b".into(), &m2, p2)]).unwrap();
        assert!((fit.r2 - 1.0).abs() < 1e-9);
        assert!((fit.points[1].sqrt_scan_rate - 0.6324555).abs() < 1e-6);
    }
}
