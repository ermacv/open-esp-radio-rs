//! Static instruction-level analysis engines.

pub(crate) use open_radio_vendor_backend_riscv::static_analysis as direct;
mod effective_code;
mod interface_tables;
mod linkage;
mod linked_ir;
mod mmio_discovery;
mod service;

pub(crate) use direct::{RiscvHarnessSpec, StructuralPointerContext, trace_binary_symbol};
#[cfg(test)]
pub(crate) use direct::{RiscvSummaryHooks, StructuralCallSite, SymbolicStack};
pub(crate) use effective_code::*;
pub(crate) use interface_tables::*;
pub(crate) use linkage::*;
pub(crate) use linked_ir::*;
pub(crate) use mmio_discovery::*;
pub(crate) use open_radio_vendor_backend_riscv::ReferenceResolver;
#[cfg(test)]
pub(crate) use open_radio_vendor_backend_riscv::reference_analysis::{
    inline_reference_summary, resolve_reference_trace,
};
pub(crate) use service::*;
