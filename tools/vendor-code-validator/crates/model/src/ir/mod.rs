//! Symbolic intermediate representation shared by analysis and code generation.

mod indexed_mmio;
mod reference;
mod trace;
mod value;

pub use indexed_mmio::{
    IndexedMmioDomain, IndexedMmioGuard, IndexedMmioRegister, collect_evaluable_input_bits,
    evaluate_for_input, indexed_mmio_domain,
};
pub use reference::*;
pub use trace::*;
pub use value::{BitSource, ExpressionOperation, PRIVATE_STACK_READ_TOKEN_FLAG, SymbolicValue};
