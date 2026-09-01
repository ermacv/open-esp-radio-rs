//! Address- and name-independent function correspondence between artifacts.
//!
//! Vendor symbol names may be regenerated independently of the function body.
//! This module therefore distinguishes reviewed/stable names, generated
//! obfuscation tokens, and normalized relocatable bodies.  Generated tokens
//! become identity anchors only after archive-wide overlap proves that both
//! artifacts belong to the same obfuscation epoch.  Duplicate compiler-
//! generated leaves and conflicting evidence remain explicit ambiguity instead
//! of receiving a guessed name.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use serde::Serialize;
use sha2::{Digest, Sha256};

use open_radio_vendor_contracts::ArtifactIdentity;

use crate::{Result, artifact, artifact_occurrence};

pub(crate) const SYMBOL_CORRESPONDENCE_SCHEMA: u32 = 8;
const MINIMUM_COMMON_OBFUSCATION_TOKENS: usize = 64;
const MINIMUM_OBFUSCATION_TOKEN_RETENTION_PARTS_PER_MILLION: u32 = 900_000;
const MINIMUM_MEMBER_ORDER_FUNCTION_SUPPORT: usize = 64;
const MINIMUM_MEMBER_ORDER_SUPPORT_PARTS_PER_MILLION: u32 = 900_000;
const MAX_CALL_GRAPH_REVIEW_CANDIDATES: usize = 16;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum SymbolCorrespondenceStatus {
    Unique,
    Ambiguous,
    Unmatched,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ObfuscationEpochStatus {
    Compatible,
    Distinct,
    Inconclusive,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct ObfuscationTokenOverlap {
    pub(crate) status: ObfuscationEpochStatus,
    pub(crate) from_tokens: usize,
    pub(crate) to_tokens: usize,
    pub(crate) common_tokens: usize,
    pub(crate) smaller_set_retention_parts_per_million: u32,
    pub(crate) automatic_matches: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct ObfuscationEpochEvidence {
    pub(crate) status: ObfuscationEpochStatus,
    pub(crate) basis: &'static str,
    pub(crate) minimum_common_tokens: usize,
    pub(crate) minimum_smaller_set_retention_parts_per_million: u32,
    pub(crate) automatic_matches: bool,
    pub(crate) functions: ObfuscationTokenOverlap,
    pub(crate) data_objects: ObfuscationTokenOverlap,
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
    pub(crate) automatic_data_matches: bool,
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
    pub(crate) name_stable: usize,
    pub(crate) token_stable: usize,
    pub(crate) reference_refined: usize,
    pub(crate) member_refined: usize,
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
    pub(crate) name_stable: usize,
    pub(crate) token_stable: usize,
    pub(crate) graph_refined: usize,
    pub(crate) review_candidates: usize,
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
    pub(crate) obfuscation_epoch: ObfuscationEpochEvidence,
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
    let from_data = artifact::load_data_objects(request.from_path)?;
    let to_data = artifact::load_data_objects(request.to_path)?;
    let obfuscation_epoch = obfuscation_epoch_evidence(
        obfuscation_token_overlap(
            function_obfuscation_tokens(&from_symbols),
            function_obfuscation_tokens(&to_symbols),
        ),
        obfuscation_token_overlap(
            data_obfuscation_tokens(&from_data),
            data_obfuscation_tokens(&to_data),
        ),
    );
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
    promote_stable_function_identity_matches(
        &from_symbols,
        &to_symbols,
        &to_identity,
        obfuscation_epoch.automatic_matches,
        &mut correspondences,
    )?;
    refine_function_matches_by_call_graph(
        &from_symbols,
        &to_symbols,
        &to_identity,
        &mut correspondences,
    )?;
    suggest_changed_function_candidates_by_mapped_callees(
        &from_symbols,
        &to_symbols,
        &to_identity,
        &mut correspondences,
    )?;
    correspondences.sort_by(|left, right| left.from.cmp(&right.from));
    let mut summary = SymbolCorrespondenceSummary::default();
    for correspondence in &correspondences {
        match correspondence.status {
            SymbolCorrespondenceStatus::Unique => {
                summary.unique += 1;
                summary.name_stable +=
                    usize::from(correspondence.basis.contains("stable-symbol-name"));
                summary.token_stable +=
                    usize::from(correspondence.basis.contains("stable-obfuscation-token"));
                summary.graph_refined += usize::from(correspondence.basis.contains("mapped-call"));
            }
            SymbolCorrespondenceStatus::Ambiguous => summary.ambiguous += 1,
            SymbolCorrespondenceStatus::Unmatched => summary.unmatched += 1,
        }
    }
    summary.review_candidates = correspondences
        .iter()
        .filter(|correspondence| correspondence.basis == "mapped-callee-multiset-review-candidates")
        .count();

    let member_order = infer_member_order(request.from_path, request.to_path)?
        .map(|mapping| member_order_evidence(mapping, &correspondences));
    let (data_summary, data_correspondences) = correlate_data_objects(
        &from_data,
        &to_data,
        &from_symbols,
        &to_symbols,
        &correspondences,
        &from_identity,
        &to_identity,
        obfuscation_epoch.automatic_matches,
        member_order.as_ref(),
    )?;
    let mut pin_candidates = function_pin_candidates(&correspondences);
    pin_candidates.extend(data_pin_candidates(&data_correspondences));
    pin_candidates.sort();

    Ok(SymbolCorrespondenceReport {
        schema_version: SYMBOL_CORRESPONDENCE_SCHEMA,
        command: "symbols correlate",
        method: "archive-epoch-gated-identities-relocatable-bodies-one-to-one-call-sites-and-bounded-review-shortlists-v7",
        from: from_artifact,
        to: to_artifact,
        obfuscation_epoch,
        member_order,
        summary,
        correspondences,
        data_summary,
        data_correspondences,
        pin_candidates,
    })
}

fn promote_stable_function_identity_matches(
    from_symbols: &[artifact::ArtifactSymbolDefinition],
    to_symbols: &[artifact::ArtifactSymbolDefinition],
    to_artifact: &ArtifactIdentity,
    allow_obfuscation_tokens: bool,
    correspondences: &mut [SymbolCorrespondence],
) -> Result<()> {
    let from_by_name = unique_stable_symbols_by_name(from_symbols);
    let to_by_name = unique_stable_symbols_by_name(to_symbols);
    let from_by_token = unique_symbols_by_obfuscation_identity(from_symbols);
    let to_by_token = unique_symbols_by_obfuscation_identity(to_symbols);
    for correspondence in correspondences {
        let mut stable_matches = Vec::<(&'static str, &artifact::ArtifactSymbolDefinition)>::new();
        if from_by_name.contains_key(correspondence.from.symbol.as_str())
            && let Some(target) = to_by_name.get(correspondence.from.symbol.as_str())
        {
            stable_matches.push(("stable-symbol-name", *target));
        }
        if allow_obfuscation_tokens
            && let Some(identity) = obfuscation_identity(&correspondence.from.symbol)
            && from_by_token.contains_key(identity.as_str())
            && let Some(target) = to_by_token.get(identity.as_str())
        {
            stable_matches.push(("stable-obfuscation-token", *target));
        }
        stable_matches.sort_by_key(|(_, target)| function_locator(target));
        stable_matches
            .dedup_by(|left, right| function_locator(left.1) == function_locator(right.1));
        let Some((basis, target)) = stable_matches.first().copied() else {
            continue;
        };
        if stable_matches
            .iter()
            .any(|(_, candidate)| function_locator(candidate) != function_locator(target))
        {
            correspondence.status = SymbolCorrespondenceStatus::Ambiguous;
            correspondence.basis = "conflicting-stable-function-identities";
            correspondence.candidates = stable_matches
                .into_iter()
                .map(|(_, candidate)| function_document(candidate, to_artifact))
                .collect::<Result<Vec<_>>>()?;
            correspondence.candidates.sort();
            correspondence.candidates.dedup();
            continue;
        }
        let target = function_document(target, to_artifact)?;
        match correspondence.status {
            SymbolCorrespondenceStatus::Unique
                if correspondence.candidates.first() == Some(&target) =>
            {
                correspondence.basis = match basis {
                    "stable-symbol-name" => "stable-symbol-name-and-exact-normalized-body",
                    "stable-obfuscation-token" => {
                        "stable-obfuscation-token-and-exact-normalized-body"
                    }
                    _ => unreachable!("closed stable function identity vocabulary"),
                };
            }
            SymbolCorrespondenceStatus::Unique => {
                correspondence.status = SymbolCorrespondenceStatus::Ambiguous;
                correspondence.basis = "conflicting-stable-function-identity-and-normalized-body";
                correspondence.candidates.push(target);
                correspondence.candidates.sort();
                correspondence.candidates.dedup();
            }
            SymbolCorrespondenceStatus::Ambiguous | SymbolCorrespondenceStatus::Unmatched => {
                correspondence.status = SymbolCorrespondenceStatus::Unique;
                correspondence.basis = basis;
                correspondence.candidates = vec![target];
            }
        }
    }
    Ok(())
}

fn unique_stable_symbols_by_name(
    symbols: &[artifact::ArtifactSymbolDefinition],
) -> BTreeMap<String, &artifact::ArtifactSymbolDefinition> {
    let stable = symbols
        .iter()
        .filter(|symbol| is_stable_source_name(&symbol.name))
        .collect::<Vec<_>>();
    unique_symbols_by_key(&stable, |symbol| symbol.name.clone())
}

fn unique_symbols_by_obfuscation_identity(
    symbols: &[artifact::ArtifactSymbolDefinition],
) -> BTreeMap<String, &artifact::ArtifactSymbolDefinition> {
    let symbols = symbols.iter().collect::<Vec<_>>();
    unique_symbols_by_key(&symbols, |symbol| {
        obfuscation_identity(&symbol.name).unwrap_or_default()
    })
    .into_iter()
    .filter(|(identity, _)| !identity.is_empty())
    .collect()
}

fn unique_symbols_by_key<'a, F>(
    symbols: &[&'a artifact::ArtifactSymbolDefinition],
    key: F,
) -> BTreeMap<String, &'a artifact::ArtifactSymbolDefinition>
where
    F: Fn(&artifact::ArtifactSymbolDefinition) -> String,
{
    let mut candidates = BTreeMap::<String, Vec<&artifact::ArtifactSymbolDefinition>>::new();
    for symbol in symbols {
        candidates.entry(key(symbol)).or_default().push(symbol);
    }
    candidates
        .into_iter()
        .filter_map(|(key, symbols)| (symbols.len() == 1).then_some((key, symbols[0])))
        .collect()
}

fn is_stable_source_name(name: &str) -> bool {
    !name.starts_with('.') && obfuscation_token(name).is_none()
}

fn obfuscation_token(name: &str) -> Option<&str> {
    let payload = name
        .strip_prefix("r_sym_")
        .or_else(|| name.strip_prefix("sym_"))?;
    payload.split('_').rev().find_map(|component| {
        let candidate = component.split('.').next().unwrap_or(component);
        (candidate.len() == 20 && candidate.bytes().all(|byte| byte.is_ascii_alphanumeric()))
            .then_some(candidate)
    })
}

fn obfuscation_identity(name: &str) -> Option<String> {
    let token = obfuscation_token(name)?;
    let token_end = name.rfind(token)? + token.len();
    Some(format!("{}{}", token, &name[token_end..]))
}

fn function_obfuscation_tokens(symbols: &[artifact::ArtifactSymbolDefinition]) -> BTreeSet<String> {
    symbols
        .iter()
        .filter_map(|symbol| obfuscation_token(&symbol.name).map(str::to_owned))
        .collect()
}

fn data_obfuscation_tokens(objects: &[artifact::ArtifactDataObjectDefinition]) -> BTreeSet<String> {
    objects
        .iter()
        .flat_map(|object| std::iter::once(&object.name).chain(&object.aliases))
        .filter_map(|name| obfuscation_token(name).map(str::to_owned))
        .collect()
}

fn obfuscation_token_overlap(
    from: BTreeSet<String>,
    to: BTreeSet<String>,
) -> ObfuscationTokenOverlap {
    let common_tokens = from.intersection(&to).count();
    let smaller = from.len().min(to.len());
    let smaller_set_retention_parts_per_million = common_tokens
        .saturating_mul(1_000_000)
        .checked_div(smaller)
        .and_then(|ratio| u32::try_from(ratio).ok())
        .unwrap_or(0);
    let compatible = common_tokens >= MINIMUM_COMMON_OBFUSCATION_TOKENS
        && smaller_set_retention_parts_per_million
            >= MINIMUM_OBFUSCATION_TOKEN_RETENTION_PARTS_PER_MILLION;
    let status = if compatible {
        ObfuscationEpochStatus::Compatible
    } else if common_tokens == 0
        && from.len() >= MINIMUM_COMMON_OBFUSCATION_TOKENS
        && to.len() >= MINIMUM_COMMON_OBFUSCATION_TOKENS
    {
        ObfuscationEpochStatus::Distinct
    } else {
        ObfuscationEpochStatus::Inconclusive
    };
    ObfuscationTokenOverlap {
        status,
        from_tokens: from.len(),
        to_tokens: to.len(),
        common_tokens,
        smaller_set_retention_parts_per_million,
        automatic_matches: compatible,
    }
}

fn obfuscation_epoch_evidence(
    functions: ObfuscationTokenOverlap,
    data_objects: ObfuscationTokenOverlap,
) -> ObfuscationEpochEvidence {
    let status = if [functions.status, data_objects.status]
        .contains(&ObfuscationEpochStatus::Compatible)
    {
        ObfuscationEpochStatus::Compatible
    } else if [functions.status, data_objects.status].contains(&ObfuscationEpochStatus::Distinct) {
        ObfuscationEpochStatus::Distinct
    } else {
        ObfuscationEpochStatus::Inconclusive
    };
    ObfuscationEpochEvidence {
        status,
        basis: "archive-wide-20-character-obfuscation-token-overlap-v2",
        minimum_common_tokens: MINIMUM_COMMON_OBFUSCATION_TOKENS,
        minimum_smaller_set_retention_parts_per_million:
            MINIMUM_OBFUSCATION_TOKEN_RETENTION_PARTS_PER_MILLION,
        automatic_matches: status == ObfuscationEpochStatus::Compatible,
        functions,
        data_objects,
    }
}

fn refine_function_matches_by_call_graph(
    from_symbols: &[artifact::ArtifactSymbolDefinition],
    to_symbols: &[artifact::ArtifactSymbolDefinition],
    to_artifact: &ArtifactIdentity,
    correspondences: &mut [SymbolCorrespondence],
) -> Result<()> {
    let from_by_name = unique_symbols_by_name(from_symbols);
    let to_by_name = unique_symbols_by_name(to_symbols);
    let mut changed = true;
    while changed {
        changed = false;
        let mappings = correspondences
            .iter()
            .filter(|correspondence| {
                correspondence.status == SymbolCorrespondenceStatus::Unique
                    && correspondence.candidates.len() == 1
                    && from_by_name.contains_key(correspondence.from.symbol.as_str())
                    && to_by_name.contains_key(correspondence.candidates[0].symbol.as_str())
            })
            .map(|correspondence| {
                (
                    correspondence.from.symbol.clone(),
                    correspondence.candidates[0].symbol.clone(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let mut edge_votes = BTreeMap::<String, BTreeSet<String>>::new();
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
                    .entry(from_call.symbol.clone())
                    .or_default()
                    .insert(to_call.symbol.clone());
            }
        }
        let edge_votes = unique_one_to_one_votes(edge_votes);

        for correspondence in correspondences.iter_mut().filter(|correspondence| {
            correspondence.status != SymbolCorrespondenceStatus::Unique
                && !correspondence.basis.starts_with("conflicting-")
        }) {
            let Some(from) = from_by_name.get(correspondence.from.symbol.as_str()) else {
                continue;
            };
            if let Some(voted_target) = edge_votes.get(correspondence.from.symbol.as_str())
                && let Some(target) = to_by_name.get(voted_target.as_str())
            {
                let target = function_document(target, to_artifact)?;
                match correspondence.status {
                    SymbolCorrespondenceStatus::Unmatched => {
                        correspondence.status = SymbolCorrespondenceStatus::Unique;
                        correspondence.basis = "unique-one-to-one-mapped-caller-call-site";
                        correspondence.candidates = vec![target];
                        changed = true;
                        continue;
                    }
                    SymbolCorrespondenceStatus::Ambiguous
                        if correspondence.candidates.contains(&target) =>
                    {
                        correspondence.status = SymbolCorrespondenceStatus::Unique;
                        correspondence.basis =
                            "exact-normalized-body-and-unique-one-to-one-mapped-caller-call-site";
                        correspondence.candidates = vec![target];
                        changed = true;
                        continue;
                    }
                    SymbolCorrespondenceStatus::Ambiguous => {
                        correspondence.basis =
                            "conflicting-normalized-body-and-one-to-one-mapped-caller-call-site";
                        correspondence.candidates.push(target);
                        correspondence.candidates.sort();
                        correspondence.candidates.dedup();
                        continue;
                    }
                    SymbolCorrespondenceStatus::Unique => {
                        unreachable!("unique correspondences are filtered above")
                    }
                }
            }
            if correspondence.status != SymbolCorrespondenceStatus::Ambiguous {
                continue;
            }
            let from_calls = call_relocations(from);
            let has_graph_evidence = from_calls
                .iter()
                .any(|call| mappings.contains_key(call.symbol.as_str()));
            if !has_graph_evidence {
                continue;
            }
            let mut viable = correspondence
                .candidates
                .iter()
                .filter(|candidate| {
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
    Ok(())
}

fn suggest_changed_function_candidates_by_mapped_callees(
    from_symbols: &[artifact::ArtifactSymbolDefinition],
    to_symbols: &[artifact::ArtifactSymbolDefinition],
    to_artifact: &ArtifactIdentity,
    correspondences: &mut [SymbolCorrespondence],
) -> Result<()> {
    let from_by_name = unique_symbols_by_name(from_symbols);
    let to_by_name = unique_symbols_by_name(to_symbols);
    let mappings = correspondences
        .iter()
        .filter(|correspondence| {
            correspondence.status == SymbolCorrespondenceStatus::Unique
                && correspondence.candidates.len() == 1
                && from_by_name.contains_key(correspondence.from.symbol.as_str())
                && to_by_name.contains_key(correspondence.candidates[0].symbol.as_str())
        })
        .map(|correspondence| {
            (
                correspondence.from.symbol.clone(),
                correspondence.candidates[0].symbol.clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let claimed_targets = correspondences
        .iter()
        .filter(|correspondence| {
            correspondence.status == SymbolCorrespondenceStatus::Unique
                && correspondence.candidates.len() == 1
        })
        .map(|correspondence| correspondence.candidates[0].locator.clone())
        .collect::<BTreeSet<_>>();
    let mapped_targets = mappings
        .values()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();

    for correspondence in correspondences.iter_mut().filter(|correspondence| {
        correspondence.status == SymbolCorrespondenceStatus::Unmatched
            && correspondence.basis == "exact-normalized-body"
    }) {
        let Some(from) = from_by_name.get(correspondence.from.symbol.as_str()) else {
            continue;
        };
        let from_calls = call_relocations(from);
        let mut expected = BTreeMap::<&str, usize>::new();
        for call in &from_calls {
            if let Some(mapped) = mappings.get(call.symbol.as_str()) {
                *expected.entry(mapped.as_str()).or_default() += 1;
            }
        }
        if expected.is_empty() {
            continue;
        }
        let candidate_definitions = to_symbols
            .iter()
            .filter(|target| {
                to_by_name
                    .get(target.name.as_str())
                    .is_some_and(|unique| std::ptr::eq(*unique, *target))
            })
            .filter(|target| !claimed_targets.contains(function_locator(target).as_str()))
            .filter(|target| {
                let target_calls = call_relocations(target);
                if target_calls.len() != from_calls.len() {
                    return false;
                }
                let target_counts = target_calls
                    .iter()
                    .filter(|call| mapped_targets.contains(call.symbol.as_str()))
                    .fold(BTreeMap::<&str, usize>::new(), |mut counts, call| {
                        *counts.entry(call.symbol.as_str()).or_default() += 1;
                        counts
                    });
                target_counts == expected
            })
            .collect::<Vec<_>>();
        if candidate_definitions.is_empty()
            || candidate_definitions.len() > MAX_CALL_GRAPH_REVIEW_CANDIDATES
        {
            continue;
        }
        let mut candidates = candidate_definitions
            .into_iter()
            .map(|target| function_document(target, to_artifact))
            .collect::<Result<Vec<_>>>()?;
        candidates.sort();
        candidates.dedup();
        correspondence.status = SymbolCorrespondenceStatus::Ambiguous;
        correspondence.basis = "mapped-callee-multiset-review-candidates";
        correspondence.candidates = candidates;
    }
    Ok(())
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
    allow_obfuscation_tokens: bool,
    member_order: Option<&SymbolMemberOrderEvidence>,
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
    promote_stable_data_identity_matches(
        from_objects,
        to_objects,
        to_artifact,
        allow_obfuscation_tokens,
        &mut correspondences,
    )?;
    if let Some(member_order) = member_order {
        refine_data_matches_by_member_order(member_order, &mut correspondences);
    }
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
        if correspondence.basis.starts_with("conflicting-") {
            continue;
        }
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
                correspondence.basis = match correspondence.basis {
                    "stable-symbol-name" => "stable-symbol-name-and-mapped-function-reference",
                    "stable-symbol-name-and-exact-normalized-data-object" => {
                        "stable-symbol-name-and-exact-normalized-data-object-and-mapped-function-reference"
                    }
                    "stable-obfuscation-token" => {
                        "stable-obfuscation-token-and-mapped-function-reference"
                    }
                    "stable-obfuscation-token-and-exact-normalized-data-object" => {
                        "stable-obfuscation-token-and-exact-normalized-data-object-and-mapped-function-reference"
                    }
                    _ => "exact-normalized-data-object-and-mapped-function-reference",
                };
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
                summary.name_stable +=
                    usize::from(correspondence.basis.contains("stable-symbol-name"));
                summary.token_stable +=
                    usize::from(correspondence.basis.contains("stable-obfuscation-token"));
                summary.reference_refined +=
                    usize::from(correspondence.basis.contains("mapped-function-reference"));
                summary.member_refined +=
                    usize::from(correspondence.basis.contains("proven-member-order"));
            }
            SymbolCorrespondenceStatus::Ambiguous => summary.ambiguous += 1,
            SymbolCorrespondenceStatus::Unmatched => summary.unmatched += 1,
        }
    }
    Ok((summary, correspondences))
}

fn refine_data_matches_by_member_order(
    evidence: &SymbolMemberOrderEvidence,
    correspondences: &mut [DataObjectCorrespondence],
) {
    if !evidence.automatic_data_matches {
        return;
    }
    let mapped_members = evidence
        .correspondences
        .iter()
        .map(|correspondence| (correspondence.from.as_str(), correspondence.to.as_str()))
        .collect::<BTreeMap<_, _>>();
    for correspondence in correspondences.iter_mut().filter(|correspondence| {
        correspondence.status == SymbolCorrespondenceStatus::Ambiguous
            && correspondence.basis == "exact-normalized-data-object"
    }) {
        let Some(from_member) = correspondence.from.member.as_deref() else {
            continue;
        };
        let Some(to_member) = mapped_members.get(from_member) else {
            continue;
        };
        let mut candidates = correspondence
            .candidates
            .iter()
            .filter(|candidate| candidate.member.as_deref() == Some(*to_member))
            .cloned()
            .collect::<Vec<_>>();
        candidates.sort();
        candidates.dedup();
        if candidates.len() == 1 {
            correspondence.status = SymbolCorrespondenceStatus::Unique;
            correspondence.basis = "exact-normalized-data-object-and-proven-member-order";
            correspondence.candidates = candidates;
        }
    }
}

fn promote_stable_data_identity_matches(
    from_objects: &[artifact::ArtifactDataObjectDefinition],
    to_objects: &[artifact::ArtifactDataObjectDefinition],
    to_artifact: &ArtifactIdentity,
    allow_obfuscation_tokens: bool,
    correspondences: &mut [DataObjectCorrespondence],
) -> Result<()> {
    let from_by_locator = from_objects
        .iter()
        .map(|object| (data_object_locator(object), object))
        .collect::<BTreeMap<_, _>>();
    let from_by_name = unique_data_objects_by_identity(from_objects, stable_data_names);
    let to_by_name = unique_data_objects_by_identity(to_objects, stable_data_names);
    let from_by_token = unique_data_objects_by_identity(from_objects, data_obfuscation_identities);
    let to_by_token = unique_data_objects_by_identity(to_objects, data_obfuscation_identities);

    for correspondence in correspondences {
        let Some(source) = from_by_locator.get(&correspondence.from.locator) else {
            continue;
        };
        let mut stable_matches =
            Vec::<(&'static str, &artifact::ArtifactDataObjectDefinition)>::new();
        for name in stable_data_names(source) {
            if from_by_name
                .get(&name)
                .is_some_and(|object| data_object_locator(object) == correspondence.from.locator)
                && let Some(target) = to_by_name.get(&name)
            {
                stable_matches.push(("stable-symbol-name", *target));
            }
        }
        if allow_obfuscation_tokens {
            for token in data_obfuscation_identities(source) {
                if from_by_token.get(&token).is_some_and(|object| {
                    data_object_locator(object) == correspondence.from.locator
                }) && let Some(target) = to_by_token.get(&token)
                {
                    stable_matches.push(("stable-obfuscation-token", *target));
                }
            }
        }
        stable_matches.sort_by_key(|(_, target)| data_object_locator(target));
        stable_matches
            .dedup_by(|left, right| data_object_locator(left.1) == data_object_locator(right.1));
        let Some((basis, target)) = stable_matches.first().copied() else {
            continue;
        };
        if stable_matches
            .iter()
            .any(|(_, candidate)| data_object_locator(candidate) != data_object_locator(target))
        {
            correspondence.status = SymbolCorrespondenceStatus::Ambiguous;
            correspondence.basis = "conflicting-stable-data-identities";
            correspondence.candidates = stable_matches
                .into_iter()
                .map(|(_, candidate)| data_object_document(candidate, to_artifact))
                .collect::<Result<Vec<_>>>()?;
            correspondence.candidates.sort();
            correspondence.candidates.dedup();
            continue;
        }
        let target = data_object_document(target, to_artifact)?;
        match correspondence.status {
            SymbolCorrespondenceStatus::Unique
                if correspondence.candidates.first() == Some(&target) =>
            {
                correspondence.basis = match basis {
                    "stable-symbol-name" => "stable-symbol-name-and-exact-normalized-data-object",
                    "stable-obfuscation-token" => {
                        "stable-obfuscation-token-and-exact-normalized-data-object"
                    }
                    _ => unreachable!("closed stable data identity vocabulary"),
                };
            }
            SymbolCorrespondenceStatus::Unique => {
                correspondence.status = SymbolCorrespondenceStatus::Ambiguous;
                correspondence.basis =
                    "conflicting-stable-data-identity-and-normalized-data-object";
                correspondence.candidates.push(target);
                correspondence.candidates.sort();
                correspondence.candidates.dedup();
            }
            SymbolCorrespondenceStatus::Ambiguous | SymbolCorrespondenceStatus::Unmatched => {
                correspondence.status = SymbolCorrespondenceStatus::Unique;
                correspondence.basis = basis;
                correspondence.candidates = vec![target];
            }
        }
    }
    Ok(())
}

fn unique_data_objects_by_identity<F>(
    objects: &[artifact::ArtifactDataObjectDefinition],
    identities: F,
) -> BTreeMap<String, &artifact::ArtifactDataObjectDefinition>
where
    F: Fn(&artifact::ArtifactDataObjectDefinition) -> Vec<String>,
{
    let mut candidates = BTreeMap::<String, Vec<&artifact::ArtifactDataObjectDefinition>>::new();
    for object in objects {
        for identity in identities(object) {
            let entry = candidates.entry(identity).or_default();
            if !entry
                .iter()
                .any(|candidate| std::ptr::eq(*candidate, object))
            {
                entry.push(object);
            }
        }
    }
    candidates
        .into_iter()
        .filter_map(|(identity, objects)| (objects.len() == 1).then_some((identity, objects[0])))
        .collect()
}

fn stable_data_names(object: &artifact::ArtifactDataObjectDefinition) -> Vec<String> {
    object
        .aliases
        .iter()
        .chain(std::iter::once(&object.name))
        .filter(|name| is_stable_source_name(name))
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn data_obfuscation_identities(object: &artifact::ArtifactDataObjectDefinition) -> Vec<String> {
    object
        .aliases
        .iter()
        .chain(std::iter::once(&object.name))
        .filter_map(|name| obfuscation_token(name).map(str::to_owned))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
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
    let occurrence = artifact_occurrence::memory_object_occurrence(
        artifact,
        object.member.as_deref(),
        &object.section,
        &object.name,
        object.object_offset,
        object.address,
        object.size,
    )?;
    Ok(DataObjectCorrespondenceObject {
        member: object.member.clone(),
        section: object.section.clone(),
        symbol: object.name.clone(),
        aliases: object.aliases.clone(),
        object_offset: object.object_offset,
        size: object.size,
        writable: object.writable,
        initialized: object.initialized,
        locator: occurrence.locator,
        occurrence: occurrence.id.to_string(),
        fingerprint: normalized_data_fingerprint(object),
    })
}

fn data_object_locator(object: &artifact::ArtifactDataObjectDefinition) -> String {
    artifact_occurrence::memory_object_locator(
        object.member.as_deref(),
        &object.section,
        &object.name,
        object.object_offset,
        object.address,
        object.size,
    )
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
                && is_reviewable_source_name(&correspondence.from.symbol)
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
                && is_reviewable_source_name(&correspondence.from.symbol)
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

pub(crate) fn is_reviewable_source_name(name: &str) -> bool {
    is_stable_source_name(name)
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
            && correspondence.from.fingerprint == correspondence.candidates[0].fingerprint
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
    let automatic_data_matches = exact_function_support >= MINIMUM_MEMBER_ORDER_FUNCTION_SUPPORT
        && exact_function_conflicts == 0
        && support_parts_per_million >= MINIMUM_MEMBER_ORDER_SUPPORT_PARTS_PER_MILLION;
    SymbolMemberOrderEvidence {
        basis: "archive-wide-exact-function-validated-alphabetical-member-ordinal-v2",
        // Member order is valuable module provenance and candidate ranking,
        // but code can move between modules across revisions. It never turns
        // an otherwise ambiguous function body into an automatic pin.
        automatic_function_matches: false,
        automatic_data_matches,
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
    let occurrence = artifact_occurrence::function_occurrence(
        artifact,
        symbol.member.as_deref(),
        &symbol.name,
        symbol.address,
    )?;
    Ok(SymbolCorrespondenceFunction {
        member: symbol.member.clone(),
        symbol: symbol.name.clone(),
        locator: occurrence.locator,
        occurrence: occurrence.id.to_string(),
        size: symbol.bytes.len(),
        fingerprint: normalized_body_fingerprint(symbol),
    })
}

fn function_locator(symbol: &artifact::ArtifactSymbolDefinition) -> String {
    artifact_occurrence::function_locator(symbol.member.as_deref(), &symbol.name, symbol.address)
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

        refine_function_matches_by_call_graph(
            &from_symbols,
            &to_symbols,
            &artifact_identity("current", '2'),
            &mut correspondences,
        )
        .unwrap();

        assert_eq!(
            correspondences[1].status,
            SymbolCorrespondenceStatus::Unique
        );
        assert_eq!(correspondences[1].candidates[0].symbol, "r_sym_leaf_b");
        assert_eq!(
            correspondences[1].basis,
            "exact-normalized-body-and-unique-one-to-one-mapped-caller-call-site"
        );
        assert_eq!(
            correspondences[2].status,
            SymbolCorrespondenceStatus::Ambiguous
        );
    }

    #[test]
    fn a_unique_mapped_caller_site_identifies_a_changed_callee() {
        let leaf = |member: &str, name: &str, bytes: &[u8]| artifact::ArtifactSymbolDefinition {
            member: Some(member.to_owned()),
            name: name.to_owned(),
            address: 0,
            bytes: bytes.to_vec(),
            addresses_resolved: false,
            memory_regions: Arc::from([]),
            relocations: Vec::new(),
        };
        let from_symbols = vec![
            symbol("named_root", &[1, 2, 3, 4], "named_changed_leaf"),
            leaf("named-leaf.o", "named_changed_leaf", &[5, 6, 7, 8]),
        ];
        let to_symbols = vec![
            symbol("current_root", &[1, 2, 3, 4], "current_changed_leaf"),
            leaf("current-leaf.o", "current_changed_leaf", &[9, 10, 11, 12]),
        ];
        let from_identity = artifact_identity("named", '1');
        let to_identity = artifact_identity("current", '2');
        let mut correspondences = vec![
            SymbolCorrespondence {
                from: function_document(&from_symbols[0], &from_identity).unwrap(),
                status: SymbolCorrespondenceStatus::Unique,
                basis: "exact-normalized-body",
                candidates: vec![function_document(&to_symbols[0], &to_identity).unwrap()],
            },
            SymbolCorrespondence {
                from: function_document(&from_symbols[1], &from_identity).unwrap(),
                status: SymbolCorrespondenceStatus::Unmatched,
                basis: "exact-normalized-body",
                candidates: Vec::new(),
            },
        ];

        refine_function_matches_by_call_graph(
            &from_symbols,
            &to_symbols,
            &to_identity,
            &mut correspondences,
        )
        .unwrap();

        assert_eq!(
            correspondences[1].status,
            SymbolCorrespondenceStatus::Unique
        );
        assert_eq!(
            correspondences[1].candidates[0].symbol,
            "current_changed_leaf"
        );
        assert_eq!(
            correspondences[1].basis,
            "unique-one-to-one-mapped-caller-call-site"
        );
    }

    #[test]
    fn mapped_caller_sites_never_merge_two_source_functions() {
        let leaf = |name: &str| artifact::ArtifactSymbolDefinition {
            member: Some("leaves.o".to_owned()),
            name: name.to_owned(),
            address: 0,
            bytes: vec![9, 8],
            addresses_resolved: false,
            memory_regions: Arc::from([]),
            relocations: Vec::new(),
        };
        let from_symbols = vec![
            symbol("named_root_a", &[1, 2, 3, 4], "named_leaf_a"),
            symbol("named_root_b", &[5, 6, 7, 8], "named_leaf_b"),
            leaf("named_leaf_a"),
            leaf("named_leaf_b"),
        ];
        let to_symbols = vec![
            symbol("current_root_a", &[1, 2, 3, 4], "current_leaf"),
            symbol("current_root_b", &[5, 6, 7, 8], "current_leaf"),
            leaf("current_leaf"),
        ];
        let from_identity = artifact_identity("named", '1');
        let to_identity = artifact_identity("current", '2');
        let mut correspondences = vec![
            SymbolCorrespondence {
                from: function_document(&from_symbols[0], &from_identity).unwrap(),
                status: SymbolCorrespondenceStatus::Unique,
                basis: "exact-normalized-body",
                candidates: vec![function_document(&to_symbols[0], &to_identity).unwrap()],
            },
            SymbolCorrespondence {
                from: function_document(&from_symbols[1], &from_identity).unwrap(),
                status: SymbolCorrespondenceStatus::Unique,
                basis: "exact-normalized-body",
                candidates: vec![function_document(&to_symbols[1], &to_identity).unwrap()],
            },
            SymbolCorrespondence {
                from: function_document(&from_symbols[2], &from_identity).unwrap(),
                status: SymbolCorrespondenceStatus::Unmatched,
                basis: "exact-normalized-body",
                candidates: Vec::new(),
            },
            SymbolCorrespondence {
                from: function_document(&from_symbols[3], &from_identity).unwrap(),
                status: SymbolCorrespondenceStatus::Unmatched,
                basis: "exact-normalized-body",
                candidates: Vec::new(),
            },
        ];

        refine_function_matches_by_call_graph(
            &from_symbols,
            &to_symbols,
            &to_identity,
            &mut correspondences,
        )
        .unwrap();

        assert!(
            correspondences[2..].iter().all(
                |correspondence| correspondence.status == SymbolCorrespondenceStatus::Unmatched
            )
        );
    }

    #[test]
    fn mapped_callees_produce_review_only_candidates_for_changed_callers() {
        let leaf = |name: &str| artifact::ArtifactSymbolDefinition {
            member: Some("leaf.o".to_owned()),
            name: name.to_owned(),
            address: 0,
            bytes: vec![9, 8],
            addresses_resolved: false,
            memory_regions: Arc::from([]),
            relocations: Vec::new(),
        };
        let from_symbols = vec![
            leaf("named_leaf"),
            symbol("changed_caller", &[1, 2, 3, 4], "named_leaf"),
        ];
        let to_symbols = vec![
            leaf("current_leaf"),
            symbol("review_candidate", &[5, 6, 7, 8], "current_leaf"),
            symbol("unrelated", &[10, 11, 12, 13], "other_leaf"),
        ];
        let from_identity = artifact_identity("named", '1');
        let to_identity = artifact_identity("current", '2');
        let mut correspondences = vec![
            SymbolCorrespondence {
                from: function_document(&from_symbols[0], &from_identity).unwrap(),
                status: SymbolCorrespondenceStatus::Unique,
                basis: "exact-normalized-body",
                candidates: vec![function_document(&to_symbols[0], &to_identity).unwrap()],
            },
            SymbolCorrespondence {
                from: function_document(&from_symbols[1], &from_identity).unwrap(),
                status: SymbolCorrespondenceStatus::Unmatched,
                basis: "exact-normalized-body",
                candidates: Vec::new(),
            },
        ];

        suggest_changed_function_candidates_by_mapped_callees(
            &from_symbols,
            &to_symbols,
            &to_identity,
            &mut correspondences,
        )
        .unwrap();

        assert_eq!(
            correspondences[1].status,
            SymbolCorrespondenceStatus::Ambiguous
        );
        assert_eq!(correspondences[1].candidates.len(), 1);
        assert_eq!(correspondences[1].candidates[0].symbol, "review_candidate");
        assert_eq!(
            correspondences[1].basis,
            "mapped-callee-multiset-review-candidates"
        );
    }

    #[test]
    fn review_candidates_reject_additional_mapped_callees() {
        let leaf = |name: &str| artifact::ArtifactSymbolDefinition {
            member: Some("leaf.o".to_owned()),
            name: name.to_owned(),
            address: 0,
            bytes: vec![9, 8],
            addresses_resolved: false,
            memory_regions: Arc::from([]),
            relocations: Vec::new(),
        };
        let mut changed_caller = symbol("changed_caller", &[1, 2, 3, 4], "named_leaf");
        changed_caller.relocations.push(artifact::SymbolRelocation {
            address: 8,
            kind: artifact::RelocationKind::Call,
            symbol: "unmapped_leaf".to_owned(),
            addend: 0,
        });
        let mut invalid_candidate = symbol("invalid_candidate", &[5, 6, 7, 8], "current_leaf");
        invalid_candidate
            .relocations
            .push(artifact::SymbolRelocation {
                address: 8,
                kind: artifact::RelocationKind::Call,
                symbol: "current_other_leaf".to_owned(),
                addend: 0,
            });
        let from_symbols = vec![leaf("named_leaf"), leaf("named_other_leaf"), changed_caller];
        let to_symbols = vec![
            leaf("current_leaf"),
            leaf("current_other_leaf"),
            invalid_candidate,
        ];
        let from_identity = artifact_identity("named", '1');
        let to_identity = artifact_identity("current", '2');
        let mut correspondences = vec![
            SymbolCorrespondence {
                from: function_document(&from_symbols[0], &from_identity).unwrap(),
                status: SymbolCorrespondenceStatus::Unique,
                basis: "exact-normalized-body",
                candidates: vec![function_document(&to_symbols[0], &to_identity).unwrap()],
            },
            SymbolCorrespondence {
                from: function_document(&from_symbols[1], &from_identity).unwrap(),
                status: SymbolCorrespondenceStatus::Unique,
                basis: "exact-normalized-body",
                candidates: vec![function_document(&to_symbols[1], &to_identity).unwrap()],
            },
            SymbolCorrespondence {
                from: function_document(&from_symbols[2], &from_identity).unwrap(),
                status: SymbolCorrespondenceStatus::Unmatched,
                basis: "exact-normalized-body",
                candidates: Vec::new(),
            },
        ];

        suggest_changed_function_candidates_by_mapped_callees(
            &from_symbols,
            &to_symbols,
            &to_identity,
            &mut correspondences,
        )
        .unwrap();

        assert_eq!(
            correspondences[2].status,
            SymbolCorrespondenceStatus::Unmatched
        );
        assert!(correspondences[2].candidates.is_empty());
    }

    #[test]
    fn review_candidate_lists_fail_closed_above_the_bound() {
        let leaf = artifact::ArtifactSymbolDefinition {
            member: Some("leaf.o".to_owned()),
            name: "named_leaf".to_owned(),
            address: 0,
            bytes: vec![9, 8],
            addresses_resolved: false,
            memory_regions: Arc::from([]),
            relocations: Vec::new(),
        };
        let from_symbols = vec![
            leaf.clone(),
            symbol("changed_caller", &[1, 2, 3, 4], "named_leaf"),
        ];
        let mut to_symbols = vec![artifact::ArtifactSymbolDefinition {
            name: "current_leaf".to_owned(),
            ..leaf
        }];
        to_symbols.extend((0..=MAX_CALL_GRAPH_REVIEW_CANDIDATES).map(|index| {
            symbol(
                format!("candidate_{index}").as_str(),
                &[5, 6, 7, 8],
                "current_leaf",
            )
        }));
        let from_identity = artifact_identity("named", '1');
        let to_identity = artifact_identity("current", '2');
        let mut correspondences = vec![
            SymbolCorrespondence {
                from: function_document(&from_symbols[0], &from_identity).unwrap(),
                status: SymbolCorrespondenceStatus::Unique,
                basis: "exact-normalized-body",
                candidates: vec![function_document(&to_symbols[0], &to_identity).unwrap()],
            },
            SymbolCorrespondence {
                from: function_document(&from_symbols[1], &from_identity).unwrap(),
                status: SymbolCorrespondenceStatus::Unmatched,
                basis: "exact-normalized-body",
                candidates: Vec::new(),
            },
        ];

        suggest_changed_function_candidates_by_mapped_callees(
            &from_symbols,
            &to_symbols,
            &to_identity,
            &mut correspondences,
        )
        .unwrap();

        assert_eq!(
            correspondences[1].status,
            SymbolCorrespondenceStatus::Unmatched
        );
        assert!(correspondences[1].candidates.is_empty());
    }

    #[test]
    fn stable_symbol_name_maps_a_changed_function_body() {
        let from = vec![symbol(
            "r_ble_controller_init",
            &[1, 2, 3, 4, 5, 6],
            "old_call",
        )];
        let to = vec![symbol(
            "r_ble_controller_init",
            &[7, 8, 9, 10, 11, 12],
            "new_call",
        )];
        let mut correspondences = vec![SymbolCorrespondence {
            from: function_document(&from[0], &artifact_identity("named", '1')).unwrap(),
            status: SymbolCorrespondenceStatus::Unmatched,
            basis: "exact-normalized-body",
            candidates: Vec::new(),
        }];

        promote_stable_function_identity_matches(
            &from,
            &to,
            &artifact_identity("current", '2'),
            false,
            &mut correspondences,
        )
        .unwrap();

        assert_eq!(
            correspondences[0].status,
            SymbolCorrespondenceStatus::Unique
        );
        assert_eq!(correspondences[0].basis, "stable-symbol-name");
        assert_eq!(
            correspondences[0].candidates[0].symbol,
            "r_ble_controller_init"
        );
    }

    #[test]
    fn stable_name_and_unique_body_disagreement_remains_ambiguous() {
        let from = vec![symbol("controller_run", &[1, 2, 3, 4, 5, 6], "old_call")];
        let to = vec![
            symbol("controller_run", &[7, 8, 9, 10, 11, 12], "new_call"),
            symbol("moved_body", &[1, 2, 3, 4, 5, 6], "old_call"),
        ];
        let mut correspondences = vec![SymbolCorrespondence {
            from: function_document(&from[0], &artifact_identity("named", '1')).unwrap(),
            status: SymbolCorrespondenceStatus::Unique,
            basis: "exact-normalized-body",
            candidates: vec![
                function_document(&to[1], &artifact_identity("current", '2')).unwrap(),
            ],
        }];

        promote_stable_function_identity_matches(
            &from,
            &to,
            &artifact_identity("current", '2'),
            false,
            &mut correspondences,
        )
        .unwrap();

        assert_eq!(
            correspondences[0].status,
            SymbolCorrespondenceStatus::Ambiguous
        );
        assert_eq!(
            correspondences[0].basis,
            "conflicting-stable-function-identity-and-normalized-body"
        );
        assert_eq!(correspondences[0].candidates.len(), 2);
    }

    #[test]
    fn obfuscation_tokens_are_revision_identity_only_inside_a_proven_epoch() {
        let from = vec![symbol(
            "r_sym_ble_ABCDEFGHIJKLMNOPQRST",
            &[1, 2, 3, 4, 5, 6],
            "old_call",
        )];
        let to = vec![symbol(
            "sym_scheduler_ABCDEFGHIJKLMNOPQRST",
            &[7, 8, 9, 10, 11, 12],
            "new_call",
        )];
        let unmatched = || SymbolCorrespondence {
            from: function_document(&from[0], &artifact_identity("old", '1')).unwrap(),
            status: SymbolCorrespondenceStatus::Unmatched,
            basis: "exact-normalized-body",
            candidates: Vec::new(),
        };

        let mut gated = vec![unmatched()];
        promote_stable_function_identity_matches(
            &from,
            &to,
            &artifact_identity("new", '2'),
            false,
            &mut gated,
        )
        .unwrap();
        assert_eq!(gated[0].status, SymbolCorrespondenceStatus::Unmatched);

        let mut proven = vec![unmatched()];
        promote_stable_function_identity_matches(
            &from,
            &to,
            &artifact_identity("new", '2'),
            true,
            &mut proven,
        )
        .unwrap();
        assert_eq!(proven[0].status, SymbolCorrespondenceStatus::Unique);
        assert_eq!(proven[0].basis, "stable-obfuscation-token");
        assert_eq!(
            proven[0].candidates[0].symbol,
            "sym_scheduler_ABCDEFGHIJKLMNOPQRST"
        );
    }

    #[test]
    fn compiler_derived_suffix_is_part_of_obfuscated_function_identity() {
        assert_eq!(
            obfuscation_token("sym_ll_ABCDEFGHIJKLMNOPQRST.part.0"),
            Some("ABCDEFGHIJKLMNOPQRST")
        );
        assert_eq!(
            obfuscation_identity("sym_ll_ABCDEFGHIJKLMNOPQRST.part.0").as_deref(),
            Some("ABCDEFGHIJKLMNOPQRST.part.0")
        );
        assert_eq!(
            obfuscation_identity(
                "sym_ll_ABCDEFGHIJKLMNOPQRST__sublinear__F_base_linear_broker_flash"
            )
            .as_deref(),
            Some("ABCDEFGHIJKLMNOPQRST__sublinear__F_base_linear_broker_flash")
        );
        assert_eq!(obfuscation_token("controller_run"), None);
    }

    #[test]
    fn archive_wide_overlap_proves_or_rejects_an_obfuscation_epoch() {
        let from = (0..70)
            .map(|index| format!("{index:020}"))
            .collect::<BTreeSet<_>>();
        let compatible_to = (0..65)
            .map(|index| format!("{index:020}"))
            .chain((100..105).map(|index| format!("{index:020}")))
            .collect::<BTreeSet<_>>();
        let compatible = obfuscation_token_overlap(from.clone(), compatible_to);
        assert_eq!(compatible.status, ObfuscationEpochStatus::Compatible);
        assert_eq!(compatible.common_tokens, 65);
        assert!(compatible.automatic_matches);

        let distinct_to = (100..170)
            .map(|index| format!("{index:020}"))
            .collect::<BTreeSet<_>>();
        let distinct = obfuscation_token_overlap(from, distinct_to);
        assert_eq!(distinct.status, ObfuscationEpochStatus::Distinct);
        assert_eq!(distinct.common_tokens, 0);
        assert!(!distinct.automatic_matches);

        let sparse_data_from = (0..70)
            .map(|index| format!("{index:020}"))
            .collect::<BTreeSet<_>>();
        let sparse_data_to = (0..62)
            .map(|index| format!("{index:020}"))
            .chain((100..108).map(|index| format!("{index:020}")))
            .collect::<BTreeSet<_>>();
        let sparse_data = obfuscation_token_overlap(sparse_data_from, sparse_data_to);
        assert_eq!(sparse_data.status, ObfuscationEpochStatus::Inconclusive);
        let archive = obfuscation_epoch_evidence(compatible, sparse_data);
        assert_eq!(archive.status, ObfuscationEpochStatus::Compatible);
        assert!(archive.automatic_matches);
    }

    #[test]
    fn proven_obfuscation_epoch_maps_a_changed_static_object() {
        let object = |name: &str, initializer: &[u8]| artifact::ArtifactDataObjectDefinition {
            member: Some("radio.o".to_owned()),
            section: ".data".to_owned(),
            name: name.to_owned(),
            aliases: Vec::new(),
            address: None,
            object_offset: 0,
            size: 4,
            writable: true,
            initialized: true,
            synthetic_from_anchor: false,
            exported: false,
            initializer: initializer.to_vec(),
            relocations: Vec::new(),
        };
        let from = vec![object("r_sym_ble_ABCDEFGHIJKLMNOPQRST", &[1, 2, 3, 4])];
        let to = vec![object("sym_scheduler_ABCDEFGHIJKLMNOPQRST", &[5, 6, 7, 8])];
        let from_identity = artifact_identity("old", '1');
        let to_identity = artifact_identity("new", '2');
        let mut correspondences = vec![DataObjectCorrespondence {
            from: data_object_document(&from[0], &from_identity).unwrap(),
            status: SymbolCorrespondenceStatus::Unmatched,
            basis: "exact-normalized-data-object",
            candidates: Vec::new(),
        }];

        promote_stable_data_identity_matches(&from, &to, &to_identity, true, &mut correspondences)
            .unwrap();

        assert_eq!(
            correspondences[0].status,
            SymbolCorrespondenceStatus::Unique
        );
        assert_eq!(correspondences[0].basis, "stable-obfuscation-token");
        assert_eq!(
            correspondences[0].candidates[0].symbol,
            "sym_scheduler_ABCDEFGHIJKLMNOPQRST"
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
            false,
            None,
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

    #[test]
    fn proven_member_order_resolves_identical_static_data_inside_one_module() {
        let object = |member: &str, name: &str| artifact::ArtifactDataObjectDefinition {
            member: Some(member.to_owned()),
            section: format!(".bss.{name}"),
            name: name.to_owned(),
            aliases: Vec::new(),
            address: None,
            object_offset: 0,
            size: 4,
            writable: true,
            initialized: false,
            synthetic_from_anchor: false,
            exported: false,
            initializer: Vec::new(),
            relocations: Vec::new(),
        };
        let source = object("stub_hci.c.o", "r_stub_hci_funcs_ptr");
        let target_a = object("0.o", "r_sym_bt_AAAAAAAAAAAAAAAAAAAA");
        let target_b = object("1.o", "r_sym_bt_BBBBBBBBBBBBBBBBBBBB");
        let from_identity = artifact_identity("named", '1');
        let to_identity = artifact_identity("current", '2');
        let mut correspondences = vec![DataObjectCorrespondence {
            from: data_object_document(&source, &from_identity).unwrap(),
            status: SymbolCorrespondenceStatus::Ambiguous,
            basis: "exact-normalized-data-object",
            candidates: [&target_a, &target_b]
                .map(|target| data_object_document(target, &to_identity).unwrap())
                .to_vec(),
        }];
        let evidence = SymbolMemberOrderEvidence {
            basis: "archive-wide-exact-function-validated-alphabetical-member-ordinal-v2",
            automatic_function_matches: false,
            automatic_data_matches: true,
            exact_function_support: 64,
            exact_function_conflicts: 0,
            support_parts_per_million: 1_000_000,
            correspondences: vec![SymbolMemberCorrespondence {
                from: "stub_hci.c.o".to_owned(),
                to: "1.o".to_owned(),
                exact_function_support: 0,
                exact_function_conflicts: 0,
            }],
        };

        refine_data_matches_by_member_order(&evidence, &mut correspondences);

        assert_eq!(
            correspondences[0].status,
            SymbolCorrespondenceStatus::Unique
        );
        assert_eq!(
            correspondences[0].candidates[0].member.as_deref(),
            Some("1.o")
        );
        assert_eq!(
            correspondences[0].basis,
            "exact-normalized-data-object-and-proven-member-order"
        );
    }

    #[test]
    fn changed_function_bodies_do_not_prove_archive_member_order() {
        let from = symbol("controller_run", &[1, 2, 3, 4], "old_call");
        let mut to = symbol("controller_run", &[5, 6, 7, 8], "new_call");
        to.member = Some("0.o".to_owned());
        let correspondence = SymbolCorrespondence {
            from: function_document(&from, &artifact_identity("named", '1')).unwrap(),
            status: SymbolCorrespondenceStatus::Unique,
            basis: "stable-symbol-name",
            candidates: vec![function_document(&to, &artifact_identity("current", '2')).unwrap()],
        };
        let evidence = member_order_evidence(
            BTreeMap::from([("radio.o".to_owned(), "0.o".to_owned())]),
            &[correspondence],
        );

        assert_eq!(evidence.exact_function_support, 0);
        assert!(!evidence.automatic_data_matches);
    }

    #[test]
    fn member_order_data_refinement_requires_archive_wide_conflict_free_support() {
        let correspondence = |index: usize, target_member: &str| {
            let fingerprint = format!("sha256:{index:064x}");
            let function = |member: &str, artifact: char| SymbolCorrespondenceFunction {
                member: Some(member.to_owned()),
                symbol: format!("function_{index}"),
                locator: format!("member:{member}/function:{index}"),
                occurrence: format!(
                    "occurrence:function:sha256:{}",
                    artifact.to_string().repeat(64)
                ),
                size: 4,
                fingerprint: fingerprint.clone(),
            };
            SymbolCorrespondence {
                from: function("radio.c.o", '1'),
                status: SymbolCorrespondenceStatus::Unique,
                basis: "exact-normalized-body",
                candidates: vec![function(target_member, '2')],
            }
        };
        let mapping = BTreeMap::from([("radio.c.o".to_owned(), "0.o".to_owned())]);
        let mut correspondences = (0..MINIMUM_MEMBER_ORDER_FUNCTION_SUPPORT)
            .map(|index| correspondence(index, "0.o"))
            .collect::<Vec<_>>();

        let proven = member_order_evidence(mapping.clone(), &correspondences);
        assert!(proven.automatic_data_matches);
        assert_eq!(
            proven.exact_function_support,
            MINIMUM_MEMBER_ORDER_FUNCTION_SUPPORT
        );

        correspondences.push(correspondence(MINIMUM_MEMBER_ORDER_FUNCTION_SUPPORT, "1.o"));
        let conflicted = member_order_evidence(mapping, &correspondences);
        assert_eq!(conflicted.exact_function_conflicts, 1);
        assert!(!conflicted.automatic_data_matches);
    }

    #[test]
    fn data_pin_candidates_exclude_compiler_and_obfuscated_names() {
        let object = |symbol: &str, occurrence: &str| DataObjectCorrespondenceObject {
            member: Some("radio.o".to_owned()),
            section: ".bss".to_owned(),
            symbol: symbol.to_owned(),
            aliases: Vec::new(),
            object_offset: 0,
            size: 4,
            writable: true,
            initialized: false,
            locator: format!("archive-member:radio.o/section:.bss/symbol:{symbol}"),
            occurrence: occurrence.to_owned(),
            fingerprint: "sha256:body".to_owned(),
        };
        let correspondence = |symbol: &str| DataObjectCorrespondence {
            from: object(symbol, "occurrence:memory-object:sha256:source"),
            status: SymbolCorrespondenceStatus::Unique,
            basis: "mapped-function-reference",
            candidates: vec![object(
                "sym_obfuscated",
                "occurrence:memory-object:sha256:target",
            )],
        };

        let candidates = data_pin_candidates(&[
            correspondence("ble_ll_env_p"),
            correspondence(".LANCHOR0"),
            correspondence(".LC0"),
            correspondence("r_sym_ble_ABCDEFGHIJKLMNOPQRST"),
            correspondence("sym_scheduler_ABCDEFGHIJKLMNOPQRST"),
        ]);

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].suggested_name, "ble_ll_env_p");
    }
}
