//! Shared host-only fixture installation contracts.

pub mod fixture_install;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
