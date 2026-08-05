//! Canonical observable-effect contract and fail-closed comparison policy.

use std::collections::{BTreeMap, BTreeSet};

use crate::{MemoryAccess, ObservableEvent, Result, SymbolicValue, u32_literal};

mod compare;
mod model;
mod parser;

pub use compare::*;
pub use model::*;
pub use parser::*;

#[cfg(test)]
mod tests;
