//! The reviewed relation between the vendor `phy_param` object and the
//! committed calibration words production publishes, in one place: the
//! hardware calibration cross-check compares them between the vendor
//! `phy_param` ([`committed`]) and a production snapshot captured on the
//! board (`projection`, feature `projection`).
#![no_std]

pub mod committed;
#[cfg(feature = "projection")]
pub mod projection;
