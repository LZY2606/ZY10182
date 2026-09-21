//! CSV 导入：保留仪器给出的电位/电流/时间/扫速与参考电极口径。
//!
//! 文件采用 UTF-8 文本：`#` 开头的行是仪器元数据（`#key=value`），
//! 数据行列为 `index,potential,current,time`（index 可省略，按行号补齐）。
//! 全部计算在内部归一化到 V / A / s / V·s^-1，原始单位字符串原样入库，
//! 供扫速标度拟合前的口径一致性校验使用。

use crate::model::{Metadata, RunData, Sample, SegmentKind};
use crate::segments;
use anyhow::{bail, Result};

pub struct ParsedCsv {
    pub meta: Metadata,
    pub samples: Vec<Sample>,
}

fn parse_unit_factor(unit: &str, family: UnitFamily) -> Result<f64> {
    let u = unit.trim().replace("μ", "u").replace("µ", "u");
    let u = u.replace(" ", "").replace("**", "^");
    let (prefix, base_ok) = match family {
        UnitFamily::Potential => (match u.as_str() {
            "V" => 1.0,
            "mV" => 1e-3,
            "uV" => 1e-6,
            _ => bail!("不支持的电位单位: {unit}"),
        }, true),
        UnitFamily::Current => (match u.as_str() {
            "A" => 1.0,
            "mA" => 1e-3,
            "uA" => 1e-6,
            "nA" => 1e-9,
            _ => bail!("不支持的电流单位: {unit}"),
        }, true),
        UnitFamily::Time => (match u.as_str() {
            "s" => 1.0,
            "ms" => 1e-3,
            _ => bail!("不支持的时间单位: {unit}"),
        }, true),
        UnitFamily::Rate => {
            // 支持 mV/s 与 V/s
            let f = if let Some(rest) = u.strip_suffix("V/s") {
                match rest {
                    "" => 1.0,
                    "m" => 1e-3,
                    "u" => 1e-6,
                    _ => bail!("不支持的扫速单位: {unit}"),
                }
            } else {
                bail!("不支持的扫速单位: {unit}（示例：V/s、mV/s）");
            };
            (f, true)
        }
        UnitFamily::Area => (match u.as_str() {
            "cm2" | "cm^2" => 1.0,
            "mm2" | "mm^2" => 0.01,
            "m2" | "m^2" => 1e4,
            _ => bail!("不支持的电极面积单位: {unit}"),
        }, true),
    };
    debug_assert!(base_ok);
    Ok(prefix)
}

#[derive(Clone, Copy)]
pub enum UnitFamily {
    Potential,
    Current,
    Time,
    Rate,
    Area,
}

fn parse_finite(s: &str, field: &str, line_no: usize) -> Result<f64> {
    let v: f64 = s
        .trim()
        .parse()
        .map_err(|_| anyhow::anyhow!("第 {line_no} 行字段 {field} 不是数字: {s:?}"))?;
    if !v.is_finite() {
        bail!("第 {line_no} 行字段 {field} 不是有限值");
    }
    Ok(v)
}

fn strip_comment_inline(line: &str) -> &str {
    line.split('#').next().unwrap_or("")
}

pub fn parse_csv(content: &str, default_name: &str) -> Result<ParsedCsv> {
    let mut meta = Metadata::default();
    meta.name = default_name.to_string();
    let mut raw_rows: Vec<(usize, Vec<String>)> = Vec::new();
    let mut header_seen = false;

    for (i, raw_line) in content.lines().enumerate() {
        let line_no = i + 1;
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('#') {
            // 允许 "# key = value" 或 "#key=value"
            let body = line.trim_start_matches('#').trim();
            if body.is_empty() {
                continue;
            }
            if let Some((k, v)) = body.split_once('=') {
                apply_meta(&mut meta, k.trim(), v.trim())?;
            }
            continue;
        }
        let stripped = strip_comment_inline(line);
        let cols: Vec<String> = stripped.split(',').map(|c| c.trim().to_string()).collect();
        if cols.iter().all(|c| c.is_empty()) {
            continue;
        }
        let first = cols[0].to_ascii_lowercase();
        if !header_seen && (first.contains("potential") || first == "e" || first == "potential_v")
        {
            // 表头行：跳过，但要求列顺序固定 index?,potential,current,time
            header_seen = true;
            if cols.len() < 3 {
                bail!("第 {line_no} 行表头至少需要 potential,current,time");
            }
            continue;
        }
        raw_rows.push((line_no, cols));
    }

    if raw_rows.is_empty() {
        bail!("CSV 中没有任何数据点");
    }

    let f_pot = parse_unit_factor(&meta.potential_unit, UnitFamily::Potential)?;
    let f_cur = parse_unit_factor(&meta.current_unit, UnitFamily::Current)?;
    let f_time = parse_unit_factor(&meta.time_unit, UnitFamily::Time)?;

    let ncols = raw_rows[0].1.len();
    for (ln, cols) in &raw_rows {
        if cols.len() != ncols {
            bail!("第 {ln} 行列数 {} 与首行 {ncols} 不一致", cols.len());
        }
    }
    if !(3..=4).contains(&ncols) {
        bail!("数据列应为 potential,current,time 或 index,potential,current,time，实际 {ncols} 列");
    }

    let mut samples = Vec::with_capacity(raw_rows.len());
    for (row_i, (ln, cols)) in raw_rows.iter().enumerate() {
        let (index, pot_s, cur_s, time_s) = if ncols == 4 {
            let idx: usize = cols[0]
                .parse()
                .map_err(|_| anyhow::anyhow!("第 {ln} 行 index 不是整数: {:?}", cols[0]))?;
            (idx, &cols[1], &cols[2], &cols[3])
        } else {
            (row_i, &cols[0], &cols[1], &cols[2])
        };
        let potential = parse_finite(pot_s, "potential", *ln)? * f_pot;
        let current = parse_finite(cur_s, "current", *ln)? * f_cur;
        let time = parse_finite(time_s, "time", *ln)? * f_time;
        samples.push(Sample { index, potential_v: potential, current_a: current, time_s: time });
    }

    // 时间必须单调非减（允许平台）；index 只作溯源展示，不参与切分。
    for w in samples.windows(2) {
        if w[1].time_s < w[0].time_s {
            bail!("时间列必须单调非减（index {} 处回退）", w[1].index);
        }
    }

    Ok(ParsedCsv { meta, samples })
}

fn apply_meta(meta: &mut Metadata, key: &str, value: &str) -> Result<()> {
    let key_l = key.to_ascii_lowercase().replace(['-', ' '], "_");
    match key_l.as_str() {
        "name" | "dataset" | "title" => meta.name = value.to_string(),
        "scan_rate" | "scanrate" | "rate" => {
            let (num, unit) = split_number_unit(value)?;
            meta.scan_rate = num;
            meta.scan_rate_unit = unit;
        }
        "potential_unit" | "e_unit" | "v_unit" => meta.potential_unit = value.to_string(),
        "current_unit" | "i_unit" => meta.current_unit = value.to_string(),
        "time_unit" | "t_unit" => meta.time_unit = value.to_string(),
        "area" | "electrode_area" => {
            let (num, unit) = split_number_unit(value)?;
            meta.area = num;
            meta.area_unit = unit;
        }
        "reference_electrode" | "reference" | "ref_electrode" | "ref" => {
            meta.reference_electrode = value.to_string()
        }
        "tolerance_v" | "plateau_tolerance_v" => {
            meta.tolerance_v = parse_finite(value, key, 0)?;
            if meta.tolerance_v < 0.0 {
                meta.tolerance_v = 0.0;
            }
        }
        other => bail!("未知元数据字段: {other}"),
    }
    Ok(())
}

fn split_number_unit(s: &str) -> Result<(f64, String)> {
    let s = s.trim();
    let cut = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+' || c == 'e' || c == 'E'))
        .unwrap_or(s.len());
    let (num_s, unit_s) = s.split_at(cut);
    let num: f64 = num_s
        .trim()
        .parse()
        .map_err(|_| anyhow::anyhow!("无法解析数值: {s:?}"))?;
    if !num.is_finite() {
        bail!("数值不是有限值: {s:?}");
    }
    Ok((num, unit_s.trim().to_string()))
}

/// 导入主入口：解析 + 按容差归一化 + 切分正/反扫段。
pub fn import_run(content: &str, default_name: &str) -> Result<RunData> {
    let ParsedCsv { mut meta, samples } = parse_csv(content, default_name)?;
    let f_rate = parse_unit_factor(&meta.scan_rate_unit, UnitFamily::Rate)?;
    if !meta.scan_rate.is_finite() || meta.scan_rate <= 0.0 {
        bail!("必须通过 #scan_rate= 给出正的扫速（带单位，如 0.1 V/s）");
    }
    meta.scan_rate *= f_rate;
    meta.scan_rate_unit = "V/s".to_string();
    let f_area = parse_unit_factor(&meta.area_unit, UnitFamily::Area)?;
    if !meta.area.is_finite() || meta.area <= 0.0 {
        bail!("必须通过 #area= 给出正的电极面积（带单位，如 0.071 cm2）");
    }
    meta.area *= f_area;
    meta.area_unit = "cm2".to_string();
    // tolerance_v 按仪器电位单位解释
    if meta.tolerance_v > 0.0 && meta.potential_unit != "V" {
        let f_pot = parse_unit_factor(&meta.potential_unit.replace('μ', "u").as_str(), UnitFamily::Potential)
            .unwrap_or(1.0);
        meta.tolerance_v *= f_pot;
    }
    if meta.reference_electrode.trim().is_empty() {
        bail!("必须通过 #reference_electrode= 声明参考电极口径，如 Ag/AgCl(3M KCl)");
    }

    let (labels, segments) = segments::segment_samples(&samples, meta.tolerance_v);
    if segments.is_empty() {
        bail!("未切分出任何扫描段：电位全程为平台");
    }
    Ok(RunData { meta, samples, labels, segments })
}

/// 重新切分（容差调整时使用）。
pub fn resegment(run: &mut RunData, tolerance_v: f64) {
    run.meta.tolerance_v = tolerance_v.max(0.0);
    let (labels, segments) = segments::segment_samples(&run.samples, run.meta.tolerance_v);
    run.labels = labels;
    run.segments = segments;
}

pub fn segment_count_by_kind(run: &RunData, kind: SegmentKind) -> usize {
    run.segments.iter().filter(|s| s.kind == kind).count()
}
