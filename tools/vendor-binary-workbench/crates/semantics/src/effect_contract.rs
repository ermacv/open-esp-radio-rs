//! Canonical observable-effect contract and fail-closed comparison policy.

use std::collections::{BTreeMap, BTreeSet};

use crate::{MemoryAccess, ObservableEvent, Result, SymbolicValue};

mod compare;
mod model;

pub use compare::*;
pub use model::*;

#[cfg(test)]
mod tests;
