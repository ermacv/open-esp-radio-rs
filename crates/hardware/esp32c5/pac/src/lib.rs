//! Closed typed ESP32-C5 radio peripheral access crate.
//!
//! The generated capability catalog comes from the reviewed
//! [register publication](../../../../registers/esp32c5/README.md). Owners
//! that expose its transactions are added per domain.

#![no_std]
#![forbid(unsafe_code)]

// The generated capability catalog is intentionally broader than this crate's
// restricted ownership facade. Reviewed leaves stay unreachable until an owner
// transition exposes them.
#[allow(
    dead_code,
    reason = "generated capability catalog is wider than the restricted ownership facade"
)]
mod generated;

#[allow(
    unused_imports,
    reason = "the generated catalog reaches the raw PAC through this name"
)]
use oer_esp32c5_pac_raw as svd;
