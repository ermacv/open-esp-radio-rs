//! Artifact-level symbol catalog and reference resolver.

use super::*;
use crate::{DirectSemanticFunctionSpec, EntryContractRef, FunctionTarget, RiscvHarnessSpec};

pub type ReferenceSymbolKey = (Option<String>, String, u64);

pub struct ReferenceResolver {
    pub symbols: Vec<artifact::ArtifactSymbolDefinition>,
    pub symbols_by_address: BTreeMap<u32, artifact::ArtifactSymbolDefinition>,
    pub symbol_ids: BTreeMap<ReferenceSymbolKey, u32>,
    pub exported_symbol_keys: BTreeSet<ReferenceSymbolKey>,
    pub relocated_calls: StructuralRelocatedCalls,
    pub pointer_context: StructuralPointerContext,
    /// Sized data definitions used to rebase absolute RAM observations.
    ///
    /// Public for construction of synthetic resolver fixtures. Production
    /// callers should use one of the `load*` constructors.
    pub data_symbols: Vec<artifact::ArtifactDataSymbolDefinition>,
    /// Bounded static objects retained for relocation-aware indexed dispatch
    /// recovery. Synthetic labels are clipped to the next symbol by the
    /// artifact loader, so this does not duplicate section tails.
    pub data_objects: Vec<artifact::ArtifactDataObjectDefinition>,
    /// Reviewed exact-body semantics projected from a unique archive origin
    /// onto the authoritative linked definition. The facade owns origin and
    /// digest validation; the backend only stores the resulting typed fact.
    pub projected_direct_semantics:
        BTreeMap<ReferenceSymbolKey, &'static DirectSemanticFunctionSpec>,
    /// Exact relocatable definitions associated with authoritative linked
    /// functions by the project layer. Kept separate from linker-selected
    /// symbols: these provide lossless structural relocation evidence only.
    pub projected_origins: BTreeMap<ReferenceSymbolKey, artifact::ArtifactSymbolDefinition>,
}

fn symbol_key(symbol: &artifact::ArtifactSymbolDefinition) -> ReferenceSymbolKey {
    (symbol.member.clone(), symbol.name.clone(), symbol.address)
}

fn insert_preferred_symbol(
    output: &mut BTreeMap<u32, artifact::ArtifactSymbolDefinition>,
    symbol: artifact::ArtifactSymbolDefinition,
    exported_symbol_keys: &BTreeSet<ReferenceSymbolKey>,
) {
    let address = symbol.address as u32;
    let replace = output.get(&address).is_none_or(|current| {
        let candidate_exported = exported_symbol_keys.contains(&symbol_key(&symbol));
        let current_exported = exported_symbol_keys.contains(&symbol_key(current));
        (candidate_exported && !current_exported)
            || (candidate_exported == current_exported
                && (&symbol.member, &symbol.name, symbol.address)
                    < (&current.member, &current.name, current.address))
    });
    if replace {
        output.insert(address, symbol);
    }
}

impl ReferenceResolver {
    pub fn register_projected_direct_semantic(
        &mut self,
        symbol: &artifact::ArtifactSymbolDefinition,
        semantic: &'static DirectSemanticFunctionSpec,
    ) {
        self.projected_direct_semantics
            .insert(symbol_key(symbol), semantic);
    }

    pub fn projected_direct_semantic(
        &self,
        symbol: &artifact::ArtifactSymbolDefinition,
    ) -> Option<&'static DirectSemanticFunctionSpec> {
        self.projected_direct_semantics
            .get(&symbol_key(symbol))
            .copied()
    }

    pub fn register_projected_origin(
        &mut self,
        linked: &artifact::ArtifactSymbolDefinition,
        origin: artifact::ArtifactSymbolDefinition,
    ) {
        self.projected_origins.insert(symbol_key(linked), origin);
    }

    pub fn projected_origin(
        &self,
        linked: &artifact::ArtifactSymbolDefinition,
    ) -> Option<&artifact::ArtifactSymbolDefinition> {
        self.projected_origins.get(&symbol_key(linked))
    }

    /// Replace a linked-address call label with the exact relocatable-origin
    /// identity selected by the project layer.
    ///
    /// A linkable analysis image may fold several deliberately opaque stubs
    /// onto one address. In that case the address does not identify a callee
    /// body: retain the origin relocation name and leave the target unresolved
    /// instead of choosing an arbitrary alias from `symbols_by_address`.
    pub fn register_projected_call_relocation(
        &mut self,
        owner: &artifact::ArtifactSymbolDefinition,
        runtime_address: u32,
        symbol: &str,
        addend: i64,
    ) {
        let matching_addresses = self
            .symbols
            .iter()
            .filter(|candidate| candidate.addresses_resolved && candidate.name == symbol)
            .map(|candidate| candidate.address as u32)
            .collect::<BTreeSet<_>>();
        let target = if addend == 0 && matching_addresses.len() == 1 {
            let address = *matching_addresses
                .first()
                .expect("one matching linked call target");
            let aliases = self
                .symbols
                .iter()
                .filter(|candidate| {
                    candidate.addresses_resolved && candidate.address as u32 == address
                })
                .map(|candidate| candidate.name.as_str())
                .collect::<BTreeSet<_>>();
            (aliases.len() == 1).then_some(address)
        } else {
            None
        };
        self.relocated_calls.insert(
            StructuralCallSite::new(owner, runtime_address),
            (symbol.to_owned(), target),
        );
    }
    pub fn load(
        artifact: &Path,
        companions: &[PathBuf],
        harness: &'static RiscvHarnessSpec,
    ) -> Result<Self> {
        let entry_contract = harness
            .contracts
            .entry_contract("none")
            .ok_or("selected RISC-V adapter has no neutral entry contract")?;
        Self::load_with_entry_contract(artifact, companions, harness, entry_contract)
    }

    pub fn load_with_entry_contract(
        artifact: &Path,
        companions: &[PathBuf],
        harness: &'static RiscvHarnessSpec,
        entry_contract: EntryContractRef,
    ) -> Result<Self> {
        Self::load_catalog_with_entry_contract(
            artifact,
            companions,
            harness,
            entry_contract,
            artifact::CodeSymbolSelection::Exported,
            &[],
        )
    }

    /// Load the broader exploratory catalog used by IR export.
    ///
    /// Unlike validation inventory, this includes local/private sized text
    /// symbols. Stripped code remains outside the catalog.
    pub fn load_all_code_with_entry_contract(
        artifact: &Path,
        companions: &[PathBuf],
        harness: &'static RiscvHarnessSpec,
        entry_contract: EntryContractRef,
    ) -> Result<Self> {
        Self::load_catalog_with_entry_contract(
            artifact,
            companions,
            harness,
            entry_contract,
            artifact::CodeSymbolSelection::All,
            &[],
        )
    }

    /// Load all ordinary symbols plus explicit human-reviewed section ranges.
    pub fn load_all_code_with_reviewed_ranges(
        artifact: &Path,
        companions: &[PathBuf],
        harness: &'static RiscvHarnessSpec,
        entry_contract: EntryContractRef,
        reviewed: &[artifact::ReviewedCodeRange],
    ) -> Result<Self> {
        Self::load_catalog_with_entry_contract(
            artifact,
            companions,
            harness,
            entry_contract,
            artifact::CodeSymbolSelection::All,
            reviewed,
        )
    }

    fn load_catalog_with_entry_contract(
        artifact: &Path,
        companions: &[PathBuf],
        harness: &'static RiscvHarnessSpec,
        entry_contract: EntryContractRef,
        selection: artifact::CodeSymbolSelection,
        reviewed: &[artifact::ReviewedCodeRange],
    ) -> Result<Self> {
        let exported_symbols =
            artifact::load_code_symbols(artifact, "", artifact::CodeSymbolSelection::Exported)?;
        let exported_symbol_keys = exported_symbols
            .iter()
            .map(symbol_key)
            .collect::<BTreeSet<_>>();
        let mut address_preferred_symbol_keys = exported_symbol_keys.clone();
        let mut symbols = if selection == artifact::CodeSymbolSelection::All {
            artifact::load_code_symbols(artifact, "", selection)?
        } else {
            exported_symbols
        };
        symbols.extend(artifact::load_reviewed_code_ranges(artifact, reviewed)?);
        symbols.sort_by(|left, right| {
            (&left.member, &left.name, left.address).cmp(&(
                &right.member,
                &right.name,
                right.address,
            ))
        });
        let mut symbols_by_address = BTreeMap::new();
        for symbol in symbols
            .iter()
            .filter(|symbol| symbol.addresses_resolved)
            .cloned()
        {
            insert_preferred_symbol(
                &mut symbols_by_address,
                symbol,
                &address_preferred_symbol_keys,
            );
        }
        let mut symbol_ids = symbols
            .iter()
            .filter(|symbol| symbol.addresses_resolved)
            .map(|symbol| (symbol_key(symbol), symbol.address as u32))
            .collect::<BTreeMap<_, _>>();
        let mut next_archive_symbol_id = 0x8000_0000_u32;
        for symbol in symbols.iter().filter(|symbol| !symbol.addresses_resolved) {
            while symbols_by_address.contains_key(&next_archive_symbol_id) {
                next_archive_symbol_id = next_archive_symbol_id.wrapping_add(1);
            }
            let identity = symbol_key(symbol);
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
        let mut data_symbols = artifact::load_data_symbols(artifact)?;
        let data_objects = artifact::load_data_objects(artifact)?;
        for companion in companions {
            let Some(image) = image.as_mut() else {
                return Err(format!(
                    "reference companions require a linked ELF primary artifact: {}",
                    artifact.display()
                )
                .into());
            };
            image.add_companion(companion)?;
            data_symbols.extend(artifact::load_data_symbols(companion)?);
            let companion_exported_symbols = artifact::load_code_symbols(
                companion,
                "",
                artifact::CodeSymbolSelection::Exported,
            )?;
            address_preferred_symbol_keys.extend(companion_exported_symbols.iter().map(symbol_key));
            let companion_symbols = if selection == artifact::CodeSymbolSelection::All {
                artifact::load_code_symbols(companion, "", selection)?
            } else {
                companion_exported_symbols
            };
            for symbol in companion_symbols
                .into_iter()
                .filter(|symbol| symbol.addresses_resolved)
            {
                insert_preferred_symbol(
                    &mut symbols_by_address,
                    symbol,
                    &address_preferred_symbol_keys,
                );
            }
        }
        data_symbols.sort_by(|left, right| {
            (
                left.size,
                !left.exported,
                left.address,
                &left.member,
                &left.name,
            )
                .cmp(&(
                    right.size,
                    !right.exported,
                    right.address,
                    &right.member,
                    &right.name,
                ))
        });
        let mut pointer_context = StructuralPointerContext::from_harness(harness);
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
            let identity = symbol_key(symbol);
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
        for (name, target) in relocated_calls.values() {
            let Some(target) = *target else {
                continue;
            };
            if symbols_by_address.contains_key(&target)
                || (harness.summaries.direct_external_semantic)(name).is_none()
            {
                continue;
            }
            symbols_by_address.insert(
                target,
                artifact::ArtifactSymbolDefinition {
                    member: None,
                    name: name.clone(),
                    address: u64::from(target),
                    bytes: Vec::new(),
                    addresses_resolved: true,
                    memory_regions: Default::default(),
                    relocations: Vec::new(),
                },
            );
        }
        Ok(Self {
            symbols,
            symbols_by_address,
            symbol_ids,
            exported_symbol_keys,
            relocated_calls,
            pointer_context,
            data_symbols,
            data_objects,
            projected_direct_semantics: BTreeMap::new(),
            projected_origins: BTreeMap::new(),
        })
    }

    pub fn trace(
        &self,
        member: Option<&str>,
        name: &str,
        svd: &MmioMap,
    ) -> Result<FunctionAnalysis> {
        let symbol = self
            .symbols
            .iter()
            .find(|candidate| {
                candidate.name == name
                    && member.is_none_or(|member| candidate.member.as_deref() == Some(member))
            })
            .ok_or_else(|| format!("symbol {name} in member {member:?} was not found"))?;
        self.trace_symbol(symbol, svd)
    }

    pub fn trace_symbol(
        &self,
        symbol: &artifact::ArtifactSymbolDefinition,
        svd: &MmioMap,
    ) -> Result<FunctionAnalysis> {
        let identity = symbol_key(symbol);
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

    pub fn trace_symbol_bounded(
        &self,
        symbol: &artifact::ArtifactSymbolDefinition,
        svd: &MmioMap,
        budget: StructuralTraceBudget,
    ) -> Result<FunctionAnalysis> {
        let identity = symbol_key(symbol);
        let symbol_id = *self
            .symbol_ids
            .get(&identity)
            .expect("catalog lookup returned a symbol without an identity");
        let mut visiting = BTreeSet::from([symbol.address as u32, symbol_id]);
        let context = ReferenceCalleeContext {
            symbols_by_address: &self.symbols_by_address,
            relocated_calls: &self.relocated_calls,
            pointer_context: &self.pointer_context,
            svd,
            budget,
        };
        resolve_reference_trace_with_budget(symbol, &context, None, &mut visiting)
    }

    pub fn symbol_is_exported(&self, symbol: &artifact::ArtifactSymbolDefinition) -> bool {
        self.exported_symbol_keys.contains(&symbol_key(symbol))
    }

    /// Resolve a concrete memory access to the narrowest containing data
    /// symbol. Exported aliases win ties, while unresolved/zero-sized symbols
    /// never participate.
    pub fn data_symbol_location(
        &self,
        address: u32,
        width: u8,
    ) -> Option<(Option<&str>, &str, i64)> {
        if width == 0 || !width.is_multiple_of(8) {
            return None;
        }
        let bytes = u32::from(width / 8);
        let end = address.checked_add(bytes)?;
        self.data_symbols
            .iter()
            .find(|symbol| {
                address >= symbol.address && end <= symbol.address.saturating_add(symbol.size)
            })
            .map(|symbol| {
                (
                    symbol.member.as_deref(),
                    symbol.name.as_str(),
                    i64::from(address - symbol.address),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolver_with_data_symbols(
        mut data_symbols: Vec<artifact::ArtifactDataSymbolDefinition>,
    ) -> ReferenceResolver {
        data_symbols.sort_by(|left, right| {
            (
                left.size,
                !left.exported,
                left.address,
                &left.member,
                &left.name,
            )
                .cmp(&(
                    right.size,
                    !right.exported,
                    right.address,
                    &right.member,
                    &right.name,
                ))
        });
        ReferenceResolver {
            symbols: Vec::new(),
            symbols_by_address: BTreeMap::new(),
            symbol_ids: BTreeMap::new(),
            exported_symbol_keys: BTreeSet::new(),
            relocated_calls: BTreeMap::new(),
            pointer_context: StructuralPointerContext::default(),
            data_symbols,
            data_objects: Vec::new(),
            projected_direct_semantics: BTreeMap::new(),
            projected_origins: BTreeMap::new(),
        }
    }

    #[test]
    fn projected_call_identity_wins_over_folded_linked_stub_alias() {
        let owner = artifact::ArtifactSymbolDefinition {
            member: None,
            name: "lmacProcessTxComplete".to_owned(),
            address: 0x1000,
            bytes: vec![0; 8],
            addresses_resolved: true,
            memory_regions: Default::default(),
            relocations: Vec::new(),
        };
        let stub = |name: &str| artifact::ArtifactSymbolDefinition {
            member: None,
            name: name.to_owned(),
            address: 0x2000,
            bytes: vec![0; 2],
            addresses_resolved: true,
            memory_regions: Default::default(),
            relocations: Vec::new(),
        };
        let mut resolver = resolver_with_data_symbols(Vec::new());
        resolver.symbols = vec![owner.clone(), stub("__adddf3"), stub("__ctzsi2")];
        resolver.relocated_calls.insert(
            StructuralCallSite::new(&owner, 0x1000),
            ("__adddf3".to_owned(), Some(0x2000)),
        );

        resolver.register_projected_call_relocation(&owner, 0x1000, "__ctzsi2", 0);

        assert_eq!(
            resolver
                .relocated_calls
                .get(&StructuralCallSite::new(&owner, 0x1000)),
            Some(&("__ctzsi2".to_owned(), None))
        );
    }

    #[test]
    fn projected_call_accepts_linker_relaxed_jal_form() {
        let owner = artifact::ArtifactSymbolDefinition {
            member: None,
            name: "lmacProcessRxSucData".to_owned(),
            address: 0x1006_941e,
            bytes: vec![
                0xef, 0xe0, 0xaf, 0xa6, // jal ra, 0x10067688 <pp_post>
                0x67, 0x80, 0x00, 0x00, // ret
            ],
            addresses_resolved: true,
            memory_regions: Default::default(),
            relocations: Vec::new(),
        };
        let callee = artifact::ArtifactSymbolDefinition {
            member: None,
            name: "pp_post".to_owned(),
            address: 0x1006_7688,
            bytes: vec![0x67, 0x80, 0x00, 0x00], // ret
            addresses_resolved: true,
            memory_regions: Default::default(),
            relocations: Vec::new(),
        };
        let mut resolver = resolver_with_data_symbols(Vec::new());
        resolver.symbols = vec![owner.clone(), callee.clone()];
        resolver
            .symbols_by_address
            .insert(owner.address as u32, owner.clone());
        resolver
            .symbols_by_address
            .insert(callee.address as u32, callee.clone());
        resolver.symbol_ids.insert(symbol_key(&owner), 1);
        resolver.symbol_ids.insert(symbol_key(&callee), 2);

        resolver.register_projected_call_relocation(&owner, owner.address as u32, "pp_post", 0);
        let analysis = resolver
            .trace_symbol(
                &owner,
                &MmioMap {
                    registers: Vec::new(),
                    regions: Vec::new(),
                },
            )
            .expect("relaxed projected call should remain analyzable");

        assert!(
            analysis
                .reference_blockers
                .iter()
                .all(|blocker| !blocker.contains("malformed-call-relocation")),
            "unexpected blockers: {:?}",
            analysis.reference_blockers
        );
        assert!(
            analysis
                .reference_dependencies
                .iter()
                .any(|dependency| dependency.contains("pp_post"))
        );
    }

    #[test]
    fn data_symbol_location_prefers_narrow_exported_evidence_and_checks_width() {
        let resolver = resolver_with_data_symbols(vec![
            artifact::ArtifactDataSymbolDefinition {
                member: None,
                name: "image".to_owned(),
                address: 0x1000,
                size: 0x100,
                exported: true,
            },
            artifact::ArtifactDataSymbolDefinition {
                member: None,
                name: "private_state".to_owned(),
                address: 0x1020,
                size: 0x20,
                exported: false,
            },
            artifact::ArtifactDataSymbolDefinition {
                member: None,
                name: "state".to_owned(),
                address: 0x1020,
                size: 0x20,
                exported: true,
            },
        ]);

        assert_eq!(
            resolver.data_symbol_location(0x1024, 32),
            Some((None, "state", 4))
        );
        assert_eq!(
            resolver.data_symbol_location(0x103f, 16),
            Some((None, "image", 0x3f))
        );
        assert_eq!(resolver.data_symbol_location(0x1024, 7), None);
        assert_eq!(resolver.data_symbol_location(u32::MAX, 32), None);
    }
}
