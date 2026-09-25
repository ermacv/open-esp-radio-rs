//! Shared host-only fixture execution and installation contracts.

#[path = "fixture/bluetooth/contract.rs"]
pub mod bluetooth_fixture_contract;

pub mod fixture_install;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
