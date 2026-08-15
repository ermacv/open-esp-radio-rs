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
pub use value::{
    ALLOCATED_EXTERNAL_RESULT_TOKEN_FLAG, BitSource, ExpressionOperation, FloatingPointOperation,
    FloatingRoundingMode, MemoryObjectLocation, MemoryObjectRoot,
    OPAQUE_POINTER_EXTERNAL_RESULT_TOKEN_FLAG, PRIVATE_STACK_READ_TOKEN_FLAG, SymbolicValue,
    SymbolicValueTree, external_result_call_token,
};
