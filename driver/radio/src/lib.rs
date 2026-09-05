#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

#[cfg(feature = "wifi-embassy")]
pub mod runtime;
#[cfg(feature = "wifi")]
pub mod wifi;

// Preserve the application facade while modules expose the responsibility split.
#[cfg(feature = "wifi")]
pub use wifi::*;

// Compatibility with the original integration contract path.
#[cfg(feature = "wifi-embassy")]
#[doc(hidden)]
pub use runtime::embassy as embassy_supervisor;
