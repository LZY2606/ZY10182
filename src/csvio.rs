use crate::model::{DatasetMeta, Point};

#[derive(Debug)]
pub struct ParseError(pub String);
impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for ParseError {}

fn default_meta() -> DatasetMeta {
    DatasetMeta {
        name: "imported".into(),
        reference_electrode: String::new(),
        potential_unit: "V".into(),
        current_unit: "A".into(),
        time_unit: "s".into(),
        scan_rate: f64::NAN,
        scan_rate_unit: "V/s".into(),
        electrode_area: f64::NAN,
        electrode_area_unit: String::new(),
    }
}

/// Parse the instrument text format:
/// ```text
/// # key: value
/// time_s,potential_v,current_a
/// 0.0,0.0,0.0
/// ```
/// Column order is flexible; recognized headers: time(s)/t, potential(v)/e/v,
/// current(a)/i. Metadata lives in `#` header lines and is preserved verbatim.
pub fn parse_csv(text: &str) -> Result<(DatasetMeta, Vec<Point>), ParseError> {
    let mut meta = default_meta();
    let mut lines = text.lines();
    // Header metadata
    while let Some(line) = lines.next() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if let Some(rest) = t.strip_prefix('#') {
            if let Some((k, v)) = rest.split_once(':') {
                apply_meta(&mut meta, k.trim(), v.trim());
            }
            continue;
        }
        // first non-comment line = column header
        let headers: Vec<String> = split_row(t).into_iter().map(|s| s.to_lowercase()).collect();
        let (ti, pi, ci) = locate_columns(&headers)?;
        let mut points = Vec::new();
        let mut idx = 0usize;
        for (ln, row) in lines.enumerate() {
            let rt = row.trim();
            if rt.is_empty() || rt.starts_with('#') {
                continue;
            }
            let cols = split_row(rt);
            let get = |i: Option<usize>| -> Result<f64, ParseError> {
                match i {
                    Some(i) if i < cols.len() => cols[i]
                        .parse::<f64>()
                        .map_err(|e| ParseError(format!("row {}: {}", ln + 2, e))),
                    Some(_) => Err(ParseError(format!("row {}: missing column", ln + 2))),
                    None => Ok(f64::NAN),
                }
            };
            let mut time = get(ti)?;
            let potential = get(pi)?;
            let current = get(ci)?;
            if time.is_nan() {
                time = idx as f64;
            }
            points.push(Point {
                idx,
                time,
                potential,
                current,
            });
            idx += 1;
        }
        if points.is_empty() {
            return Err(ParseError("no data rows".into()));
        }
        return Ok((meta, points));
    }
    Err(ParseError("no header row found".into()))
}

fn split_row(s: &str) -> Vec<&str> {
    s.split(',').map(|c| c.trim()).collect()
}

fn locate_columns(h: &[String]) -> Result<(Option<usize>, Option<usize>, Option<usize>), ParseError> {
    let mut ti = None;
    let mut pi = None;
    let mut ci = None;
    for (i, name) in h.iter().enumerate() {
        let n = name.trim_start_matches('_');
        if (n.contains("time") || n == "t" || n.contains("_s")) && ti.is_none() && !n.contains("unit") {
            if n.contains("time") || n == "t" {
                ti = Some(i);
            }
        }
        if (n.contains("potential") || n.contains("pot") || n == "e" || n == "v")
            && pi.is_none()
        {
            pi = Some(i);
        }
        if (n.contains("current") || n == "i" || n == "a") && ci.is_none() {
            ci = Some(i);
        }
    }
    if pi.is_none() || ci.is_none() {
        return Err(ParseError(
            "header must contain potential and current columns".into(),
        ));
    }
    Ok((ti, pi, ci))
}

fn apply_meta(meta: &mut DatasetMeta, key: &str, val: &str) {
    match key {
        "name" => meta.name = val.into(),
        "reference_electrode" | "reference" | "ref_electrode" => {
            meta.reference_electrode = val.into()
        }
        "potential_unit" => meta.potential_unit = val.into(),
        "current_unit" => meta.current_unit = val.into(),
        "time_unit" => meta.time_unit = val.into(),
        "scan_rate" | "scanrate" => {
            if let Ok(v) = val.trim_end_matches(['V', 'v']).trim().parse::<f64>() {
                meta.scan_rate = v;
            }
        }
        "scan_rate_unit" => meta.scan_rate_unit = val.into(),
        "electrode_area" | "area" => {
            if let Ok(v) = val.split_whitespace().next().unwrap_or(val).parse::<f64>() {
                meta.electrode_area = v;
            }
        }
        "electrode_area_unit" | "area_unit" => meta.electrode_area_unit = val.into(),
        _ => {}
    }
}

/// Serialize back to the same instrument-flavoured text (round trip / export).
pub fn emit_csv(meta: &DatasetMeta, points: &[Point]) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    let _ = writeln!(s, "# name: {}", meta.name);
    let _ = writeln!(s, "# reference_electrode: {}", meta.reference_electrode);
    let _ = writeln!(s, "# potential_unit: {}", meta.potential_unit);
    let _ = writeln!(s, "# current_unit: {}", meta.current_unit);
    let _ = writeln!(s, "# time_unit: {}", meta.time_unit);
    let _ = writeln!(s, "# scan_rate: {}", meta.scan_rate);
    let _ = writeln!(s, "# scan_rate_unit: {}", meta.scan_rate_unit);
    let _ = writeln!(s, "# electrode_area: {}", meta.electrode_area);
    let _ = writeln!(s, "# electrode_area_unit: {}", meta.electrode_area_unit);
    s.push_str("time_s,potential_v,current_a\n");
    for p in points {
        let _ = writeln!(
            s,
            "{},{},{}",
            p.time, p.potential, p.current
        );
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_headers_and_rows() {
        let text = "# name: demo\n# reference_electrode: SCE\n# scan_rate: 0.2\n\ntime_s,potential_v,current_a\n0,0.0,1e-7\n0.1,0.01,2e-7\n";
        let (m, pts) = parse_csv(text).unwrap();
        assert_eq!(m.name, "demo");
        assert_eq!(m.reference_electrode, "SCE");
        assert_eq!(m.scan_rate, 0.2);
        assert_eq!(pts.len(), 2);
        assert_eq!(pts[1].potential, 0.01);
    }

    #[test]
    fn round_trips() {
        let text = "# name: demo\n# reference_electrode: Ag/AgCl\n# scan_rate: 0.1\n# electrode_area: 0.07\ntime_s,potential_v,current_a\n0,0.0,1e-7\n";
        let (m, pts) = parse_csv(text).unwrap();
        let again = emit_csv(&m, &pts);
        let (m2, pts2) = parse_csv(&again).unwrap();
        assert_eq!(m2.reference_electrode, m.reference_electrode);
        assert_eq!(pts2.len(), pts.len());
        assert_eq!(pts2[0].current, pts[0].current);
    }
}
