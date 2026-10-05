//! PHY HIL workloads: the vendor-versus-production calibration cross-check
//! (`[phy] kind = "vendor-calibration"`).
//!
//! The workload alternates the pinned vendor firmware and the production
//! image on the board under test within the run's lease
//! ([`oer_hil_workload::context::BoardImages`]), records each boot as a
//! typed observation (`oer-phy-calibration-capture`) and the chip's
//! comparison, reached through the [`comparison::Comparison`] port, as the
//! repetition's result. The family names no chip: a chip-specific
//! composition implements the port and registers [`family`] for it.
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

pub mod comparison;
pub mod scenario;
mod workload;

pub(crate) use oer_hil_workload::Result;

/// The PHY family, under its key `phy`, with the comparison `C`.
pub const fn family<C: comparison::Comparison>() -> oer_hil_workload::family::Kind {
    oer_hil_workload::family::Kind::of::<scenario::PhyScenario<C>>("phy")
}
