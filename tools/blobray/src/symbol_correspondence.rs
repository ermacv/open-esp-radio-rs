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

use crate::{Result, artifact};

pub(crate) const SYMBOL_CORRESPONDENCE_SCHEMA: u32 = 1;

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
    pub(crate) size: usize,
    pub(crate) fingerprint: String,
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
    pub(crate) summary: SymbolCorrespondenceSummary,
    pub(crate) correspondences: Vec<SymbolCorrespondence>,
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
    let mut targets = BTreeMap::<String, Vec<SymbolCorrespondenceFunction>>::new();
    for symbol in &to_symbols {
        targets
            .entry(normalized_body_fingerprint(symbol))
            .or_default()
            .push(function_document(symbol));
    }
    for candidates in targets.values_mut() {
        candidates.sort();
    }

    let mut correspondences = from_symbols
        .iter()
        .map(|symbol| {
            let from = function_document(symbol);
            let candidates = targets.get(&from.fingerprint).cloned().unwrap_or_default();
            let status = match candidates.len() {
                0 => SymbolCorrespondenceStatus::Unmatched,
                1 => SymbolCorrespondenceStatus::Unique,
                _ => SymbolCorrespondenceStatus::Ambiguous,
            };
            SymbolCorrespondence {
                from,
                status,
                basis: "exact-normalized-body",
                candidates,
            }
        })
        .collect::<Vec<_>>();
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

    Ok(SymbolCorrespondenceReport {
        schema_version: SYMBOL_CORRESPONDENCE_SCHEMA,
        command: "symbols correlate",
        method: "sha256-relocatable-body-and-relocation-shape-v1",
        from: artifact_document(request.from_source, request.from_path, from_symbols.len())?,
        to: artifact_document(request.to_source, request.to_path, to_symbols.len())?,
        summary,
        correspondences,
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

fn load_functions(path: &Path, prefix: &str) -> Result<Vec<artifact::ArtifactSymbolDefinition>> {
    let mut symbols =
        artifact::load_code_symbols(path, prefix, artifact::CodeSymbolSelection::Exported)?;
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

fn function_document(symbol: &artifact::ArtifactSymbolDefinition) -> SymbolCorrespondenceFunction {
    SymbolCorrespondenceFunction {
        member: symbol.member.clone(),
        symbol: symbol.name.clone(),
        size: symbol.bytes.len(),
        fingerprint: normalized_body_fingerprint(symbol),
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
            .map(function_document)
            .collect::<Vec<_>>();
        let mut correspondences = vec![
            SymbolCorrespondence {
                from: function_document(&from_symbols[0]),
                status: SymbolCorrespondenceStatus::Unique,
                basis: "exact-normalized-body",
                candidates: vec![function_document(&to_symbols[0])],
            },
            SymbolCorrespondence {
                from: function_document(&from_symbols[1]),
                status: SymbolCorrespondenceStatus::Ambiguous,
                basis: "exact-normalized-body",
                candidates: target_candidates.clone(),
            },
            SymbolCorrespondence {
                from: function_document(&from_symbols[2]),
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
}
