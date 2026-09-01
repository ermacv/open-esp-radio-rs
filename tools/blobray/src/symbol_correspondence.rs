//! Address- and name-independent function correspondence between artifacts.
//!
//! Vendor symbol names may be regenerated independently of the function body.
//! This module therefore treats names as locators and compares normalized
//! relocatable bodies.  A match is publishable only when the fingerprint is
//! unique on both sides; duplicate compiler-generated leaves remain explicit
//! ambiguity instead of receiving a guessed name.

use std::{collections::BTreeMap, path::Path};

use serde::Serialize;
use sha2::{Digest, Sha256};

use open_radio_vendor_contracts::{ArtifactIdentity, EntityDomain, RevisionOccurrenceId};

use crate::{Result, artifact};

pub(crate) const SYMBOL_CORRESPONDENCE_SCHEMA: u32 = 2;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum SymbolCorrespondenceStatus {
    Unique,
    Ambiguous,
    Unmatched,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct SymbolCorrespondenceArtifact {
    pub(crate) source: String,
    pub(crate) path: String,
    pub(crate) sha256: String,
    pub(crate) functions: usize,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub(crate) struct SymbolCorrespondenceFunction {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) member: Option<String>,
    pub(crate) symbol: String,
    pub(crate) locator: String,
    pub(crate) occurrence: String,
    pub(crate) size: usize,
    pub(crate) fingerprint: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct SymbolMemberCorrespondence {
    pub(crate) from: String,
    pub(crate) to: String,
    pub(crate) exact_function_support: usize,
    pub(crate) exact_function_conflicts: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct SymbolMemberOrderEvidence {
    pub(crate) basis: &'static str,
    pub(crate) automatic_function_matches: bool,
    pub(crate) exact_function_support: usize,
    pub(crate) exact_function_conflicts: usize,
    pub(crate) support_parts_per_million: u32,
    pub(crate) correspondences: Vec<SymbolMemberCorrespondence>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub(crate) struct DataObjectCorrespondenceObject {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) member: Option<String>,
    pub(crate) section: String,
    pub(crate) symbol: String,
    pub(crate) aliases: Vec<String>,
    pub(crate) object_offset: u64,
    pub(crate) size: u64,
    pub(crate) writable: bool,
    pub(crate) initialized: bool,
    pub(crate) locator: String,
    pub(crate) occurrence: String,
    pub(crate) fingerprint: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct DataObjectCorrespondence {
    pub(crate) from: DataObjectCorrespondenceObject,
    pub(crate) status: SymbolCorrespondenceStatus,
    pub(crate) basis: &'static str,
    pub(crate) candidates: Vec<DataObjectCorrespondenceObject>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub(crate) struct DataObjectCorrespondenceSummary {
    pub(crate) from_objects: usize,
    pub(crate) to_objects: usize,
    pub(crate) unique: usize,
    pub(crate) reference_refined: usize,
    pub(crate) ambiguous: usize,
    pub(crate) unmatched: usize,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub(crate) struct SemanticPinCandidate {
    pub(crate) domain: &'static str,
    pub(crate) review: &'static str,
    pub(crate) suggested_name: String,
    pub(crate) target_occurrence: String,
    pub(crate) target_locator: String,
    pub(crate) correspondence_basis: &'static str,
    pub(crate) source_occurrence: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct SymbolCorrespondence {
    pub(crate) from: SymbolCorrespondenceFunction,
    pub(crate) status: SymbolCorrespondenceStatus,
    pub(crate) basis: &'static str,
    pub(crate) candidates: Vec<SymbolCorrespondenceFunction>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub(crate) struct SymbolCorrespondenceSummary {
    pub(crate) unique: usize,
    pub(crate) graph_refined: usize,
    pub(crate) ambiguous: usize,
    pub(crate) unmatched: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct SymbolCorrespondenceReport {
    pub(crate) schema_version: u32,
    pub(crate) command: &'static str,
    pub(crate) method: &'static str,
    pub(crate) from: SymbolCorrespondenceArtifact,
    pub(crate) to: SymbolCorrespondenceArtifact,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) member_order: Option<SymbolMemberOrderEvidence>,
    pub(crate) summary: SymbolCorrespondenceSummary,
    pub(crate) correspondences: Vec<SymbolCorrespondence>,
    pub(crate) data_summary: DataObjectCorrespondenceSummary,
    pub(crate) data_correspondences: Vec<DataObjectCorrespondence>,
    pub(crate) pin_candidates: Vec<SemanticPinCandidate>,
}

pub(crate) struct SymbolCorrespondenceRequest<'a> {
    pub(crate) from_source: &'a str,
    pub(crate) from_path: &'a Path,
    pub(crate) from_prefix: &'a str,
    pub(crate) to_source: &'a str,
    pub(crate) to_path: &'a Path,
    pub(crate) to_prefix: &'a str,
}

pub(crate) fn correlate(
    request: SymbolCorrespondenceRequest<'_>,
) -> Result<SymbolCorrespondenceReport> {
    let from_symbols = load_functions(request.from_path, request.from_prefix)?;
    let to_symbols = load_functions(request.to_path, request.to_prefix)?;
    let from_artifact =
        artifact_document(request.from_source, request.from_path, from_symbols.len())?;
    let to_artifact = artifact_document(request.to_source, request.to_path, to_symbols.len())?;
    let from_identity = ArtifactIdentity::new(&from_artifact.source, &from_artifact.sha256)
        .map_err(|error| crate::Error::invalid(error.to_string()))?;
    let to_identity = ArtifactIdentity::new(&to_artifact.source, &to_artifact.sha256)
        .map_err(|error| crate::Error::invalid(error.to_string()))?;
    let mut source_fingerprints = BTreeMap::<String, usize>::new();
    for symbol in &from_symbols {
        *source_fingerprints
            .entry(normalized_body_fingerprint(symbol))
            .or_default() += 1;
    }
    let mut targets = BTreeMap::<String, Vec<SymbolCorrespondenceFunction>>::new();
    for symbol in &to_symbols {
        targets
            .entry(normalized_body_fingerprint(symbol))
            .or_default()
            .push(function_document(symbol, &to_identity)?);
    }
    for candidates in targets.values_mut() {
        candidates.sort();
    }

    let mut correspondences = from_symbols
        .iter()
        .map(|symbol| {
            let from = function_document(symbol, &from_identity)?;
            let candidates = targets.get(&from.fingerprint).cloned().unwrap_or_default();
            let status = match (
                source_fingerprints.get(&from.fingerprint).copied(),
                candidates.len(),
            ) {
                (_, 0) => SymbolCorrespondenceStatus::Unmatched,
                (Some(1), 1) => SymbolCorrespondenceStatus::Unique,
                _ => SymbolCorrespondenceStatus::Ambiguous,
            };
            Ok(SymbolCorrespondence {
                from,
                status,
                basis: "exact-normalized-body",
                candidates,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    refine_ambiguous_matches(&from_symbols, &to_symbols, &mut correspondences);
    correspondences.sort_by(|left, right| left.from.cmp(&right.from));
    let mut summary = SymbolCorrespondenceSummary::default();
    for correspondence in &correspondences {
        match correspondence.status {
            SymbolCorrespondenceStatus::Unique => {
                summary.unique += 1;
                summary.graph_refined += usize::from(
                    correspondence.basis == "exact-normalized-body-and-mapped-call-graph",
                );
            }
            SymbolCorrespondenceStatus::Ambiguous => summary.ambiguous += 1,
            SymbolCorrespondenceStatus::Unmatched => summary.unmatched += 1,
        }
    }

    let member_order = infer_member_order(request.from_path, request.to_path)?
        .map(|mapping| member_order_evidence(mapping, &correspondences));
    let from_data = artifact::load_data_objects(request.from_path)?;
    let to_data = artifact::load_data_objects(request.to_path)?;
    let (data_summary, data_correspondences) = correlate_data_objects(
        &from_data,
        &to_data,
        &from_symbols,
        &to_symbols,
        &correspondences,
        &from_identity,
        &to_identity,
    )?;
    let mut pin_candidates = function_pin_candidates(&correspondences);
    pin_candidates.extend(data_pin_candidates(&data_correspondences));
    pin_candidates.sort();

    Ok(SymbolCorrespondenceReport {
        schema_version: SYMBOL_CORRESPONDENCE_SCHEMA,
        command: "symbols correlate",
        method: "sha256-relocatable-body-and-relocation-shape-v1",
        from: from_artifact,
        to: to_artifact,
        member_order,
        summary,
        correspondences,
        data_summary,
        data_correspondences,
        pin_candidates,
    })
}

fn refine_ambiguous_matches(
    from_symbols: &[artifact::ArtifactSymbolDefinition],
    to_symbols: &[artifact::ArtifactSymbolDefinition],
    correspondences: &mut [SymbolCorrespondence],
) {
    let from_by_name = unique_symbols_by_name(from_symbols);
    let to_by_name = unique_symbols_by_name(to_symbols);
    let mut changed = true;
    while changed {
        changed = false;
        let mut mappings = correspondences
            .iter()
            .filter(|correspondence| {
                correspondence.status == SymbolCorrespondenceStatus::Unique
                    && correspondence.candidates.len() == 1
            })
            .map(|correspondence| {
                (
                    correspondence.from.symbol.clone(),
                    correspondence.candidates[0].symbol.clone(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        // Stable, non-obfuscated exported names are safe graph anchors even
        // when their bodies changed between releases. They do not themselves
        // become an automatic body-correspondence result.
        for name in from_by_name.keys() {
            if to_by_name.contains_key(name) {
                mappings
                    .entry((*name).to_owned())
                    .or_insert_with(|| (*name).to_owned());
            }
        }

        let mut edge_votes = BTreeMap::<&str, Vec<&str>>::new();
        for (from_name, to_name) in &mappings {
            let (Some(from), Some(to)) = (
                from_by_name.get(from_name.as_str()),
                to_by_name.get(to_name.as_str()),
            ) else {
                continue;
            };
            let from_calls = call_relocations(from);
            let to_calls = call_relocations(to);
            if !aligned_call_shape(&from_calls, &to_calls) {
                continue;
            }
            for (from_call, to_call) in from_calls.iter().zip(to_calls) {
                edge_votes
                    .entry(from_call.symbol.as_str())
                    .or_default()
                    .push(to_call.symbol.as_str());
            }
        }

        for correspondence in correspondences
            .iter_mut()
            .filter(|correspondence| correspondence.status == SymbolCorrespondenceStatus::Ambiguous)
        {
            let Some(from) = from_by_name.get(correspondence.from.symbol.as_str()) else {
                continue;
            };
            let from_calls = call_relocations(from);
            let voted_targets = edge_votes
                .get(correspondence.from.symbol.as_str())
                .map(Vec::as_slice)
                .unwrap_or_default();
            let has_graph_evidence = !voted_targets.is_empty()
                || from_calls
                    .iter()
                    .any(|call| mappings.contains_key(call.symbol.as_str()));
            if !has_graph_evidence {
                continue;
            }
            let mut viable = correspondence
                .candidates
                .iter()
                .filter(|candidate| {
                    if !voted_targets.is_empty()
                        && !voted_targets
                            .iter()
                            .all(|target| *target == candidate.symbol)
                    {
                        return false;
                    }
                    let Some(target) = to_by_name.get(candidate.symbol.as_str()) else {
                        return false;
                    };
                    let to_calls = call_relocations(target);
                    aligned_call_shape(&from_calls, &to_calls)
                        && from_calls.iter().zip(to_calls).all(|(from_call, to_call)| {
                            mappings
                                .get(from_call.symbol.as_str())
                                .is_none_or(|mapped| mapped == &to_call.symbol)
                        })
                })
                .cloned()
                .collect::<Vec<_>>();
            viable.sort();
            viable.dedup();
            if viable.len() == 1 {
                correspondence.status = SymbolCorrespondenceStatus::Unique;
                correspondence.basis = "exact-normalized-body-and-mapped-call-graph";
                correspondence.candidates = viable;
                changed = true;
            }
        }
    }
}

fn unique_symbols_by_name(
    symbols: &[artifact::ArtifactSymbolDefinition],
) -> BTreeMap<&str, &artifact::ArtifactSymbolDefinition> {
    let mut candidates = BTreeMap::<&str, Vec<&artifact::ArtifactSymbolDefinition>>::new();
    for symbol in symbols {
        candidates.entry(&symbol.name).or_default().push(symbol);
    }
    candidates
        .into_iter()
        .filter_map(|(name, symbols)| (symbols.len() == 1).then_some((name, symbols[0])))
        .collect()
}

fn call_relocations(
    symbol: &artifact::ArtifactSymbolDefinition,
) -> Vec<&artifact::SymbolRelocation> {
    symbol
        .relocations
        .iter()
        .filter(|relocation| {
            matches!(
                relocation.kind,
                artifact::RelocationKind::Call | artifact::RelocationKind::CallPlt
            )
        })
        .collect()
}

fn aligned_call_shape(
    from: &[&artifact::SymbolRelocation],
    to: &[&artifact::SymbolRelocation],
) -> bool {
    from.len() == to.len()
        && from.iter().zip(to).all(|(from, to)| {
            from.address == to.address && from.kind == to.kind && from.addend == to.addend
        })
}

#[allow(clippy::too_many_arguments)]
fn correlate_data_objects(
    from_objects: &[artifact::ArtifactDataObjectDefinition],
    to_objects: &[artifact::ArtifactDataObjectDefinition],
    from_functions: &[artifact::ArtifactSymbolDefinition],
    to_functions: &[artifact::ArtifactSymbolDefinition],
    function_correspondences: &[SymbolCorrespondence],
    from_artifact: &ArtifactIdentity,
    to_artifact: &ArtifactIdentity,
) -> Result<(
    DataObjectCorrespondenceSummary,
    Vec<DataObjectCorrespondence>,
)> {
    let mut source_fingerprints = BTreeMap::<String, usize>::new();
    for object in from_objects {
        *source_fingerprints
            .entry(normalized_data_fingerprint(object))
            .or_default() += 1;
    }
    let mut targets = BTreeMap::<String, Vec<DataObjectCorrespondenceObject>>::new();
    for object in to_objects {
        targets
            .entry(normalized_data_fingerprint(object))
            .or_default()
            .push(data_object_document(object, to_artifact)?);
    }
    for candidates in targets.values_mut() {
        candidates.sort();
    }
    let mut correspondences = from_objects
        .iter()
        .map(|object| {
            let from = data_object_document(object, from_artifact)?;
            let candidates = targets.get(&from.fingerprint).cloned().unwrap_or_default();
            let status = match (
                source_fingerprints.get(&from.fingerprint).copied(),
                candidates.len(),
            ) {
                (_, 0) => SymbolCorrespondenceStatus::Unmatched,
                (Some(1), 1) => SymbolCorrespondenceStatus::Unique,
                _ => SymbolCorrespondenceStatus::Ambiguous,
            };
            Ok(DataObjectCorrespondence {
                from,
                status,
                basis: "exact-normalized-data-object",
                candidates,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let votes = mapped_function_data_votes(
        from_functions,
        to_functions,
        function_correspondences,
        from_objects,
        to_objects,
    );
    let target_documents = to_objects
        .iter()
        .map(|object| {
            let document = data_object_document(object, to_artifact)?;
            Ok((document.locator.clone(), document))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    for correspondence in &mut correspondences {
        let Some(target_locator) = votes.get(&correspondence.from.locator) else {
            continue;
        };
        let Some(target) = target_documents.get(target_locator) else {
            continue;
        };
        match correspondence.status {
            SymbolCorrespondenceStatus::Unique
                if correspondence.candidates.first() == Some(target) =>
            {
                correspondence.basis = "exact-normalized-data-object-and-mapped-function-reference";
            }
            SymbolCorrespondenceStatus::Unique => {
                correspondence.status = SymbolCorrespondenceStatus::Ambiguous;
                correspondence.basis = "conflicting-data-body-and-mapped-function-reference";
                correspondence.candidates.push(target.clone());
                correspondence.candidates.sort();
                correspondence.candidates.dedup();
            }
            SymbolCorrespondenceStatus::Ambiguous | SymbolCorrespondenceStatus::Unmatched => {
                correspondence.status = SymbolCorrespondenceStatus::Unique;
                correspondence.basis = "mapped-function-reference";
                correspondence.candidates = vec![target.clone()];
            }
        }
    }
    correspondences.sort_by(|left, right| left.from.cmp(&right.from));
    let mut summary = DataObjectCorrespondenceSummary {
        from_objects: from_objects.len(),
        to_objects: to_objects.len(),
        ..DataObjectCorrespondenceSummary::default()
    };
    for correspondence in &correspondences {
        match correspondence.status {
            SymbolCorrespondenceStatus::Unique => {
                summary.unique += 1;
                summary.reference_refined +=
                    usize::from(correspondence.basis == "mapped-function-reference");
            }
            SymbolCorrespondenceStatus::Ambiguous => summary.ambiguous += 1,
            SymbolCorrespondenceStatus::Unmatched => summary.unmatched += 1,
        }
    }
    Ok((summary, correspondences))
}

fn mapped_function_data_votes(
    from_functions: &[artifact::ArtifactSymbolDefinition],
    to_functions: &[artifact::ArtifactSymbolDefinition],
    correspondences: &[SymbolCorrespondence],
    from_objects: &[artifact::ArtifactDataObjectDefinition],
    to_objects: &[artifact::ArtifactDataObjectDefinition],
) -> BTreeMap<String, String> {
    let from_functions = from_functions
        .iter()
        .map(|function| (function_locator(function), function))
        .collect::<BTreeMap<_, _>>();
    let to_functions = to_functions
        .iter()
        .map(|function| (function_locator(function), function))
        .collect::<BTreeMap<_, _>>();
    let from_aliases = data_alias_index(from_objects);
    let to_aliases = data_alias_index(to_objects);
    let mut votes = BTreeMap::<String, std::collections::BTreeSet<String>>::new();
    for correspondence in correspondences.iter().filter(|correspondence| {
        correspondence.status == SymbolCorrespondenceStatus::Unique
            && correspondence.candidates.len() == 1
    }) {
        let (Some(from), Some(to)) = (
            from_functions.get(&correspondence.from.locator),
            to_functions.get(&correspondence.candidates[0].locator),
        ) else {
            continue;
        };
        if !aligned_relocation_shape(&from.relocations, &to.relocations) {
            continue;
        }
        for (from_relocation, to_relocation) in from.relocations.iter().zip(&to.relocations) {
            let (Some(from_object), Some(to_object)) = (
                resolve_data_alias(
                    &from_aliases,
                    from.member.as_deref(),
                    &from_relocation.symbol,
                ),
                resolve_data_alias(&to_aliases, to.member.as_deref(), &to_relocation.symbol),
            ) else {
                continue;
            };
            votes.entry(from_object).or_default().insert(to_object);
        }
    }
    unique_one_to_one_votes(votes)
}

fn aligned_relocation_shape(
    from: &[artifact::SymbolRelocation],
    to: &[artifact::SymbolRelocation],
) -> bool {
    from.len() == to.len()
        && from.iter().zip(to).all(|(from, to)| {
            from.address == to.address && from.kind == to.kind && from.addend == to.addend
        })
}

fn data_alias_index(
    objects: &[artifact::ArtifactDataObjectDefinition],
) -> BTreeMap<(Option<String>, String), std::collections::BTreeSet<String>> {
    let mut index = BTreeMap::<_, std::collections::BTreeSet<_>>::new();
    for object in objects {
        let locator = data_object_locator(object);
        for alias in object.aliases.iter().chain(std::iter::once(&object.name)) {
            index
                .entry((object.member.clone(), alias.clone()))
                .or_default()
                .insert(locator.clone());
            if object.member.is_some() {
                index
                    .entry((None, alias.clone()))
                    .or_default()
                    .insert(locator.clone());
            }
        }
    }
    index
}

fn resolve_data_alias(
    index: &BTreeMap<(Option<String>, String), std::collections::BTreeSet<String>>,
    owner_member: Option<&str>,
    symbol: &str,
) -> Option<String> {
    let member_key = (owner_member.map(str::to_owned), symbol.to_owned());
    let candidates = index
        .get(&member_key)
        .filter(|candidates| candidates.len() == 1)
        .or_else(|| {
            index
                .get(&(None, symbol.to_owned()))
                .filter(|set| set.len() == 1)
        })?;
    candidates.first().cloned()
}

fn unique_one_to_one_votes(
    votes: BTreeMap<String, std::collections::BTreeSet<String>>,
) -> BTreeMap<String, String> {
    let mut reverse = BTreeMap::<String, std::collections::BTreeSet<String>>::new();
    for (from, candidates) in &votes {
        if let Some(to) = candidates.first().filter(|_| candidates.len() == 1) {
            reverse.entry(to.clone()).or_default().insert(from.clone());
        }
    }
    votes
        .into_iter()
        .filter_map(|(from, candidates)| {
            let to = candidates.into_iter().next()?;
            (reverse.get(&to).is_some_and(|sources| sources.len() == 1)).then_some((from, to))
        })
        .collect()
}

fn data_object_document(
    object: &artifact::ArtifactDataObjectDefinition,
    artifact: &ArtifactIdentity,
) -> Result<DataObjectCorrespondenceObject> {
    let locator = data_object_locator(object);
    let occurrence = RevisionOccurrenceId::derive(
        EntityDomain::MemoryObject,
        std::slice::from_ref(artifact),
        &locator,
    )
    .map_err(|error| crate::Error::invalid(error.to_string()))?;
    Ok(DataObjectCorrespondenceObject {
        member: object.member.clone(),
        section: object.section.clone(),
        symbol: object.name.clone(),
        aliases: object.aliases.clone(),
        object_offset: object.object_offset,
        size: object.size,
        writable: object.writable,
        initialized: object.initialized,
        locator,
        occurrence: occurrence.to_string(),
        fingerprint: normalized_data_fingerprint(object),
    })
}

fn data_object_locator(object: &artifact::ArtifactDataObjectDefinition) -> String {
    match object.member.as_deref() {
        Some(member) => format!(
            "archive-member:{member}/section:{}/symbol:{}/object-offset:{:#x}/size:{:#x}",
            object.section, object.name, object.object_offset, object.size
        ),
        None => format!(
            "section:{}/symbol:{}/address:{:#x}/size:{:#x}",
            object.section,
            object.name,
            object.address.unwrap_or(object.object_offset as u32),
            object.size
        ),
    }
}

fn normalized_data_fingerprint(object: &artifact::ArtifactDataObjectDefinition) -> String {
    let mut hash = Sha256::new();
    hash.update(b"blobray/symbol-correspondence/data-object/v1\0");
    hash.update(object.size.to_le_bytes());
    hash.update([u8::from(object.writable), u8::from(object.initialized)]);
    hash.update((object.initializer.len() as u64).to_le_bytes());
    hash.update(&object.initializer);
    hash.update((object.relocations.len() as u64).to_le_bytes());
    for relocation in &object.relocations {
        hash.update(relocation.offset.to_le_bytes());
        hash.update(relocation.elf_type.unwrap_or(u32::MAX).to_le_bytes());
        hash.update(relocation.addend.to_le_bytes());
    }
    format!("sha256:{:x}", hash.finalize())
}

fn function_pin_candidates(correspondences: &[SymbolCorrespondence]) -> Vec<SemanticPinCandidate> {
    correspondences
        .iter()
        .filter(|correspondence| {
            correspondence.status == SymbolCorrespondenceStatus::Unique
                && correspondence.candidates.len() == 1
        })
        .map(|correspondence| SemanticPinCandidate {
            domain: "function",
            review: "required",
            suggested_name: correspondence.from.symbol.clone(),
            target_occurrence: correspondence.candidates[0].occurrence.clone(),
            target_locator: correspondence.candidates[0].locator.clone(),
            correspondence_basis: correspondence.basis,
            source_occurrence: correspondence.from.occurrence.clone(),
        })
        .collect()
}

fn data_pin_candidates(correspondences: &[DataObjectCorrespondence]) -> Vec<SemanticPinCandidate> {
    correspondences
        .iter()
        .filter(|correspondence| {
            correspondence.status == SymbolCorrespondenceStatus::Unique
                && correspondence.candidates.len() == 1
        })
        .map(|correspondence| SemanticPinCandidate {
            domain: "memory-object",
            review: "required",
            suggested_name: correspondence.from.symbol.clone(),
            target_occurrence: correspondence.candidates[0].occurrence.clone(),
            target_locator: correspondence.candidates[0].locator.clone(),
            correspondence_basis: correspondence.basis,
            source_occurrence: correspondence.from.occurrence.clone(),
        })
        .collect()
}

fn infer_member_order(from: &Path, to: &Path) -> Result<Option<BTreeMap<String, String>>> {
    let Some(from_members) = artifact::load_archive_member_names(from)? else {
        return Ok(None);
    };
    let Some(to_members) = artifact::load_archive_member_names(to)? else {
        return Ok(None);
    };
    Ok(infer_member_order_from_names(&from_members, &to_members))
}

fn infer_member_order_from_names(
    from_members: &[String],
    to_members: &[String],
) -> Option<BTreeMap<String, String>> {
    if from_members.len() != to_members.len() || from_members.is_empty() {
        return None;
    }
    if let Some(numeric) = contiguous_numeric_members(to_members) {
        let mut named = named_members(from_members)?;
        named.sort();
        return Some(
            named
                .into_iter()
                .enumerate()
                .map(|(ordinal, name)| (name, numeric[&ordinal].clone()))
                .collect(),
        );
    }
    if let Some(numeric) = contiguous_numeric_members(from_members) {
        let mut named = named_members(to_members)?;
        named.sort();
        return Some(
            named
                .into_iter()
                .enumerate()
                .map(|(ordinal, name)| (numeric[&ordinal].clone(), name))
                .collect(),
        );
    }
    None
}

fn named_members(members: &[String]) -> Option<Vec<String>> {
    let names = members
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    (names.len() == members.len()
        && names
            .iter()
            .all(|name| numeric_member_ordinal(name).is_none()))
    .then(|| names.into_iter().collect())
}

fn contiguous_numeric_members(members: &[String]) -> Option<BTreeMap<usize, String>> {
    let numeric = members
        .iter()
        .map(|name| numeric_member_ordinal(name).map(|ordinal| (ordinal, name.clone())))
        .collect::<Option<BTreeMap<_, _>>>()?;
    (numeric.len() == members.len() && numeric.keys().copied().eq(0..members.len()))
        .then_some(numeric)
}

fn numeric_member_ordinal(name: &str) -> Option<usize> {
    let digits = name.strip_suffix(".o")?;
    if digits.is_empty() || (digits.len() > 1 && digits.starts_with('0')) {
        return None;
    }
    digits.parse().ok()
}

fn member_order_evidence(
    mapping: BTreeMap<String, String>,
    correspondences: &[SymbolCorrespondence],
) -> SymbolMemberOrderEvidence {
    let mut support = BTreeMap::<&str, usize>::new();
    let mut conflicts = BTreeMap::<&str, usize>::new();
    for correspondence in correspondences.iter().filter(|correspondence| {
        correspondence.status == SymbolCorrespondenceStatus::Unique
            && correspondence.candidates.len() == 1
    }) {
        let (Some(from_member), Some(to_member)) = (
            correspondence.from.member.as_deref(),
            correspondence.candidates[0].member.as_deref(),
        ) else {
            continue;
        };
        let Some(expected) = mapping.get(from_member) else {
            continue;
        };
        if expected == to_member {
            *support.entry(from_member).or_default() += 1;
        } else {
            *conflicts.entry(from_member).or_default() += 1;
        }
    }
    let exact_function_support: usize = support.values().sum();
    let exact_function_conflicts: usize = conflicts.values().sum();
    let total = exact_function_support + exact_function_conflicts;
    let support_parts_per_million = exact_function_support
        .saturating_mul(1_000_000)
        .checked_div(total)
        .and_then(|ratio| u32::try_from(ratio).ok())
        .unwrap_or(0);
    SymbolMemberOrderEvidence {
        basis: "alphabetical-named-member-to-numeric-member-ordinal",
        // Member order is valuable module provenance and candidate ranking,
        // but code can move between modules across revisions. It never turns
        // an otherwise ambiguous function body into an automatic pin.
        automatic_function_matches: false,
        exact_function_support,
        exact_function_conflicts,
        support_parts_per_million,
        correspondences: mapping
            .into_iter()
            .map(|(from, to)| SymbolMemberCorrespondence {
                exact_function_support: support.get(from.as_str()).copied().unwrap_or(0),
                exact_function_conflicts: conflicts.get(from.as_str()).copied().unwrap_or(0),
                from,
                to,
            })
            .collect(),
    }
}

fn load_functions(path: &Path, prefix: &str) -> Result<Vec<artifact::ArtifactSymbolDefinition>> {
    let mut symbols =
        artifact::load_code_symbols(path, prefix, artifact::CodeSymbolSelection::All)?;
    symbols.sort_by(|left, right| {
        (&left.member, &left.name, left.address).cmp(&(&right.member, &right.name, right.address))
    });
    Ok(symbols)
}

fn artifact_document(
    source: &str,
    path: &Path,
    functions: usize,
) -> Result<SymbolCorrespondenceArtifact> {
    Ok(SymbolCorrespondenceArtifact {
        source: source.to_owned(),
        path: path.display().to_string(),
        sha256: crate::artifact_sha256(path)?,
        functions,
    })
}

fn function_document(
    symbol: &artifact::ArtifactSymbolDefinition,
    artifact: &ArtifactIdentity,
) -> Result<SymbolCorrespondenceFunction> {
    let locator = function_locator(symbol);
    let occurrence = RevisionOccurrenceId::derive(
        EntityDomain::Function,
        std::slice::from_ref(artifact),
        &locator,
    )
    .map_err(|error| crate::Error::invalid(error.to_string()))?;
    Ok(SymbolCorrespondenceFunction {
        member: symbol.member.clone(),
        symbol: symbol.name.clone(),
        locator,
        occurrence: occurrence.to_string(),
        size: symbol.bytes.len(),
        fingerprint: normalized_body_fingerprint(symbol),
    })
}

fn function_locator(symbol: &artifact::ArtifactSymbolDefinition) -> String {
    match symbol.member.as_deref() {
        Some(member) => format!(
            "archive-member:{member}/symbol:{}/object-offset:{:#x}",
            symbol.name, symbol.address
        ),
        None => format!("symbol:{}/address:{:#x}", symbol.name, symbol.address),
    }
}

fn normalized_body_fingerprint(symbol: &artifact::ArtifactSymbolDefinition) -> String {
    let mut hash = Sha256::new();
    hash.update(b"blobray/symbol-correspondence/relocatable-body/v1\0");
    hash.update((symbol.bytes.len() as u64).to_le_bytes());
    hash.update(&symbol.bytes);
    hash.update((symbol.relocations.len() as u64).to_le_bytes());
    for relocation in &symbol.relocations {
        hash.update(
            relocation
                .address
                .wrapping_sub(symbol.address as u32)
                .to_le_bytes(),
        );
        hash.update(relocation_kind(relocation.kind).as_bytes());
        hash.update([0]);
        hash.update(relocation.addend.to_le_bytes());
        // The relocation target name is deliberately excluded.  It is the
        // exact datum that vendor obfuscation rewrites, while the offset, kind
        // and addend retain the body-local linkage shape.
    }
    format!("sha256:{:x}", hash.finalize())
}

const fn relocation_kind(kind: artifact::RelocationKind) -> &'static str {
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn artifact_identity(source: &str, byte: char) -> ArtifactIdentity {
        ArtifactIdentity::new(source, byte.to_string().repeat(64)).unwrap()
    }

    fn symbol(name: &str, bytes: &[u8], target: &str) -> artifact::ArtifactSymbolDefinition {
        artifact::ArtifactSymbolDefinition {
            member: Some("radio.o".to_owned()),
            name: name.to_owned(),
            address: 0,
            bytes: bytes.to_vec(),
            addresses_resolved: false,
            memory_regions: Arc::from([]),
            relocations: vec![artifact::SymbolRelocation {
                address: 4,
                kind: artifact::RelocationKind::Call,
                symbol: target.to_owned(),
                addend: 0,
            }],
        }
    }

    #[test]
    fn normalized_body_ignores_obfuscated_symbol_names_only() {
        let named = symbol("r_ble_adv_start", &[1, 2, 3, 4, 5, 6], "r_ble_sched_run");
        let obfuscated = symbol("r_sym_ble_random", &[1, 2, 3, 4, 5, 6], "r_sym_ble_other");
        assert_eq!(
            normalized_body_fingerprint(&named),
            normalized_body_fingerprint(&obfuscated)
        );

        let mut changed = obfuscated.clone();
        changed.bytes[0] ^= 1;
        assert_ne!(
            normalized_body_fingerprint(&named),
            normalized_body_fingerprint(&changed)
        );

        let mut relocated = obfuscated;
        relocated.relocations[0].addend = 4;
        assert_ne!(
            normalized_body_fingerprint(&named),
            normalized_body_fingerprint(&relocated)
        );
    }

    #[test]
    fn mapped_caller_edges_disambiguate_identical_callees() {
        let mut named_root = symbol("r_ble_root", &[1, 2, 3, 4, 5, 6], "r_ble_leaf_a");
        named_root.member = Some("named-root.o".to_owned());
        let mut current_root = symbol("r_sym_root", &[1, 2, 3, 4, 5, 6], "r_sym_leaf_b");
        current_root.member = Some("current-root.o".to_owned());
        let leaf = |member: &str, name: &str| artifact::ArtifactSymbolDefinition {
            member: Some(member.to_owned()),
            name: name.to_owned(),
            address: 0,
            bytes: vec![9, 8],
            addresses_resolved: false,
            memory_regions: Arc::from([]),
            relocations: Vec::new(),
        };
        let from_symbols = vec![
            named_root,
            leaf("named-a.o", "r_ble_leaf_a"),
            leaf("named-b.o", "r_ble_leaf_b"),
        ];
        let to_symbols = vec![
            current_root,
            leaf("current-a.o", "r_sym_leaf_a"),
            leaf("current-b.o", "r_sym_leaf_b"),
        ];
        let target_candidates = to_symbols[1..]
            .iter()
            .map(|symbol| function_document(symbol, &artifact_identity("current", '2')).unwrap())
            .collect::<Vec<_>>();
        let mut correspondences = vec![
            SymbolCorrespondence {
                from: function_document(&from_symbols[0], &artifact_identity("named", '1'))
                    .unwrap(),
                status: SymbolCorrespondenceStatus::Unique,
                basis: "exact-normalized-body",
                candidates: vec![
                    function_document(&to_symbols[0], &artifact_identity("current", '2')).unwrap(),
                ],
            },
            SymbolCorrespondence {
                from: function_document(&from_symbols[1], &artifact_identity("named", '1'))
                    .unwrap(),
                status: SymbolCorrespondenceStatus::Ambiguous,
                basis: "exact-normalized-body",
                candidates: target_candidates.clone(),
            },
            SymbolCorrespondence {
                from: function_document(&from_symbols[2], &artifact_identity("named", '1'))
                    .unwrap(),
                status: SymbolCorrespondenceStatus::Ambiguous,
                basis: "exact-normalized-body",
                candidates: target_candidates,
            },
        ];

        refine_ambiguous_matches(&from_symbols, &to_symbols, &mut correspondences);

        assert_eq!(
            correspondences[1].status,
            SymbolCorrespondenceStatus::Unique
        );
        assert_eq!(correspondences[1].candidates[0].symbol, "r_sym_leaf_b");
        assert_eq!(
            correspondences[1].basis,
            "exact-normalized-body-and-mapped-call-graph"
        );
        assert_eq!(
            correspondences[2].status,
            SymbolCorrespondenceStatus::Ambiguous
        );
    }

    #[test]
    fn numeric_archive_members_follow_alphabetical_named_member_ordinals() {
        let named = vec![
            "ble_lll_dtm.c.o".to_owned(),
            "ble_ll_adv.c.o".to_owned(),
            "advertise_filter.c.o".to_owned(),
        ];
        let numeric = vec!["0.o".to_owned(), "1.o".to_owned(), "2.o".to_owned()];
        let mapping = infer_member_order_from_names(&named, &numeric).unwrap();
        assert_eq!(mapping["advertise_filter.c.o"], "0.o");
        assert_eq!(mapping["ble_ll_adv.c.o"], "1.o");
        assert_eq!(mapping["ble_lll_dtm.c.o"], "2.o");
        assert_eq!(
            infer_member_order_from_names(&numeric, &named).unwrap()["2.o"],
            "ble_lll_dtm.c.o"
        );

        let incomplete = vec!["0.o".to_owned(), "2.o".to_owned(), "3.o".to_owned()];
        assert!(infer_member_order_from_names(&named, &incomplete).is_none());
    }

    #[test]
    fn mapped_function_reference_resolves_identical_static_data() {
        let object = |name: &str, offset: u64| artifact::ArtifactDataObjectDefinition {
            member: Some("radio.o".to_owned()),
            section: ".bss".to_owned(),
            name: name.to_owned(),
            aliases: Vec::new(),
            address: None,
            object_offset: offset,
            size: 4,
            writable: true,
            initialized: false,
            synthetic_from_anchor: false,
            exported: false,
            initializer: Vec::new(),
            relocations: Vec::new(),
        };
        let from_objects = vec![object("controller_state", 0), object("scheduler_state", 4)];
        let to_objects = vec![object("r_data_a", 0), object("r_data_b", 4)];
        let mut from_root = symbol("controller_run", &[1, 2, 3, 4], "controller_state");
        from_root.relocations[0].kind = artifact::RelocationKind::Hi20;
        let mut to_root = symbol("r_sym_root", &[1, 2, 3, 4], "r_data_b");
        to_root.relocations[0].kind = artifact::RelocationKind::Hi20;
        let from_identity = artifact_identity("named", '1');
        let to_identity = artifact_identity("current", '2');
        let functions = vec![SymbolCorrespondence {
            from: function_document(&from_root, &from_identity).unwrap(),
            status: SymbolCorrespondenceStatus::Unique,
            basis: "exact-normalized-body",
            candidates: vec![function_document(&to_root, &to_identity).unwrap()],
        }];

        let (summary, correspondences) = correlate_data_objects(
            &from_objects,
            &to_objects,
            std::slice::from_ref(&from_root),
            std::slice::from_ref(&to_root),
            &functions,
            &from_identity,
            &to_identity,
        )
        .unwrap();

        assert_eq!(summary.unique, 1);
        assert_eq!(summary.reference_refined, 1);
        assert_eq!(summary.ambiguous, 1);
        assert_eq!(correspondences[0].from.symbol, "controller_state");
        assert_eq!(
            correspondences[0].status,
            SymbolCorrespondenceStatus::Unique
        );
        assert_eq!(correspondences[0].candidates[0].symbol, "r_data_b");
        assert_eq!(correspondences[0].basis, "mapped-function-reference");
        assert_eq!(
            correspondences[1].status,
            SymbolCorrespondenceStatus::Ambiguous
        );
    }
}
