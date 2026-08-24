//! Linked ELF image model and stable executable-analysis facade.

mod access;
mod closure_identity;
mod coverage;
mod loader;

use std::collections::{BTreeMap, BTreeSet, HashMap};

pub(super) const RETURN_SENTINEL: u32 = 0xffff_fffc;
pub(super) const STACK_POINTER: u32 = 0x3fff_f000;
pub(super) const STACK_SIZE: u32 = 0x1_0000;

pub(super) fn execution_stack_contains(address: u32) -> bool {
    address
        .checked_sub(STACK_POINTER.wrapping_sub(STACK_SIZE))
        .is_some_and(|offset| offset < STACK_SIZE)
}

#[derive(Clone, Debug)]
pub(super) struct Segment {
    pub(super) address: u32,
    pub(super) bytes: Vec<u8>,
    pub(super) memory_size: u32,
    pub(super) writable: bool,
}

#[derive(Clone, Debug)]
pub(super) struct RelocatedCall {
    pub(super) name: String,
    pub(super) target: Option<u32>,
}

#[derive(Clone, Debug)]
pub(super) struct UnresolvedRelocation {
    pub(super) name: String,
    pub(super) r_type: u32,
    pub(super) width: u8,
}

#[derive(Clone, Debug)]
pub struct ExecutableImage {
    pub(super) segments: Vec<Segment>,
    pub(super) symbols_by_name: HashMap<String, u32>,
    pub(super) symbols_by_address: BTreeMap<u32, String>,
    /// Exact linked sizes for text symbols which carry one in the ELF symbol
    /// table. Unlike `symbols_by_address`, this deliberately excludes
    /// absolute call targets and zero-sized labels.
    pub(super) symbol_sizes_by_address: BTreeMap<u32, u32>,
    /// Text definitions with local ELF binding. These form the implementation
    /// closure of a selected public probe; calls to another global definition
    /// remain named ABI boundaries even when that definition is linked into
    /// the same diagnostic image.
    pub(super) local_text_symbols: BTreeSet<u32>,
    pub(super) call_trampoline_addresses: BTreeSet<u32>,
    pub(super) relocated_calls_by_address: BTreeMap<u32, RelocatedCall>,
    /// Allocated bytes whose linked value still depends on an undefined
    /// symbol. Keeping these as poison lets an unrelated function in a large
    /// linked oracle run while any reachable use still fails closed.
    pub(super) unresolved_relocations_by_address: BTreeMap<u32, UnresolvedRelocation>,
    /// Reviewed opaque diagnostic boundaries supplied by composed target
    /// knowledge. The artifact proves the call site; this map classifies the
    /// named ABI arguments as diagnostic/non-observable, but supplies no
    /// return or register-clobber semantics.
    pub(super) diagnostic_calls: BTreeMap<String, u8>,
    pub(super) global_pointer: Option<u32>,
}

#[derive(Clone, Debug, Default)]
pub struct CoverageInventory {
    pub branch_sites: BTreeSet<u32>,
    pub branch_outcomes: BTreeSet<(u32, bool)>,
    pub unresolved_edges: BTreeMap<u32, String>,
}
