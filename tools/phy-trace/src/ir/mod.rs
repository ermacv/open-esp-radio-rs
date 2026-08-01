//! Symbolic intermediate representation shared by analysis and code generation.

mod indexed_mmio;
mod reference;
mod trace;
mod value;

#[cfg(test)]
pub(crate) use indexed_mmio::evaluate_for_input;
pub(crate) use indexed_mmio::{
    IndexedMmioDomain, IndexedMmioGuard, IndexedMmioRegister, indexed_mmio_domain,
};
pub(crate) use reference::*;
pub(crate) use trace::*;
pub(crate) use value::{BitSource, ExpressionOperation, SymbolicValue};
