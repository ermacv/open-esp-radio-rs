//! Static instruction-level analysis engines.

pub(crate) use open_radio_vendor_backend_riscv::static_analysis as direct;
mod service;

pub(crate) use direct::{RiscvHarnessSpec, StructuralPointerContext, trace_binary_symbol};
#[cfg(test)]
pub(crate) use direct::{RiscvSummaryHooks, StructuralCallSite, SymbolicStack};
pub(crate) use open_radio_vendor_backend_riscv::ReferenceResolver;
#[cfg(test)]
pub(crate) use open_radio_vendor_backend_riscv::reference_analysis::{
    inline_reference_summary, resolve_reference_trace,
};
pub(crate) use service::*;
