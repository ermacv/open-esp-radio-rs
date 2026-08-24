//! Portable cross-version snapshots, deterministic diffs and review rebase plans.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::ProjectSession;
use crate::{
    Result,
    artifacts::LinkedIrReader,
    interfaces::{InterfaceFactRoot, InterfaceFactStep, InterfaceFacts},
    registers::{RegisterFacts, load_effective_register_model},
};

pub(crate) const REVISION_SCHEMA: u32 = 1;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RevisionSnapshot {
    pub(crate) schema_version: u32,
    pub(crate) command: String,
    pub(crate) name: String,
    pub(crate) project: String,
    pub(crate) artifacts: Vec<RevisionArtifact>,
    pub(crate) functions: Vec<RevisionFunction>,
    pub(crate) registers: Vec<RevisionRegister>,
    pub(crate) interfaces: Vec<RevisionInterface>,
    pub(crate) assertions: Vec<RevisionReviewedRecord>,
    pub(crate) vendor_bugs: Vec<RevisionReviewedRecord>,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RevisionArtifact {
    pub(crate) source: String,
    pub(crate) sha256: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RevisionCompleteness {
    pub(crate) body: bool,
    pub(crate) call_targets: bool,
    pub(crate) transitive_effects: bool,
    pub(crate) executable: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RevisionFunction {
    pub(crate) id: String,
    pub(crate) source: String,
    pub(crate) member: Option<String>,
    pub(crate) symbol: String,
    pub(crate) profiles: Vec<String>,
    pub(crate) fingerprint: String,
    pub(crate) features: Vec<String>,
    pub(crate) completeness: RevisionCompleteness,
    pub(crate) blocker_roots: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RevisionRegister {
    pub(crate) id: String,
    pub(crate) address: u32,
    pub(crate) width: u8,
    pub(crate) name: Option<String>,
    pub(crate) fingerprint: String,
    pub(crate) features: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RevisionInterface {
    pub(crate) id: String,
    pub(crate) fingerprint: String,
    pub(crate) features: Vec<String>,
    pub(crate) functions: usize,
}

/// The complete serialized record is retained so a rebase plan never drops
/// provenance/applicability fields it does not need for matching.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RevisionReviewedRecord {
    pub(crate) id: String,
    pub(crate) subject: Option<String>,
    pub(crate) record: serde_json::Value,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum RevisionChangeClass {
    Unchanged,
    Moved,
    Modified,
    Added,
    Removed,
    Split,
    Merged,
    Ambiguous,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RevisionEntityChange {
    pub(crate) domain: String,
    pub(crate) classification: RevisionChangeClass,
    pub(crate) before: Vec<String>,
    pub(crate) after: Vec<String>,
    pub(crate) confidence: String,
    pub(crate) reason: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RevisionDiffSummary {
    pub(crate) unchanged: usize,
    pub(crate) moved: usize,
    pub(crate) modified: usize,
    pub(crate) added: usize,
    pub(crate) removed: usize,
    pub(crate) split: usize,
    pub(crate) merged: usize,
    pub(crate) ambiguous: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RevisionDiffReport {
    pub(crate) schema_version: u32,
    pub(crate) command: String,
    pub(crate) from: String,
    pub(crate) to: String,
    pub(crate) artifacts_changed: bool,
    pub(crate) summary: RevisionDiffSummary,
    pub(crate) changes: Vec<RevisionEntityChange>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum RevisionRebaseStatus {
    AlreadyPresent,
    CarryExact,
    CarryRemapped,
    ReviewRequired,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RevisionRebaseRecord {
    pub(crate) id: String,
    pub(crate) kind: String,
    pub(crate) status: RevisionRebaseStatus,
    pub(crate) old_subject: Option<String>,
    pub(crate) proposed_subject: Option<String>,
    pub(crate) reason: String,
    pub(crate) record: serde_json::Value,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RevisionRebaseSummary {
    pub(crate) already_present: usize,
    pub(crate) carry_exact: usize,
    pub(crate) carry_remapped: usize,
    pub(crate) review_required: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RevisionRebaseReport {
    pub(crate) schema_version: u32,
    pub(crate) command: String,
    pub(crate) from: String,
    pub(crate) to: String,
    pub(crate) summary: RevisionRebaseSummary,
    pub(crate) records: Vec<RevisionRebaseRecord>,
}

pub(crate) fn default_path(manifest: &Path, name: &str) -> Result<PathBuf> {
    validate_revision_name(name)?;
    Ok(manifest
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("generated/revisions")
        .join(format!("{name}.json")))
}

pub(crate) fn resolve_path(manifest: &Path, value: &str) -> Result<PathBuf> {
    let candidate = Path::new(value);
    if candidate.is_absolute()
        || candidate.components().count() > 1
        || candidate.extension().is_some()
    {
        return Ok(if candidate.is_absolute() {
            candidate.to_owned()
        } else {
            manifest
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(candidate)
        });
    }
    default_path(manifest, value)
}

pub(crate) fn load(path: &Path) -> Result<RevisionSnapshot> {
    let input = fs::read_to_string(path)
        .map_err(|error| crate::Error::read("revision snapshot", path, error))?;
    let snapshot: RevisionSnapshot = serde_json::from_str(&input).map_err(|error| {
        crate::Error::manifest_source("revision snapshot", path, &input, error, None)
    })?;
    validate_snapshot(&snapshot)?;
    Ok(snapshot)
}

pub(crate) fn snapshot(session: &ProjectSession, name: &str) -> Result<RevisionSnapshot> {
    validate_revision_name(name)?;
    let (artifacts, functions) = snapshot_functions(session)?;
    let registers = snapshot_registers(session)?;
    let (interface_artifacts, interfaces) = snapshot_interfaces(session)?;
    let artifacts = artifacts
        .into_iter()
        .chain(interface_artifacts)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let knowledge =
        open_radio_vendor_review::ReviewKnowledge::load_all(&session.project.reviewed_knowledge)
            .map_err(|error| {
                crate::Error::invalid(format!("cannot snapshot reviewed knowledge: {error}"))
            })?;
    let assertions = knowledge
        .assertions()
        .values()
        .map(|assertion| {
            Ok(RevisionReviewedRecord {
                id: assertion.id.clone(),
                subject: Some(assertion.subject.clone()),
                record: serde_json::to_value(assertion)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let vendor_bugs = knowledge
        .vendor_bugs()
        .values()
        .map(|bug| {
            Ok(RevisionReviewedRecord {
                id: bug.id.clone(),
                subject: Some(bug.function.clone()),
                record: serde_json::to_value(bug)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let snapshot = RevisionSnapshot {
        schema_version: REVISION_SCHEMA,
        command: "revision snapshot".to_owned(),
        name: name.to_owned(),
        project: session.project.id.clone(),
        artifacts,
        functions,
        registers,
        interfaces,
        assertions,
        vendor_bugs,
    };
    validate_snapshot(&snapshot)?;
    Ok(snapshot)
}

fn snapshot_functions(
    session: &ProjectSession,
) -> Result<(Vec<RevisionArtifact>, Vec<RevisionFunction>)> {
    let mut artifacts = BTreeSet::new();
    let mut functions = BTreeMap::<String, RevisionFunction>::new();
    for profile in &session.project.ir_profiles {
        let reader = LinkedIrReader::open(&profile.output)?;
        let projection = reader.read_review_projection()?;
        artifacts.extend(
            projection
                .inputs
                .into_iter()
                .map(|(source, sha256)| RevisionArtifact { source, sha256 }),
        );
        for function in projection.functions {
            let mut features = Vec::new();
            features.extend(function.loops.iter().map(|loop_| {
                format!(
                    "loop:{}:{}:{}",
                    loop_.kind,
                    loop_.depth,
                    loop_
                        .counted
                        .as_ref()
                        .map_or(0, |counted| counted.trip_count)
                )
            }));
            features.extend(function.calls.iter().map(|call| {
                format!(
                    "call:{}:{}",
                    call.kind,
                    call.semantic_operation
                        .as_deref()
                        .or(call.project_symbol.as_deref())
                        .unwrap_or(&call.target)
                )
            }));
            features.extend(
                function
                    .mmio
                    .iter()
                    .map(|mmio| format!("mmio:{:#010x}/{}", mmio.address, mmio.width)),
            );
            features.extend(function.direct_effects.iter().map(|effect| {
                format!(
                    "effect:{}:{}:{}:{:?}:{:?}:{:?}:{:?}:{:?}:{}",
                    effect.kind,
                    effect.operation,
                    effect.target,
                    effect.width,
                    effect.modified_mask,
                    effect.preserved_mask,
                    effect.forced_zero_mask,
                    effect.forced_one_mask,
                    effect.arguments.join(",")
                )
            }));
            features.sort();
            features.dedup();
            let fingerprint = fingerprint(&features)?;
            let mut blocker_roots = function
                .diagnostics
                .iter()
                .map(|diagnostic| format!("{}:{}", diagnostic.kind, diagnostic.root_id))
                .chain(
                    function
                        .decode_blockers
                        .iter()
                        .map(|blocker| format!("decode:{}:{}", blocker.class, blocker.width)),
                )
                .collect::<Vec<_>>();
            blocker_roots.sort();
            blocker_roots.dedup();
            let entity = RevisionFunction {
                id: function.identity.clone(),
                source: function.source,
                member: function.member,
                symbol: function.symbol,
                profiles: vec![profile.id.clone()],
                fingerprint,
                features,
                completeness: RevisionCompleteness {
                    body: function.completeness.body_complete,
                    call_targets: function.completeness.call_targets_complete,
                    transitive_effects: function.completeness.transitive_effects_complete,
                    executable: function.completeness.executable_complete,
                },
                blocker_roots,
            };
            if let Some(existing) = functions.get_mut(&entity.id) {
                if existing.fingerprint != entity.fingerprint
                    || existing.source != entity.source
                    || existing.member != entity.member
                    || existing.symbol != entity.symbol
                {
                    return Err(crate::Error::invalid(format!(
                        "revision snapshot found inconsistent projections for function {:?}",
                        entity.id
                    )));
                }
                existing.profiles.push(profile.id.clone());
                existing.profiles.sort();
                existing.profiles.dedup();
                existing.completeness.body &= entity.completeness.body;
                existing.completeness.call_targets &= entity.completeness.call_targets;
                existing.completeness.transitive_effects &= entity.completeness.transitive_effects;
                existing.completeness.executable &= entity.completeness.executable;
                existing.blocker_roots.extend(entity.blocker_roots);
                existing.blocker_roots.sort();
                existing.blocker_roots.dedup();
            } else {
                functions.insert(entity.id.clone(), entity);
            }
        }
    }
    Ok((
        artifacts.into_iter().collect(),
        functions.into_values().collect(),
    ))
}

fn snapshot_registers(session: &ProjectSession) -> Result<Vec<RevisionRegister>> {
    let Some(paths) = session.project.registers.as_ref() else {
        return Ok(Vec::new());
    };
    let facts = RegisterFacts::load(&paths.facts)?;
    let identities = load_effective_register_model(paths)?.register_identities()?;
    facts
        .registers
        .into_iter()
        .map(|register| {
            let mut features = vec![
                format!("reads:{}", register.reads),
                format!("writes:{}", register.writes),
            ];
            features.extend(
                register
                    .read_functions
                    .iter()
                    .map(|function| format!("read:{function}")),
            );
            features.extend(
                register
                    .write_functions
                    .iter()
                    .map(|function| format!("write:{function}")),
            );
            features.extend(register.write_patterns.iter().map(|pattern| {
                format!(
                    "pattern:{:#010x}:{:#010x}:{:#010x}:{:#010x}:{:#010x}:{:#010x}",
                    pattern.modified_mask,
                    pattern.preserved_mask,
                    pattern.inverted_mask,
                    pattern.forced_zero_mask,
                    pattern.forced_one_mask,
                    pattern.dynamic_mask
                )
            }));
            features.sort();
            features.dedup();
            Ok(RevisionRegister {
                id: format!("mmio:cpu:{:#010x}/{}", register.address, register.width),
                address: register.address,
                width: register.width,
                name: identities
                    .get(&(u64::from(register.address), u32::from(register.width)))
                    .cloned(),
                fingerprint: fingerprint(&features)?,
                features,
            })
        })
        .collect()
}

fn snapshot_interfaces(
    session: &ProjectSession,
) -> Result<(Vec<RevisionArtifact>, Vec<RevisionInterface>)> {
    let Some(paths) = session.project.interfaces.as_ref() else {
        return Ok((Vec::new(), Vec::new()));
    };
    let facts = InterfaceFacts::load(&paths.facts)?;
    let artifacts = facts
        .artifacts
        .iter()
        .flat_map(|artifact| {
            artifact.sha256.iter().flat_map(|sha256| {
                artifact.sources.iter().map(|source| RevisionArtifact {
                    source: source.clone(),
                    sha256: sha256.clone(),
                })
            })
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut entities = facts
        .tables
        .iter()
        .map(|table| {
            let artifact = facts.artifact(table.artifact).ok_or_else(|| {
                crate::Error::invalid("interface table references an absent artifact")
            })?;
            let sources = artifact
                .sources
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(",");
            let path = table
                .container_path
                .iter()
                .map(interface_step)
                .collect::<Vec<_>>()
                .join("/");
            let id = format!("interface:{sources}:{}:{path}", interface_root(&table.root));
            let mut features = table
                .slots
                .iter()
                .map(|slot| {
                    format!(
                        "slot:{:+#x}/{}:{}",
                        slot.offset,
                        slot.width,
                        slot.selector
                            .map_or_else(|| "-".to_owned(), |selector| selector.canonical())
                    )
                })
                .collect::<Vec<_>>();
            features.sort();
            features.dedup();
            Ok(RevisionInterface {
                id,
                fingerprint: fingerprint(&features)?,
                features,
                functions: table.functions.len(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    entities.sort_by(|left, right| left.id.cmp(&right.id));
    Ok((artifacts, entities))
}

fn interface_root(root: &InterfaceFactRoot) -> String {
    match root {
        InterfaceFactRoot::RelocatedSymbol {
            member,
            symbol,
            addend,
            addressing,
        } => format!(
            "reloc:{}:{symbol}:{addend}:{addressing}",
            member.as_deref().unwrap_or("-")
        ),
        InterfaceFactRoot::FunctionArgument { argument } => format!("arg:{argument}"),
        InterfaceFactRoot::BoundedDataAddress {
            canonical,
            member,
            symbol,
            ..
        } => format!(
            "data:{canonical}:{}:{symbol}",
            member.as_deref().unwrap_or("-")
        ),
        InterfaceFactRoot::AbsoluteAddress { address } => format!("absolute:{address:#010x}"),
    }
}

fn interface_step(step: &InterfaceFactStep) -> String {
    format!(
        "{:+#x}/{}:{}",
        step.offset,
        step.width,
        step.selector
            .map_or_else(|| "-".to_owned(), |selector| selector.canonical())
    )
}

fn fingerprint(value: &impl Serialize) -> Result<String> {
    let encoded = serde_json::to_vec(value)?;
    let mut hash = Sha256::new();
    hash.update(b"blobray/revision-feature/v1\0");
    hash.update(encoded);
    Ok(hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn validate_revision_name(name: &str) -> Result<()> {
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(crate::Error::invalid(format!(
            "invalid revision name {name:?}"
        )));
    }
    Ok(())
}

fn validate_snapshot(snapshot: &RevisionSnapshot) -> Result<()> {
    if snapshot.schema_version != REVISION_SCHEMA || snapshot.command != "revision snapshot" {
        return Err(crate::Error::invalid(format!(
            "unsupported revision snapshot schema/command for {:?}",
            snapshot.name
        )));
    }
    validate_revision_name(&snapshot.name)?;
    validate_unique(
        "function",
        snapshot.functions.iter().map(|entity| &entity.id),
    )?;
    validate_unique(
        "register",
        snapshot.registers.iter().map(|entity| &entity.id),
    )?;
    validate_unique(
        "interface",
        snapshot.interfaces.iter().map(|entity| &entity.id),
    )?;
    validate_unique(
        "assertion",
        snapshot.assertions.iter().map(|record| &record.id),
    )?;
    validate_unique(
        "vendor bug",
        snapshot.vendor_bugs.iter().map(|record| &record.id),
    )?;
    Ok(())
}

fn validate_unique<'a>(label: &str, mut values: impl Iterator<Item = &'a String>) -> Result<()> {
    let mut seen = BTreeSet::new();
    if let Some(duplicate) = values.find(|value| !seen.insert(value.as_str())) {
        return Err(crate::Error::invalid(format!(
            "revision snapshot contains duplicate {label} {duplicate:?}"
        )));
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct EntityView<'a> {
    id: &'a str,
    fingerprint: &'a str,
}

pub(crate) fn diff(from: &RevisionSnapshot, to: &RevisionSnapshot) -> RevisionDiffReport {
    let mut summary = RevisionDiffSummary::default();
    let mut changes = Vec::new();
    diff_domain(
        "function",
        from.functions.iter().map(|entity| EntityView {
            id: &entity.id,
            fingerprint: &entity.fingerprint,
        }),
        to.functions.iter().map(|entity| EntityView {
            id: &entity.id,
            fingerprint: &entity.fingerprint,
        }),
        &mut summary,
        &mut changes,
    );
    diff_domain(
        "register",
        from.registers.iter().map(|entity| EntityView {
            id: &entity.id,
            fingerprint: &entity.fingerprint,
        }),
        to.registers.iter().map(|entity| EntityView {
            id: &entity.id,
            fingerprint: &entity.fingerprint,
        }),
        &mut summary,
        &mut changes,
    );
    diff_domain(
        "interface",
        from.interfaces.iter().map(|entity| EntityView {
            id: &entity.id,
            fingerprint: &entity.fingerprint,
        }),
        to.interfaces.iter().map(|entity| EntityView {
            id: &entity.id,
            fingerprint: &entity.fingerprint,
        }),
        &mut summary,
        &mut changes,
    );
    changes.sort_by(|left, right| {
        (&left.domain, left.classification, &left.before, &left.after).cmp(&(
            &right.domain,
            right.classification,
            &right.before,
            &right.after,
        ))
    });
    RevisionDiffReport {
        schema_version: REVISION_SCHEMA,
        command: "revision diff".to_owned(),
        from: from.name.clone(),
        to: to.name.clone(),
        artifacts_changed: from.artifacts != to.artifacts,
        summary,
        changes,
    }
}

fn diff_domain<'a>(
    domain: &str,
    before: impl Iterator<Item = EntityView<'a>>,
    after: impl Iterator<Item = EntityView<'a>>,
    summary: &mut RevisionDiffSummary,
    changes: &mut Vec<RevisionEntityChange>,
) {
    let before = before
        .map(|entity| (entity.id, entity))
        .collect::<BTreeMap<_, _>>();
    let after = after
        .map(|entity| (entity.id, entity))
        .collect::<BTreeMap<_, _>>();
    let mut before_remaining = before.keys().copied().collect::<BTreeSet<_>>();
    let mut after_remaining = after.keys().copied().collect::<BTreeSet<_>>();
    for id in before.keys().filter(|id| after.contains_key(**id)) {
        before_remaining.remove(id);
        after_remaining.remove(id);
        if before[*id].fingerprint == after[*id].fingerprint {
            summary.unchanged += 1;
        } else {
            summary.modified += 1;
            changes.push(change(
                domain,
                RevisionChangeClass::Modified,
                vec![(*id).to_owned()],
                vec![(*id).to_owned()],
                "high",
                "stable entity identity remained but its normalized features changed",
            ));
        }
    }
    let before_by_fingerprint = group_by_fingerprint(&before, &before_remaining);
    let after_by_fingerprint = group_by_fingerprint(&after, &after_remaining);
    for fingerprint in before_by_fingerprint
        .keys()
        .filter(|fingerprint| after_by_fingerprint.contains_key(*fingerprint))
    {
        let old = &before_by_fingerprint[fingerprint];
        let new = &after_by_fingerprint[fingerprint];
        for id in old {
            before_remaining.remove(id.as_str());
        }
        for id in new {
            after_remaining.remove(id.as_str());
        }
        let (classification, confidence, reason) = match (old.len(), new.len()) {
            (1, 1) => (
                RevisionChangeClass::Moved,
                "high",
                "unique normalized feature fingerprint survived under a new identity",
            ),
            (1, _) => (
                RevisionChangeClass::Split,
                "low",
                "one old entity has several exact feature twins; manual split review is required",
            ),
            (_, 1) => (
                RevisionChangeClass::Merged,
                "low",
                "several old entities have one exact feature twin; manual merge review is required",
            ),
            _ => (
                RevisionChangeClass::Ambiguous,
                "low",
                "normalized fingerprints are non-unique on both revisions",
            ),
        };
        increment(summary, classification);
        changes.push(change(
            domain,
            classification,
            old.clone(),
            new.clone(),
            confidence,
            reason,
        ));
    }
    for id in before_remaining {
        summary.removed += 1;
        changes.push(change(
            domain,
            RevisionChangeClass::Removed,
            vec![id.to_owned()],
            Vec::new(),
            "high",
            "no stable identity or exact normalized feature match exists",
        ));
    }
    for id in after_remaining {
        summary.added += 1;
        changes.push(change(
            domain,
            RevisionChangeClass::Added,
            Vec::new(),
            vec![id.to_owned()],
            "high",
            "no stable identity or exact normalized feature match exists",
        ));
    }
}

fn group_by_fingerprint(
    entities: &BTreeMap<&str, EntityView<'_>>,
    remaining: &BTreeSet<&str>,
) -> BTreeMap<String, Vec<String>> {
    let mut grouped = BTreeMap::<String, Vec<String>>::new();
    for id in remaining {
        grouped
            .entry(entities[id].fingerprint.to_owned())
            .or_default()
            .push((*id).to_owned());
    }
    grouped
}

fn change(
    domain: &str,
    classification: RevisionChangeClass,
    before: Vec<String>,
    after: Vec<String>,
    confidence: &str,
    reason: &str,
) -> RevisionEntityChange {
    RevisionEntityChange {
        domain: domain.to_owned(),
        classification,
        before,
        after,
        confidence: confidence.to_owned(),
        reason: reason.to_owned(),
    }
}

fn increment(summary: &mut RevisionDiffSummary, classification: RevisionChangeClass) {
    match classification {
        RevisionChangeClass::Unchanged => summary.unchanged += 1,
        RevisionChangeClass::Moved => summary.moved += 1,
        RevisionChangeClass::Modified => summary.modified += 1,
        RevisionChangeClass::Added => summary.added += 1,
        RevisionChangeClass::Removed => summary.removed += 1,
        RevisionChangeClass::Split => summary.split += 1,
        RevisionChangeClass::Merged => summary.merged += 1,
        RevisionChangeClass::Ambiguous => summary.ambiguous += 1,
    }
}

pub(crate) fn rebase(from: &RevisionSnapshot, to: &RevisionSnapshot) -> RevisionRebaseReport {
    let diff = diff(from, to);
    let mappings = automatic_mappings(&diff);
    let target_subjects = to
        .functions
        .iter()
        .map(|entity| entity.id.as_str())
        .chain(to.registers.iter().map(|entity| entity.id.as_str()))
        .chain(to.interfaces.iter().map(|entity| entity.id.as_str()))
        .collect::<BTreeSet<_>>();
    let current = to
        .assertions
        .iter()
        .chain(&to.vendor_bugs)
        .map(|record| (record.id.as_str(), record))
        .collect::<BTreeMap<_, _>>();
    let target_artifacts = to
        .artifacts
        .iter()
        .map(|artifact| (artifact.source.as_str(), artifact.sha256.as_str()))
        .collect::<BTreeSet<_>>();
    let mut records = from
        .assertions
        .iter()
        .map(|record| {
            rebase_record(
                "assertion",
                record,
                &current,
                &target_subjects,
                &target_artifacts,
                &mappings,
            )
        })
        .chain(from.vendor_bugs.iter().map(|record| {
            rebase_record(
                "vendor-bug",
                record,
                &current,
                &target_subjects,
                &target_artifacts,
                &mappings,
            )
        }))
        .collect::<Vec<_>>();
    records.sort_by(|left, right| left.id.cmp(&right.id));
    let mut summary = RevisionRebaseSummary::default();
    for record in &records {
        match record.status {
            RevisionRebaseStatus::AlreadyPresent => summary.already_present += 1,
            RevisionRebaseStatus::CarryExact => summary.carry_exact += 1,
            RevisionRebaseStatus::CarryRemapped => summary.carry_remapped += 1,
            RevisionRebaseStatus::ReviewRequired => summary.review_required += 1,
        }
    }
    RevisionRebaseReport {
        schema_version: REVISION_SCHEMA,
        command: "revision rebase".to_owned(),
        from: from.name.clone(),
        to: to.name.clone(),
        summary,
        records,
    }
}

fn automatic_mappings(diff: &RevisionDiffReport) -> BTreeMap<&str, &str> {
    diff.changes
        .iter()
        .filter(|change| change.classification == RevisionChangeClass::Moved)
        .filter_map(|change| {
            Some((
                change.before.first()?.as_str(),
                change.after.first()?.as_str(),
            ))
        })
        .collect()
}

fn rebase_record(
    kind: &str,
    record: &RevisionReviewedRecord,
    current: &BTreeMap<&str, &RevisionReviewedRecord>,
    target_subjects: &BTreeSet<&str>,
    target_artifacts: &BTreeSet<(&str, &str)>,
    mappings: &BTreeMap<&str, &str>,
) -> RevisionRebaseRecord {
    let old_subject = record.subject.clone();
    let subject_prefix = old_subject
        .as_deref()
        .filter(|subject| subject.starts_with("function:"))
        .map_or("", |_| "function:");
    let (base, suffix) = old_subject
        .as_deref()
        .map(split_subject_suffix)
        .unwrap_or(("", ""));
    let exact_target = target_subjects.contains(base);
    let mapped = mappings.get(base).copied();
    let proposed_subject = mapped
        .map(|target| format!("{subject_prefix}{target}{suffix}"))
        .or_else(|| exact_target.then(|| old_subject.clone()).flatten());
    let applicability_current = applicability_matches(&record.record, target_artifacts);
    let (status, reason) = if !applicability_current {
        (
            RevisionRebaseStatus::ReviewRequired,
            "record applicability names artifact bytes absent from the target revision",
        )
    } else if current
        .get(record.id.as_str())
        .is_some_and(|current| *current == record)
    {
        (
            RevisionRebaseStatus::AlreadyPresent,
            "the target snapshot already contains the identical reviewed record",
        )
    } else if mapped.is_some() {
        (
            RevisionRebaseStatus::CarryRemapped,
            "the subject has one high-confidence normalized-feature move",
        )
    } else if exact_target {
        (
            RevisionRebaseStatus::CarryExact,
            "the stable subject exists unchanged in the target revision",
        )
    } else {
        (
            RevisionRebaseStatus::ReviewRequired,
            "no unique unchanged or moved target subject exists",
        )
    };
    RevisionRebaseRecord {
        id: record.id.clone(),
        kind: kind.to_owned(),
        status,
        old_subject,
        proposed_subject,
        reason: reason.to_owned(),
        record: record.record.clone(),
    }
}

fn split_subject_suffix(subject: &str) -> (&str, &str) {
    let subject = subject.strip_prefix("function:").unwrap_or(subject);
    subject
        .find('#')
        .map_or((subject, ""), |index| subject.split_at(index))
}

fn applicability_matches(
    record: &serde_json::Value,
    target_artifacts: &BTreeSet<(&str, &str)>,
) -> bool {
    let Some(artifacts) = record
        .get("applies-to")
        .and_then(|value| value.get("artifacts"))
        .and_then(serde_json::Value::as_array)
    else {
        return true;
    };
    artifacts.iter().all(|artifact| {
        let source = artifact.get("source").and_then(serde_json::Value::as_str);
        let sha256 = artifact.get("sha256").and_then(serde_json::Value::as_str);
        source
            .zip(sha256)
            .is_some_and(|identity| target_artifacts.contains(&identity))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn function(id: &str, fingerprint: &str) -> RevisionFunction {
        RevisionFunction {
            id: id.to_owned(),
            source: "vendor".to_owned(),
            member: None,
            symbol: id.to_owned(),
            profiles: vec!["all".to_owned()],
            fingerprint: fingerprint.to_owned(),
            features: Vec::new(),
            completeness: RevisionCompleteness {
                body: true,
                call_targets: true,
                transitive_effects: true,
                executable: true,
            },
            blocker_roots: Vec::new(),
        }
    }

    fn snapshot(name: &str, functions: Vec<RevisionFunction>) -> RevisionSnapshot {
        RevisionSnapshot {
            schema_version: REVISION_SCHEMA,
            command: "revision snapshot".to_owned(),
            name: name.to_owned(),
            project: "fixture".to_owned(),
            artifacts: Vec::new(),
            functions,
            registers: Vec::new(),
            interfaces: Vec::new(),
            assertions: Vec::new(),
            vendor_bugs: Vec::new(),
        }
    }

    #[test]
    fn diff_classifies_stable_modified_moved_and_ambiguous_entities() {
        let before = snapshot(
            "old",
            vec![
                function("same", "a"),
                function("changed", "b"),
                function("old-moved", "c"),
                function("old-twin-1", "d"),
                function("old-twin-2", "d"),
                function("removed", "e"),
            ],
        );
        let after = snapshot(
            "new",
            vec![
                function("same", "a"),
                function("changed", "z"),
                function("new-moved", "c"),
                function("new-twin-1", "d"),
                function("new-twin-2", "d"),
                function("added", "f"),
            ],
        );

        let report = diff(&before, &after);

        assert_eq!(report.summary.unchanged, 1);
        assert_eq!(report.summary.modified, 1);
        assert_eq!(report.summary.moved, 1);
        assert_eq!(report.summary.ambiguous, 1);
        assert_eq!(report.summary.removed, 1);
        assert_eq!(report.summary.added, 1);
    }

    #[test]
    fn rebase_carries_only_exact_or_uniquely_moved_subjects() {
        let mut before = snapshot("old", vec![function("old", "a"), function("gone", "b")]);
        before.assertions = vec![
            RevisionReviewedRecord {
                id: "fact.moved".to_owned(),
                subject: Some("old".to_owned()),
                record: serde_json::json!({"id":"fact.moved","subject":"old"}),
            },
            RevisionReviewedRecord {
                id: "fact.gone".to_owned(),
                subject: Some("gone".to_owned()),
                record: serde_json::json!({"id":"fact.gone","subject":"gone"}),
            },
        ];
        let after = snapshot("new", vec![function("new", "a")]);

        let report = rebase(&before, &after);

        assert_eq!(report.summary.carry_remapped, 1);
        assert_eq!(report.summary.review_required, 1);
        assert_eq!(report.records[1].proposed_subject.as_deref(), Some("new"));
    }

    #[test]
    fn rebase_rejects_an_artifact_guard_from_the_old_revision() {
        let mut before = snapshot("old", vec![function("stable", "a")]);
        before.artifacts = vec![RevisionArtifact {
            source: "vendor".to_owned(),
            sha256: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
        }];
        before.assertions = vec![RevisionReviewedRecord {
            id: "fact.guarded".to_owned(),
            subject: Some("stable".to_owned()),
            record: serde_json::json!({
                "id": "fact.guarded",
                "subject": "stable",
                "applies-to": {"artifacts": [{
                    "source": "vendor",
                    "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                }]}
            }),
        }];
        let mut after = snapshot("new", vec![function("stable", "a")]);
        after.artifacts = vec![RevisionArtifact {
            source: "vendor".to_owned(),
            sha256: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_owned(),
        }];

        let report = rebase(&before, &after);

        assert_eq!(report.summary.review_required, 1);
        assert!(report.records[0].reason.contains("artifact bytes absent"));
    }
}
