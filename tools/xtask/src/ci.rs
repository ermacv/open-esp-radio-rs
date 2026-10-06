//! Whole-tree CI input planning, pure coverage decisions and the GitHub
//! runner adapter. The check registry declares the workflow topology.

pub mod coverage;
mod environment;
mod github;
pub mod model;
pub mod planning;
mod runner;

pub use environment::write_environment;
pub use planning::plan;
pub use runner::{check_environment, prepare, verify};
