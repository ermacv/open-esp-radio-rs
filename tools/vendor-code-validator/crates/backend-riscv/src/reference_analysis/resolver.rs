//! Artifact-level symbol catalog and reference resolver.

use super::*;
use crate::{EntryContractRef, FunctionTarget, RiscvHarnessSpec};

pub struct ReferenceResolver {
    pub symbols: Vec<artifact::ArtifactSymbolDefinition>,
    pub symbols_by_address: BTreeMap<u32, artifact::ArtifactSymbolDefinition>,
    pub symbol_ids: BTreeMap<(Option<String>, String), u32>,
    pub relocated_calls: StructuralRelocatedCalls,
    pub pointer_context: StructuralPointerContext,
}

impl ReferenceResolver {
    pub fn load(
        artifact: &Path,
        companions: &[PathBuf],
        harness: &'static RiscvHarnessSpec,
    ) -> Result<Self> {
        let entry_contract = harness
            .contracts
            .entry_contract("none")
            .ok_or("selected harness has no neutral entry contract")?;
        Self::load_with_entry_contract(artifact, companions, harness, entry_contract)
    }

    pub fn load_with_entry_contract(
        artifact: &Path,
        companions: &[PathBuf],
        harness: &'static RiscvHarnessSpec,
        entry_contract: EntryContractRef,
    ) -> Result<Self> {
        let symbols = artifact::load_symbols(artifact, "")?;
        let mut symbols_by_address = symbols
            .iter()
            .filter(|symbol| symbol.addresses_resolved)
            .map(|symbol| (symbol.address as u32, symbol.clone()))
            .collect::<BTreeMap<_, _>>();
        let mut symbol_ids = symbols
            .iter()
            .filter(|symbol| symbol.addresses_resolved)
            .map(|symbol| {
                (
                    (symbol.member.clone(), symbol.name.clone()),
                    symbol.address as u32,
                )
            })
            .collect::<BTreeMap<_, _>>();
        let mut next_archive_symbol_id = 0x8000_0000_u32;
        for symbol in symbols.iter().filter(|symbol| !symbol.addresses_resolved) {
            while symbols_by_address.contains_key(&next_archive_symbol_id) {
                next_archive_symbol_id = next_archive_symbol_id.wrapping_add(1);
            }
            let identity = (symbol.member.clone(), symbol.name.clone());
            if symbol_ids
                .insert(identity.clone(), next_archive_symbol_id)
                .is_some()
            {
                return Err(format!(
                    "duplicate archive symbol identity {:?}::{}",
                    identity.0, identity.1
                )
                .into());
            }
            symbols_by_address.insert(next_archive_symbol_id, symbol.clone());
            next_archive_symbol_id = next_archive_symbol_id.wrapping_add(1);
        }
        let mut image = if symbols.iter().any(|symbol| symbol.addresses_resolved) {
            Some(execution::ExecutableImage::load(artifact)?)
        } else {
            None
        };
        for companion in companions {
            let Some(image) = image.as_mut() else {
                return Err(format!(
                    "reference companions require a linked ELF primary artifact: {}",
                    artifact.display()
                )
                .into());
            };
            image.add_companion(companion)?;
            symbols_by_address.extend(
                artifact::load_symbols(companion, "")?
                    .into_iter()
                    .filter(|symbol| symbol.addresses_resolved)
                    .map(|symbol| (symbol.address as u32, symbol)),
            );
        }
        let mut pointer_context = StructuralPointerContext::from_harness(harness);
        for &table in harness.contracts.external_tables {
            let spec = table.spec();
            pointer_context.relocated_pointer_symbols.insert(
                spec.pointer_symbol.to_owned(),
                SymbolicValue::ExternalTable(table),
            );
            if let Some(address) = image
                .as_ref()
                .and_then(|image| image.symbol_address(spec.pointer_symbol))
            {
                pointer_context
                    .external_pointer_cells
                    .insert(address, table);
            }
        }
        let entry_spec = entry_contract.spec();
        if let Some(table) = entry_spec.function_table {
            let image = image.as_ref().ok_or_else(|| {
                format!(
                    "entry contract {} requires a linked ELF artifact",
                    entry_contract.id()
                )
            })?;
            for &pointer_symbol in entry_spec.pointer_symbols {
                let address = image.symbol_address(pointer_symbol).ok_or_else(|| {
                    format!(
                        "entry contract {} requires pointer symbol {pointer_symbol}",
                        entry_contract.id()
                    )
                })?;
                pointer_context
                    .function_pointer_cells
                    .insert(address, table);
                pointer_context.relocated_pointer_symbols.insert(
                    pointer_symbol.to_owned(),
                    SymbolicValue::FunctionTable(table),
                );
            }
            for (offset, target) in table.targets() {
                let target = match target {
                    FunctionTarget::Address(address) => address,
                    FunctionTarget::Symbol(symbol) => {
                        image.symbol_address(symbol).ok_or_else(|| {
                            format!(
                                "entry contract {} requires function symbol {symbol}",
                                entry_contract.id()
                            )
                        })?
                    }
                };
                if !symbols_by_address.contains_key(&target) {
                    return Err(format!(
                        "entry contract {} target {target:#010x} has no code symbol",
                        entry_contract.id()
                    )
                    .into());
                }
                pointer_context
                    .function_table_slots
                    .insert((table, offset), target);
            }
            if let Some(binding) = entry_spec.data_pointer_binding {
                let pointer_symbol = binding.pointer_symbol;
                let target_symbol = binding.target_symbol;
                let pointer_address = image.symbol_address(pointer_symbol).ok_or_else(|| {
                    format!(
                        "entry contract {} requires pointer symbol {pointer_symbol}",
                        entry_contract.id()
                    )
                })?;
                image.symbol_address(target_symbol).ok_or_else(|| {
                    format!(
                        "entry contract {} requires data symbol {target_symbol}",
                        entry_contract.id()
                    )
                })?;
                let value = SymbolicValue::SymbolAddress {
                    member: None,
                    symbol: target_symbol.to_owned(),
                    hi_addend: 0,
                    lo_addend: Some(0),
                    post_offset: 0,
                };
                pointer_context
                    .data_pointer_cells
                    .insert(pointer_address, value.clone());
                pointer_context
                    .relocated_pointer_symbols
                    .insert(pointer_symbol.to_owned(), value);
            }
        }
        let mut relocated_calls = StructuralRelocatedCalls::new();
        if let Some(image) = image.as_ref() {
            for (address, call) in image.relocated_calls() {
                let Some(owner) = symbols_by_address.values().find(|symbol| {
                    symbol.addresses_resolved
                        && address >= symbol.address as u32
                        && address < (symbol.address as u32).wrapping_add(symbol.bytes.len() as u32)
                }) else {
                    continue;
                };
                relocated_calls.insert(StructuralCallSite::new(owner, address), call);
            }
        }

        let mut archive_definitions = BTreeMap::<String, Vec<(Option<String>, u32)>>::new();
        for symbol in symbols.iter().filter(|symbol| !symbol.addresses_resolved) {
            let identity = (symbol.member.clone(), symbol.name.clone());
            archive_definitions
                .entry(symbol.name.clone())
                .or_default()
                .push((
                    symbol.member.clone(),
                    *symbol_ids
                        .get(&identity)
                        .expect("every archive symbol received a synthetic identity"),
                ));
        }
        for owner in symbols.iter().filter(|symbol| !symbol.addresses_resolved) {
            for relocation in owner.relocations.iter().filter(|relocation| {
                matches!(
                    relocation.kind,
                    artifact::RelocationKind::Call | artifact::RelocationKind::CallPlt
                )
            }) {
                let candidates = archive_definitions
                    .get(&relocation.symbol)
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                let same_member = candidates
                    .iter()
                    .filter(|(member, _)| member == &owner.member)
                    .map(|(_, target)| *target)
                    .collect::<Vec<_>>();
                let target = if relocation.addend != 0 {
                    None
                } else if same_member.len() == 1 {
                    Some(same_member[0])
                } else if candidates.len() == 1 {
                    Some(candidates[0].1)
                } else {
                    None
                };
                relocated_calls.insert(
                    StructuralCallSite::new(owner, relocation.address),
                    (relocation.symbol.clone(), target),
                );
            }
        }
        Ok(Self {
            symbols,
            symbols_by_address,
            symbol_ids,
            relocated_calls,
            pointer_context,
        })
    }

    pub fn trace(
        &self,
        member: Option<&str>,
        name: &str,
        svd: &MmioRegisterMap,
    ) -> Result<FunctionAnalysis> {
        let symbol = self
            .symbols
            .iter()
            .find(|candidate| {
                candidate.name == name
                    && member.is_none_or(|member| candidate.member.as_deref() == Some(member))
            })
            .ok_or_else(|| format!("symbol {name} in member {member:?} was not found"))?;
        let identity = (symbol.member.clone(), symbol.name.clone());
        let symbol_id = *self
            .symbol_ids
            .get(&identity)
            .expect("catalog lookup returned a symbol without an identity");
        let mut visiting = BTreeSet::from([symbol.address as u32, symbol_id]);
        resolve_reference_trace(
            symbol,
            &self.symbols_by_address,
            &self.relocated_calls,
            &self.pointer_context,
            None,
            svd,
            &mut visiting,
        )
    }
}
