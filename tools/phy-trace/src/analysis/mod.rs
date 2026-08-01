//! Static instruction-level analysis engines.

mod direct;
mod reference;
mod service;

pub(super) const ESP32S31_ROM_DIVDI3_ADDRESS: u32 = 0x2f81_ce6e;

#[cfg(test)]
pub(crate) use direct::{StructuralCallSite, SymbolicStack};
pub(crate) use direct::{StructuralPointerContext, trace_binary_symbol};
pub(crate) use reference::ReferenceResolver;
#[cfg(test)]
pub(crate) use reference::{inline_reference_summary, resolve_reference_trace};
pub(crate) use service::*;
