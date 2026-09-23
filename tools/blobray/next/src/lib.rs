//! Host composition for supervised imports. Platform effects stay out of the core.
#[cfg(target_os = "linux")]
pub mod linux;
