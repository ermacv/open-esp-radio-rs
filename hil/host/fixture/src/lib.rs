//! Versioned contract between the unprivileged runner and the finite Linux
//! fixture helpers, which are this package's binaries.
//!
//! `open-radio-bluetooth` and `open-radio-probe` run through the fixed
//! launchers of `open-esp-radio-hil-fixture-install`. The runner parses their
//! requests and reports with the same types.

pub mod bluetooth;
pub mod probe;
