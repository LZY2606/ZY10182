pub mod baseline;
pub mod csvio;
pub mod db;
pub mod model;
pub mod pairs;
pub mod peaks;
pub mod scaling;
pub mod segment;

pub use segment::segment_potential;

pub const EPS: f64 = 1e-12;

pub const FIXTURE_MAIN: &str = include_str!("../fixtures/cv_main.csv");
pub const FIXTURE_FAST: &str = include_str!("../fixtures/cv_fast.csv");
pub const FIXTURE_SCE: &str = include_str!("../fixtures/cv_sce.csv");
pub const FIXTURE_PLATEAU_EXT: &str = include_str!("../fixtures/cv_plateau_ext.csv");

pub fn fixtures() -> Vec<(&'static str, &'static str)> {
    vec![
        ("cv_main", FIXTURE_MAIN),
        ("cv_fast", FIXTURE_FAST),
        ("cv_sce", FIXTURE_SCE),
        ("cv_plateau_ext", FIXTURE_PLATEAU_EXT),
    ]
}
