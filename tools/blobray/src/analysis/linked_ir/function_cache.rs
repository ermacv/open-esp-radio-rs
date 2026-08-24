//! Persistent, address-independent facts for one structurally analyzed function.
//!
//! Only call-free, blocker-free leaf results enter this first cache domain.
//! Call identity and resolver projection remain link-owned and are deliberately
//! recomputed. Instruction sites are stored as function-relative offsets and
//! materialized against the authoritative linked definition on every hit.

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

const FUNCTION_FACT_DOMAIN: &[u8] = b"blobray/direct-function-facts/v10\0";
const MAX_CACHED_CALL_VARIANTS: usize = 1_024;
const MAX_COMPRESSED_FACT_BYTES: usize = 4 * 1024 * 1024;

pub(super) struct FunctionCacheRun {
    hits: Mutex<BTreeMap<String, Vec<u8>>>,
    loaded: AtomicUsize,
    looked_up: Mutex<BTreeSet<String>>,
    pending: Mutex<BTreeMap<String, Vec<u8>>>,
    mmio_fingerprint: [u8; 32],
    resolver_fingerprint: [u8; 32],
    namespace: String,
    reused: AtomicUsize,
}

impl FunctionCacheRun {
    pub(super) fn prepare<'a>(
        resolver: &ReferenceResolver,
        symbols: impl IntoIterator<Item = &'a artifact::ArtifactSymbolDefinition>,
        svd: &MmioMap,
        namespace: &str,
        store: Option<&dyn FunctionFactStore>,
    ) -> Self {
        let mmio_fingerprint = mmio_fingerprint(svd);
        let resolver_fingerprint = resolver_fingerprint(resolver);
        let cache = Self {
            hits: Mutex::new(BTreeMap::new()),
            loaded: AtomicUsize::new(0),
            looked_up: Mutex::new(BTreeSet::new()),
            pending: Mutex::new(BTreeMap::new()),
            mmio_fingerprint,
            resolver_fingerprint,
            namespace: namespace.to_owned(),
            reused: AtomicUsize::new(0),
        };
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
        let Some(store) = store else {
            return;
        };
        let keys = symbols
            .into_iter()
            .map(|symbol| {
                function_fact_key(symbol, &self.mmio_fingerprint, &self.resolver_fingerprint)
            })
            .collect::<BTreeSet<_>>();
        let keys = {
            let mut looked_up = self.looked_up.lock().expect("function cache lookup lock");
            keys.into_iter()
                .filter(|key| looked_up.insert(key.clone()))
                .collect::<Vec<_>>()
        };
        if keys.is_empty() {
            return;
        }
        match store.load_function_facts(&keys) {
            Ok(values) => {
                let mut hits = self.hits.lock().expect("function cache hit lock");
                let before = hits.len();
                for (key, value) in values {
                    if keys.binary_search(&key).is_ok() {
                        hits.entry(key).or_insert(value);
                    }
                }
                self.loaded
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
        let key = function_fact_key(symbol, &self.mmio_fingerprint, &self.resolver_fingerprint);
        let cached = self
            .hits
            .lock()
            .expect("function cache hit lock")
            .remove(&key);
        if let Some(value) = cached
            && let Ok(fact) = decode_fact(&value)
            && let Some(graph) = fact.materialize(symbol, &self.namespace)
        {
            self.reused.fetch_add(1, Ordering::Relaxed);
            tracing::trace!(symbol = symbol.name, "reused persistent function fact");
            return graph;
        }
        let started = std::time::Instant::now();
        let graph = analyze();
        let elapsed = started.elapsed();
        if let Some(fact) = PortableDirectFacts::capture(symbol, &graph, &self.namespace)
            && let Ok(value) = encode_fact(&fact)
            && value.len() <= MAX_COMPRESSED_FACT_BYTES
        {
            self.pending
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
        let facts = std::mem::take(&mut *self.pending.lock().expect("function cache pending lock"))
            .into_iter()
            .collect::<Vec<_>>();
        if !facts.is_empty()
            && let Err(error) = store.store_function_facts(&facts)
        {
            tracing::warn!(%error, facts = facts.len(), "function-fact cache write failed");
        }
        tracing::debug!(
            loaded = self.loaded.load(Ordering::Relaxed),
            reused = self.reused.load(Ordering::Relaxed),
            stored = facts.len(),
            "completed persistent function-fact cache session"
        );
    }
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
) -> String {
    let mut hash = Sha256::new();
    hash.update(FUNCTION_FACT_DOMAIN);
    hash.update(mmio_fingerprint);
    hash.update(resolver_fingerprint);
    hash.update(symbol.name.as_bytes());
    hash.update([0]);
    hash.update([u8::from(symbol.addresses_resolved)]);
    hash.update((symbol.bytes.len() as u64).to_le_bytes());
    hash.update(&symbol.bytes);
    for relocation in &symbol.relocations {
        hash.update(
            relocation
                .address
                .wrapping_sub(symbol.address as u32)
                .to_le_bytes(),
        );
        hash.update(relocation_kind(relocation.kind).as_bytes());
        hash.update([0]);
        hash.update(relocation.symbol.as_bytes());
        hash.update([0]);
        hash.update(relocation.addend.to_le_bytes());
    }
    for region in symbol.memory_regions.iter() {
        hash.update(region.start.to_le_bytes());
        hash.update(region.length.to_le_bytes());
        hash.update([u8::from(region.writable)]);
        hash.update(region.name.as_bytes());
        hash.update([0]);
    }
    format!("function-direct:{:x}", hash.finalize())
}

fn resolver_fingerprint(resolver: &ReferenceResolver) -> [u8; 32] {
    let base = resolver
        .symbols
        .iter()
        .map(|symbol| symbol.address)
        .min()
        .unwrap_or(0);
    let mut symbols = resolver.symbols.iter().collect::<Vec<_>>();
    symbols.sort_by_key(|symbol| (&symbol.member, &symbol.name, symbol.address));
    let mut hash = Sha256::new();
    hash.update(b"blobray/resolver-layout/v1\0");
    for symbol in symbols {
        if let Some(member) = &symbol.member {
            hash.update(member.as_bytes());
        }
        hash.update([0]);
        hash.update(symbol.name.as_bytes());
        hash.update([0]);
        hash.update(symbol.address.wrapping_sub(base).to_le_bytes());
        hash.update((symbol.bytes.len() as u64).to_le_bytes());
        hash.update(Sha256::digest(&symbol.bytes));
    }
    hash.finalize().into()
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
    for register in &svd.registers {
        hash.update(register.address.to_le_bytes());
        hash.update(register.name.as_bytes());
        hash.update([0]);
    }
    for region in &svd.regions {
        hash.update(region.start.to_le_bytes());
        hash.update(region.end.to_le_bytes());
        hash.update([u8::from(region.readable), u8::from(region.writable)]);
        hash.update(region.name.as_bytes());
        hash.update([0]);
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
    tail: bool,
    result_modeled: bool,
    semantics: Option<String>,
    semantic_operation: Option<String>,
    replacement_hint: Option<String>,
    argument_shapes: usize,
    arguments: Vec<String>,
    argument_bindings: Vec<PortableArgumentBinding>,
    typed_arguments: Vec<PortableCallArgument>,
    guard_paths: Option<Vec<Vec<u32>>>,
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
            tail: call.tail,
            result_modeled: call.result_modeled,
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
            tail: self.tail,
            result_modeled: self.result_modeled,
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
    use std::sync::Arc;

    use super::*;

    fn symbol(address: u64) -> artifact::ArtifactSymbolDefinition {
        artifact::ArtifactSymbolDefinition {
            member: Some("radio.o".to_owned()),
            name: "leaf".to_owned(),
            address,
            bytes: vec![0x13; 0x24],
            addresses_resolved: true,
            memory_regions: Arc::from([]),
            relocations: Vec::new(),
        }
    }

    #[test]
    fn key_ignores_linked_base_and_sites_rebase_on_materialization() {
        let mmio = MmioMap {
            registers: Vec::new(),
            regions: Vec::new(),
        };
        let fingerprint = mmio_fingerprint(&mmio);
        let first = symbol(0x4000);
        let second = symbol(0x8000);
        assert_eq!(
            function_fact_key(&first, &fingerprint, &[0; 32]),
            function_fact_key(&second, &fingerprint, &[0; 32])
        );
        let graph = DirectCallGraph {
            calls: BTreeSet::from([LinkedCall {
                kind: "internal",
                target: "first::callee".to_owned(),
                site: Some(0x4000),
                tail: false,
                result_modeled: false,
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
                argument_bindings: Vec::new(),
                typed_arguments: Vec::new(),
                guard_paths: Some(vec![LinkedCallGuardPath { guards: Vec::new() }]),
            }]),
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
        assert_eq!(call.site, Some(0x8000));
        assert_eq!(call.target, "second::callee");
        assert_eq!(call.arguments, ["second::callee(arg0)"]);
        assert_eq!(
            rebound.blockers,
            BTreeSet::from([
                "branch at 0x00008000 is incomplete".to_owned(),
                "branch at 0x00008020 is incomplete".to_owned(),
            ])
        );
    }
}
