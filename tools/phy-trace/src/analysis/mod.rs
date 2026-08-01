//! Static instruction-level analysis engines.

mod direct;
mod reference;
mod service;

pub(crate) use direct::trace_binary_symbol;
#[cfg(test)]
pub(crate) use direct::{StructuralCallSite, SymbolicStack};
pub(crate) use reference::ReferenceResolver;
#[cfg(test)]
pub(crate) use reference::{inline_reference_summary, resolve_reference_trace};
pub(crate) use service::*;
