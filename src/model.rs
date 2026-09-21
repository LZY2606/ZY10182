use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Anodic,
    Cathodic,
}

impl Direction {
    pub fn sign(self) -> i8 {
        match self {
            Direction::Anodic => 1,
            Direction::Cathodic => -1,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Direction::Anodic => "anodic",
            Direction::Cathodic => "cathodic",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatasetMeta {
    pub name: String,
    pub reference_electrode: String,
    pub potential_unit: String,
    pub current_unit: String,
    pub time_unit: String,
    pub scan_rate: f64,
    pub scan_rate_unit: String,
    pub electrode_area: f64,
    pub electrode_area_unit: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Point {
    pub idx: usize,
    pub time: f64,
    pub potential: f64,
    pub current: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Segment {
    pub index: usize,
    pub direction: Direction,
    pub cycle: usize,
    /// inclusive owned point range: every raw point belongs to exactly one segment
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BaselineKind {
    EndpointLinear,
    LocalPoly,
    Derivative,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeakMetrics {
    pub interval_id: i64,
    pub segment_index: usize,
    pub direction: Direction,
    pub peak_idx: usize,
    pub peak_potential: f64,
    pub peak_current: f64,
    pub peak_current_raw: f64,
    pub charge: f64,
    pub interval_start: usize,
    pub interval_end: usize,
    /// effective integration range after overlap de-duplication
    pub eff_start: usize,
    pub eff_end: usize,
    pub overlap_adjusted: bool,
    pub baseline: BaselineKind,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeakPair {
    pub anodic: PeakMetrics,
    pub cathodic: PeakMetrics,
    pub delta_ep: f64,
    pub current_ratio: f64,
    pub charge_ratio: f64,
    pub score: f64,
    pub rank: usize,
    pub tied: bool,
}
