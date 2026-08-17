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

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct StructuralCallOwner {
    member: Option<String>,
    symbol: String,
}

impl StructuralCallOwner {
    fn from_site(site: &StructuralCallSite) -> Self {
        Self {
            member: site.member.clone(),
            symbol: site.symbol.clone(),
        }
    }

    fn new(owner: &artifact::ArtifactSymbolDefinition) -> Self {
        Self {
            member: owner.member.clone(),
            symbol: owner.name.clone(),
        }
    }
}

/// Exact relocated-call catalog with an owner-local execution index.
///
/// The exact map remains the evidence identity used by resolver operations.
/// Structural tracing never scans it: every insertion also updates the
/// immutable function-local `pc -> call` projection.
#[derive(Clone, Debug, Default)]
pub struct StructuralRelocatedCalls {
    exact: BTreeMap<StructuralCallSite, (String, Option<u32>)>,
    by_owner: BTreeMap<StructuralCallOwner, BTreeMap<u32, (String, Option<u32>)>>,
}

impl StructuralRelocatedCalls {
    pub const fn new() -> Self {
        Self {
            exact: BTreeMap::new(),
            by_owner: BTreeMap::new(),
        }
    }

    pub fn insert(
        &mut self,
        site: StructuralCallSite,
        call: (String, Option<u32>),
    ) -> Option<(String, Option<u32>)> {
        self.by_owner
            .entry(StructuralCallOwner::from_site(&site))
            .or_default()
            .insert(site.address, call.clone());
        self.exact.insert(site, call)
    }

    pub fn get(&self, site: &StructuralCallSite) -> Option<&(String, Option<u32>)> {
        self.exact.get(site)
    }

    pub fn values(&self) -> impl Iterator<Item = &(String, Option<u32>)> {
        self.exact.values()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&StructuralCallSite, &(String, Option<u32>))> {
        self.exact.iter()
    }
}

impl<const N: usize> From<[(StructuralCallSite, (String, Option<u32>)); N]>
    for StructuralRelocatedCalls
{
    fn from(calls: [(StructuralCallSite, (String, Option<u32>)); N]) -> Self {
        let mut catalog = Self::new();
        for (site, call) in calls {
            catalog.insert(site, call);
        }
        catalog
    }
}

/// Function-local integer index derived once before tracing. The project
/// catalog remains keyed by exact owner identity; the execution hot path does
/// not rebuild that string identity for every instruction.
#[derive(Debug, Default)]
pub struct StructuralRelocatedCallView<'a> {
    calls: Option<&'a BTreeMap<u32, (String, Option<u32>)>>,
}

impl<'a> StructuralRelocatedCallView<'a> {
    pub fn new(
        owner: &artifact::ArtifactSymbolDefinition,
        calls: &'a StructuralRelocatedCalls,
    ) -> Self {
        Self {
            calls: calls.by_owner.get(&StructuralCallOwner::new(owner)),
        }
    }

    pub fn get(&self, address: u32) -> Option<&(String, Option<u32>)> {
        self.calls.and_then(|calls| calls.get(&address))
    }
}

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
