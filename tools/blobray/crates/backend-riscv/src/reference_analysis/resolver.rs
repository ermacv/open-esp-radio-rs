//! Artifact-level symbol catalog and reference resolver.

use super::*;
use crate::{DirectSemanticFunctionSpec, EntryContractRef, FunctionTarget, RiscvHarnessSpec};

pub type ReferenceSymbolKey = artifact::CodeIdentity;

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
    /// Body-bound proofs and explicit gaps for reviewed origin candidates.
    /// Entries can only be added through backend verification.
    pub projected_direct_semantics: SemanticProjectionCatalog,
    /// Exact relocatable definitions associated with authoritative linked
    /// functions by the project layer. Kept separate from linker-selected
    /// symbols: these provide lossless structural relocation evidence only.
    pub projected_origins: BTreeMap<ReferenceSymbolKey, Vec<artifact::ArtifactSymbolDefinition>>,
}

fn symbol_key(symbol: &artifact::ArtifactSymbolDefinition) -> ReferenceSymbolKey {
    symbol.identity.clone()
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

fn unique_exported_address(
    addresses_by_name: &BTreeMap<String, BTreeSet<u32>>,
    name: &str,
) -> Option<u32> {
    let addresses = addresses_by_name.get(name)?;
    (addresses.len() == 1).then(|| *addresses.first().expect("one exported address"))
}

impl ReferenceResolver {
    pub fn review_projected_direct_semantic(
        &mut self,
        linked: &artifact::ArtifactSymbolDefinition,
        origin: &artifact::ArtifactSymbolDefinition,
    ) -> Option<SemanticProjectionStatus> {
        let hooks = self.pointer_context.summary_hooks?;
        self.projected_direct_semantics
            .observe(origin, linked, hooks)
    }

    pub fn projected_direct_semantic(
        &self,
        symbol: &artifact::ArtifactSymbolDefinition,
    ) -> Option<&'static DirectSemanticFunctionSpec> {
        self.projected_direct_semantics.semantic(symbol)
    }

    pub fn register_projected_origin(
        &mut self,
        linked: &artifact::ArtifactSymbolDefinition,
        origin: artifact::ArtifactSymbolDefinition,
    ) {
        let candidates = self
            .projected_origins
            .entry(symbol_key(linked))
            .or_default();
        if !candidates.contains(&origin) {
            candidates.push(origin);
        }
    }

    pub fn projected_origin(
        &self,
        linked: &artifact::ArtifactSymbolDefinition,
    ) -> Option<&artifact::ArtifactSymbolDefinition> {
        match self.projected_origin_candidates(linked) {
            [only] => Some(only),
            _ => None,
        }
    }

    pub fn projected_origin_candidates(
        &self,
        linked: &artifact::ArtifactSymbolDefinition,
    ) -> &[artifact::ArtifactSymbolDefinition] {
        self.projected_origins
            .get(&symbol_key(linked))
            .map(Vec::as_slice)
            .unwrap_or_default()
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
        let site = StructuralCallSite::new(owner, runtime_address);
        // Resolver construction may already have joined this exact unresolved
        // relocation name to one exported companion definition.  Origin
        // projection refines provenance; it must not erase that proven target.
        // A different projected name is deliberately not preserved because
        // folded/aliased linked stubs are precisely why origin identity wins.
        let preserved_target = (addend == 0)
            .then(|| self.relocated_calls.get(&site))
            .flatten()
            .and_then(|(existing_name, target)| {
                (existing_name == symbol).then_some(*target).flatten()
            });
        let matching_addresses = self
            .symbols
            .iter()
            .filter(|candidate| candidate.addresses_resolved && candidate.name == symbol)
            .map(|candidate| candidate.address as u32)
            .collect::<BTreeSet<_>>();
        let target = preserved_target.or_else(|| {
            if addend == 0 && matching_addresses.len() == 1 {
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
            }
        });
        self.relocated_calls
            .insert(site, (symbol.to_owned(), target));
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
        let capture = artifact::CapturedArtifact::open(artifact)?;
        let captured_companions = companions
            .iter()
            .map(|path| artifact::CapturedArtifact::open(path))
            .collect::<Result<Vec<_>>>()?;
        Self::from_captured(
            &capture,
            &captured_companions.iter().collect::<Vec<_>>(),
            harness,
            entry_contract,
            selection,
            reviewed,
        )
    }

    /// Construct one resolver from a caller-owned immutable binary input set.
    /// Code, data initializers and the private execution image consume these
    /// exact captures. This constructor performs no filesystem reads.
    pub fn from_captured(
        capture: &artifact::CapturedArtifact<'_>,
        companions: &[&artifact::CapturedArtifact<'_>],
        harness: &'static RiscvHarnessSpec,
        entry_contract: EntryContractRef,
        selection: artifact::CodeSymbolSelection,
        reviewed: &[artifact::ReviewedCodeRange],
    ) -> Result<Self> {
        let exported_symbols = capture
            .code_symbols("", artifact::CodeSymbolSelection::Exported)?
            .into_iter()
            .map(|symbol| symbol.definition.clone())
            .collect::<Vec<_>>();
        let exported_symbol_keys = exported_symbols
            .iter()
            .map(symbol_key)
            .collect::<BTreeSet<_>>();
        let mut exported_addresses_by_name = BTreeMap::<String, BTreeSet<u32>>::new();
        for symbol in exported_symbols
            .iter()
            .filter(|symbol| symbol.addresses_resolved)
        {
            exported_addresses_by_name
                .entry(symbol.name.clone())
                .or_default()
                .insert(symbol.address as u32);
        }
        let mut address_preferred_symbol_keys = exported_symbol_keys.clone();
        let mut symbols = if selection == artifact::CodeSymbolSelection::All {
            capture
                .code_symbols("", selection)?
                .into_iter()
                .map(|symbol| symbol.definition.clone())
                .collect()
        } else {
            exported_symbols
        };
        symbols.extend(capture.reviewed_code_ranges(reviewed)?);
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
                return Err(format!("duplicate archive symbol identity {identity:?}").into());
            }
            symbols_by_address.insert(next_archive_symbol_id, symbol.clone());
            next_archive_symbol_id = next_archive_symbol_id.wrapping_add(1);
        }
        let mut image = if symbols.iter().any(|symbol| symbol.addresses_resolved) {
            Some(execution::ExecutableImage::from_captured(capture)?)
        } else {
            None
        };
        let mut data_symbols = capture.data_symbols()?.to_vec();
        let data_objects = capture.data_objects()?.to_vec();
        for companion in companions {
            let Some(image) = image.as_mut() else {
                return Err(format!(
                    "reference companions require a linked ELF primary artifact: {}",
                    capture.sha256()
                )
                .into());
            };
            image.add_captured_companion(companion)?;
            data_symbols.extend_from_slice(companion.data_symbols()?);
            let companion_exported_symbols = companion
                .code_symbols("", artifact::CodeSymbolSelection::Exported)?
                .into_iter()
                .map(|symbol| symbol.definition.clone())
                .collect::<Vec<_>>();
            for symbol in companion_exported_symbols
                .iter()
                .filter(|symbol| symbol.addresses_resolved)
            {
                exported_addresses_by_name
                    .entry(symbol.name.clone())
                    .or_default()
                    .insert(symbol.address as u32);
            }
            address_preferred_symbol_keys.extend(companion_exported_symbols.iter().map(symbol_key));
            let companion_symbols = if selection == artifact::CodeSymbolSelection::All {
                companion
                    .code_symbols("", selection)?
                    .into_iter()
                    .map(|symbol| symbol.definition.clone())
                    .collect()
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
        data_symbols.sort_by(|left, right| left.identity.cmp(&right.identity));
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
            for (offset, target_spec) in table.targets() {
                let (target, qualified_identity) = match target_spec {
                    FunctionTarget::Address(address) => (address, None),
                    FunctionTarget::Symbol(symbol) => (
                        image.symbol_address(symbol).ok_or_else(|| {
                            format!(
                                "entry contract {} requires function symbol {symbol}",
                                entry_contract.id()
                            )
                        })?,
                        None,
                    ),
                    FunctionTarget::SourceSymbol { source, symbol } => {
                        let address = image.symbol_address(symbol).ok_or_else(|| {
                            format!(
                                "entry contract {} requires function symbol {source}:{symbol}",
                                entry_contract.id()
                            )
                        })?;
                        (address, Some(format!("{source}::{symbol}")))
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
                if let Some(identity) = qualified_identity {
                    match pointer_context.function_target_identities.get(&target) {
                        Some(existing) if existing != &identity => {
                            return Err(format!(
                                "entry contract {} assigns conflicting source identities {existing:?} and {identity:?} to target {target:#010x}",
                                entry_contract.id()
                            )
                            .into());
                        }
                        _ => {
                            pointer_context
                                .function_target_identities
                                .insert(target, identity);
                        }
                    }
                }
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
                    reference: crate::SymbolReference::Unknown {
                        reason:
                            "entry contract binds symbol names without a physical symbol occurrence"
                                .to_owned(),
                    },
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
            for (address, (name, target)) in image.relocated_calls() {
                let Some(owner) = symbols_by_address.values().find(|symbol| {
                    symbol.addresses_resolved
                        && address >= symbol.address as u32
                        && address < (symbol.address as u32).wrapping_add(symbol.bytes.len() as u32)
                }) else {
                    continue;
                };
                // A deliberately partial linked oracle may leave a call
                // undefined while a supplied companion image owns the exact
                // exported implementation. Resolve only a unique exact-name
                // address; aliases at one address are harmless, competing
                // definitions remain explicitly unresolved.
                let target =
                    target.or_else(|| unique_exported_address(&exported_addresses_by_name, &name));
                relocated_calls.insert(StructuralCallSite::new(owner, address), (name, target));
            }
        }

        let mut archive_definitions = BTreeMap::<String, Vec<(artifact::CodeIdentity, u32)>>::new();
        for symbol in symbols.iter().filter(|symbol| !symbol.addresses_resolved) {
            let identity = symbol_key(symbol);
            archive_definitions
                .entry(symbol.name.clone())
                .or_default()
                .push((
                    symbol.identity.clone(),
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
                    .filter(|(identity, _)| {
                        owner
                            .identity
                            .object()
                            .is_some_and(|object| identity.object() == Some(object))
                    })
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
                    identity: artifact::CodeIdentity::ModelBoundary {
                        artifact_sha256: capture.sha256().to_owned(),
                        symbol: name.clone(),
                        address: u64::from(target),
                    },
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
            projected_direct_semantics: Default::default(),
            projected_origins: BTreeMap::new(),
        })
    }

    /// Resolve a human selector without choosing the first ambiguous occurrence.
    /// Exact callers can select by `CodeIdentity` from `symbols` directly.
    pub fn select_symbol(
        &self,
        member: Option<&str>,
        name: &str,
        address: Option<u64>,
    ) -> Result<&artifact::ArtifactSymbolDefinition> {
        let mut candidates = self.symbols.iter().filter(|candidate| {
            candidate.name == name
                && member.is_none_or(|member| candidate.member.as_deref() == Some(member))
                && address.is_none_or(|address| candidate.address == address)
        });
        let first = candidates
            .next()
            .ok_or_else(|| format!("symbol {name} in member {member:?} was not found"))?;
        if candidates.next().is_some() {
            return Err(format!(
                "symbol {name} in member {member:?} is ambiguous; select its physical code identity"
            )
            .into());
        }
        Ok(first)
    }

    pub fn trace(
        &self,
        member: Option<&str>,
        name: &str,
        svd: &MmioMap,
    ) -> Result<FunctionAnalysis> {
        let symbol = self.select_symbol(member, name, None)?;
        self.trace_symbol(symbol, svd)
    }

    /// Run the direct analysis domain with this resolver's captured call and
    /// pointer context. Reference analysis remains a separate domain over the
    /// same definitions; it additionally composes interprocedural summaries.
    pub fn trace_direct_symbol(
        &self,
        symbol: &artifact::ArtifactSymbolDefinition,
        svd: &MmioMap,
    ) -> Result<FunctionAnalysis> {
        crate::static_analysis::trace_binary_symbol(
            symbol,
            svd,
            &self.relocated_calls,
            &self.pointer_context,
            None,
        )
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
        self.trace_symbol_bounded_with_memo(
            symbol,
            svd,
            budget,
            &super::ReferenceAnalysisMemo::default(),
        )
    }

    pub fn trace_symbol_bounded_with_memo(
        &self,
        symbol: &artifact::ArtifactSymbolDefinition,
        svd: &MmioMap,
        budget: StructuralTraceBudget,
        memo: &super::ReferenceAnalysisMemo,
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
            memo,
        };
        resolve_reference_trace_with_budget(symbol, &context, None, &mut visiting)
    }

    pub fn symbol_is_exported(&self, symbol: &artifact::ArtifactSymbolDefinition) -> bool {
        self.exported_symbol_keys.contains(&symbol_key(symbol))
    }

    /// Return every captured data definition containing the complete access.
    /// Aliases, overlapping definitions and different artifact owners remain
    /// candidates; size, export visibility and input order confer no authority.
    pub fn data_address_resolution(&self, address: u32, width: u8) -> crate::DataAddressResolution {
        Self::resolve_data_address(&self.data_symbols, address, Some(width))
    }

    /// Resolve a numeric range against a captured catalog without selecting an owner.
    /// A missing width checks only the addressed byte, not the extent of a load.
    pub fn resolve_data_address(
        data_symbols: &[artifact::ArtifactDataSymbolDefinition],
        address: u32,
        width: Option<u8>,
    ) -> crate::DataAddressResolution {
        use crate::{DataAddressCandidate, DataAddressGap, DataAddressResolution};
        if width.is_some_and(|width| width == 0 || !width.is_multiple_of(8)) {
            return DataAddressResolution::Unknown {
                reason: DataAddressGap::InvalidAccessWidth,
            };
        }
        let Some(end) = address.checked_add(u32::from(width.map_or(1, |width| width / 8))) else {
            return DataAddressResolution::Unknown {
                reason: DataAddressGap::AddressOverflow,
            };
        };
        let candidates = data_symbols
            .iter()
            .filter(|symbol| {
                address >= symbol.address
                    && symbol
                        .address
                        .checked_add(symbol.size)
                        .is_some_and(|limit| end <= limit)
            })
            .map(|symbol| DataAddressCandidate {
                identity: symbol.identity.clone(),
                member: symbol.member.clone(),
                symbol: symbol.name.clone(),
                symbol_address: symbol.address,
                symbol_size: symbol.size,
                exported: symbol.exported,
                offset: i64::from(address - symbol.address),
            })
            .collect();
        DataAddressResolution::from_candidates(address, width, candidates)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RiscvSummaryHooks;
    use open_radio_vendor_analysis_model::{
        DirectSemanticFunctionSpec, ExternalReturnModel, ExternalSemanticSpec,
        SemanticFunctionBodyPolicy,
    };

    static TEST_TAIL_BOUNDARY: DirectSemanticFunctionSpec = DirectSemanticFunctionSpec {
        id: "test.tail-boundary",
        source: "backend-riscv-test",
        c_name: "test_tail_boundary",
        argument_count: 0,
        body_policy: SemanticFunctionBodyPolicy::OpaqueBoundary,
        return_model: ExternalReturnModel::Constant(7),
        semantic: ExternalSemanticSpec {
            operation: "test.tail-boundary",
            arguments: &[],
            return_type: "uint32_t",
            replacement: None,
            event_dispatch: None,
        },
        evidence: "synthetic exact relocated tail-call regression",
    };

    fn test_direct_external_semantic(name: &str) -> Option<&'static DirectSemanticFunctionSpec> {
        (name == TEST_TAIL_BOUNDARY.c_name).then_some(&TEST_TAIL_BOUNDARY)
    }

    #[test]
    fn unresolved_call_uses_only_a_unique_exact_exported_companion_address() {
        let unique = BTreeMap::from([(
            "read_hw_noisefloor".to_owned(),
            BTreeSet::from([0x2f83_f2fc]),
        )]);
        assert_eq!(
            unique_exported_address(&unique, "read_hw_noisefloor"),
            Some(0x2f83_f2fc)
        );

        let ambiguous = BTreeMap::from([(
            "memcpy".to_owned(),
            BTreeSet::from([0x1000_1000, 0x2f80_2000]),
        )]);
        assert_eq!(unique_exported_address(&ambiguous, "memcpy"), None);
        assert_eq!(unique_exported_address(&unique, "missing"), None);
    }

    fn resolver_with_data_symbols(
        mut data_symbols: Vec<artifact::ArtifactDataSymbolDefinition>,
    ) -> ReferenceResolver {
        data_symbols.sort_by(|left, right| left.identity.cmp(&right.identity));
        ReferenceResolver {
            symbols: Vec::new(),
            symbols_by_address: BTreeMap::new(),
            symbol_ids: BTreeMap::new(),
            exported_symbol_keys: BTreeSet::new(),
            relocated_calls: StructuralRelocatedCalls::new(),
            pointer_context: StructuralPointerContext::default(),
            data_symbols,
            data_objects: Vec::new(),
            projected_direct_semantics: Default::default(),
            projected_origins: BTreeMap::new(),
        }
    }

    #[test]
    fn projected_call_identity_wins_over_folded_linked_stub_alias() {
        let owner = artifact::ArtifactSymbolDefinition {
            identity: artifact::ArtifactSymbolDefinition::synthetic_identity(
                module_path!(),
                &(None),
                "lmacProcessTxComplete",
                0x1000,
            ),
            member: None,
            name: "lmacProcessTxComplete".to_owned(),
            address: 0x1000,
            bytes: vec![0; 8],
            addresses_resolved: true,
            memory_regions: Default::default(),
            relocations: Vec::new(),
        };
        let stub = |name: &str| artifact::ArtifactSymbolDefinition {
            identity: artifact::ArtifactSymbolDefinition::synthetic_identity(
                module_path!(),
                &(None),
                name,
                0x2000,
            ),
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
    fn projected_origin_preserves_an_exact_same_name_companion_target() {
        let owner = artifact::ArtifactSymbolDefinition {
            identity: artifact::ArtifactSymbolDefinition::synthetic_identity(
                module_path!(),
                &(None),
                "read_hw_noisefloor",
                0x1000,
            ),
            member: None,
            name: "read_hw_noisefloor".to_owned(),
            address: 0x1000,
            bytes: vec![0; 8],
            addresses_resolved: true,
            memory_regions: Default::default(),
            relocations: Vec::new(),
        };
        let mut resolver = resolver_with_data_symbols(Vec::new());
        resolver.relocated_calls.insert(
            StructuralCallSite::new(&owner, 0x1000),
            ("phy_read_hw_noisefloor".to_owned(), Some(0x2f82_7d72)),
        );

        resolver.register_projected_call_relocation(&owner, 0x1000, "phy_read_hw_noisefloor", 0);

        assert_eq!(
            resolver
                .relocated_calls
                .get(&StructuralCallSite::new(&owner, 0x1000)),
            Some(&("phy_read_hw_noisefloor".to_owned(), Some(0x2f82_7d72)))
        );
    }

    #[test]
    fn projected_folded_intrinsic_uses_exact_origin_name_and_value_semantics() {
        let owner = artifact::ArtifactSymbolDefinition {
            identity: artifact::ArtifactSymbolDefinition::synthetic_identity(
                module_path!(),
                &(None),
                "lmacProcessTxComplete",
                0x1000,
            ),
            member: None,
            name: "lmacProcessTxComplete".to_owned(),
            address: 0x1000,
            bytes: vec![
                0xef, 0x00, 0x00, 0x00, // jal ra, 0 (origin relocation supplies identity)
                0x67, 0x80, 0x00, 0x00, // ret
            ],
            addresses_resolved: true,
            memory_regions: Default::default(),
            relocations: Vec::new(),
        };
        let hooks = Box::leak(Box::new(RiscvSummaryHooks {
            secondary_return_target: |_| false,
            direct_semantic: |_| None,
            direct_external_semantic: |_| None,
            direct_external_intrinsic: |name, arguments| {
                (name == "__ctzsi2").then(|| {
                    (
                        SymbolicValue::expression(
                            crate::ExpressionOperation::CountTrailingZeros,
                            arguments[0].clone(),
                            SymbolicValue::Constant(0),
                        ),
                        None,
                    )
                })
            },
            reference_intrinsic: |_, _, _| None,
            caller_memory_input_domain: |_, _, _| None,
            standard_memory_function: |_| None,
            wide_signed_divide: |_, _| None,
        }));
        let context = StructuralPointerContext {
            summary_hooks: Some(hooks),
            ..StructuralPointerContext::default()
        };
        let mut relocated_calls = StructuralRelocatedCalls::new();
        relocated_calls.insert(
            StructuralCallSite::new(&owner, 0x1000),
            ("__ctzsi2".to_owned(), None),
        );
        let mut visiting = BTreeSet::from([0x1000]);

        let trace = resolve_reference_trace(
            &owner,
            &BTreeMap::new(),
            &relocated_calls,
            &context,
            None,
            &MmioMap::new(crate::RegisterCatalog::default(), Vec::new()).unwrap(),
            &mut visiting,
        )
        .unwrap();

        assert_eq!(
            trace.return_value,
            SymbolicValue::expression(
                crate::ExpressionOperation::CountTrailingZeros,
                SymbolicValue::input(0),
                SymbolicValue::Constant(0),
            )
        );
        assert!(
            trace
                .reference_blockers
                .iter()
                .all(|blocker| !blocker.contains("unresolved-call-relocation"))
        );
    }

    #[test]
    fn modeled_direct_boundary_accepts_relocated_tail_call() {
        let owner = artifact::ArtifactSymbolDefinition {
            identity: artifact::ArtifactSymbolDefinition::synthetic_identity(
                module_path!(),
                &(None),
                "controller_tail_wrapper",
                0x1000,
            ),
            member: None,
            name: "controller_tail_wrapper".to_owned(),
            address: 0x1000,
            bytes: vec![
                0x6f, 0x00, 0x00, 0x00, // jal zero, 0; relocation supplies identity
            ],
            addresses_resolved: true,
            memory_regions: Default::default(),
            relocations: Vec::new(),
        };
        let hooks = Box::leak(Box::new(RiscvSummaryHooks {
            secondary_return_target: |_| false,
            direct_semantic: |_| None,
            direct_external_semantic: test_direct_external_semantic,
            direct_external_intrinsic: |_, _| None,
            reference_intrinsic: |_, _, _| None,
            caller_memory_input_domain: |_, _, _| None,
            standard_memory_function: |_| None,
            wide_signed_divide: |_, _| None,
        }));
        let context = StructuralPointerContext {
            summary_hooks: Some(hooks),
            ..StructuralPointerContext::default()
        };
        let mut relocated_calls = StructuralRelocatedCalls::new();
        relocated_calls.insert(
            StructuralCallSite::new(&owner, 0x1000),
            (TEST_TAIL_BOUNDARY.c_name.to_owned(), None),
        );
        let mut visiting = BTreeSet::from([0x1000]);

        let trace = resolve_reference_trace(
            &owner,
            &BTreeMap::new(),
            &relocated_calls,
            &context,
            None,
            &MmioMap::new(crate::RegisterCatalog::default(), Vec::new()).unwrap(),
            &mut visiting,
        )
        .unwrap();

        assert_eq!(trace.return_value, SymbolicValue::Constant(7));
        assert!(
            trace.reference_blockers.is_empty(),
            "unexpected blockers: {:?}",
            trace.reference_blockers
        );
        assert!(matches!(
            trace.reference_events.as_slice(),
            [DraftReferenceEvent::ModeledDirectCall { function, .. }]
                if function.id == TEST_TAIL_BOUNDARY.id
        ));
    }

    #[test]
    fn projected_call_accepts_linker_relaxed_jal_form() {
        let owner = artifact::ArtifactSymbolDefinition {
            identity: artifact::ArtifactSymbolDefinition::synthetic_identity(
                module_path!(),
                &(None),
                "lmacProcessRxSucData",
                0x1006_941e,
            ),
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
            identity: artifact::ArtifactSymbolDefinition::synthetic_identity(
                module_path!(),
                &(None),
                "pp_post",
                0x1006_7688,
            ),
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
    fn data_address_resolution_preserves_overlaps_and_checks_width() {
        let resolver = resolver_with_data_symbols(vec![
            artifact::ArtifactDataSymbolDefinition {
                identity: open_radio_vendor_analysis_model::DataIdentity::Synthetic {
                    namespace: module_path!().to_owned(),
                    key: "image".to_owned(),
                },
                member: None,
                name: "image".to_owned(),
                address: 0x1000,
                size: 0x100,
                exported: true,
            },
            artifact::ArtifactDataSymbolDefinition {
                identity: open_radio_vendor_analysis_model::DataIdentity::Synthetic {
                    namespace: module_path!().to_owned(),
                    key: "private_state".to_owned(),
                },
                member: None,
                name: "private_state".to_owned(),
                address: 0x1020,
                size: 0x20,
                exported: false,
            },
            artifact::ArtifactDataSymbolDefinition {
                identity: open_radio_vendor_analysis_model::DataIdentity::Synthetic {
                    namespace: module_path!().to_owned(),
                    key: "state".to_owned(),
                },
                member: None,
                name: "state".to_owned(),
                address: 0x1020,
                size: 0x20,
                exported: true,
            },
        ]);

        use crate::{DataAddressGap, DataAddressResolution};
        let resolution = resolver.data_address_resolution(0x1024, 32);
        assert!(matches!(
            resolution,
            DataAddressResolution::Ambiguous { .. }
        ));
        assert_eq!(resolution.candidates().len(), 3);
        let candidates = resolution
            .candidates()
            .iter()
            .map(|candidate| (candidate.symbol.as_str(), candidate.offset))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(
            candidates,
            BTreeMap::from([("image", 0x24), ("private_state", 4), ("state", 4)])
        );
        let mut reversed = resolver.data_symbols.clone();
        reversed.reverse();
        assert_eq!(
            resolution,
            ReferenceResolver::resolve_data_address(&reversed, 0x1024, Some(32))
        );
        let crossing = resolver.data_address_resolution(0x103f, 16);
        assert_eq!(crossing.candidates().len(), 1);
        assert_eq!(crossing.candidates()[0].symbol, "image");
        // A location hint without a known width retains all ranges containing
        // that byte; it must not invent a 32-bit read and reject narrow ranges.
        let point = ReferenceResolver::resolve_data_address(&resolver.data_symbols, 0x103f, None);
        assert!(matches!(
            point,
            DataAddressResolution::Ambiguous { width: None, .. }
        ));
        assert_eq!(point.candidates().len(), 3);
        for (address, width, reason) in [
            (0x1024, 0, DataAddressGap::InvalidAccessWidth),
            (0x1024, 7, DataAddressGap::InvalidAccessWidth),
            (u32::MAX, 32, DataAddressGap::AddressOverflow),
            (0x2000, 32, DataAddressGap::NoContainingDefinition),
        ] {
            assert_eq!(
                resolver.data_address_resolution(address, width),
                DataAddressResolution::Unknown { reason }
            );
        }
    }
    #[test]
    fn competing_origin_observations_are_retained_without_last_writer_wins() {
        let linked = artifact::ArtifactSymbolDefinition {
            identity: artifact::ArtifactSymbolDefinition::synthetic_identity(
                module_path!(),
                &None,
                "linked",
                0x1000,
            ),
            member: None,
            name: "linked".to_owned(),
            address: 0x1000,
            bytes: vec![0x67, 0x80, 0, 0],
            addresses_resolved: true,
            memory_regions: Default::default(),
            relocations: Vec::new(),
        };
        let mut first = linked.clone();
        first.identity =
            artifact::ArtifactSymbolDefinition::synthetic_identity("origin-a", &None, "linked", 0);
        first.addresses_resolved = false;
        let mut second = first.clone();
        second.identity =
            artifact::ArtifactSymbolDefinition::synthetic_identity("origin-b", &None, "linked", 0);
        let mut resolver = resolver_with_data_symbols(Vec::new());
        resolver.register_projected_origin(&linked, first.clone());
        resolver.register_projected_origin(&linked, first.clone());
        assert_eq!(resolver.projected_origin(&linked), Some(&first));
        resolver.register_projected_origin(&linked, second.clone());
        assert_eq!(
            resolver.projected_origin_candidates(&linked),
            &[first, second]
        );
        assert!(resolver.projected_origin(&linked).is_none());
    }
}
