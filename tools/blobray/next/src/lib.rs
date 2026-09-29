//! Host adapters of Blobray's in-process operations and the CLI's JSON wire
//! documents. Platform effects stay out of the core.
#[cfg(target_os = "linux")]
pub mod linux;
pub mod wire;
