//! Declarative facts for the ESP32-S31 investigation.
//!
//! This crate owns semantic declarations, reviewed artifact-local RAM roles,
//! and ABI/entry contracts. It does not construct traces, execute intrinsics,
//! install summary hooks, or depend on an executable model provider.

pub use open_radio_vendor_harness_esp32s31::{CONTRACTS, entry_contract, external_abi};

mod reviewed_memory_accesses;
mod semantic_facts;

pub use reviewed_memory_accesses::CLASSIFICATIONS as REVIEWED_MEMORY_ACCESSES;
pub use semantic_facts::{PP_POST_EVENT_ROLES, PP_POST_SEMANTIC};
