//! 贯穿全系统的数据类型：仪器原始口径与分析结果。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentKind {
    Forward = 1,
    Reverse = 2,
}

impl SegmentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SegmentKind::Forward => "forward",
            SegmentKind::Reverse => "reverse",
        }
    }
    pub fn from_i64(v: i64) -> Self {
        match v {
            2 => SegmentKind::Reverse,
            _ => SegmentKind::Forward,
        }
    }
    pub fn other(self) -> Self {
        match self {
            SegmentKind::Forward => SegmentKind::Reverse,
            SegmentKind::Reverse => SegmentKind::Forward,
        }
    }
}

/// 仪器导入时声明的数据口径（归一化计算用，原始单位也原样保留）。
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Metadata {
    pub name: String,
    pub scan_rate: f64,
    pub scan_rate_unit: String,
    pub potential_unit: String,
    pub current_unit: String,
    pub time_unit: String,
    pub area: f64,
    pub area_unit: String,
    pub reference_electrode: String,
    pub tolerance_v: f64,
}

impl Default for Metadata {
    fn default() -> Self {
        Metadata {
            name: String::new(),
            scan_rate: f64::NAN,
            scan_rate_unit: "V/s".to_string(),
            potential_unit: "V".to_string(),
            current_unit: "A".to_string(),
            time_unit: "s".to_string(),
            area: f64::NAN,
            area_unit: "cm2".to_string(),
            reference_electrode: String::new(),
            tolerance_v: 0.0,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Sample {
    pub index: usize,
    pub potential_v: f64,
    pub current_a: f64,
    pub time_s: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub kind: SegmentKind,
    pub cycle: i64,
    pub start_idx: usize,
    pub end_idx: usize,
}

impl Segment {
    pub fn contains(&self, idx: usize) -> bool {
        idx >= self.start_idx && idx <= self.end_idx
    }
}

#[derive(Debug, Clone)]
pub struct RunData {
    pub meta: Metadata,
    pub samples: Vec<Sample>,
    pub labels: Vec<i8>,
    pub segments: Vec<Segment>,
}

impl RunData {
    pub fn segment_samples<'a>(&'a self, seg: &'a Segment) -> impl Iterator<Item = &'a Sample> + 'a {
        self.samples[seg.start_idx..=seg.end_idx.min(self.samples.len() - 1)].iter()
    }
}
