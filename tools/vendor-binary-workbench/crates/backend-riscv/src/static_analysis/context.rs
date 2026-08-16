//! Harness-provided pointer cells and relocated call identities.

use super::*;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct StructuralCallSite {
    member: Option<String>,
    symbol: String,
    address: u32,
}

impl StructuralCallSite {
    pub fn new(owner: &artifact::ArtifactSymbolDefinition, address: u32) -> Self {
        Self {
            member: owner.member.clone(),
            symbol: owner.name.clone(),
            address,
        }
    }

    pub fn from_identity(member: Option<String>, symbol: String, address: u32) -> Self {
        Self {
            member,
            symbol,
            address,
        }
    }

    pub fn belongs_to(&self, owner: &artifact::ArtifactSymbolDefinition) -> bool {
        self.member == owner.member && self.symbol == owner.name
    }

    pub const fn address(&self) -> u32 {
        self.address
    }
}

pub type StructuralRelocatedCalls = BTreeMap<StructuralCallSite, (String, Option<u32>)>;

/// One data relocation conservatively projected from an exact relocatable
/// origin onto an instruction in the authoritative linked image.
///
/// Linkers may relax two origin instructions into one linked instruction, so
/// the original offsets are retained as evidence instead of pretending that
/// function-relative offsets are stable across linking.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructuralProjectedRelocation {
    pub origin_member: Option<String>,
    pub origin_symbol: String,
    pub origin_offsets: Vec<u32>,
    pub kind: artifact::RelocationKind,
    pub symbol: String,
    pub addend: i64,
    pub correspondence: &'static str,
}

#[derive(Clone, Debug, Default)]
pub struct StructuralPointerContext {
    pub reviewed_external_pointer_cells: BTreeMap<u32, String>,
    pub function_pointer_cells: BTreeMap<u32, FunctionTableRef>,
    pub data_pointer_cells: BTreeMap<u32, SymbolicValue>,
    pub relocated_pointer_symbols: BTreeMap<String, SymbolicValue>,
    /// Exact linked instruction sites carrying archive relocation evidence.
    /// Entries are registered only after project-level digest, origin and
    /// instruction-correspondence validation.
    pub projected_relocations: BTreeMap<StructuralCallSite, Vec<StructuralProjectedRelocation>>,
    pub function_table_slots: BTreeMap<(FunctionTableRef, u32), u32>,
    /// Source-qualified identities for table targets that cross an artifact
    /// boundary. Direct code addresses remain authoritative; this map only
    /// prevents a companion-library function from inheriting the caller's
    /// source label in linked IR.
    pub function_target_identities: BTreeMap<u32, String>,
    pub diagnostic_calls: BTreeMap<String, u8>,
    pub reviewed_external_calls: BTreeMap<StructuralCallSite, Vec<ReviewedExternalCall>>,
    pub reviewed_external_slots: BTreeMap<(String, u32), Vec<ReviewedExternalCall>>,
    /// Exact internal code targets joined from observed table-slot stores.
    /// A slot is present only when one reviewed layout/name selects one
    /// uniquely linked relocation target.
    pub reviewed_internal_calls: BTreeMap<StructuralCallSite, u32>,
    pub reviewed_internal_slots: BTreeMap<(String, u32), u32>,
    pub summary_hooks: Option<&'static RiscvSummaryHooks>,
}

impl StructuralPointerContext {
    pub fn from_harness(harness: &'static RiscvHarnessSpec) -> Self {
        let contracts = harness.contracts;
        let mut context = Self::default();
        context.diagnostic_calls.extend(
            contracts
                .diagnostic_calls
                .iter()
                .map(|call| (call.symbol.to_owned(), call.argument_count)),
        );
        context.summary_hooks = Some(harness.summaries);
        context
    }
}
