#![forbid(unsafe_code)]

mod analyze;
mod policy;
mod render;

use std::{io, path::PathBuf};

pub use analyze::{
    Allocation, AuditReport, ConsumerReport, MemoryReport, RegionReport, SectionReport, analyze,
    audit,
};
pub use policy::{
    ConsumerRule, ConsumerScope, MemoryPolicy, PlacementReason, PlacementRequirement, RegionKind,
    RegionPolicy, ReservePolicy,
};
pub use render::{MemoryDiff, diff, render_audit, render_diff, render_report};
use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("failed to read {path}: {source}")]
    Read { path: PathBuf, source: io::Error },
    #[error("failed to parse memory policy {path}: {source}")]
    Policy {
        path: PathBuf,
        source: toml_edit::de::Error,
    },
    #[error("invalid memory policy: {0}")]
    InvalidPolicy(String),
    #[error("failed to parse ELF {path}: {message}")]
    Elf { path: PathBuf, message: String },
    #[error("memory audit failed\n{0}")]
    Audit(String),
    #[error("failed to serialize JSON: {0}")]
    Json(#[from] serde_json::Error),
}
