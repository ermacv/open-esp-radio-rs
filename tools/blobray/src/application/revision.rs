//! Portable cross-version snapshots, deterministic diffs and review rebase plans.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{BufReader, Read, Write},
    path::Component,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{ProjectContext, ProjectSession};
use crate::{
    Result,
    artifacts::LinkedIrReader,
    interfaces::{InterfaceFactRoot, InterfaceFactStep, InterfaceFacts},
    registers::{RegisterFacts, load_effective_register_model},
};

pub(crate) const REVISION_SCHEMA: u32 = 1;
pub(crate) const REVISION_LEDGER_SCHEMA: u32 = 1;

static LEDGER_STAGE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

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

/// Small, reviewable index for immutable revision snapshots.
///
/// The ledger contains content identities and relative locations only. Vendor
/// bytes, decoded instructions and analysis payloads remain in the snapshot or
/// in caller-owned artifact storage.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct RevisionLedger {
    pub(crate) schema: u32,
    pub(crate) project: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) baseline: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) current: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) prepared_update: Option<PreparedRevisionUpdate>,
    #[serde(default, rename = "revisions")]
    pub(crate) entries: Vec<RevisionLedgerEntry>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct RevisionLedgerEntry {
    pub(crate) name: String,
    pub(crate) snapshot: String,
    pub(crate) snapshot_sha256: String,
    pub(crate) artifacts_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct PreparedRevisionUpdate {
    pub(crate) from: String,
    pub(crate) snapshot_sha256: String,
    pub(crate) artifacts_sha256: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum RevisionLedgerHealth {
    Missing,
    BaselineMissing,
    Ready,
    Invalid,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RevisionLedgerInspection {
    pub(crate) path: String,
    pub(crate) health: RevisionLedgerHealth,
    pub(crate) baseline: Option<String>,
    pub(crate) current: Option<String>,
    pub(crate) revisions: usize,
    pub(crate) update_prepared: bool,
    pub(crate) diagnostic: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RevisionPrepareUpdateReport {
    pub(crate) schema_version: u32,
    pub(crate) command: String,
    pub(crate) status: String,
    pub(crate) ledger: String,
    pub(crate) baseline: String,
    pub(crate) current: String,
    pub(crate) snapshot_sha256: String,
    pub(crate) artifacts_sha256: String,
    pub(crate) artifact_bindings_verified: usize,
}

pub(crate) fn default_path(manifest: &Path, name: &str) -> Result<PathBuf> {
    validate_revision_name(name)?;
    Ok(manifest
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("revisions/snapshots")
        .join(format!("{name}.json")))
}

pub(crate) fn ledger_path(manifest: &Path) -> PathBuf {
    manifest
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("revisions/ledger.toml")
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
    if let Some(ledger) = load_ledger_optional(manifest, None)?
        && let Some(entry) = ledger.entries.iter().find(|entry| entry.name == value)
    {
        return snapshot_path(manifest, &entry.snapshot);
    }
    let durable = default_path(manifest, value)?;
    let legacy = manifest
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("generated/revisions")
        .join(format!("{value}.json"));
    Ok(if durable.is_file() || !legacy.is_file() {
        durable
    } else {
        legacy
    })
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

/// Persist one immutable snapshot and advance the durable revision pointers.
///
/// The snapshot is atomically published before the ledger. On Unix both file
/// and parent-directory metadata are synced in that order, so a crash can
/// leave an unreferenced immutable snapshot but not a durable ledger entry for
/// missing or partially written bytes. Other platforms retain atomic rename
/// and file-sync guarantees where `std` cannot open a directory for syncing.
pub(crate) fn persist_snapshot(
    manifest: &Path,
    snapshot: &RevisionSnapshot,
    path: &Path,
    check: bool,
) -> Result<()> {
    validate_snapshot(snapshot)?;
    let path = project_relative_output(manifest, path);
    let location = durable_snapshot_location(manifest, &path)?;
    let snapshot_sha256 = encoded_snapshot_sha256(snapshot)?;
    let artifacts_sha256 = artifacts_sha256(&snapshot.artifacts)?;
    let entry = RevisionLedgerEntry {
        name: snapshot.name.clone(),
        snapshot: location,
        snapshot_sha256,
        artifacts_sha256,
    };

    let mut ledger =
        load_ledger_optional(manifest, Some(&snapshot.project))?.unwrap_or_else(|| {
            RevisionLedger {
                schema: REVISION_LEDGER_SCHEMA,
                project: snapshot.project.clone(),
                baseline: None,
                current: None,
                prepared_update: None,
                entries: Vec::new(),
            }
        });
    if check {
        let stored = ledger
            .entries
            .iter()
            .find(|stored| stored.name == entry.name)
            .ok_or_else(|| {
                crate::Error::invalid(format!(
                    "revision ledger {} has no immutable entry {:?}",
                    ledger_path(manifest).display(),
                    entry.name
                ))
            })?;
        if stored != &entry {
            return Err(crate::Error::invalid(format!(
                "revision ledger entry {:?} differs from the requested snapshot; revision names are immutable",
                entry.name
            )));
        }
        write_snapshot_or_check(&path, snapshot, true)?;
        verify_ledger_entry(manifest, &ledger, stored)?;
        return Ok(());
    }

    if let Some(stored) = ledger
        .entries
        .iter()
        .find(|stored| stored.name == entry.name)
    {
        if stored != &entry {
            return Err(crate::Error::invalid(format!(
                "revision {:?} is immutable and already names different snapshot content; choose a new revision name",
                entry.name
            )));
        }
        write_snapshot_or_check(&path, snapshot, false)?;
        verify_ledger_entry(manifest, &ledger, stored)?;
        return Ok(());
    }

    if let Some(current) = ledger.current.as_deref() {
        let previous = ledger_entry(&ledger, current)?;
        verify_ledger_entry(manifest, &ledger, previous)?;
        if previous.artifacts_sha256 != entry.artifacts_sha256 {
            let prepared = ledger.prepared_update.as_ref().ok_or_else(|| {
                crate::Error::invalid(format!(
                    "artifact bindings changed since revision {current:?} without a recorded preflight; restore the old bindings and run `project revision prepare-update` before replacing them"
                ))
            })?;
            if prepared.from != current
                || prepared.snapshot_sha256 != previous.snapshot_sha256
                || prepared.artifacts_sha256 != previous.artifacts_sha256
            {
                return Err(crate::Error::invalid(
                    "revision update preflight does not identify the current immutable snapshot",
                ));
            }
        }
        ledger.baseline = Some(current.to_owned());
    } else {
        ledger.baseline = Some(entry.name.clone());
    }
    ledger.current = Some(entry.name.clone());
    ledger.prepared_update = None;
    ledger.entries.push(entry);
    ledger
        .entries
        .sort_by(|left, right| left.name.cmp(&right.name));
    validate_ledger(&ledger, Some(&snapshot.project))?;

    write_snapshot_or_check(&path, snapshot, false)?;
    write_ledger_atomic(manifest, &ledger)?;
    Ok(())
}

pub(crate) fn prepare_update(
    session: &ProjectSession,
    accept_current: bool,
    check: bool,
) -> Result<RevisionPrepareUpdateReport> {
    let run_spec = session.run_spec.as_ref().ok_or_else(|| {
        crate::Error::invalid(
            "revision update preflight requires the current caller-owned run spec",
        )
    })?;
    prepare_update_with_bindings(
        &session.manifest,
        &session.project.id,
        run_spec,
        accept_current,
        check,
    )
}

pub(crate) fn verify_snapshot_bindings(
    session: &ProjectSession,
    snapshot: &RevisionSnapshot,
) -> Result<usize> {
    let run_spec = session.run_spec.as_ref().ok_or_else(|| {
        crate::Error::invalid("revision snapshot requires the current caller-owned run spec")
    })?;
    verify_current_artifact_bindings(run_spec, snapshot)
}

fn prepare_update_with_bindings(
    manifest: &Path,
    project: &str,
    run_spec: &crate::run_spec::RunSpec,
    accept_current: bool,
    check: bool,
) -> Result<RevisionPrepareUpdateReport> {
    let mut ledger = load_ledger_optional(manifest, Some(project))?
        .ok_or_else(|| {
            crate::Error::invalid(format!(
                "durable revision ledger is missing at {}; run `project revision snapshot BASELINE` before replacing artifact bindings",
                ledger_path(manifest).display()
            ))
        })?;
    validate_ledger_files(manifest, &ledger)?;
    let current = ledger.current.clone().ok_or_else(|| {
        crate::Error::invalid(
            "revision ledger has no current revision; snapshot the current vendor inputs first",
        )
    })?;
    let baseline = ledger.baseline.clone().ok_or_else(|| {
        crate::Error::invalid(
            "revision ledger has no baseline; snapshot the current vendor inputs first",
        )
    })?;
    if baseline != current && !accept_current {
        return Err(crate::Error::invalid(format!(
            "revision {current:?} has not been accepted as the next baseline (ledger baseline is {baseline:?}); finish diff/rebase review, then rerun with --accept-current"
        )));
    }
    let entry = ledger_entry(&ledger, &current)?.clone();
    let snapshot = load(&snapshot_path(manifest, &entry.snapshot)?)?;
    let verified = verify_current_artifact_bindings(run_spec, &snapshot)?;
    let prepared = PreparedRevisionUpdate {
        from: current.clone(),
        snapshot_sha256: entry.snapshot_sha256.clone(),
        artifacts_sha256: entry.artifacts_sha256.clone(),
    };
    if check {
        if accept_current && ledger.baseline.as_deref() != Some(current.as_str()) {
            return Err(crate::Error::invalid(format!(
                "revision {current:?} has not been accepted in {}; rerun without --check",
                ledger_path(manifest).display()
            )));
        }
        if ledger.prepared_update.as_ref() != Some(&prepared)
            || ledger.baseline.as_deref() != Some(current.as_str())
        {
            return Err(crate::Error::invalid(format!(
                "revision update is not prepared in {}; rerun without --check after completing review",
                ledger_path(manifest).display()
            )));
        }
    } else {
        if accept_current {
            ledger.baseline = Some(current.clone());
        }
        ledger.prepared_update = Some(prepared);
        write_ledger_atomic(manifest, &ledger)?;
    }
    Ok(RevisionPrepareUpdateReport {
        schema_version: REVISION_LEDGER_SCHEMA,
        command: "revision prepare-update".to_owned(),
        status: if check { "verified" } else { "prepared" }.to_owned(),
        ledger: ledger_path(manifest).display().to_string(),
        baseline: ledger
            .baseline
            .clone()
            .expect("baseline established before report"),
        current,
        snapshot_sha256: entry.snapshot_sha256,
        artifacts_sha256: entry.artifacts_sha256,
        artifact_bindings_verified: verified,
    })
}

pub(crate) fn inspect_ledger(
    manifest: &Path,
    project: &str,
    deep: bool,
) -> RevisionLedgerInspection {
    let path = ledger_path(manifest);
    let result = (|| -> Result<RevisionLedgerInspection> {
        let Some(ledger) = load_ledger_optional(manifest, Some(project))? else {
            return Ok(RevisionLedgerInspection {
                path: path.display().to_string(),
                health: RevisionLedgerHealth::Missing,
                baseline: None,
                current: None,
                revisions: 0,
                update_prepared: false,
                diagnostic: Some(
                    "durable revision baseline is absent; run `project revision snapshot BASELINE` before a vendor update"
                        .to_owned(),
                ),
            });
        };
        let health = if ledger.baseline.is_none() || ledger.current.is_none() {
            RevisionLedgerHealth::BaselineMissing
        } else {
            if deep {
                validate_ledger_files(manifest, &ledger)?;
            }
            RevisionLedgerHealth::Ready
        };
        Ok(RevisionLedgerInspection {
            path: path.display().to_string(),
            health,
            baseline: ledger.baseline.clone(),
            current: ledger.current.clone(),
            revisions: ledger.entries.len(),
            update_prepared: ledger.prepared_update.is_some(),
            diagnostic: (health == RevisionLedgerHealth::BaselineMissing).then(|| {
                "revision ledger has no baseline/current snapshot; run `project revision snapshot BASELINE` before replacing artifact bindings"
                    .to_owned()
            }),
        })
    })();
    result.unwrap_or_else(|error| RevisionLedgerInspection {
        path: path.display().to_string(),
        health: RevisionLedgerHealth::Invalid,
        baseline: None,
        current: None,
        revisions: 0,
        update_prepared: false,
        diagnostic: Some(error.to_string()),
    })
}

pub(crate) fn verify_ledger_bindings_from_context(context: &ProjectContext<'_>) -> Result<usize> {
    let ledger = load_ledger_optional(context.project_path, Some(&context.project.id))?
        .ok_or_else(|| crate::Error::invalid("durable revision ledger is missing"))?;
    let current = ledger
        .current
        .as_deref()
        .ok_or_else(|| crate::Error::invalid("revision ledger has no current snapshot"))?;
    let entry = ledger_entry(&ledger, current)?;
    let snapshot = load(&snapshot_path(context.project_path, &entry.snapshot)?)?;
    let run_spec = context
        .run_spec
        .ok_or_else(|| crate::Error::invalid("current run spec is missing"))?;
    verify_current_artifact_bindings(run_spec, &snapshot)
}

fn load_ledger_optional(
    manifest: &Path,
    expected_project: Option<&str>,
) -> Result<Option<RevisionLedger>> {
    let path = ledger_path(manifest);
    if !path.is_file() {
        return Ok(None);
    }
    let input = fs::read_to_string(&path)
        .map_err(|error| crate::Error::read("revision ledger", &path, error))?;
    let ledger: RevisionLedger = toml_edit::de::from_str(&input).map_err(|error| {
        crate::Error::manifest_source("revision ledger", &path, &input, error, None)
    })?;
    validate_ledger(&ledger, expected_project)?;
    Ok(Some(ledger))
}

fn validate_ledger(ledger: &RevisionLedger, expected_project: Option<&str>) -> Result<()> {
    if ledger.schema != REVISION_LEDGER_SCHEMA {
        return Err(crate::Error::invalid(format!(
            "revision ledger requires schema = {REVISION_LEDGER_SCHEMA}"
        )));
    }
    if ledger.project.is_empty() {
        return Err(crate::Error::invalid(
            "revision ledger requires a non-empty project identity",
        ));
    }
    if expected_project.is_some_and(|project| project != ledger.project) {
        return Err(crate::Error::invalid(format!(
            "revision ledger project {:?} does not match project {:?}",
            ledger.project,
            expected_project.unwrap_or_default()
        )));
    }
    let mut names = BTreeSet::new();
    for entry in &ledger.entries {
        validate_revision_name(&entry.name)?;
        if !names.insert(entry.name.as_str()) {
            return Err(crate::Error::invalid(format!(
                "revision ledger contains duplicate revision {:?}",
                entry.name
            )));
        }
        validate_snapshot_location(&entry.snapshot)?;
        validate_sha256("snapshot-sha256", &entry.snapshot_sha256)?;
        validate_sha256("artifacts-sha256", &entry.artifacts_sha256)?;
    }
    for (label, name) in [
        ("baseline", ledger.baseline.as_deref()),
        ("current", ledger.current.as_deref()),
    ] {
        if let Some(name) = name
            && !names.contains(name)
        {
            return Err(crate::Error::invalid(format!(
                "revision ledger {label} {name:?} does not name an immutable revision entry"
            )));
        }
    }
    if ledger.baseline.is_some() != ledger.current.is_some() {
        return Err(crate::Error::invalid(
            "revision ledger baseline and current must either both be set or both be absent",
        ));
    }
    if let Some(prepared) = &ledger.prepared_update {
        validate_sha256("prepared-update.snapshot-sha256", &prepared.snapshot_sha256)?;
        validate_sha256(
            "prepared-update.artifacts-sha256",
            &prepared.artifacts_sha256,
        )?;
        if ledger.current.as_deref() != Some(prepared.from.as_str()) {
            return Err(crate::Error::invalid(
                "revision ledger prepared-update must identify the current revision",
            ));
        }
        let entry = ledger_entry(ledger, &prepared.from)?;
        if prepared.snapshot_sha256 != entry.snapshot_sha256
            || prepared.artifacts_sha256 != entry.artifacts_sha256
        {
            return Err(crate::Error::invalid(
                "revision ledger prepared-update digests do not match the current immutable revision",
            ));
        }
    }
    Ok(())
}

fn validate_ledger_files(manifest: &Path, ledger: &RevisionLedger) -> Result<()> {
    for entry in &ledger.entries {
        verify_ledger_entry(manifest, ledger, entry)?;
    }
    Ok(())
}

fn verify_ledger_entry(
    manifest: &Path,
    ledger: &RevisionLedger,
    entry: &RevisionLedgerEntry,
) -> Result<()> {
    let path = snapshot_path(manifest, &entry.snapshot)?;
    let actual = sha256_file(&path).map_err(|error| {
        crate::Error::invalid(format!(
            "cannot verify immutable revision snapshot {}: {error}",
            path.display()
        ))
    })?;
    if actual != entry.snapshot_sha256 {
        return Err(crate::Error::invalid(format!(
            "immutable revision snapshot {} digest differs from ledger entry {:?}",
            path.display(),
            entry.name
        )));
    }
    let snapshot = load(&path)?;
    if snapshot.name != entry.name || snapshot.project != ledger.project {
        return Err(crate::Error::invalid(format!(
            "immutable revision snapshot {} identity does not match its ledger entry",
            path.display()
        )));
    }
    if artifacts_sha256(&snapshot.artifacts)? != entry.artifacts_sha256 {
        return Err(crate::Error::invalid(format!(
            "immutable revision snapshot {} artifact-set digest differs from its ledger entry",
            path.display()
        )));
    }
    Ok(())
}

fn verify_current_artifact_bindings(
    run_spec: &crate::run_spec::RunSpec,
    snapshot: &RevisionSnapshot,
) -> Result<usize> {
    if snapshot.artifacts.is_empty() {
        return Err(crate::Error::invalid(
            "current revision snapshot contains no vendor artifact identities",
        ));
    }
    let expected = snapshot
        .artifacts
        .iter()
        .map(|artifact| (artifact.source.as_str(), artifact.sha256.as_str()))
        .collect::<BTreeSet<_>>();
    let mut actual_owned = Vec::new();
    // Every scannable input can contribute functions, registers or interfaces
    // to the revision snapshot. Treat that typed role set as revision-owned;
    // filtering by sources already observed in the old snapshot would let a
    // newly added analysis input bypass the update preflight.
    for input in run_spec
        .inputs()
        .iter()
        .filter(|input| input.role.is_scannable())
    {
        let sha256 = if input.path.is_file() {
            sha256_file(&input.path).map_err(|error| {
                crate::Error::invalid(format!(
                    "cannot hash current artifact binding {} at {}: {error}",
                    input.role,
                    input.path.display()
                ))
            })?
        } else {
            crate::artifact_path_sha256(&input.path).map_err(|error| {
                crate::Error::invalid(format!(
                    "cannot hash current artifact binding {} at {}: {error}",
                    input.role,
                    input.path.display()
                ))
            })?
        };
        actual_owned.push((input.role.source_id().to_owned(), sha256));
    }
    let actual = actual_owned
        .iter()
        .map(|(source, digest)| (source.as_str(), digest.as_str()))
        .collect::<BTreeSet<_>>();
    let missing = expected.difference(&actual).copied().collect::<Vec<_>>();
    let unexpected = actual.difference(&expected).copied().collect::<Vec<_>>();
    if !missing.is_empty() || !unexpected.is_empty() {
        let missing_summary = missing
            .iter()
            .take(8)
            .map(|(source, digest)| format!("{source}@{}", &digest[..12]))
            .collect::<Vec<_>>()
            .join(", ");
        let unexpected_summary = unexpected
            .iter()
            .take(8)
            .map(|(source, digest)| format!("{source}@{}", &digest[..12]))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(crate::Error::invalid(format!(
            "current artifact bindings no longer reproduce revision {:?}: {} expected source/digest pair(s) are missing ({}); {} unexpected scannable source/digest pair(s) are present ({})",
            snapshot.name,
            missing.len(),
            missing_summary,
            unexpected.len(),
            unexpected_summary,
        )));
    }
    Ok(expected.len())
}

fn ledger_entry<'a>(ledger: &'a RevisionLedger, name: &str) -> Result<&'a RevisionLedgerEntry> {
    ledger
        .entries
        .iter()
        .find(|entry| entry.name == name)
        .ok_or_else(|| {
            crate::Error::invalid(format!("revision ledger has no immutable entry {name:?}"))
        })
}

fn project_relative_output(manifest: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_owned()
    } else {
        manifest
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(path)
    }
}

fn durable_snapshot_location(manifest: &Path, path: &Path) -> Result<String> {
    let root = manifest
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("revisions");
    let relative = path.strip_prefix(&root).map_err(|_| {
        crate::Error::invalid(format!(
            "durable revision snapshot {} must be stored below {}; generated/cache paths are not revision baselines",
            path.display(),
            root.display()
        ))
    })?;
    validate_relative_components(relative, "revision snapshot path")?;
    if relative == Path::new("ledger.toml") {
        return Err(crate::Error::invalid(
            "revision snapshot path cannot overwrite revisions/ledger.toml",
        ));
    }
    Ok(relative.to_string_lossy().into_owned())
}

fn snapshot_path(manifest: &Path, location: &str) -> Result<PathBuf> {
    validate_snapshot_location(location)?;
    Ok(ledger_path(manifest)
        .parent()
        .expect("ledger path always has a parent")
        .join(location))
}

fn validate_snapshot_location(location: &str) -> Result<()> {
    if location.is_empty() {
        return Err(crate::Error::invalid(
            "revision snapshot location must not be empty",
        ));
    }
    validate_relative_components(Path::new(location), "revision snapshot location")
}

fn validate_relative_components(path: &Path, label: &str) -> Result<()> {
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(crate::Error::invalid(format!(
            "{label} must be a normalized relative path without parent traversal"
        )));
    }
    Ok(())
}

fn validate_sha256(label: &str, value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(crate::Error::invalid(format!(
            "revision ledger {label} must be a lowercase SHA-256 digest"
        )));
    }
    Ok(())
}

fn encoded_snapshot_sha256(snapshot: &RevisionSnapshot) -> Result<String> {
    let mut writer = Sha256Writer(Sha256::new());
    serde_json::to_writer_pretty(&mut writer, snapshot)?;
    writer.write_all(b"\n")?;
    Ok(format!("{:x}", writer.0.finalize()))
}

fn artifacts_sha256(artifacts: &[RevisionArtifact]) -> Result<String> {
    let encoded = serde_json::to_vec(artifacts)?;
    let mut digest = Sha256::new();
    digest.update(b"blobray/revision-artifact-set/v1\0");
    digest.update(encoded);
    Ok(format!("{:x}", digest.finalize()))
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut input = BufReader::new(fs::File::open(path)?);
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

struct Sha256Writer(Sha256);

impl Write for Sha256Writer {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.0.update(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn write_ledger_atomic(manifest: &Path, ledger: &RevisionLedger) -> Result<()> {
    validate_ledger(ledger, Some(&ledger.project))?;
    let path = ledger_path(manifest);
    let parent = path.parent().expect("ledger path always has a parent");
    fs::create_dir_all(parent)?;
    let sequence = LEDGER_STAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let stage = parent.join(format!(
        ".ledger.toml.stage-{}-{sequence}",
        std::process::id()
    ));
    let result = (|| -> Result<()> {
        let mut output = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&stage)?;
        let mut encoded = toml_edit::ser::to_string_pretty(ledger)?;
        if !encoded.ends_with('\n') {
            encoded.push('\n');
        }
        output.write_all(encoded.as_bytes())?;
        output.sync_all()?;
        fs::rename(&stage, &path)?;
        sync_parent_directory(parent)?;
        Ok(())
    })();
    if stage.exists() {
        let _ = fs::remove_file(stage);
    }
    result
}

fn write_snapshot_or_check(path: &Path, snapshot: &RevisionSnapshot, check: bool) -> Result<()> {
    crate::application::generated_file::write_or_check_json(
        path,
        snapshot,
        check,
        "revision snapshot",
        true,
    )?;
    if !check {
        // The shared JSON writer publishes by atomic rename. Sync the renamed
        // file and then its directory before publishing the ledger pointer.
        // That ordering permits an orphan snapshot after a crash, never a
        // durable ledger entry whose snapshot was not durably published first.
        fs::File::open(path)?.sync_all()?;
        sync_parent_directory(path.parent().unwrap_or_else(|| Path::new(".")))?;
    }
    Ok(())
}

#[cfg(unix)]
fn sync_parent_directory(parent: &Path) -> Result<()> {
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

// Opening directories is not portable through std on every target. Atomic
// rename and file sync still apply; platforms without directory handles get
// the strongest portable guarantee available here.
#[cfg(not(unix))]
fn sync_parent_directory(_parent: &Path) -> Result<()> {
    Ok(())
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

    fn temporary_manifest(label: &str) -> (PathBuf, PathBuf) {
        let directory =
            std::env::temp_dir().join(format!("blobray-revision-{label}-{}", std::process::id()));
        if directory.exists() {
            std::fs::remove_dir_all(&directory).unwrap();
        }
        std::fs::create_dir_all(&directory).unwrap();
        let manifest = directory.join("vendor-project.toml");
        std::fs::write(&manifest, "schema = 3\nid = \"fixture\"\n").unwrap();
        (directory, manifest)
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
        let old_digest = "aa".repeat(32);
        let new_digest = "bb".repeat(32);
        let mut before = snapshot("old", vec![function("stable", "a")]);
        before.artifacts = vec![RevisionArtifact {
            source: "vendor".to_owned(),
            sha256: old_digest.clone(),
        }];
        before.assertions = vec![RevisionReviewedRecord {
            id: "fact.guarded".to_owned(),
            subject: Some("stable".to_owned()),
            record: serde_json::json!({
                "id": "fact.guarded",
                "subject": "stable",
                "applies-to": {"artifacts": [{
                    "source": "vendor",
                    "sha256": old_digest
                }]}
            }),
        }];
        let mut after = snapshot("new", vec![function("stable", "a")]);
        after.artifacts = vec![RevisionArtifact {
            source: "vendor".to_owned(),
            sha256: new_digest,
        }];

        let report = rebase(&before, &after);

        assert_eq!(report.summary.review_required, 1);
        assert!(report.records[0].reason.contains("artifact bytes absent"));
    }

    #[test]
    fn durable_snapshot_initializes_content_addressed_ledger() {
        let (directory, manifest) = temporary_manifest("ledger-init");
        let snapshot = snapshot("vendor-1", vec![function("stable", "a")]);
        let output = default_path(&manifest, &snapshot.name).unwrap();

        persist_snapshot(&manifest, &snapshot, &output, false).unwrap();
        persist_snapshot(&manifest, &snapshot, &output, true).unwrap();

        let ledger = load_ledger_optional(&manifest, Some("fixture"))
            .unwrap()
            .unwrap();
        assert_eq!(ledger.baseline.as_deref(), Some("vendor-1"));
        assert_eq!(ledger.current.as_deref(), Some("vendor-1"));
        assert_eq!(ledger.entries.len(), 1);
        assert_eq!(ledger.entries[0].snapshot, "snapshots/vendor-1.json");
        assert_eq!(ledger.entries[0].snapshot_sha256.len(), 64);
        assert_eq!(ledger.entries[0].artifacts_sha256.len(), 64);
        for path in [
            manifest.parent().unwrap().join("revisions"),
            manifest.parent().unwrap().join("revisions/snapshots"),
        ] {
            assert!(std::fs::read_dir(path).unwrap().all(|entry| {
                !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains(".stage-")
            }));
        }
        assert_eq!(
            inspect_ledger(&manifest, "fixture", true).health,
            RevisionLedgerHealth::Ready
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn immutable_revision_name_cannot_be_reused_for_different_content() {
        let (directory, manifest) = temporary_manifest("ledger-immutable");
        let output = default_path(&manifest, "vendor-1").unwrap();
        persist_snapshot(
            &manifest,
            &snapshot("vendor-1", vec![function("stable", "a")]),
            &output,
            false,
        )
        .unwrap();
        let error = persist_snapshot(
            &manifest,
            &snapshot("vendor-1", vec![function("stable", "b")]),
            &output,
            false,
        )
        .unwrap_err();
        assert!(error.to_string().contains("immutable"));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn artifact_revision_change_requires_matching_preflight_marker() {
        let (directory, manifest) = temporary_manifest("ledger-preflight");
        let mut old = snapshot("vendor-1", vec![function("stable", "a")]);
        old.artifacts.push(RevisionArtifact {
            source: "vendor".to_owned(),
            sha256: "aa".repeat(32),
        });
        persist_snapshot(
            &manifest,
            &old,
            &default_path(&manifest, "vendor-1").unwrap(),
            false,
        )
        .unwrap();
        let mut new = snapshot("vendor-2", vec![function("stable", "a")]);
        new.artifacts.push(RevisionArtifact {
            source: "vendor".to_owned(),
            sha256: "bb".repeat(32),
        });
        let output = default_path(&manifest, "vendor-2").unwrap();
        let error = persist_snapshot(&manifest, &new, &output, false).unwrap_err();
        assert!(error.to_string().contains("prepare-update"));
        assert!(!output.exists());

        let mut ledger = load_ledger_optional(&manifest, Some("fixture"))
            .unwrap()
            .unwrap();
        let current = ledger.entries[0].clone();
        ledger.prepared_update = Some(PreparedRevisionUpdate {
            from: current.name.clone(),
            snapshot_sha256: current.snapshot_sha256,
            artifacts_sha256: current.artifacts_sha256,
        });
        write_ledger_atomic(&manifest, &ledger).unwrap();
        persist_snapshot(&manifest, &new, &output, false).unwrap();
        let ledger = load_ledger_optional(&manifest, Some("fixture"))
            .unwrap()
            .unwrap();
        assert_eq!(ledger.baseline.as_deref(), Some("vendor-1"));
        assert_eq!(ledger.current.as_deref(), Some("vendor-2"));
        assert!(ledger.prepared_update.is_none());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn generated_directory_cannot_hold_a_durable_baseline() {
        let (directory, manifest) = temporary_manifest("ledger-generated");
        let error = persist_snapshot(
            &manifest,
            &snapshot("vendor-1", Vec::new()),
            &directory.join("generated/revisions/vendor-1.json"),
            false,
        )
        .unwrap_err();
        assert!(error.to_string().contains("must be stored below"));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn preflight_compares_snapshot_identities_with_live_bindings() {
        let (directory, manifest) = temporary_manifest("binding-check");
        let artifact = directory.join("vendor.elf");
        std::fs::write(&artifact, b"vendor bytes").unwrap();
        let run_path = directory.join("local.toml");
        std::fs::write(
            &run_path,
            format!(
                "schema = 1\n\n[[inputs]]\nrole = \"vendor-artifact\"\npath = {:?}\n",
                artifact
            ),
        )
        .unwrap();
        let run_spec = crate::run_spec::RunSpec::load(&run_path).unwrap();
        let mut current_snapshot = snapshot("vendor-1", Vec::new());
        current_snapshot.artifacts.push(RevisionArtifact {
            source: "vendor".to_owned(),
            sha256: crate::artifact_path_sha256(&artifact).unwrap(),
        });
        persist_snapshot(
            &manifest,
            &current_snapshot,
            &default_path(&manifest, "vendor-1").unwrap(),
            false,
        )
        .unwrap();
        assert_eq!(
            verify_current_artifact_bindings(&run_spec, &current_snapshot).unwrap(),
            1
        );
        let report =
            prepare_update_with_bindings(&manifest, "fixture", &run_spec, false, false).unwrap();
        assert_eq!(report.status, "prepared");
        assert_eq!(report.artifact_bindings_verified, 1);
        assert_eq!(
            prepare_update_with_bindings(&manifest, "fixture", &run_spec, false, true)
                .unwrap()
                .status,
            "verified"
        );
        let mut next = snapshot("vendor-2", Vec::new());
        next.artifacts = current_snapshot.artifacts.clone();
        persist_snapshot(
            &manifest,
            &next,
            &default_path(&manifest, "vendor-2").unwrap(),
            false,
        )
        .unwrap();
        assert!(
            prepare_update_with_bindings(&manifest, "fixture", &run_spec, false, false)
                .unwrap_err()
                .to_string()
                .contains("--accept-current")
        );
        let accepted =
            prepare_update_with_bindings(&manifest, "fixture", &run_spec, true, false).unwrap();
        assert_eq!(accepted.baseline, "vendor-2");

        let unexpected_artifact = directory.join("unexpected.elf");
        std::fs::write(&unexpected_artifact, b"unexpected vendor bytes").unwrap();
        std::fs::write(
            &run_path,
            format!(
                "schema = 1\n\n[[inputs]]\nrole = \"vendor-artifact\"\npath = {:?}\n\n[[inputs]]\nrole = \"source-artifact:unexpected\"\npath = {:?}\n",
                artifact, unexpected_artifact
            ),
        )
        .unwrap();
        let run_spec_with_unexpected = crate::run_spec::RunSpec::load(&run_path).unwrap();
        let unexpected =
            verify_current_artifact_bindings(&run_spec_with_unexpected, &current_snapshot)
                .unwrap_err()
                .to_string();
        assert!(unexpected.contains("unexpected scannable"));
        assert!(unexpected.contains("unexpected@"));

        std::fs::write(&artifact, b"new vendor bytes").unwrap();
        assert!(
            verify_current_artifact_bindings(&run_spec, &current_snapshot)
                .unwrap_err()
                .to_string()
                .contains("no longer reproduce")
        );
        std::fs::remove_dir_all(directory).unwrap();
    }
}
