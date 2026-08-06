//! Architecture-neutral semantic verification interfaces.
//!
//! The workbench facade dispatches these requests to an explicitly selected
//! platform harness. Implementations may bind a production driver, while the
//! interface itself contains no chip registry or driver dependency.

mod driver_plan;
mod effect_contract;

use std::path::Path;

pub use driver_plan::*;
pub use effect_contract::*;
pub use open_radio_vendor_analysis_model::*;

pub type Error = Box<dyn std::error::Error>;
pub type Result<T> = std::result::Result<T, Error>;

pub struct DriverAdapterRequest<'a> {
    pub id: &'a str,
    pub source: &'a str,
    pub vendor_symbol: &'a str,
    pub svd: &'a MmioRegisterMap,
    /// Caller-owned authoritative symbol inventory (for example a raw `.a`).
    pub vendor_inventory: Option<&'a Path>,
    /// Executable linked view used for concrete verification.
    pub vendor_artifact: &'a Path,
    pub vendor_companion: Option<&'a Path>,
    pub rust_artifact: &'a Path,
    pub rust_companion: Option<&'a Path>,
    pub rust_symbol: &'a str,
    pub policy: &'a EffectPolicy,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DriverAdapterVerification {
    pub matched: bool,
    pub canonical: String,
}

pub struct SemanticContractRequest<'a> {
    pub id: &'a str,
    pub source: &'a str,
    pub vendor_symbol: &'a str,
    pub svd: &'a MmioRegisterMap,
    pub vendor_artifact: &'a Path,
    pub vendor_companion: Option<&'a Path>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EvidenceSource {
    pub name: &'static str,
    pub contents: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DriverAdapterEvidenceSources {
    pub adapter: &'static [EvidenceSource],
    pub reviewed_summary: EvidenceSource,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SemanticContractEvidenceSources {
    pub common: &'static [EvidenceSource],
    pub contract: EvidenceSource,
}

pub(crate) fn u32_literal(value: &str) -> Option<u32> {
    let value = value.trim();
    if let Some(hex) = value.strip_prefix("0x") {
        u32::from_str_radix(hex, 16).ok()
    } else {
        value.parse().ok()
    }
}
