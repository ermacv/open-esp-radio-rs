//! Observable traces and the current reference-control-flow representation.

use std::collections::{BTreeMap, BTreeSet};

use super::{
    indexed_mmio::{IndexedMmioGuard, IndexedMmioRegister},
    value::{BitSource, SymbolicValue},
};

pub const DEFERRED_CALLER_MEMORY_REGION: &str = "deferred call-composed caller memory";
pub const SECONDARY_CALL_RESULT_TOKEN_FLAG: u32 = 1 << 31;

mod events;
mod flow;
mod function;
mod validation;

pub use events::*;
pub use flow::*;
pub use function::*;
pub use validation::*;

#[cfg(test)]
mod tests;
