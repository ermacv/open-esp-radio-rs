//! Wi-Fi fixtures grouped by provider.
//!
//! `local` and `openwrt` own their provider's AP, client, monitors and
//! evidence; `controlled_ap` selects between them and `prepared` owns the
//! scenario's AP lifetime.
mod capture_process;
mod channel;
pub mod controlled_ap;
pub mod local;
pub mod openwrt;
pub mod prepared;
pub mod probe_load;
pub mod station_fixture;
#[cfg(all(test, unix))]
mod test_support;

pub use hil_core::fixture::Error;
