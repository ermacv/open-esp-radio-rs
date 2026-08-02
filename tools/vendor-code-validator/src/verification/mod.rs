//! Verification manifests and concrete equivalence profiles.

pub(crate) mod bindings;
pub(crate) mod dispositions;
pub(crate) use open_radio_vendor_validator_semantic as effect_contract;
mod evidence;
mod execution;
pub(crate) mod profiles;
mod verify;

pub(crate) use evidence::*;
pub(crate) use execution::*;
pub(crate) use verify::*;
