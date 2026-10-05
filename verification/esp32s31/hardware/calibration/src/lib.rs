//! The vendor-versus-production PHY calibration cross-check of the
//! ESP32-S31: what is compared and how.
//!
//! The capture is the HIL `phy` family's `vendor-calibration` workload
//! (`oer-hil-family-phy`): it flashes the vendor calibration firmware and
//! the production image alternately under the run's lease and records each
//! boot as a typed observation ([`boots`]). This library defines the
//! register images both sides read ([`registers`]), the vendor console
//! protocol ([`vendor`]), the production side's projection of its retained
//! calibration ([`production`]) and the comparison ([`compare`]): the
//! reviewed `phy_param` relation the tracking scenario compares, with
//! `src/tolerances.toml` as the review, yielding a
//! MATCH, DIFF or INCOMPLETE [`compare::Summary`].

pub mod boots;
pub mod compare;
pub mod production;
pub mod registers;
pub mod vendor;

use oer_esp32s31_phy_relation::{committed, projection as calibration_projection};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
