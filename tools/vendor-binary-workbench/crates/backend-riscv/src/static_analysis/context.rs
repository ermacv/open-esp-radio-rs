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
}

pub type StructuralRelocatedCalls = BTreeMap<StructuralCallSite, (String, Option<u32>)>;

#[derive(Clone, Debug, Default)]
pub struct StructuralPointerContext {
    pub external_pointer_cells: BTreeMap<u32, ExternalTableRef>,
    pub function_pointer_cells: BTreeMap<u32, FunctionTableRef>,
    pub data_pointer_cells: BTreeMap<u32, SymbolicValue>,
    pub relocated_pointer_symbols: BTreeMap<String, SymbolicValue>,
    pub function_table_slots: BTreeMap<(FunctionTableRef, u32), u32>,
    pub diagnostic_calls: BTreeMap<String, u8>,
    pub reviewed_external_calls: BTreeMap<StructuralCallSite, Vec<ReviewedExternalCall>>,
    pub reviewed_external_slots: BTreeMap<(String, u32), Vec<ReviewedExternalCall>>,
    pub summary_hooks: Option<&'static RiscvSummaryHooks>,
}

impl StructuralPointerContext {
    pub fn from_harness(harness: &'static RiscvHarnessSpec) -> Self {
        let contracts = harness.contracts;
        let mut context = Self::default();
        for &table in contracts.external_tables {
            context.relocated_pointer_symbols.insert(
                table.spec().pointer_symbol.to_owned(),
                SymbolicValue::ExternalTable(table),
            );
        }
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
