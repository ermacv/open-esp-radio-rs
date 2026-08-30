//! Persistent direct facts for one structurally analyzed function.
//!
//! The owner body is keyed exactly, while the resolver key retains the
//! conservative semantic/layout projection read by direct tracing without
//! hashing unrelated function bodies. Instruction sites inside the value are
//! stored as owner-relative offsets and materialized against the exact keyed
//! definition on every hit.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::*;

const MAX_CACHED_CALL_VARIANTS: usize = 1_024;
const MAX_COMPRESSED_FACT_BYTES: usize = 4 * 1024 * 1024;

pub(super) struct FunctionCacheRun {
    state: FunctionCacheState,
}

enum FunctionCacheState {
    DisabledStoreAbsent,
    DisabledUnsafeSemanticDomain,
    Enabled(Box<FunctionCacheEnabled>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FunctionCacheDisabledReason {
    StoreAbsent,
    UnsafeSemanticDomain,
}

struct FunctionCacheEnabled {
    hits: Mutex<BTreeMap<String, Vec<u8>>>,
    loaded: AtomicUsize,
    keys: Mutex<FunctionCacheKeys>,
    pending: Mutex<BTreeMap<String, Vec<u8>>>,
    mmio_fingerprint: [u8; 32],
    namespace_identities: bool,
    resolver_fingerprint: [u8; 32],
    namespace: String,
    reused: AtomicUsize,
}

#[derive(Default)]
struct FunctionCacheKeys {
    by_symbol: BTreeMap<SymbolKey, FunctionCacheKey>,
}

struct FunctionCacheKey {
    value: String,
    looked_up: bool,
}

impl FunctionCacheRun {
    pub(super) fn prepare<'a>(
        resolver: &ReferenceResolver,
        symbols: impl IntoIterator<Item = &'a artifact::ArtifactSymbolDefinition>,
        svd: &MmioMap,
        namespace: &str,
        namespace_identities: bool,
        store: Option<&dyn FunctionFactStore>,
    ) -> Self {
        let state = match store {
            None => FunctionCacheState::DisabledStoreAbsent,
            Some(_)
                if !semantic_cache_domain_is_safe(
                    resolver.pointer_context.summary_hooks.is_some(),
                    resolver.pointer_context.semantic_cache_domain,
                ) =>
            {
                FunctionCacheState::DisabledUnsafeSemanticDomain
            }
            Some(_) => FunctionCacheState::Enabled(Box::new(FunctionCacheEnabled {
                hits: Mutex::new(BTreeMap::new()),
                loaded: AtomicUsize::new(0),
                keys: Mutex::new(FunctionCacheKeys::default()),
                pending: Mutex::new(BTreeMap::new()),
                mmio_fingerprint: mmio_fingerprint(svd),
                namespace_identities,
                resolver_fingerprint: resolver_fingerprint(resolver),
                namespace: namespace.to_owned(),
                reused: AtomicUsize::new(0),
            })),
        };
        let cache = Self { state };
        if cache.disabled_reason() == Some(FunctionCacheDisabledReason::UnsafeSemanticDomain) {
            tracing::warn!(
                "persistent function facts disabled: registered summary hooks have no stable semantic cache domain"
            );
        }
        cache.load_symbols(symbols, store);
        cache
    }

    /// Load facts for symbols discovered after the initial root selection.
    ///
    /// Reachability is demand-driven, so these keys are not known during
    /// [`Self::prepare`]. Reserve and sort every new key before touching the
    /// store: one caller's newly discovered callees become one deterministic
    /// lookup, while converging call paths never issue duplicate reads.
    pub(super) fn load_symbols<'a>(
        &self,
        symbols: impl IntoIterator<Item = &'a artifact::ArtifactSymbolDefinition>,
        store: Option<&dyn FunctionFactStore>,
    ) {
        let FunctionCacheState::Enabled(cache) = &self.state else {
            return;
        };
        let Some(store) = store else {
            return;
        };
        let keys = {
            let mut keys = cache.keys.lock().expect("function cache key lock");
            let mut lookup = Vec::new();
            for symbol in symbols {
                let key = cache.key_for_locked(&mut keys, symbol);
                if !key.looked_up {
                    key.looked_up = true;
                    lookup.push(key.value.clone());
                }
            }
            lookup.sort();
            lookup
        };
        if keys.is_empty() {
            return;
        }
        match store.load_function_facts(&keys) {
            Ok(values) => {
                let mut hits = cache.hits.lock().expect("function cache hit lock");
                let before = hits.len();
                for (key, value) in values {
                    if keys.binary_search(&key).is_ok() {
                        hits.entry(key).or_insert(value);
                    }
                }
                cache
                    .loaded
                    .fetch_add(hits.len().saturating_sub(before), Ordering::Relaxed);
            }
            Err(error) => {
                tracing::warn!(%error, keys = keys.len(), "function-fact cache lookup failed")
            }
        }
    }

    pub(super) fn direct_graph(
        &self,
        symbol: &artifact::ArtifactSymbolDefinition,
        analyze: impl FnOnce() -> DirectCallGraph,
    ) -> DirectCallGraph {
        let FunctionCacheState::Enabled(cache) = &self.state else {
            return analyze();
        };
        let key = cache.key_for(symbol);
        let cached = cache
            .hits
            .lock()
            .expect("function cache hit lock")
            .remove(&key);
        if let Some(value) = cached
            && let Ok(fact) = decode_fact(&value)
            && let Some(graph) = fact.materialize(symbol, &cache.namespace)
        {
            cache.reused.fetch_add(1, Ordering::Relaxed);
            tracing::trace!(symbol = symbol.name, "reused persistent function fact");
            return graph;
        }
        let started = std::time::Instant::now();
        let graph = analyze();
        let elapsed = started.elapsed();
        if let Some(fact) = PortableDirectFacts::capture(symbol, &graph, &cache.namespace)
            && let Ok(value) = encode_fact(&fact)
            && value.len() <= MAX_COMPRESSED_FACT_BYTES
        {
            cache
                .pending
                .lock()
                .expect("function cache pending lock")
                .entry(key)
                .or_insert(value);
        } else if elapsed >= std::time::Duration::from_millis(50) {
            let call_kinds = graph.calls.iter().fold(
                BTreeMap::<&'static str, usize>::new(),
                |mut kinds, call| {
                    *kinds.entry(call.kind).or_default() += 1;
                    kinds
                },
            );
            tracing::debug!(
                symbol = symbol.name,
                elapsed_ms = elapsed.as_millis(),
                blockers = graph.blockers.len(),
                calls = graph.calls.len(),
                nonportable_calls = graph
                    .calls
                    .iter()
                    .filter(|call| !call_is_portable(call))
                    .count(),
                guarded_calls = graph
                    .calls
                    .iter()
                    .filter(|call| call
                        .guard_paths
                        .as_ref()
                        .is_some_and(|paths| { paths.iter().any(|path| !path.guards.is_empty()) }))
                    .count(),
                ?call_kinds,
                effects = graph.site_effects.len(),
                "expensive function fact was not cacheable"
            );
        }
        graph
    }

    pub(super) fn persist(&self, store: &mut dyn FunctionFactStore) {
        let FunctionCacheState::Enabled(cache) = &self.state else {
            return;
        };
        let facts =
            std::mem::take(&mut *cache.pending.lock().expect("function cache pending lock"))
                .into_iter()
                .collect::<Vec<_>>();
        if !facts.is_empty()
            && let Err(error) = store.store_function_facts(&facts)
        {
            tracing::warn!(%error, facts = facts.len(), "function-fact cache write failed");
        }
        let lookups = cache
            .keys
            .lock()
            .expect("function cache key lock")
            .by_symbol
            .values()
            .filter(|key| key.looked_up)
            .count();
        let hits = cache.reused.load(Ordering::Relaxed);
        tracing::debug!(
            cache_lookups = lookups,
            cache_loaded = cache.loaded.load(Ordering::Relaxed),
            cache_hits = hits,
            cache_misses = lookups.saturating_sub(hits),
            cache_recomputed_published = facts.len(),
            "completed persistent function-fact cache session"
        );
    }

    fn disabled_reason(&self) -> Option<FunctionCacheDisabledReason> {
        match &self.state {
            FunctionCacheState::DisabledStoreAbsent => {
                Some(FunctionCacheDisabledReason::StoreAbsent)
            }
            FunctionCacheState::DisabledUnsafeSemanticDomain => {
                Some(FunctionCacheDisabledReason::UnsafeSemanticDomain)
            }
            FunctionCacheState::Enabled(_) => None,
        }
    }

    #[cfg(test)]
    fn computed_key_count(&self) -> usize {
        match &self.state {
            FunctionCacheState::DisabledStoreAbsent
            | FunctionCacheState::DisabledUnsafeSemanticDomain => 0,
            FunctionCacheState::Enabled(cache) => cache
                .keys
                .lock()
                .expect("function cache key lock")
                .by_symbol
                .len(),
        }
    }
}

impl FunctionCacheEnabled {
    /// One resolver run treats `(member, name, address)` as the unique symbol
    /// identity everywhere else in linked IR. Cache the expensive body-bound
    /// digest under that same immutable identity so initial lookup and direct
    /// analysis cannot hash one body twice, even when the catalog supplies a
    /// cloned definition.
    fn key_for(&self, symbol: &artifact::ArtifactSymbolDefinition) -> String {
        let mut keys = self.keys.lock().expect("function cache key lock");
        self.key_for_locked(&mut keys, symbol).value.clone()
    }

    fn key_for_locked<'a>(
        &self,
        keys: &'a mut FunctionCacheKeys,
        symbol: &artifact::ArtifactSymbolDefinition,
    ) -> &'a mut FunctionCacheKey {
        keys.by_symbol
            .entry(symbol_key(symbol))
            .or_insert_with(|| FunctionCacheKey {
                value: function_fact_key(
                    symbol,
                    &self.mmio_fingerprint,
                    &self.resolver_fingerprint,
                    self.namespace_identities,
                ),
                looked_up: false,
            })
    }
}

fn semantic_cache_domain_is_safe(has_summary_hooks: bool, domain: &str) -> bool {
    !has_summary_hooks || !domain.trim().is_empty()
}

fn encode_fact(fact: &PortableDirectFacts) -> crate::Result<Vec<u8>> {
    use flate2::{Compression, write::GzEncoder};

    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    serde_json::to_writer(&mut encoder, fact)?;
    Ok(encoder.finish()?)
}

fn decode_fact(value: &[u8]) -> crate::Result<PortableDirectFacts> {
    use flate2::read::GzDecoder;

    Ok(serde_json::from_reader(GzDecoder::new(value))?)
}

fn function_fact_key(
    symbol: &artifact::ArtifactSymbolDefinition,
    mmio_fingerprint: &[u8; 32],
    resolver_fingerprint: &[u8; 32],
    namespace_identities: bool,
) -> String {
    let mut hash = Sha256::new();
    hash.update(FUNCTION_FACT_CACHE_DOMAIN);
    hash.update(mmio_fingerprint);
    hash.update(resolver_fingerprint);
    hash.update([u8::from(namespace_identities)]);
    hash_optional_str(&mut hash, symbol.member.as_deref());
    hash_str(&mut hash, &symbol.name);
    hash.update(symbol.address.to_le_bytes());
    hash.update([u8::from(symbol.addresses_resolved)]);
    hash_bytes(&mut hash, &symbol.bytes);
    hash.update((symbol.relocations.len() as u64).to_le_bytes());
    for relocation in &symbol.relocations {
        hash.update(relocation.address.to_le_bytes());
        hash_str(&mut hash, relocation_kind(relocation.kind));
        hash_str(&mut hash, &relocation.symbol);
        hash.update(relocation.addend.to_le_bytes());
    }
    hash.update((symbol.memory_regions.len() as u64).to_le_bytes());
    for region in symbol.memory_regions.iter() {
        hash.update(region.start.to_le_bytes());
        hash.update(region.length.to_le_bytes());
        hash.update([u8::from(region.writable)]);
        hash_str(&mut hash, &region.name);
    }
    format!("function-direct:{:x}", hash.finalize())
}

fn resolver_fingerprint(resolver: &ReferenceResolver) -> [u8; 32] {
    let symbols = resolver
        .symbols
        .iter()
        .chain(resolver.symbols_by_address.values())
        .map(|symbol| {
            (
                symbol.member.as_deref(),
                symbol.name.as_str(),
                symbol.address,
            )
        })
        .collect::<BTreeSet<_>>();
    let mut hash = Sha256::new();
    hash.update(b"blobray/resolver-semantic-layout/v2\0");
    hash.update((symbols.len() as u64).to_le_bytes());
    for (member, name, address) in symbols {
        hash_optional_str(&mut hash, member);
        hash_str(&mut hash, name);
        hash.update(address.to_le_bytes());
    }

    hash.update((resolver.symbols_by_address.len() as u64).to_le_bytes());
    for (target, symbol) in &resolver.symbols_by_address {
        hash.update(target.to_le_bytes());
        hash_symbol_identity(&mut hash, symbol);
    }

    // data_symbol_location intentionally observes the ordered narrowest-first
    // projection. Preserve that order rather than treating aliases as a set.
    hash.update((resolver.data_symbols.len() as u64).to_le_bytes());
    for symbol in &resolver.data_symbols {
        hash_optional_str(&mut hash, symbol.member.as_deref());
        hash_str(&mut hash, &symbol.name);
        hash.update(symbol.address.to_le_bytes());
        hash.update(symbol.size.to_le_bytes());
        hash.update([u8::from(symbol.exported)]);
    }

    // These maps are immutable for a resolver run and backed by ordered
    // collections. Some value types deliberately expose only semantic Debug;
    // framing that deterministic representation gives this build-local cache
    // a conservative projection without ever hashing raw summary fn pointers.
    hash_debug_projection(&mut hash, "relocated-calls", &resolver.relocated_calls);
    let context = &resolver.pointer_context;
    hash_str(&mut hash, context.semantic_cache_domain);
    hash.update([u8::from(context.summary_hooks.is_some())]);
    hash_debug_projection(
        &mut hash,
        "reviewed-external-pointer-cells",
        &context.reviewed_external_pointer_cells,
    );
    hash_debug_projection(
        &mut hash,
        "function-pointer-cells",
        &context.function_pointer_cells,
    );
    hash_debug_projection(&mut hash, "data-pointer-cells", &context.data_pointer_cells);
    hash_debug_projection(
        &mut hash,
        "relocated-pointer-symbols",
        &context.relocated_pointer_symbols,
    );
    hash_debug_projection(
        &mut hash,
        "projected-relocations",
        &context.projected_relocations,
    );
    hash_debug_projection(
        &mut hash,
        "function-table-slots",
        &context.function_table_slots,
    );
    hash_debug_projection(
        &mut hash,
        "function-target-identities",
        &context.function_target_identities,
    );
    hash_debug_projection(&mut hash, "diagnostic-calls", &context.diagnostic_calls);
    hash_debug_projection(
        &mut hash,
        "reviewed-external-calls",
        &context.reviewed_external_calls,
    );
    hash_debug_projection(
        &mut hash,
        "reviewed-external-slots",
        &context.reviewed_external_slots,
    );
    hash_debug_projection(
        &mut hash,
        "reviewed-internal-calls",
        &context.reviewed_internal_calls,
    );
    hash_debug_projection(
        &mut hash,
        "reviewed-internal-slots",
        &context.reviewed_internal_slots,
    );
    hash.finalize().into()
}

fn hash_symbol_identity(hash: &mut Sha256, symbol: &artifact::ArtifactSymbolDefinition) {
    hash_optional_str(hash, symbol.member.as_deref());
    hash_str(hash, &symbol.name);
    hash.update(symbol.address.to_le_bytes());
}

fn hash_bytes(hash: &mut Sha256, value: &[u8]) {
    hash.update((value.len() as u64).to_le_bytes());
    hash.update(value);
}

fn hash_str(hash: &mut Sha256, value: &str) {
    hash_bytes(hash, value.as_bytes());
}

fn hash_optional_str(hash: &mut Sha256, value: Option<&str>) {
    match value {
        Some(value) => {
            hash.update([1]);
            hash_str(hash, value);
        }
        None => hash.update([0]),
    }
}

fn hash_debug_projection(hash: &mut Sha256, label: &str, value: &impl std::fmt::Debug) {
    hash_str(hash, label);
    hash_str(hash, &format!("{value:?}"));
}

fn relocation_kind(kind: artifact::RelocationKind) -> &'static str {
    match kind {
        artifact::RelocationKind::GotHi20 => "got-hi20",
        artifact::RelocationKind::Hi20 => "hi20",
        artifact::RelocationKind::Lo12I => "lo12-i",
        artifact::RelocationKind::Lo12S => "lo12-s",
        artifact::RelocationKind::PcRelHi20 => "pc-rel-hi20",
        artifact::RelocationKind::PcRelLo12I => "pc-rel-lo12-i",
        artifact::RelocationKind::PcRelLo12S => "pc-rel-lo12-s",
        artifact::RelocationKind::GotPcRelLo12I => "got-pc-rel-lo12-i",
        artifact::RelocationKind::Call => "call",
        artifact::RelocationKind::CallPlt => "call-plt",
    }
}

fn mmio_fingerprint(svd: &MmioMap) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"blobray/mmio-semantic-layout/v1\0");
    hash.update((svd.registers.len() as u64).to_le_bytes());
    for register in &svd.registers {
        hash.update(register.address.to_le_bytes());
        hash_str(&mut hash, &register.name);
    }
    hash.update((svd.regions.len() as u64).to_le_bytes());
    for region in &svd.regions {
        hash.update(region.start.to_le_bytes());
        hash.update(region.end.to_le_bytes());
        hash.update([u8::from(region.readable), u8::from(region.writable)]);
        hash_str(&mut hash, &region.name);
    }
    hash.finalize().into()
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct PortableDirectFacts {
    calls: Vec<PortableCall>,
    guards: Vec<PortableGuard>,
    blockers: Vec<String>,
    predicates: Vec<PortablePredicate>,
    effects: Vec<PortableEffect>,
}

impl PortableDirectFacts {
    fn capture(
        symbol: &artifact::ArtifactSymbolDefinition,
        graph: &DirectCallGraph,
        namespace: &str,
    ) -> Option<Self> {
        if graph.calls.len() > MAX_CACHED_CALL_VARIANTS || !graph.calls.iter().all(call_is_portable)
        {
            return None;
        }
        let base = symbol.address as u32;
        let mut guard_ids = BTreeMap::<PortableGuard, u32>::new();
        let mut guards = Vec::new();
        let calls = graph
            .calls
            .iter()
            .map(|call| PortableCall::capture(base, namespace, call, &mut guard_ids, &mut guards))
            .collect::<Option<_>>()?;
        let blockers = graph
            .blockers
            .iter()
            .map(|blocker| hide_instruction_sites(blocker, symbol))
            .collect();
        let predicates = graph
            .direct_mmio_predicates
            .iter()
            .map(|predicate| PortablePredicate::capture(base, predicate))
            .collect::<Option<Vec<_>>>()?;
        let effects = graph
            .site_effects
            .iter()
            .map(|effect| PortableEffect::capture(base, effect))
            .collect::<Option<Vec<_>>>()?;
        Some(Self {
            calls,
            guards,
            blockers,
            predicates,
            effects,
        })
    }

    fn materialize(
        &self,
        symbol: &artifact::ArtifactSymbolDefinition,
        namespace: &str,
    ) -> Option<DirectCallGraph> {
        let base = symbol.address as u32;
        let calls = self
            .calls
            .iter()
            .map(|call| call.materialize(base, namespace, &self.guards))
            .collect::<Option<Vec<_>>>()?;
        Some(DirectCallGraph {
            calls: calls.into_iter().collect(),
            direct_mmio_predicates: self
                .predicates
                .iter()
                .map(|predicate| predicate.materialize(base))
                .collect::<Option<_>>()?,
            blockers: self
                .blockers
                .iter()
                .map(|blocker| show_instruction_sites(blocker, symbol))
                .collect(),
            site_effects: self
                .effects
                .iter()
                .map(|effect| effect.materialize(base))
                .collect::<Option<_>>()?,
        })
    }
}

fn call_is_portable(call: &LinkedCall) -> bool {
    call.kind == "internal"
        && call.execution_model.is_none()
        && call.semantic_contract.is_none()
        && call.trampoline.is_none()
        && call.project_symbol.is_none()
        && call.project_candidates.is_empty()
}

fn hide_instruction_sites(value: &str, symbol: &artifact::ArtifactSymbolDefinition) -> String {
    let base = symbol.address as u32;
    let mut value = value.to_owned();
    for offset in (0..symbol.bytes.len()).step_by(2) {
        let Ok(offset) = u32::try_from(offset) else {
            break;
        };
        let address = base.wrapping_add(offset);
        let marker = format!("$blobray-site[{offset:x}]");
        value = value.replace(&format!("{address:#010x}"), &marker);
        value = value.replace(&format!("{address:#x}"), &marker);
    }
    value
}

fn show_instruction_sites(value: &str, symbol: &artifact::ArtifactSymbolDefinition) -> String {
    let base = symbol.address as u32;
    let mut value = value.to_owned();
    for offset in (0..symbol.bytes.len()).step_by(2) {
        let Ok(offset) = u32::try_from(offset) else {
            break;
        };
        value = value.replace(
            &format!("$blobray-site[{offset:x}]"),
            &format!("{:#010x}", base.wrapping_add(offset)),
        );
    }
    value
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct PortableCall {
    target: String,
    site_offset: Option<u32>,
    direct: bool,
    tail: bool,
    result_modeled: bool,
    result_provenance: Option<PortableCallResultProvenance>,
    semantics: Option<String>,
    semantic_operation: Option<String>,
    replacement_hint: Option<String>,
    argument_shapes: usize,
    arguments: Vec<String>,
    argument_exact: Vec<bool>,
    argument_result_provenance: Vec<PortableCallArgumentResultProvenance>,
    argument_bindings: Vec<PortableArgumentBinding>,
    typed_arguments: Vec<PortableCallArgument>,
    guard_paths: Option<Vec<Vec<u32>>>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct PortableCallResultProvenance {
    kind: String,
    function: String,
    site_offset: u32,
    target: String,
    operation: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct PortableCallArgumentResultProvenance {
    position: usize,
    producer: PortableCallResultProvenance,
}

impl PortableCallResultProvenance {
    fn capture(
        base: u32,
        namespace: &str,
        provenance: &LinkedCallResultProvenance,
    ) -> Option<Self> {
        Some(Self {
            kind: provenance.kind.to_owned(),
            function: hide_namespace(&provenance.function, namespace),
            site_offset: provenance.site.checked_sub(base)?,
            target: hide_namespace(&provenance.target, namespace),
            operation: provenance.operation.clone(),
        })
    }

    fn materialize(&self, base: u32, namespace: &str) -> Option<LinkedCallResultProvenance> {
        Some(LinkedCallResultProvenance {
            kind: static_vocabulary(&self.kind)?,
            function: show_namespace(&self.function, namespace),
            site: base.checked_add(self.site_offset)?,
            target: show_namespace(&self.target, namespace),
            operation: self.operation.clone(),
        })
    }
}

impl PortableCall {
    fn capture(
        base: u32,
        namespace: &str,
        call: &LinkedCall,
        guard_ids: &mut BTreeMap<PortableGuard, u32>,
        guards: &mut Vec<PortableGuard>,
    ) -> Option<Self> {
        Some(Self {
            target: hide_namespace(&call.target, namespace),
            site_offset: match call.site {
                Some(site) => Some(site.checked_sub(base)?),
                None => None,
            },
            direct: call.direct,
            tail: call.tail,
            result_modeled: call.result_modeled,
            result_provenance: match &call.result_provenance {
                Some(provenance) => Some(PortableCallResultProvenance::capture(
                    base, namespace, provenance,
                )?),
                None => None,
            },
            semantics: call
                .semantics
                .as_deref()
                .map(|value| hide_namespace(value, namespace)),
            semantic_operation: call.semantic_operation.clone(),
            replacement_hint: call.replacement_hint.clone(),
            argument_shapes: call.argument_shapes,
            arguments: call
                .arguments
                .iter()
                .map(|value| hide_namespace(value, namespace))
                .collect(),
            argument_exact: call.argument_exact.clone(),
            argument_result_provenance: call
                .argument_result_provenance
                .iter()
                .map(|provenance| {
                    Some(PortableCallArgumentResultProvenance {
                        position: provenance.position,
                        producer: PortableCallResultProvenance::capture(
                            base,
                            namespace,
                            &provenance.producer,
                        )?,
                    })
                })
                .collect::<Option<Vec<_>>>()?,
            argument_bindings: call
                .argument_bindings
                .iter()
                .map(|binding| PortableArgumentBinding {
                    position: binding.position,
                    caller_argument: binding.caller_argument,
                    offset: binding.offset,
                    expression: hide_namespace(&binding.expression, namespace),
                })
                .collect(),
            typed_arguments: call
                .typed_arguments
                .iter()
                .map(|argument| PortableCallArgument {
                    position: argument.position,
                    name: argument.name.clone(),
                    c_type: argument.c_type.clone(),
                    direction: argument.direction.to_owned(),
                    value: hide_namespace(&argument.value, namespace),
                })
                .collect(),
            guard_paths: match call.guard_paths.as_ref() {
                Some(paths) => Some(
                    paths
                        .iter()
                        .map(|path| {
                            path.guards
                                .iter()
                                .map(|guard| {
                                    let guard = PortableGuard::capture(base, namespace, guard)?;
                                    if let Some(index) = guard_ids.get(&guard) {
                                        return Some(*index);
                                    }
                                    let index = u32::try_from(guards.len()).ok()?;
                                    guards.push(guard.clone());
                                    guard_ids.insert(guard, index);
                                    Some(index)
                                })
                                .collect::<Option<Vec<_>>>()
                        })
                        .collect::<Option<Vec<_>>>()?,
                ),
                None => None,
            },
        })
    }

    fn materialize(
        &self,
        base: u32,
        namespace: &str,
        guards: &[PortableGuard],
    ) -> Option<LinkedCall> {
        Some(LinkedCall {
            kind: "internal",
            target: show_namespace(&self.target, namespace),
            site: match self.site_offset {
                Some(offset) => Some(base.checked_add(offset)?),
                None => None,
            },
            direct: self.direct,
            tail: self.tail,
            result_modeled: self.result_modeled,
            result_provenance: match &self.result_provenance {
                Some(provenance) => Some(provenance.materialize(base, namespace)?),
                None => None,
            },
            execution_model: None,
            semantics: self
                .semantics
                .as_deref()
                .map(|value| show_namespace(value, namespace)),
            semantic_operation: self.semantic_operation.clone(),
            semantic_contract: None,
            replacement_hint: self.replacement_hint.clone(),
            project_symbol: None,
            project_candidates: Vec::new(),
            trampoline: None,
            argument_shapes: self.argument_shapes,
            arguments: self
                .arguments
                .iter()
                .map(|value| show_namespace(value, namespace))
                .collect(),
            argument_exact: self.argument_exact.clone(),
            argument_result_provenance: self
                .argument_result_provenance
                .iter()
                .map(|provenance| {
                    Some(LinkedCallArgumentResultProvenance {
                        position: provenance.position,
                        producer: provenance.producer.materialize(base, namespace)?,
                    })
                })
                .collect::<Option<Vec<_>>>()?,
            argument_bindings: self
                .argument_bindings
                .iter()
                .map(|binding| LinkedArgumentBinding {
                    position: binding.position,
                    caller_argument: binding.caller_argument,
                    offset: binding.offset,
                    expression: show_namespace(&binding.expression, namespace),
                })
                .collect(),
            typed_arguments: self
                .typed_arguments
                .iter()
                .map(|argument| LinkedCallArgument {
                    position: argument.position,
                    name: argument.name.clone(),
                    c_type: argument.c_type.clone(),
                    direction: static_vocabulary(&argument.direction).unwrap_or("unknown"),
                    value: show_namespace(&argument.value, namespace),
                })
                .collect(),
            guard_paths: match self.guard_paths.as_ref() {
                Some(paths) => Some(
                    paths
                        .iter()
                        .map(|path| {
                            Some(LinkedCallGuardPath {
                                guards: path
                                    .iter()
                                    .map(|index| {
                                        guards
                                            .get(usize::try_from(*index).ok()?)?
                                            .materialize(base, namespace)
                                    })
                                    .collect::<Option<Vec<_>>>()?,
                            })
                        })
                        .collect::<Option<Vec<_>>>()?,
                ),
                None => None,
            },
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
struct PortableGuard {
    site_offset: u32,
    condition: String,
    operation: String,
    taken: bool,
    result_sources: Vec<PortableGuardResultSource>,
    direct_mmio_sources: Vec<PortablePredicateSource>,
}

impl PortableGuard {
    fn capture(base: u32, namespace: &str, guard: &LinkedCallGuard) -> Option<Self> {
        Some(Self {
            site_offset: guard.site.checked_sub(base)?,
            condition: hide_namespace(&guard.condition, namespace),
            operation: guard.operation.to_owned(),
            taken: guard.taken,
            result_sources: guard
                .result_sources
                .iter()
                .map(|source| PortableGuardResultSource::capture(namespace, source))
                .collect(),
            direct_mmio_sources: guard
                .direct_mmio_sources
                .iter()
                .map(PortablePredicateSource::from)
                .collect(),
        })
    }

    fn materialize(&self, base: u32, namespace: &str) -> Option<LinkedCallGuard> {
        Some(LinkedCallGuard {
            site: base.checked_add(self.site_offset)?,
            condition: show_namespace(&self.condition, namespace),
            operation: static_vocabulary(&self.operation)?,
            taken: self.taken,
            result_sources: self
                .result_sources
                .iter()
                .map(|source| source.materialize(namespace))
                .collect::<Option<_>>()?,
            direct_mmio_sources: self.direct_mmio_sources.iter().map(Into::into).collect(),
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
struct PortableGuardResultSource {
    kind: String,
    token: u32,
    target: Option<String>,
    operand: String,
    value_bits: Option<u32>,
    source_bits: u32,
    inverted: bool,
    comparison_value: Option<u32>,
    source_comparison_value: Option<u32>,
    producer_return_exact: Option<bool>,
    mmio_sources: Vec<PortableGuardMmioSource>,
}

impl PortableGuardResultSource {
    fn capture(namespace: &str, source: &LinkedCallGuardResultSource) -> Self {
        Self {
            kind: source.kind.to_owned(),
            token: source.token,
            target: source
                .target
                .as_deref()
                .map(|target| hide_namespace(target, namespace)),
            operand: source.operand.to_owned(),
            value_bits: source.value_bits,
            source_bits: source.source_bits,
            inverted: source.inverted,
            comparison_value: source.comparison_value,
            source_comparison_value: source.source_comparison_value,
            producer_return_exact: source.producer_return_exact,
            mmio_sources: source
                .mmio_sources
                .iter()
                .map(PortableGuardMmioSource::from)
                .collect(),
        }
    }

    fn materialize(&self, namespace: &str) -> Option<LinkedCallGuardResultSource> {
        Some(LinkedCallGuardResultSource {
            kind: static_vocabulary(&self.kind)?,
            token: self.token,
            target: self
                .target
                .as_deref()
                .map(|target| show_namespace(target, namespace)),
            operand: static_vocabulary(&self.operand)?,
            value_bits: self.value_bits,
            source_bits: self.source_bits,
            inverted: self.inverted,
            comparison_value: self.comparison_value,
            source_comparison_value: self.source_comparison_value,
            producer_return_exact: self.producer_return_exact,
            mmio_sources: self.mmio_sources.iter().map(Into::into).collect(),
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
struct PortableGuardMmioSource {
    address: u32,
    register: String,
    producer_path: Vec<String>,
    result_bits: u32,
    register_bits: u32,
    inverted: bool,
    result_comparison_value: Option<u32>,
    register_comparison_value: Option<u32>,
}

impl From<&LinkedCallGuardMmioSource> for PortableGuardMmioSource {
    fn from(source: &LinkedCallGuardMmioSource) -> Self {
        Self {
            address: source.address,
            register: source.register.clone(),
            producer_path: source.producer_path.clone(),
            result_bits: source.result_bits,
            register_bits: source.register_bits,
            inverted: source.inverted,
            result_comparison_value: source.result_comparison_value,
            register_comparison_value: source.register_comparison_value,
        }
    }
}

impl From<&PortableGuardMmioSource> for LinkedCallGuardMmioSource {
    fn from(source: &PortableGuardMmioSource) -> Self {
        Self {
            address: source.address,
            register: source.register.clone(),
            producer_path: source.producer_path.clone(),
            result_bits: source.result_bits,
            register_bits: source.register_bits,
            inverted: source.inverted,
            result_comparison_value: source.result_comparison_value,
            register_comparison_value: source.register_comparison_value,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct PortableArgumentBinding {
    position: usize,
    caller_argument: u8,
    offset: i32,
    expression: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct PortableCallArgument {
    position: usize,
    name: String,
    c_type: String,
    direction: String,
    value: String,
}

fn hide_namespace(value: &str, namespace: &str) -> String {
    value.replace(&format!("{namespace}::"), "$blobray-local::")
}

fn show_namespace(value: &str, namespace: &str) -> String {
    value.replace("$blobray-local::", &format!("{namespace}::"))
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct PortablePredicate {
    site_offset: u32,
    condition: String,
    operation: String,
    sources: Vec<PortablePredicateSource>,
}

impl PortablePredicate {
    fn capture(base: u32, value: &LinkedDirectMmioPredicate) -> Option<Self> {
        Some(Self {
            site_offset: value.site.checked_sub(base)?,
            condition: value.condition.clone(),
            operation: value.operation.to_owned(),
            sources: value
                .sources
                .iter()
                .map(PortablePredicateSource::from)
                .collect(),
        })
    }

    fn materialize(&self, base: u32) -> Option<LinkedDirectMmioPredicate> {
        Some(LinkedDirectMmioPredicate {
            site: base.checked_add(self.site_offset)?,
            condition: self.condition.clone(),
            operation: static_vocabulary(&self.operation)?,
            sources: self.sources.iter().map(Into::into).collect(),
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
struct PortablePredicateSource {
    operand: String,
    read_token: u32,
    address: u32,
    register: String,
    value_bits: u32,
    register_bits: u32,
    inverted: bool,
    comparison_value: Option<u32>,
    register_comparison_value: Option<u32>,
}

impl From<&LinkedDirectMmioPredicateSource> for PortablePredicateSource {
    fn from(value: &LinkedDirectMmioPredicateSource) -> Self {
        Self {
            operand: value.operand.to_owned(),
            read_token: value.read_token,
            address: value.address,
            register: value.register.clone(),
            value_bits: value.value_bits,
            register_bits: value.register_bits,
            inverted: value.inverted,
            comparison_value: value.comparison_value,
            register_comparison_value: value.register_comparison_value,
        }
    }
}

impl From<&PortablePredicateSource> for LinkedDirectMmioPredicateSource {
    fn from(value: &PortablePredicateSource) -> Self {
        Self {
            operand: static_vocabulary(&value.operand).unwrap_or("unknown"),
            read_token: value.read_token,
            address: value.address,
            register: value.register.clone(),
            value_bits: value.value_bits,
            register_bits: value.register_bits,
            inverted: value.inverted,
            comparison_value: value.comparison_value,
            register_comparison_value: value.register_comparison_value,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum PortableEffect {
    Mmio(PortableMmioEffect),
    Memory(PortableMemoryEffect),
}

impl PortableEffect {
    fn capture(base: u32, effect: &LinkedInstructionEffect) -> Option<Self> {
        Some(match effect {
            LinkedInstructionEffect::Mmio { .. } => {
                Self::Mmio(PortableMmioEffect::capture(base, effect)?)
            }
            LinkedInstructionEffect::Memory {
                site,
                access,
                width,
                object,
                offset,
                paths,
                value,
                value_pseudo,
                write_mask,
                preserved_mask,
                forced_zero_mask,
                forced_one_mask,
                ..
            } => Self::Memory(PortableMemoryEffect {
                site_offset: site.checked_sub(base)?,
                access: (*access).to_owned(),
                width: *width,
                object: object.clone(),
                offset: *offset,
                paths: paths.clone(),
                value: value.clone(),
                value_pseudo: value_pseudo.clone(),
                write_mask: *write_mask,
                preserved_mask: *preserved_mask,
                forced_zero_mask: *forced_zero_mask,
                forced_one_mask: *forced_one_mask,
            }),
        })
    }

    fn materialize(&self, base: u32) -> Option<LinkedInstructionEffect> {
        match self {
            Self::Mmio(effect) => effect.materialize(base),
            Self::Memory(effect) => effect.materialize(base),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct PortableMmioEffect {
    site_offset: u32,
    access: String,
    width: u8,
    address: u32,
    register: String,
    mode: String,
    paths: Vec<String>,
    guards: Vec<String>,
    value: Option<String>,
    modified_mask: Option<u32>,
    preserved_mask: Option<u32>,
    forced_zero_mask: Option<u32>,
    forced_one_mask: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct PortableMemoryEffect {
    site_offset: u32,
    access: String,
    width: u8,
    object: LinkedMemoryObject,
    offset: i64,
    paths: Vec<String>,
    value: Option<String>,
    value_pseudo: Option<String>,
    write_mask: Option<u32>,
    preserved_mask: Option<u32>,
    forced_zero_mask: Option<u32>,
    forced_one_mask: Option<u32>,
}

impl PortableMemoryEffect {
    fn materialize(&self, base: u32) -> Option<LinkedInstructionEffect> {
        Some(LinkedInstructionEffect::Memory {
            site: base.checked_add(self.site_offset)?,
            block: None,
            access: static_vocabulary(&self.access)?,
            width: self.width,
            object: self.object.clone(),
            offset: self.offset,
            paths: self.paths.clone(),
            value: self.value.clone(),
            value_pseudo: self.value_pseudo.clone(),
            write_mask: self.write_mask,
            preserved_mask: self.preserved_mask,
            forced_zero_mask: self.forced_zero_mask,
            forced_one_mask: self.forced_one_mask,
        })
    }
}

impl PortableMmioEffect {
    fn capture(base: u32, effect: &LinkedInstructionEffect) -> Option<Self> {
        let LinkedInstructionEffect::Mmio {
            site,
            access,
            width,
            address,
            register,
            mode,
            paths,
            guards,
            value,
            modified_mask,
            preserved_mask,
            forced_zero_mask,
            forced_one_mask,
            ..
        } = effect
        else {
            return None;
        };
        Some(Self {
            site_offset: site.checked_sub(base)?,
            access: (*access).to_owned(),
            width: *width,
            address: *address,
            register: register.clone(),
            mode: (*mode).to_owned(),
            paths: paths.clone(),
            guards: guards.clone(),
            value: value.clone(),
            modified_mask: *modified_mask,
            preserved_mask: *preserved_mask,
            forced_zero_mask: *forced_zero_mask,
            forced_one_mask: *forced_one_mask,
        })
    }

    fn materialize(&self, base: u32) -> Option<LinkedInstructionEffect> {
        Some(LinkedInstructionEffect::Mmio {
            site: base.checked_add(self.site_offset)?,
            block: None,
            access: static_vocabulary(&self.access)?,
            width: self.width,
            address: self.address,
            register: self.register.clone(),
            mode: static_vocabulary(&self.mode)?,
            paths: self.paths.clone(),
            guards: self.guards.clone(),
            value: self.value.clone(),
            modified_mask: self.modified_mask,
            preserved_mask: self.preserved_mask,
            forced_zero_mask: self.forced_zero_mask,
            forced_one_mask: self.forced_one_mask,
        })
    }
}

fn static_vocabulary(value: &str) -> Option<&'static str> {
    Some(match value {
        "left" => "left",
        "right" => "right",
        "read" => "read",
        "write" => "write",
        "in" => "in",
        "out" => "out",
        "inout" => "inout",
        "call-result" => "call-result",
        "external-result" => "external-result",
        "direct" => "direct",
        "static" => "static",
        "indexed-candidate" => "indexed-candidate",
        "structural-poll" => "structural-poll",
        "indexed" => "indexed",
        "poll" => "poll",
        "equal" => "equal",
        "not-equal" => "not-equal",
        "signed-less-than" => "signed-less-than",
        "signed-greater-or-equal" => "signed-greater-or-equal",
        "unsigned-less-than" => "unsigned-less-than",
        "unsigned-greater-or-equal" => "unsigned-greater-or-equal",
        "eq" => "eq",
        "ne" => "ne",
        "lt" => "lt",
        "ge" => "ge",
        "ltu" => "ltu",
        "geu" => "geu",
        _ => return intern_static_vocabulary(value),
    })
}

fn intern_static_vocabulary(value: &str) -> Option<&'static str> {
    use std::sync::OnceLock;

    static VALUES: OnceLock<Mutex<BTreeMap<String, &'static str>>> = OnceLock::new();
    if value.len() > 128 {
        return None;
    }
    let mut values = VALUES
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .ok()?;
    if let Some(value) = values.get(value) {
        return Some(value);
    }
    if values.len() >= 256 {
        return None;
    }
    let interned = Box::leak(value.to_owned().into_boxed_str());
    values.insert(value.to_owned(), interned);
    Some(interned)
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, sync::Arc};

    use super::*;

    static UNKEYED_SUMMARY_HOOKS: direct::RiscvSummaryHooks = direct::RiscvSummaryHooks {
        secondary_return_target: |_| false,
        direct_semantic: |_| None,
        direct_external_semantic: |_| None,
        direct_external_intrinsic: |_, _| None,
        reference_intrinsic: |_, _, _| None,
        caller_memory_input_domain: |_, _, _| None,
        standard_memory_function: |_| None,
        wide_signed_divide: |_, _| None,
    };

    #[derive(Default)]
    struct CountingStore {
        loads: Cell<usize>,
        stores: usize,
    }

    impl FunctionFactStore for CountingStore {
        fn load_function_facts(&self, _keys: &[String]) -> crate::Result<Vec<(String, Vec<u8>)>> {
            self.loads.set(self.loads.get() + 1);
            Ok(Vec::new())
        }

        fn store_function_facts(&mut self, _facts: &[(String, Vec<u8>)]) -> crate::Result<()> {
            self.stores += 1;
            Ok(())
        }
    }

    fn symbol(address: u64) -> artifact::ArtifactSymbolDefinition {
        named_symbol(Some("radio.o"), "leaf", address, vec![0x13; 0x24])
    }

    fn named_symbol(
        member: Option<&str>,
        name: &str,
        address: u64,
        bytes: Vec<u8>,
    ) -> artifact::ArtifactSymbolDefinition {
        artifact::ArtifactSymbolDefinition {
            member: member.map(str::to_owned),
            name: name.to_owned(),
            address,
            bytes,
            addresses_resolved: true,
            memory_regions: Arc::from([]),
            relocations: Vec::new(),
        }
    }

    fn resolver(symbols: Vec<artifact::ArtifactSymbolDefinition>) -> ReferenceResolver {
        let symbols_by_address = symbols
            .iter()
            .map(|symbol| (symbol.address as u32, symbol.clone()))
            .collect();
        ReferenceResolver {
            symbols,
            symbols_by_address,
            symbol_ids: BTreeMap::new(),
            exported_symbol_keys: BTreeSet::new(),
            relocated_calls: direct::StructuralRelocatedCalls::new(),
            pointer_context: direct::StructuralPointerContext::default(),
            data_symbols: Vec::new(),
            data_objects: Vec::new(),
            projected_direct_semantics: BTreeMap::new(),
            projected_origins: BTreeMap::new(),
        }
    }

    #[test]
    fn key_binds_owner_base_but_sites_rebase_on_materialization() {
        let mmio = MmioMap {
            registers: Vec::new(),
            regions: Vec::new(),
        };
        let fingerprint = mmio_fingerprint(&mmio);
        let first = symbol(0x4000);
        let second = symbol(0x8000);
        assert_ne!(
            function_fact_key(&first, &fingerprint, &[0; 32], true),
            function_fact_key(&second, &fingerprint, &[0; 32], true)
        );
        let graph = DirectCallGraph {
            calls: BTreeSet::from([
                LinkedCall {
                    kind: "internal",
                    target: "first::callee".to_owned(),
                    site: Some(0x4004),
                    direct: true,
                    tail: false,
                    result_modeled: false,
                    result_provenance: None,
                    execution_model: None,
                    semantics: None,
                    semantic_operation: None,
                    semantic_contract: None,
                    replacement_hint: None,
                    project_symbol: None,
                    project_candidates: Vec::new(),
                    trampoline: None,
                    argument_shapes: 1,
                    arguments: vec!["first::callee(arg0)".to_owned()],
                    argument_exact: vec![true],
                    argument_result_provenance: vec![LinkedCallArgumentResultProvenance {
                        position: 0,
                        producer: LinkedCallResultProvenance {
                            kind: "call-result",
                            function: "first::owner".to_owned(),
                            site: 0x4000,
                            target: "first::receive".to_owned(),
                            operation: None,
                        },
                    }],
                    argument_bindings: Vec::new(),
                    typed_arguments: Vec::new(),
                    guard_paths: Some(vec![LinkedCallGuardPath { guards: Vec::new() }]),
                },
                LinkedCall {
                    kind: "internal",
                    target: "first::receive".to_owned(),
                    site: Some(0x4000),
                    direct: true,
                    tail: false,
                    result_modeled: true,
                    result_provenance: Some(LinkedCallResultProvenance {
                        kind: "call-result",
                        function: "first::owner".to_owned(),
                        site: 0x4000,
                        target: "first::receive".to_owned(),
                        operation: None,
                    }),
                    execution_model: None,
                    semantics: None,
                    semantic_operation: None,
                    semantic_contract: None,
                    replacement_hint: None,
                    project_symbol: None,
                    project_candidates: Vec::new(),
                    trampoline: None,
                    argument_shapes: 1,
                    arguments: Vec::new(),
                    argument_exact: Vec::new(),
                    argument_result_provenance: Vec::new(),
                    argument_bindings: Vec::new(),
                    typed_arguments: Vec::new(),
                    guard_paths: Some(vec![LinkedCallGuardPath { guards: Vec::new() }]),
                },
            ]),
            direct_mmio_predicates: BTreeSet::new(),
            blockers: BTreeSet::from([
                "branch at 0x00004000 is incomplete".to_owned(),
                "branch at 0x00004020 is incomplete".to_owned(),
            ]),
            site_effects: BTreeSet::from([LinkedInstructionEffect::Mmio {
                site: 0x4002,
                block: None,
                access: "write",
                width: 32,
                address: 0x6000_1000,
                register: "REG".to_owned(),
                mode: "direct",
                paths: vec!["entry".to_owned()],
                guards: Vec::new(),
                value: Some("arg0".to_owned()),
                modified_mask: None,
                preserved_mask: None,
                forced_zero_mask: None,
                forced_one_mask: None,
            }]),
        };
        let fact = PortableDirectFacts::capture(&first, &graph, "first").unwrap();
        let rebound = fact.materialize(&second, "second").unwrap();
        assert_eq!(rebound.site_effects.iter().next().unwrap().site(), 0x8002);
        let call = rebound.calls.iter().next().unwrap();
        assert_eq!(call.site, Some(0x8004));
        assert_eq!(call.target, "second::callee");
        assert_eq!(call.arguments, ["second::callee(arg0)"]);
        assert_eq!(call.argument_exact, [true]);
        assert_eq!(
            call.argument_result_provenance,
            [LinkedCallArgumentResultProvenance {
                position: 0,
                producer: LinkedCallResultProvenance {
                    kind: "call-result",
                    function: "second::owner".to_owned(),
                    site: 0x8000,
                    target: "second::receive".to_owned(),
                    operation: None,
                },
            }]
        );
        let producer = rebound
            .calls
            .iter()
            .find(|call| call.target == "second::receive")
            .expect("rebound result producer");
        assert_eq!(
            producer.result_provenance,
            Some(call.argument_result_provenance[0].producer.clone())
        );
        assert_eq!(
            rebound.blockers,
            BTreeSet::from([
                "branch at 0x00008000 is incomplete".to_owned(),
                "branch at 0x00008020 is incomplete".to_owned(),
            ])
        );
    }

    #[test]
    fn unrelated_symbol_body_bytes_do_not_change_the_resolver_projection() {
        let owner = symbol(0x4000);
        let unrelated = named_symbol(Some("other.o"), "unrelated", 0x8000, vec![0x13, 0, 0, 0]);
        let first_resolver = resolver(vec![owner.clone(), unrelated.clone()]);
        let mut changed = unrelated;
        changed.bytes = vec![0x67, 0x80, 0, 0, 0x13, 0, 0, 0];
        let second_resolver = resolver(vec![owner.clone(), changed]);

        let first_projection = resolver_fingerprint(&first_resolver);
        let second_projection = resolver_fingerprint(&second_resolver);
        assert_eq!(first_projection, second_projection);
        assert_eq!(
            function_fact_key(&owner, &[0; 32], &first_projection, true),
            function_fact_key(&owner, &[0; 32], &second_projection, true),
        );
    }

    #[test]
    fn owner_member_and_absolute_base_are_part_of_the_function_key() {
        let first = symbol(0x4000);
        let mut other_member = first.clone();
        other_member.member = Some("other.o".to_owned());
        let mut other_base = first.clone();
        other_base.address = 0x8000;

        let first_key = function_fact_key(&first, &[0; 32], &[0; 32], true);
        assert_ne!(
            first_key,
            function_fact_key(&other_member, &[0; 32], &[0; 32], true)
        );
        assert_ne!(
            first_key,
            function_fact_key(&other_base, &[0; 32], &[0; 32], true)
        );
        assert_ne!(
            first_key,
            function_fact_key(&first, &[0; 32], &[0; 32], false),
            "namespaced and unnamespaced portable facts must not share a key",
        );
    }

    #[test]
    fn resolver_semantic_inputs_change_the_projection() {
        let owner = symbol(0x4000);
        let baseline = resolver_fingerprint(&resolver(vec![owner.clone()]));

        let mut selected_target = resolver(vec![owner.clone()]);
        selected_target.symbols_by_address.insert(
            0x9000,
            named_symbol(None, "selected", 0x9000, vec![0x13; 4]),
        );
        assert_ne!(baseline, resolver_fingerprint(&selected_target));

        let mut data_symbol = resolver(vec![owner.clone()]);
        data_symbol
            .data_symbols
            .push(artifact::ArtifactDataSymbolDefinition {
                member: Some("data.o".to_owned()),
                name: "state".to_owned(),
                address: 0x1000_8000,
                size: 64,
                exported: true,
            });
        assert_ne!(baseline, resolver_fingerprint(&data_symbol));

        let mut relocated_call = resolver(vec![owner.clone()]);
        relocated_call.relocated_calls.insert(
            direct::StructuralCallSite::new(&owner, 0x4004),
            ("callee".to_owned(), Some(0x9000)),
        );
        assert_ne!(baseline, resolver_fingerprint(&relocated_call));

        let mut context_domain = resolver(vec![owner]);
        context_domain.pointer_context.semantic_cache_domain = "test-harness/v2";
        assert_ne!(baseline, resolver_fingerprint(&context_domain));
    }

    #[test]
    fn ordered_context_maps_ignore_insertion_order() {
        let owner = symbol(0x4000);
        let mut first = resolver(vec![owner.clone()]);
        first
            .pointer_context
            .reviewed_external_pointer_cells
            .insert(0x1000, "first".to_owned());
        first
            .pointer_context
            .reviewed_external_pointer_cells
            .insert(0x2000, "second".to_owned());
        first
            .pointer_context
            .diagnostic_calls
            .insert("alpha".to_owned(), 1);
        first
            .pointer_context
            .diagnostic_calls
            .insert("beta".to_owned(), 2);
        first.relocated_calls.insert(
            direct::StructuralCallSite::new(&owner, 0x4004),
            ("alpha".to_owned(), Some(0x8000)),
        );
        first.relocated_calls.insert(
            direct::StructuralCallSite::new(&owner, 0x4008),
            ("beta".to_owned(), Some(0x9000)),
        );

        let mut second = resolver(vec![owner.clone()]);
        second
            .pointer_context
            .reviewed_external_pointer_cells
            .insert(0x2000, "second".to_owned());
        second
            .pointer_context
            .reviewed_external_pointer_cells
            .insert(0x1000, "first".to_owned());
        second
            .pointer_context
            .diagnostic_calls
            .insert("beta".to_owned(), 2);
        second
            .pointer_context
            .diagnostic_calls
            .insert("alpha".to_owned(), 1);
        second.relocated_calls.insert(
            direct::StructuralCallSite::new(&owner, 0x4008),
            ("beta".to_owned(), Some(0x9000)),
        );
        second.relocated_calls.insert(
            direct::StructuralCallSite::new(&owner, 0x4004),
            ("alpha".to_owned(), Some(0x8000)),
        );

        assert_eq!(resolver_fingerprint(&first), resolver_fingerprint(&second));
    }

    #[test]
    fn unkeyed_summary_hooks_bypass_initial_late_and_persistent_store_io() {
        let owner = symbol(0x4000);
        let mut resolver = resolver(vec![owner.clone()]);
        resolver.pointer_context.summary_hooks = Some(&UNKEYED_SUMMARY_HOOKS);
        assert!(resolver.pointer_context.semantic_cache_domain.is_empty());
        let mmio = MmioMap {
            registers: Vec::new(),
            regions: Vec::new(),
        };
        let mut store = CountingStore::default();
        let cache =
            FunctionCacheRun::prepare(&resolver, [&owner], &mmio, "test", true, Some(&store));
        assert_eq!(
            cache.disabled_reason(),
            Some(FunctionCacheDisabledReason::UnsafeSemanticDomain)
        );

        cache.load_symbols([&owner], Some(&store));
        let analyses = Cell::new(0);
        let graph = cache.direct_graph(&owner, || {
            analyses.set(analyses.get() + 1);
            DirectCallGraph::default()
        });
        assert!(graph.calls.is_empty());
        cache.persist(&mut store);

        assert_eq!(analyses.get(), 1);
        assert_eq!(store.loads.get(), 0);
        assert_eq!(store.stores, 0);
    }

    #[test]
    fn absent_store_disables_initial_late_and_persistent_cache_work() {
        let owner = symbol(0x4000);
        let resolver = resolver(vec![owner.clone()]);
        let mmio = MmioMap {
            registers: Vec::new(),
            regions: Vec::new(),
        };
        let cache = FunctionCacheRun::prepare(&resolver, [&owner], &mmio, "test", true, None);
        assert_eq!(
            cache.disabled_reason(),
            Some(FunctionCacheDisabledReason::StoreAbsent)
        );
        assert_eq!(cache.computed_key_count(), 0);

        let mut late_store = CountingStore::default();
        cache.load_symbols([&owner], Some(&late_store));
        let analyses = Cell::new(0);
        let graph = cache.direct_graph(&owner, || {
            analyses.set(analyses.get() + 1);
            DirectCallGraph::default()
        });
        assert!(graph.calls.is_empty());
        cache.persist(&mut late_store);

        assert_eq!(analyses.get(), 1);
        assert_eq!(cache.computed_key_count(), 0);
        assert_eq!(late_store.loads.get(), 0);
        assert_eq!(late_store.stores, 0);
    }

    #[test]
    fn initial_lookup_and_direct_analysis_share_one_precomputed_symbol_key() {
        let owner = symbol(0x4000);
        let cloned_definition = owner.clone();
        let resolver = resolver(vec![owner.clone()]);
        let mmio = MmioMap {
            registers: Vec::new(),
            regions: Vec::new(),
        };
        let store = CountingStore::default();
        let cache =
            FunctionCacheRun::prepare(&resolver, [&owner], &mmio, "test", true, Some(&store));
        assert_eq!(cache.disabled_reason(), None);
        assert_eq!(cache.computed_key_count(), 1);
        assert_eq!(store.loads.get(), 1);

        cache.load_symbols([&cloned_definition], Some(&store));
        let analyses = Cell::new(0);
        cache.direct_graph(&cloned_definition, || {
            analyses.set(analyses.get() + 1);
            DirectCallGraph::default()
        });

        assert_eq!(analyses.get(), 1);
        assert_eq!(cache.computed_key_count(), 1);
        assert_eq!(store.loads.get(), 1);
    }
}
