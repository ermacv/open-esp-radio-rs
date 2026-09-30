//! The failure class a terminal lifecycle event reports.
//!
//! Every radio port classifies its failures alike (`docs/architecture.md`,
//! "Radio ports"): `Rejected` is a call's inner `Err`, and admitted work
//! that fails reports
//! whether the port stays usable. This module is the lower-MAC port's copy
//! of that vocabulary; it has no dependency so that a shared contract
//! package can take it over when a second port reports lifecycle failures
//! as events.

/// Whether a failed operation left the port usable.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FailureClass {
    /// The operation did not take effect and the port stays usable in the
    /// state the terminal event names.
    Recoverable,
    /// The backend's state is unknown; only a reset restores the port, and
    /// every later call returns the port's error.
    Poisoned,
}
