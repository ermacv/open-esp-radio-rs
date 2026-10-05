//! PHY HIL workloads: the vendor-versus-production calibration cross-check
//! (`[phy] kind = "vendor-calibration"`).
//!
//! The workload alternates the pinned vendor firmware and the production
//! image on the board under test within the run's lease
//! ([`oer_hil_workload::context::BoardImages`]), records each boot as a
//! typed observation and the comparison of
//! `oer-esp32s31-phy-vendor-calibration` as the repetition's result.
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

pub mod scenario;
mod workload;

pub(crate) use oer_hil_workload::Result;

/// The PHY family's key in a scenario document.
pub const FAMILY: oer_hil_workload::family::Kind =
    oer_hil_workload::family::Kind::of::<scenario::PhyScenario>("phy");
