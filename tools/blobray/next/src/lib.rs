//! Host composition for supervised imports and the CLI's JSON wire documents.
//! Platform effects stay out of the core.
#[cfg(target_os = "linux")]
pub mod linux;
pub mod wire;
