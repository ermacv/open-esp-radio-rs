//! Portable cross-version snapshots, deterministic diffs and review rebase plans.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{BufReader, Read, Write},
    path::Component,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use flate2::{Compression, GzBuilder, read::GzDecoder};
use open_radio_vendor_contracts::{
    Applicability, ApplicabilityContext, ArtifactIdentity, FactProvenance, RevisionOccurrenceId,
    SemanticEntityId,
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

pub(crate) const REVISION_SCHEMA: u32 = 4;
pub(crate) const REVISION_DIFF_REPORT_SCHEMA: u32 = 2;
pub(crate) const REVISION_REBASE_REPORT_SCHEMA: u32 = 2;
pub(crate) const REVISION_PREPARE_UPDATE_REPORT_SCHEMA: u32 = 2;
pub(crate) const REVISION_SNAPSHOT_REPORT_SCHEMA: u32 = 2;
pub(crate) const LIVE_REVISION_SELECTOR: &str = "@live";

const REVISION_STATE_HEADER: &str = "blobray-revision-state 1";
const REVISION_CUTOVER_INSTRUCTION: &str = "remove revisions/state.blobray and create a new current state with `project revision snapshot CURRENT`";

static STATE_STAGE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RevisionSnapshot {
    pub(crate) schema_version: u32,
    pub(crate) command: String,
    pub(crate) name: String,
    pub(crate) project: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) artifact_scope: Option<RevisionArtifactScope>,
    pub(crate) artifacts: Vec<RevisionArtifact>,
    pub(crate) applicability: ApplicabilityContext,
    pub(crate) functions: Vec<RevisionFunction>,
    pub(crate) registers: Vec<RevisionRegister>,
    pub(crate) interfaces: Vec<RevisionInterface>,
    pub(crate) assertions: Vec<RevisionReviewedRecord>,
    pub(crate) vendor_bugs: Vec<RevisionReviewedRecord>,
    pub(crate) bindings: Vec<RevisionReviewedRecord>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum RevisionArtifactScope {
    VendorInputs,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RevisionArtifact {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) role: Option<String>,
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
    pub(crate) anchor: RevisionReviewedAnchor,
    pub(crate) record: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum RevisionReviewedAnchor {
    Assertion {
        subject: SemanticEntityId,
    },
    VendorBug {
        function: SemanticEntityId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        register: Option<SemanticEntityId>,
    },
    EntityBinding {
        occurrence: RevisionOccurrenceId,
        semantic: SemanticEntityId,
    },
}

impl RevisionReviewedAnchor {
    fn primary_semantic(&self) -> &SemanticEntityId {
        match self {
            Self::Assertion { subject } => subject,
            Self::VendorBug { function, .. } => function,
            Self::EntityBinding { semantic, .. } => semantic,
        }
    }

    fn semantic_entities(&self) -> impl Iterator<Item = &SemanticEntityId> {
        let (primary, secondary) = match self {
            Self::Assertion { subject } => (subject, None),
            Self::VendorBug { function, register } => (function, register.as_ref()),
            Self::EntityBinding { semantic, .. } => (semantic, None),
        };
        std::iter::once(primary).chain(secondary)
    }
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

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RevisionFunctionDelta {
    pub(crate) changed: Vec<String>,
    pub(crate) added: Vec<String>,
    pub(crate) removed: Vec<String>,
    pub(crate) remapped: Vec<RevisionFunctionRemap>,
    pub(crate) uncertain: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RevisionFunctionRemap {
    pub(crate) before: String,
    pub(crate) after: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RevisionResearchInvalidation {
    pub(crate) area: String,
    pub(crate) subjects: Vec<String>,
    pub(crate) reviewed_records: Vec<String>,
    pub(crate) reason: String,
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
    pub(crate) functions: RevisionFunctionDelta,
    pub(crate) invalidated_research: Vec<RevisionResearchInvalidation>,
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
/// The state contains content identities and relative locations only. Vendor
/// bytes, decoded instructions and analysis payloads remain in the snapshot or
/// in caller-owned artifact storage.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RevisionState {
    pub(crate) project: String,
    pub(crate) baseline: Option<String>,
    pub(crate) current: Option<String>,
    pub(crate) prepared_update: Option<PreparedRevisionUpdate>,
    pub(crate) entries: Vec<RevisionStateEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RevisionStateEntry {
    pub(crate) name: String,
    pub(crate) snapshot: String,
    pub(crate) snapshot_sha256: String,
    pub(crate) artifacts_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PreparedRevisionUpdate {
    pub(crate) from: String,
    pub(crate) snapshot_sha256: String,
    pub(crate) artifacts_sha256: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum RevisionStateHealth {
    Missing,
    BaselineMissing,
    RevisionReviewPending,
    Ready,
    Invalid,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RevisionStateInspection {
    pub(crate) path: String,
    pub(crate) health: RevisionStateHealth,
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
    pub(crate) state: String,
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
        .join(format!("{name}.json.gz")))
}

pub(crate) fn state_path(manifest: &Path) -> PathBuf {
    manifest
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("revisions/state.blobray")
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
    if let Some(state) = load_state_optional(manifest, None)?
        && let Some(entry) = state.entries.iter().find(|entry| entry.name == value)
    {
        return snapshot_path(manifest, &entry.snapshot);
    }
    default_path(manifest, value)
}

/// Load an immutable named snapshot or build a read-only snapshot from the
/// currently analyzed vendor bindings. The live projection is never persisted
/// and therefore cannot advance the durable state before review.
pub(crate) fn load_operand(session: &ProjectSession, value: &str) -> Result<RevisionSnapshot> {
    if value == LIVE_REVISION_SELECTOR {
        let mut snapshot = snapshot(session, "live-analysis")?;
        snapshot.name = LIVE_REVISION_SELECTOR.to_owned();
        return Ok(snapshot);
    }
    load(&resolve_path(&session.manifest, value)?)
}

pub(crate) fn load(path: &Path) -> Result<RevisionSnapshot> {
    let encoded =
        fs::read(path).map_err(|error| crate::Error::read("revision snapshot", path, error))?;
    let bytes = if snapshot_is_gzip(path) {
        let mut decoder = GzDecoder::new(encoded.as_slice());
        let mut decoded = Vec::new();
        decoder.read_to_end(&mut decoded).map_err(|error| {
            crate::Error::invalid(format!(
                "cannot decompress revision snapshot {}: {error}",
                path.display()
            ))
        })?;
        decoded
    } else {
        encoded
    };
    let input = String::from_utf8(bytes).map_err(|error| {
        crate::Error::invalid(format!(
            "revision snapshot {} is not UTF-8 JSON: {error}",
            path.display()
        ))
    })?;
    let envelope: serde_json::Value = serde_json::from_str(&input).map_err(|error| {
        crate::Error::manifest_source("revision snapshot", path, &input, error, None)
    })?;
    if envelope
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        != Some(u64::from(REVISION_SCHEMA))
    {
        return Err(crate::Error::invalid(format!(
            "revision snapshot {} is not current schema {REVISION_SCHEMA}; {REVISION_CUTOVER_INSTRUCTION}",
            path.display()
        )));
    }
    let snapshot: RevisionSnapshot = serde_json::from_value(envelope).map_err(|error| {
        crate::Error::manifest_source("revision snapshot", path, &input, error, None)
    })?;
    validate_snapshot(&snapshot)?;
    Ok(snapshot)
}

/// Persist one immutable snapshot and advance the durable revision pointers.
///
/// The snapshot is atomically published before the state. On Unix both file
/// and parent-directory metadata are synced in that order, so a crash can
/// leave an unreferenced immutable snapshot but not a durable state entry for
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
    // The durable identity belongs to normalized snapshot content, not to its
    // storage codec. A compressor upgrade may legally change gzip bytes while
    // leaving every reviewed fact unchanged.
    let snapshot_sha256 = logical_snapshot_sha256(snapshot)?;
    let artifacts_sha256 = artifacts_sha256(&snapshot.artifacts)?;
    let entry = RevisionStateEntry {
        name: snapshot.name.clone(),
        snapshot: location,
        snapshot_sha256,
        artifacts_sha256,
    };

    let mut state =
        load_state_optional(manifest, Some(&snapshot.project))?.unwrap_or_else(|| RevisionState {
            project: snapshot.project.clone(),
            baseline: None,
            current: None,
            prepared_update: None,
            entries: Vec::new(),
        });
    if check {
        let stored = state
            .entries
            .iter()
            .find(|stored| stored.name == entry.name)
            .ok_or_else(|| {
                crate::Error::invalid(format!(
                    "revision state {} has no immutable entry {:?}",
                    state_path(manifest).display(),
                    entry.name
                ))
            })?;
        if stored != &entry {
            return Err(crate::Error::invalid(format!(
                "revision state entry {:?} differs from the requested snapshot; revision names are immutable",
                entry.name
            )));
        }
        write_snapshot_or_check(&path, snapshot, true)?;
        verify_state_entry(manifest, &state, stored)?;
        return Ok(());
    }

    if let Some(stored) = state
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
        verify_state_entry(manifest, &state, stored)?;
        return Ok(());
    }

    if let Some(current) = state.current.as_deref() {
        let previous = state_entry(&state, current)?;
        verify_state_entry(manifest, &state, previous)?;
        if previous.artifacts_sha256 != entry.artifacts_sha256 {
            let prepared = state.prepared_update.as_ref().ok_or_else(|| {
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
        state.baseline = Some(current.to_owned());
    } else {
        state.baseline = Some(entry.name.clone());
    }
    state.current = Some(entry.name.clone());
    state.prepared_update = None;
    state.entries.push(entry);
    state
        .entries
        .sort_by(|left, right| left.name.cmp(&right.name));
    validate_state(&state, Some(&snapshot.project))?;

    write_snapshot_or_check(&path, snapshot, false)?;
    write_state_atomic(manifest, &state)?;
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
    let mut state = load_state_optional(manifest, Some(project))?
        .ok_or_else(|| {
            crate::Error::invalid(format!(
                "durable revision state is missing at {}; run `project revision snapshot BASELINE` before replacing artifact bindings",
                state_path(manifest).display()
            ))
        })?;
    validate_state_files(manifest, &state)?;
    let current = state.current.clone().ok_or_else(|| {
        crate::Error::invalid(
            "revision state has no current revision; snapshot the current vendor inputs first",
        )
    })?;
    let baseline = state.baseline.clone().ok_or_else(|| {
        crate::Error::invalid(
            "revision state has no baseline; snapshot the current vendor inputs first",
        )
    })?;
    if baseline != current && !accept_current {
        return Err(crate::Error::invalid(format!(
            "revision {current:?} has not been accepted as the next baseline (state baseline is {baseline:?}); finish diff/rebase review, then rerun with --accept-current"
        )));
    }
    let entry = state_entry(&state, &current)?.clone();
    let snapshot = load(&snapshot_path(manifest, &entry.snapshot)?)?;
    let verified = verify_current_artifact_bindings(run_spec, &snapshot)?;
    let prepared = PreparedRevisionUpdate {
        from: current.clone(),
        snapshot_sha256: entry.snapshot_sha256.clone(),
        artifacts_sha256: entry.artifacts_sha256.clone(),
    };
    if check {
        if accept_current && state.baseline.as_deref() != Some(current.as_str()) {
            return Err(crate::Error::invalid(format!(
                "revision {current:?} has not been accepted in {}; rerun without --check",
                state_path(manifest).display()
            )));
        }
        if state.prepared_update.as_ref() != Some(&prepared)
            || state.baseline.as_deref() != Some(current.as_str())
        {
            return Err(crate::Error::invalid(format!(
                "revision update is not prepared in {}; rerun without --check after completing review",
                state_path(manifest).display()
            )));
        }
    } else {
        if accept_current {
            state.baseline = Some(current.clone());
        }
        state.prepared_update = Some(prepared);
        write_state_atomic(manifest, &state)?;
    }
    Ok(RevisionPrepareUpdateReport {
        schema_version: REVISION_PREPARE_UPDATE_REPORT_SCHEMA,
        command: "revision prepare-update".to_owned(),
        status: if check { "verified" } else { "prepared" }.to_owned(),
        state: state_path(manifest).display().to_string(),
        baseline: state
            .baseline
            .clone()
            .expect("baseline established before report"),
        current,
        snapshot_sha256: entry.snapshot_sha256,
        artifacts_sha256: entry.artifacts_sha256,
        artifact_bindings_verified: verified,
    })
}

pub(crate) fn inspect_state(manifest: &Path, project: &str, deep: bool) -> RevisionStateInspection {
    let path = state_path(manifest);
    let result = (|| -> Result<RevisionStateInspection> {
        let Some(state) = load_state_optional(manifest, Some(project))? else {
            return Ok(RevisionStateInspection {
                path: path.display().to_string(),
                health: RevisionStateHealth::Missing,
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
        let health = if state.baseline.is_none() || state.current.is_none() {
            RevisionStateHealth::BaselineMissing
        } else {
            if deep {
                validate_state_files(manifest, &state)?;
            }
            let current = state
                .current
                .as_deref()
                .expect("current checked before snapshot scope inspection");
            let entry = state_entry(&state, current)?;
            load(&snapshot_path(manifest, &entry.snapshot)?)?;
            if state.baseline.as_deref() != Some(current) {
                RevisionStateHealth::RevisionReviewPending
            } else {
                RevisionStateHealth::Ready
            }
        };
        let diagnostic = match health {
            RevisionStateHealth::BaselineMissing => Some(
                "revision state has no baseline/current snapshot; run `project revision snapshot BASELINE` before replacing artifact bindings"
                    .to_owned(),
            ),
            RevisionStateHealth::RevisionReviewPending => Some(
                "current revision has not been accepted as the new baseline; review the baseline/current diff and rebase, then run `project revision prepare-update --accept-current`"
                    .to_owned(),
            ),
            _ => None,
        };
        Ok(RevisionStateInspection {
            path: path.display().to_string(),
            health,
            baseline: state.baseline.clone(),
            current: state.current.clone(),
            revisions: state.entries.len(),
            update_prepared: state.prepared_update.is_some(),
            diagnostic,
        })
    })();
    result.unwrap_or_else(|error| RevisionStateInspection {
        path: path.display().to_string(),
        health: RevisionStateHealth::Invalid,
        baseline: None,
        current: None,
        revisions: 0,
        update_prepared: false,
        diagnostic: Some(error.to_string()),
    })
}

pub(crate) fn verify_state_bindings_from_context(context: &ProjectContext<'_>) -> Result<usize> {
    let state = load_state_optional(context.project_path, Some(&context.project.id))?
        .ok_or_else(|| crate::Error::invalid("durable revision state is missing"))?;
    let current = state
        .current
        .as_deref()
        .ok_or_else(|| crate::Error::invalid("revision state has no current snapshot"))?;
    let entry = state_entry(&state, current)?;
    let snapshot = load(&snapshot_path(context.project_path, &entry.snapshot)?)?;
    let run_spec = context
        .run_spec
        .ok_or_else(|| crate::Error::invalid("current run spec is missing"))?;
    verify_current_artifact_bindings(run_spec, &snapshot)
}

fn load_state_optional(
    manifest: &Path,
    expected_project: Option<&str>,
) -> Result<Option<RevisionState>> {
    let path = state_path(manifest);
    if !path.is_file() {
        return Ok(None);
    }
    let input = fs::read_to_string(&path)
        .map_err(|error| crate::Error::read("revision state", &path, error))?;
    let state = decode_revision_state(&path, &input)?;
    validate_state(&state, expected_project)?;
    Ok(Some(state))
}

fn decode_revision_state(path: &Path, input: &str) -> Result<RevisionState> {
    let mut lines = input.lines().enumerate();
    let Some((_, header)) = lines.next() else {
        return Err(crate::Error::invalid(format!(
            "revision state {} is empty; {REVISION_CUTOVER_INSTRUCTION}",
            path.display()
        )));
    };
    if header != REVISION_STATE_HEADER {
        return Err(crate::Error::invalid(format!(
            "revision state {} must start with {REVISION_STATE_HEADER:?}; {REVISION_CUTOVER_INSTRUCTION}",
            path.display()
        )));
    }

    let mut project = None;
    let mut baseline = None;
    let mut current = None;
    let mut prepared_update = None;
    let mut entries = Vec::new();
    for (line_index, line) in lines {
        if line.is_empty() {
            continue;
        }
        let Some((directive, arguments)) = line.split_once(char::is_whitespace) else {
            return Err(revision_state_line_error(
                path,
                line_index,
                "a directive followed by a JSON string array",
            ));
        };
        let arguments = arguments.trim_start();
        let values: Vec<String> = serde_json::from_str(arguments).map_err(|error| {
            revision_state_line_error(
                path,
                line_index,
                &format!("a JSON string array after {directive:?}: {error}"),
            )
        })?;
        match directive {
            "project" => {
                require_revision_state_arity(path, line_index, directive, &values, 1)?;
                if project.replace(values[0].clone()).is_some() {
                    return Err(revision_state_line_error(
                        path,
                        line_index,
                        "exactly one project directive",
                    ));
                }
            }
            "baseline" => {
                require_revision_state_arity(path, line_index, directive, &values, 1)?;
                if baseline.replace(values[0].clone()).is_some() {
                    return Err(revision_state_line_error(
                        path,
                        line_index,
                        "at most one baseline directive",
                    ));
                }
            }
            "current" => {
                require_revision_state_arity(path, line_index, directive, &values, 1)?;
                if current.replace(values[0].clone()).is_some() {
                    return Err(revision_state_line_error(
                        path,
                        line_index,
                        "at most one current directive",
                    ));
                }
            }
            "revision" => {
                require_revision_state_arity(path, line_index, directive, &values, 4)?;
                entries.push(RevisionStateEntry {
                    name: values[0].clone(),
                    snapshot: values[1].clone(),
                    snapshot_sha256: values[2].clone(),
                    artifacts_sha256: values[3].clone(),
                });
            }
            "prepared-update" => {
                require_revision_state_arity(path, line_index, directive, &values, 3)?;
                let prepared = PreparedRevisionUpdate {
                    from: values[0].clone(),
                    snapshot_sha256: values[1].clone(),
                    artifacts_sha256: values[2].clone(),
                };
                if prepared_update.replace(prepared).is_some() {
                    return Err(revision_state_line_error(
                        path,
                        line_index,
                        "at most one prepared-update directive",
                    ));
                }
            }
            _ => {
                return Err(revision_state_line_error(
                    path,
                    line_index,
                    &format!("a known revision-state directive, not {directive:?}"),
                ));
            }
        }
    }
    let project = project.ok_or_else(|| {
        crate::Error::invalid(format!(
            "revision state {} has no project directive",
            path.display()
        ))
    })?;
    Ok(RevisionState {
        project,
        baseline,
        current,
        prepared_update,
        entries,
    })
}

fn require_revision_state_arity(
    path: &Path,
    line_index: usize,
    directive: &str,
    values: &[String],
    expected: usize,
) -> Result<()> {
    if values.len() != expected {
        return Err(revision_state_line_error(
            path,
            line_index,
            &format!(
                "{directive:?} with {expected} string arguments, got {}",
                values.len()
            ),
        ));
    }
    Ok(())
}

fn revision_state_line_error(path: &Path, line_index: usize, expected: &str) -> crate::Error {
    crate::Error::invalid(format!(
        "revision state {} line {} requires {expected}",
        path.display(),
        line_index + 1
    ))
}

fn validate_state(state: &RevisionState, expected_project: Option<&str>) -> Result<()> {
    if state.project.is_empty() {
        return Err(crate::Error::invalid(
            "revision state requires a non-empty project identity",
        ));
    }
    if expected_project.is_some_and(|project| project != state.project) {
        return Err(crate::Error::invalid(format!(
            "revision state project {:?} does not match project {:?}",
            state.project,
            expected_project.unwrap_or_default()
        )));
    }
    let mut names = BTreeSet::new();
    for entry in &state.entries {
        validate_revision_name(&entry.name)?;
        if !names.insert(entry.name.as_str()) {
            return Err(crate::Error::invalid(format!(
                "revision state contains duplicate revision {:?}",
                entry.name
            )));
        }
        validate_snapshot_location(&entry.snapshot)?;
        validate_sha256("snapshot-sha256", &entry.snapshot_sha256)?;
        validate_sha256("artifacts-sha256", &entry.artifacts_sha256)?;
    }
    if !state
        .entries
        .windows(2)
        .all(|pair| pair[0].name < pair[1].name)
    {
        return Err(crate::Error::invalid(
            "revision state entries must be sorted by unique revision name",
        ));
    }
    for (label, name) in [
        ("baseline", state.baseline.as_deref()),
        ("current", state.current.as_deref()),
    ] {
        if let Some(name) = name
            && !names.contains(name)
        {
            return Err(crate::Error::invalid(format!(
                "revision state {label} {name:?} does not name an immutable revision entry"
            )));
        }
    }
    if state.baseline.is_some() != state.current.is_some() {
        return Err(crate::Error::invalid(
            "revision state baseline and current must either both be set or both be absent",
        ));
    }
    if let Some(prepared) = &state.prepared_update {
        validate_sha256("prepared-update.snapshot-sha256", &prepared.snapshot_sha256)?;
        validate_sha256(
            "prepared-update.artifacts-sha256",
            &prepared.artifacts_sha256,
        )?;
        if state.current.as_deref() != Some(prepared.from.as_str()) {
            return Err(crate::Error::invalid(
                "revision state prepared-update must identify the current revision",
            ));
        }
        let entry = state_entry(state, &prepared.from)?;
        if prepared.snapshot_sha256 != entry.snapshot_sha256
            || prepared.artifacts_sha256 != entry.artifacts_sha256
        {
            return Err(crate::Error::invalid(
                "revision state prepared-update digests do not match the current immutable revision",
            ));
        }
    }
    Ok(())
}

fn validate_state_files(manifest: &Path, state: &RevisionState) -> Result<()> {
    for entry in &state.entries {
        verify_state_entry(manifest, state, entry)?;
    }
    Ok(())
}

fn verify_state_entry(
    manifest: &Path,
    state: &RevisionState,
    entry: &RevisionStateEntry,
) -> Result<()> {
    let path = snapshot_path(manifest, &entry.snapshot)?;
    let snapshot = load(&path)?;
    let actual = logical_snapshot_sha256(&snapshot)?;
    if actual != entry.snapshot_sha256 {
        return Err(crate::Error::invalid(format!(
            "immutable revision snapshot {} logical digest differs from state entry {:?}",
            path.display(),
            entry.name
        )));
    }
    if snapshot.name != entry.name || snapshot.project != state.project {
        return Err(crate::Error::invalid(format!(
            "immutable revision snapshot {} identity does not match its state entry",
            path.display()
        )));
    }
    if artifacts_sha256(&snapshot.artifacts)? != entry.artifacts_sha256 {
        return Err(crate::Error::invalid(format!(
            "immutable revision snapshot {} artifact-set digest differs from its state entry",
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
    let expected_owned = snapshot.artifacts.iter().cloned().collect::<BTreeSet<_>>();
    let actual_owned = current_revision_artifacts(run_spec)?;
    let missing = expected_owned
        .difference(&actual_owned)
        .cloned()
        .collect::<Vec<_>>();
    let unexpected = actual_owned
        .difference(&expected_owned)
        .cloned()
        .collect::<Vec<_>>();
    if !missing.is_empty() || !unexpected.is_empty() {
        let missing_summary = missing
            .iter()
            .take(8)
            .map(revision_artifact_summary)
            .collect::<Vec<_>>()
            .join(", ");
        let unexpected_summary = unexpected
            .iter()
            .take(8)
            .map(revision_artifact_summary)
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
    Ok(expected_owned.len())
}

fn revision_artifact_summary(artifact: &RevisionArtifact) -> String {
    format!(
        "{}:{}@{}",
        artifact.role.as_deref().unwrap_or("projection"),
        artifact.source,
        artifact.sha256.get(..12).unwrap_or(&artifact.sha256)
    )
}

fn revision_source_sets(
    run_spec: &crate::run_spec::RunSpec,
) -> Result<(BTreeSet<String>, BTreeSet<String>)> {
    let vendor_sources = run_spec
        .inputs()
        .iter()
        .filter(|input| input.role.is_revision_owned())
        .map(|input| input.role.source_id().to_owned())
        .collect::<BTreeSet<_>>();
    let rust_sources = run_spec
        .inputs()
        .iter()
        .filter(|input| input.role.is_rust_lineage())
        .map(|input| input.role.source_id().to_owned())
        .collect::<BTreeSet<_>>();
    let ambiguous = run_spec
        .inputs()
        .iter()
        .filter(|input| input.role.is_ambiguous_lineage())
        .map(|input| input.role.to_string())
        .collect::<BTreeSet<_>>();
    if !ambiguous.is_empty() {
        return Err(crate::Error::invalid(format!(
            "revision workflow requires typed vendor or Rust roles; ambiguous scannable role(s): {}",
            ambiguous.into_iter().collect::<Vec<_>>().join(", ")
        )));
    }
    let overlaps = vendor_sources
        .intersection(&rust_sources)
        .cloned()
        .collect::<Vec<_>>();
    if !overlaps.is_empty() {
        return Err(crate::Error::invalid(format!(
            "revision source id(s) are shared by vendor and Rust artifact roles: {}; use distinct source ids so vendor lineage is unambiguous",
            overlaps.join(", ")
        )));
    }
    Ok((vendor_sources, rust_sources))
}

fn current_revision_artifacts(
    run_spec: &crate::run_spec::RunSpec,
) -> Result<BTreeSet<RevisionArtifact>> {
    Ok(current_revision_artifacts_with_digests(run_spec)?.0)
}

fn current_revision_artifacts_with_digests(
    run_spec: &crate::run_spec::RunSpec,
) -> Result<(BTreeSet<RevisionArtifact>, BTreeMap<PathBuf, String>)> {
    revision_source_sets(run_spec)?;
    let mut artifacts = BTreeSet::new();
    let mut digests = BTreeMap::<PathBuf, String>::new();
    for input in run_spec
        .inputs()
        .iter()
        .filter(|input| input.role.is_revision_owned())
    {
        let sha256 = if let Some(sha256) = digests.get(&input.path) {
            sha256.clone()
        } else {
            let sha256 = hash_revision_input(input)?;
            digests.insert(input.path.clone(), sha256.clone());
            sha256
        };
        artifacts.insert(RevisionArtifact {
            role: Some(input.role.to_string()),
            source: input.role.source_id().to_owned(),
            sha256,
        });
    }
    Ok((artifacts, digests))
}

fn hash_revision_input(input: &crate::run_spec::RunInput) -> Result<String> {
    if input.path.is_file() {
        sha256_file(&input.path).map_err(|error| {
            crate::Error::invalid(format!(
                "cannot hash current vendor artifact binding {} at {}: {error}",
                input.role,
                input.path.display()
            ))
        })
    } else {
        crate::artifact_path_sha256(&input.path).map_err(|error| {
            crate::Error::invalid(format!(
                "cannot hash current vendor artifact binding {} at {}: {error}",
                input.role,
                input.path.display()
            ))
        })
    }
}

fn validate_vendor_projection_artifacts(
    label: &str,
    observed: &[RevisionArtifact],
    live: &BTreeSet<RevisionArtifact>,
    vendor_sources: &BTreeSet<String>,
    inventories_allowed: bool,
) -> Result<()> {
    if observed.is_empty() {
        return Err(crate::Error::invalid(format!(
            "{label} contain no artifact provenance for a vendor revision snapshot"
        )));
    }
    for artifact in observed {
        if !vendor_sources.contains(&artifact.source) {
            return Err(crate::Error::invalid(format!(
                "{label} contain non-vendor source {:?}; regenerate a vendor-only projection before snapshotting",
                artifact.source
            )));
        }
        let current = if inventories_allowed {
            live_revision_analysis_input_matches(live, &artifact.source, &artifact.sha256)
        } else {
            live_revision_primary_matches(live, &artifact.source, &artifact.sha256)
        };
        if !current {
            return Err(crate::Error::invalid(format!(
                "{label} retain stale vendor artifact identity {}@{}; rerun project analysis against the current bindings",
                artifact.source,
                artifact.sha256.get(..12).unwrap_or(&artifact.sha256)
            )));
        }
    }
    Ok(())
}

fn live_revision_primary_matches(
    live: &BTreeSet<RevisionArtifact>,
    source: &str,
    sha256: &str,
) -> bool {
    live.iter().any(|artifact| {
        artifact.source == source
            && artifact.sha256 == sha256
            && artifact
                .role
                .as_deref()
                .and_then(crate::run_spec::InputRole::parse)
                .is_some_and(|role| role.is_revision_primary())
    })
}

fn live_revision_analysis_input_matches(
    live: &BTreeSet<RevisionArtifact>,
    source: &str,
    sha256: &str,
) -> bool {
    live.iter().any(|artifact| {
        artifact.source == source
            && artifact.sha256 == sha256
            && artifact
                .role
                .as_deref()
                .and_then(crate::run_spec::InputRole::parse)
                .is_some_and(|role| role.is_revision_primary() || role.is_revision_inventory())
    })
}

fn validate_linked_ir_inventories(
    profile: &crate::project_ir::ProjectIrProfile,
    observed: &[(String, String)],
    run_spec: &crate::run_spec::RunSpec,
    live_digests: &BTreeMap<PathBuf, String>,
) -> Result<()> {
    let expected = run_spec
        .inputs()
        .iter()
        .filter(|input| {
            input.role.is_revision_inventory()
                && (profile.sources.is_empty()
                    || profile
                        .sources
                        .iter()
                        .any(|source| source == input.role.source_id()))
        })
        .map(|input| {
            Ok((
                input.role.source_id().to_owned(),
                live_digests.get(&input.path).cloned().ok_or_else(|| {
                    crate::Error::invalid(format!(
                        "current inventory binding {} was not included in vendor revision provenance",
                        input.role
                    ))
                })?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    let expected = counted_identities(expected);
    let actual = counted_identities(observed.to_vec());
    if actual != expected {
        return Err(crate::Error::invalid(format!(
            "linked-IR profile {:?} inventory provenance differs from the current typed bindings; rerun project analysis",
            profile.id
        )));
    }
    Ok(())
}

fn counted_identities(values: Vec<(String, String)>) -> BTreeMap<(String, String), usize> {
    let mut counts = BTreeMap::new();
    for value in values {
        *counts.entry(value).or_default() += 1;
    }
    counts
}

fn validate_linked_ir_companions(
    profile: &crate::project_ir::ProjectIrProfile,
    primary_sources: &BTreeSet<String>,
    observed: &[(String, String)],
    run_spec: &crate::run_spec::RunSpec,
    live_digests: &BTreeMap<PathBuf, String>,
) -> Result<()> {
    let expected = if primary_sources.len() == 1 {
        let source = primary_sources
            .first()
            .expect("one primary source established above");
        run_spec
            .inputs()
            .iter()
            .filter(|input| {
                input.role.is_revision_companion() && input.role.source_id() == source
            })
            .map(|input| {
                Ok((
                    input.path.display().to_string(),
                    live_digests.get(&input.path).cloned().ok_or_else(|| {
                        crate::Error::invalid(format!(
                            "current companion binding {} was not included in vendor revision provenance",
                            input.role
                        ))
                    })?,
                ))
            })
            .collect::<Result<BTreeSet<_>>>()?
    } else {
        BTreeSet::new()
    };
    let actual = observed.iter().cloned().collect::<BTreeSet<_>>();
    if actual.len() != observed.len() {
        return Err(crate::Error::invalid(format!(
            "linked-IR profile {:?} contains duplicate companion provenance",
            profile.id
        )));
    }
    if actual != expected {
        let missing = expected
            .difference(&actual)
            .take(4)
            .cloned()
            .collect::<Vec<_>>();
        let unexpected = actual
            .difference(&expected)
            .take(4)
            .cloned()
            .collect::<Vec<_>>();
        return Err(crate::Error::invalid(format!(
            "linked-IR profile {:?} companion provenance differs from the current typed bindings (missing: {missing:?}; unexpected: {unexpected:?}); rerun project analysis",
            profile.id
        )));
    }
    Ok(())
}

fn state_entry<'a>(state: &'a RevisionState, name: &str) -> Result<&'a RevisionStateEntry> {
    state
        .entries
        .iter()
        .find(|entry| entry.name == name)
        .ok_or_else(|| {
            crate::Error::invalid(format!("revision state has no immutable entry {name:?}"))
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
    if relative == Path::new("state.blobray") {
        return Err(crate::Error::invalid(
            "revision snapshot path cannot overwrite revisions/state.blobray",
        ));
    }
    Ok(relative.to_string_lossy().into_owned())
}

fn snapshot_path(manifest: &Path, location: &str) -> Result<PathBuf> {
    validate_snapshot_location(location)?;
    Ok(state_path(manifest)
        .parent()
        .expect("state path always has a parent")
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
            "revision state {label} must be a lowercase SHA-256 digest"
        )));
    }
    Ok(())
}

fn snapshot_is_gzip(path: &Path) -> bool {
    path.extension().is_some_and(|extension| extension == "gz")
}

fn encode_snapshot(snapshot: &RevisionSnapshot, path: &Path) -> Result<Vec<u8>> {
    let mut json = serde_json::to_vec_pretty(snapshot)?;
    json.push(b'\n');
    if !snapshot_is_gzip(path) {
        return Ok(json);
    }
    let mut encoder = GzBuilder::new()
        .mtime(0)
        .write(Vec::new(), Compression::default());
    encoder.write_all(&json)?;
    Ok(encoder.finish()?)
}

fn logical_snapshot_sha256(snapshot: &RevisionSnapshot) -> Result<String> {
    let encoded = serde_json::to_vec(snapshot)?;
    let mut digest = Sha256::new();
    digest.update(encoded);
    Ok(format!("{:x}", digest.finalize()))
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

fn write_state_atomic(manifest: &Path, state: &RevisionState) -> Result<()> {
    validate_state(state, Some(&state.project))?;
    let path = state_path(manifest);
    let parent = path.parent().expect("state path always has a parent");
    fs::create_dir_all(parent)?;
    let sequence = STATE_STAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let stage = parent.join(format!(
        ".state.blobray.stage-{}-{sequence}",
        std::process::id()
    ));
    let result = (|| -> Result<()> {
        let mut output = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&stage)?;
        let encoded = encode_revision_state(state)?;
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

fn encode_revision_state(state: &RevisionState) -> Result<String> {
    validate_state(state, Some(&state.project))?;
    let mut encoded = String::from(REVISION_STATE_HEADER);
    encoded.push('\n');
    push_revision_state_directive(&mut encoded, "project", &[&state.project])?;
    if let Some(baseline) = state.baseline.as_deref() {
        push_revision_state_directive(&mut encoded, "baseline", &[baseline])?;
    }
    if let Some(current) = state.current.as_deref() {
        push_revision_state_directive(&mut encoded, "current", &[current])?;
    }
    for entry in &state.entries {
        push_revision_state_directive(
            &mut encoded,
            "revision",
            &[
                &entry.name,
                &entry.snapshot,
                &entry.snapshot_sha256,
                &entry.artifacts_sha256,
            ],
        )?;
    }
    if let Some(prepared) = &state.prepared_update {
        push_revision_state_directive(
            &mut encoded,
            "prepared-update",
            &[
                &prepared.from,
                &prepared.snapshot_sha256,
                &prepared.artifacts_sha256,
            ],
        )?;
    }
    Ok(encoded)
}

fn push_revision_state_directive(
    output: &mut String,
    directive: &str,
    values: &[&str],
) -> Result<()> {
    output.push_str(directive);
    output.push(' ');
    output.push_str(&serde_json::to_string(values)?);
    output.push('\n');
    Ok(())
}

fn write_snapshot_or_check(path: &Path, snapshot: &RevisionSnapshot, check: bool) -> Result<()> {
    if check {
        let existing = load(path)?;
        if existing != *snapshot {
            return Err(crate::Error::invalid(format!(
                "generated revision snapshot differs from {}; rerun without --check",
                path.display()
            )));
        }
        return Ok(());
    }
    let encoded = encode_snapshot(snapshot, path)?;
    crate::application::generated_file::write_or_check_bytes(
        path,
        &encoded,
        false,
        "revision snapshot",
    )?;
    // The shared binary writer publishes by atomic rename. Sync the renamed
    // file and then its directory before publishing the state pointer. That
    // ordering permits an orphan snapshot after a crash, never a durable
    // state entry whose snapshot was not durably published first.
    fs::File::open(path)?.sync_all()?;
    sync_parent_directory(path.parent().unwrap_or_else(|| Path::new(".")))?;
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
    session.validate_active_artifacts()?;
    let run_spec = session.run_spec.as_ref().ok_or_else(|| {
        crate::Error::invalid("revision snapshot requires the current caller-owned run spec")
    })?;
    let (artifacts, live_digests) = current_revision_artifacts_with_digests(run_spec)?;
    if artifacts.is_empty() {
        return Err(crate::Error::invalid(
            "revision snapshot requires at least one vendor artifact or inventory binding",
        ));
    }
    let (vendor_sources, rust_sources) = revision_source_sets(run_spec)?;
    let (function_artifacts, functions) = snapshot_functions(session, run_spec, &live_digests)?;
    validate_vendor_projection_artifacts(
        "linked-IR",
        &function_artifacts,
        &artifacts,
        &vendor_sources,
        false,
    )?;
    if let Some(function) = functions
        .iter()
        .find(|function| !vendor_sources.contains(&function.source))
    {
        return Err(crate::Error::invalid(format!(
            "linked-IR revision projection contains non-vendor function {:?} from source {:?}",
            function.id, function.source
        )));
    }
    let registers = snapshot_registers(session, &artifacts, &vendor_sources)?;
    let (_, interfaces) = snapshot_interfaces(session, &artifacts, &vendor_sources, &rust_sources)?;
    let applicability = snapshot_applicability_context(session, &artifacts)?;
    let knowledge = open_radio_vendor_review::ReviewKnowledge::load_all(
        &session.project.reviewed_knowledge,
    )
    .and_then(|knowledge| knowledge.select_for(&applicability))
    .map_err(|error| {
        crate::Error::invalid(format!(
            "cannot snapshot reviewed knowledge for the authenticated active inputs: {error}"
        ))
    })?;
    let assertions = knowledge
        .assertions()
        .values()
        .map(|assertion| {
            Ok(RevisionReviewedRecord {
                id: assertion.id.clone(),
                anchor: RevisionReviewedAnchor::Assertion {
                    subject: assertion.subject.clone(),
                },
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
                anchor: RevisionReviewedAnchor::VendorBug {
                    function: bug.function.clone(),
                    register: bug.register.clone(),
                },
                record: serde_json::to_value(bug)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let bindings = knowledge
        .bindings()
        .values()
        .map(|binding| {
            Ok(RevisionReviewedRecord {
                id: binding.id.clone(),
                anchor: RevisionReviewedAnchor::EntityBinding {
                    occurrence: binding.occurrence.clone(),
                    semantic: binding.semantic.clone(),
                },
                record: serde_json::to_value(binding)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let snapshot = RevisionSnapshot {
        schema_version: REVISION_SCHEMA,
        command: "revision snapshot".to_owned(),
        name: name.to_owned(),
        project: session.project.id.clone(),
        artifact_scope: Some(RevisionArtifactScope::VendorInputs),
        artifacts: artifacts.into_iter().collect(),
        applicability,
        functions,
        registers,
        interfaces,
        assertions,
        vendor_bugs,
        bindings,
    };
    validate_snapshot(&snapshot)?;
    Ok(snapshot)
}

fn snapshot_applicability_context(
    session: &ProjectSession,
    artifacts: &BTreeSet<RevisionArtifact>,
) -> Result<ApplicabilityContext> {
    let exact = artifacts
        .iter()
        .map(|artifact| ArtifactIdentity::new(&artifact.source, &artifact.sha256))
        .collect::<std::result::Result<BTreeSet<_>, _>>()
        .map_err(|error| crate::Error::invalid(error.to_string()))?
        .into_iter()
        .collect();
    ApplicabilityContext::new(
        session.project.review_context.ecosystems.clone(),
        session.project.review_context.chips.clone(),
        session.project.review_context.chip_revisions.clone(),
        session.project.review_context.artifact_lineages.clone(),
        exact,
    )
    .map_err(|error| crate::Error::invalid(error.to_string()))
}

fn snapshot_functions(
    session: &ProjectSession,
    run_spec: &crate::run_spec::RunSpec,
    live_digests: &BTreeMap<PathBuf, String>,
) -> Result<(Vec<RevisionArtifact>, Vec<RevisionFunction>)> {
    let mut artifacts = BTreeSet::new();
    let mut functions = BTreeMap::<String, RevisionFunction>::new();
    for profile in &session.project.ir_profiles {
        let reader = LinkedIrReader::open(&profile.output)?;
        let projection = reader.read_review_projection()?;
        let primary_sources = projection
            .inputs
            .iter()
            .map(|(source, _)| source.clone())
            .collect::<BTreeSet<_>>();
        validate_linked_ir_companions(
            profile,
            &primary_sources,
            &projection.companions,
            run_spec,
            live_digests,
        )?;
        validate_linked_ir_inventories(profile, &projection.inventories, run_spec, live_digests)?;
        artifacts.extend(
            projection
                .inputs
                .into_iter()
                .map(|(source, sha256)| RevisionArtifact {
                    role: None,
                    source,
                    sha256,
                }),
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

fn snapshot_registers(
    session: &ProjectSession,
    revision_artifacts: &BTreeSet<RevisionArtifact>,
    vendor_sources: &BTreeSet<String>,
) -> Result<Vec<RevisionRegister>> {
    let Some(paths) = session.project.registers.as_ref() else {
        return Ok(Vec::new());
    };
    let facts = RegisterFacts::load(&paths.facts)?;
    let fact_artifacts = facts
        .artifacts
        .iter()
        .map(|artifact| RevisionArtifact {
            role: None,
            source: artifact.source.clone(),
            sha256: artifact.sha256.clone(),
        })
        .collect::<Vec<_>>();
    validate_vendor_projection_artifacts(
        "MMIO facts",
        &fact_artifacts,
        revision_artifacts,
        vendor_sources,
        true,
    )?;
    let model = load_effective_register_model(paths)?;
    let projections = model.register_projections()?;
    let identities = projections
        .iter()
        .map(|(key, projection)| (*key, projection.identity.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut reviewed_features = BTreeMap::<(u64, u32), Vec<String>>::new();
    for (key, projection) in projections {
        if let Some(annotation) = projection.review {
            let encoded = serde_json::to_string(&annotation)?;
            reviewed_features
                .entry(key)
                .or_default()
                .push(format!("reviewed-model:{}", fingerprint(&[encoded])?));
        }
    }
    for assertion in model.reviewed_register_facts() {
        let key = match &assertion.subject {
            SemanticEntityId::Register { address, width, .. } => (*address, *width),
            SemanticEntityId::RegisterField {
                address,
                register_width,
                ..
            } => (*address, *register_width),
            _ => continue,
        };
        let encoded = serde_json::to_string(assertion)?;
        reviewed_features
            .entry(key)
            .or_default()
            .push(format!("reviewed:{}", fingerprint(&[encoded])?));
    }
    let encode_id = |address: u64, width: u32| {
        SemanticEntityId::register(model.chip(), model.address_space(), address, width)
            .map(|identity| identity.to_string())
            .map_err(|error| {
                crate::Error::invalid(format!(
                    "cannot encode revision register identity at {address:#010x}: {error}"
                ))
            })
    };
    let mut registers = BTreeMap::<(u64, u32), RevisionRegister>::new();
    for register in facts.registers {
        let key = (u64::from(register.address), u32::from(register.width));
        let name = identities.get(&key).cloned();
        let mut features = vec![
            "observed".to_owned(),
            format!("reads:{}", register.reads),
            format!("writes:{}", register.writes),
        ];
        if let Some(name) = &name {
            features.push(format!("name:{name}"));
        }
        features.extend(reviewed_features.get(&key).into_iter().flatten().cloned());
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
        registers.insert(
            key,
            RevisionRegister {
                id: encode_id(key.0, key.1)?,
                address: register.address,
                width: register.width,
                name,
                fingerprint: fingerprint(&features)?,
                features,
            },
        );
    }
    for (key, name) in identities {
        if registers.contains_key(&key) {
            continue;
        }
        let address = u32::try_from(key.0).map_err(|_| {
            crate::Error::invalid(format!(
                "model-only revision register address {:#x} exceeds the snapshot address domain",
                key.0
            ))
        })?;
        let width = u8::try_from(key.1).map_err(|_| {
            crate::Error::invalid(format!(
                "model-only revision register width {} exceeds the snapshot width domain",
                key.1
            ))
        })?;
        let mut features = vec!["model-only".to_owned(), format!("name:{name}")];
        features.extend(reviewed_features.get(&key).into_iter().flatten().cloned());
        features.sort();
        features.dedup();
        registers.insert(
            key,
            RevisionRegister {
                id: encode_id(key.0, key.1)?,
                address,
                width,
                name: Some(name),
                fingerprint: fingerprint(&features)?,
                features,
            },
        );
    }
    Ok(registers.into_values().collect())
}

fn snapshot_interfaces(
    session: &ProjectSession,
    revision_artifacts: &BTreeSet<RevisionArtifact>,
    vendor_sources: &BTreeSet<String>,
    rust_sources: &BTreeSet<String>,
) -> Result<(Vec<RevisionArtifact>, Vec<RevisionInterface>)> {
    let Some(paths) = session.project.interfaces.as_ref() else {
        return Ok((Vec::new(), Vec::new()));
    };
    let facts = InterfaceFacts::load(&paths.facts)?;
    let mut artifacts = BTreeSet::new();
    let mut revision_artifact_indices = BTreeSet::new();
    for artifact in &facts.artifacts {
        let has_vendor = artifact
            .sources
            .iter()
            .any(|source| vendor_sources.contains(source));
        let has_rust = artifact
            .sources
            .iter()
            .any(|source| rust_sources.contains(source));
        if has_vendor && has_rust {
            return Err(crate::Error::invalid(format!(
                "interface facts artifact {} mixes vendor and Rust source lineages; regenerate separate projections before snapshotting",
                artifact.index
            )));
        }
        let mut vendor_artifact = false;
        for source in &artifact.sources {
            if vendor_sources.contains(source) {
                let sha256 = artifact.sha256.as_ref().ok_or_else(|| {
                    crate::Error::invalid(format!(
                        "interface facts omit the digest for vendor source {source:?}"
                    ))
                })?;
                let identity = RevisionArtifact {
                    role: None,
                    source: source.clone(),
                    sha256: sha256.clone(),
                };
                if !live_revision_analysis_input_matches(
                    revision_artifacts,
                    &identity.source,
                    &identity.sha256,
                ) {
                    return Err(crate::Error::invalid(format!(
                        "interface facts retain stale vendor artifact identity {}@{}; rerun project analysis against the current bindings",
                        identity.source,
                        identity.sha256.get(..12).unwrap_or(&identity.sha256)
                    )));
                }
                artifacts.insert(identity);
                vendor_artifact = true;
            } else if !rust_sources.contains(source) {
                return Err(crate::Error::invalid(format!(
                    "interface facts contain unclassified source {source:?}; restore its typed run-spec binding before snapshotting"
                )));
            }
        }
        if vendor_artifact {
            revision_artifact_indices.insert(artifact.index);
        }
    }
    if artifacts.is_empty() {
        return Err(crate::Error::invalid(
            "interface facts contain no current vendor artifact provenance",
        ));
    }
    let mut entities = facts
        .tables
        .iter()
        .filter(|table| revision_artifact_indices.contains(&table.artifact))
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
    Ok((artifacts.into_iter().collect(), entities))
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

fn fingerprint(value: &(impl Serialize + ?Sized)) -> Result<String> {
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
    if snapshot.schema_version != REVISION_SCHEMA {
        return Err(crate::Error::invalid(format!(
            "revision snapshot {:?} uses unsupported schema {}; {REVISION_CUTOVER_INSTRUCTION}",
            snapshot.name, snapshot.schema_version
        )));
    }
    if snapshot.command != "revision snapshot" {
        return Err(crate::Error::invalid(format!(
            "unsupported revision snapshot schema/command for {:?}",
            snapshot.name
        )));
    }
    if snapshot.artifact_scope != Some(RevisionArtifactScope::VendorInputs) {
        return Err(crate::Error::invalid(format!(
            "revision snapshot {:?} must declare vendor-inputs artifact scope",
            snapshot.name
        )));
    }
    snapshot.applicability.validate().map_err(|error| {
        crate::Error::invalid(format!(
            "revision snapshot {:?} has invalid applicability context: {error}",
            snapshot.name
        ))
    })?;
    validate_revision_name(&snapshot.name)?;
    let mut artifact_identities = BTreeSet::new();
    let mut exact_artifacts = BTreeSet::new();
    for artifact in &snapshot.artifacts {
        crate::source_id::validate_source_id(&artifact.source).map_err(|_| {
            crate::Error::invalid(format!(
                "revision snapshot {:?} has invalid artifact source {:?}",
                snapshot.name, artifact.source
            ))
        })?;
        validate_sha256("revision artifact sha256", &artifact.sha256)?;
        match artifact.role.as_deref() {
            Some(role_text) => {
                let role = crate::run_spec::InputRole::parse(role_text).ok_or_else(|| {
                    crate::Error::invalid(format!(
                        "revision snapshot {:?} has invalid artifact role {role_text:?}",
                        snapshot.name
                    ))
                })?;
                if !role.is_revision_owned()
                    || role.is_ambiguous_lineage()
                    || role.source_id() != artifact.source
                    || role.to_string() != role_text
                {
                    return Err(crate::Error::invalid(format!(
                        "revision snapshot {:?} artifact role {role_text:?} does not canonically identify vendor source {:?}",
                        snapshot.name, artifact.source
                    )));
                }
            }
            None => {
                return Err(crate::Error::invalid(format!(
                    "revision snapshot {:?} requires a canonical role for every artifact",
                    snapshot.name
                )));
            }
        }
        if !artifact_identities.insert(artifact) {
            return Err(crate::Error::invalid(format!(
                "revision snapshot {:?} contains duplicate artifact binding {}",
                snapshot.name,
                revision_artifact_summary(artifact)
            )));
        }
        exact_artifacts.insert(
            ArtifactIdentity::new(&artifact.source, &artifact.sha256)
                .map_err(|error| crate::Error::invalid(error.to_string()))?,
        );
    }
    if snapshot
        .applicability
        .artifacts
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>()
        != exact_artifacts
    {
        return Err(crate::Error::invalid(format!(
            "revision snapshot {:?} applicability artifacts do not match its authenticated vendor inputs",
            snapshot.name
        )));
    }
    validate_unique(
        "function",
        snapshot.functions.iter().map(|entity| &entity.id),
    )?;
    for function in &snapshot.functions {
        validate_revision_features(
            "function",
            &function.id,
            &function.features,
            &function.fingerprint,
        )?;
    }
    validate_unique(
        "register",
        snapshot.registers.iter().map(|entity| &entity.id),
    )?;
    for register in &snapshot.registers {
        validate_revision_features(
            "register",
            &register.id,
            &register.features,
            &register.fingerprint,
        )?;
        let identity = register.id.parse::<SemanticEntityId>().map_err(|error| {
            crate::Error::invalid(format!(
                "revision register {:?} is not a canonical semantic identity: {error}",
                register.id
            ))
        })?;
        let SemanticEntityId::Register { address, width, .. } = &identity else {
            return Err(crate::Error::invalid(format!(
                "revision register {:?} does not use the register semantic domain",
                register.id
            )));
        };
        if identity.to_string() != register.id
            || *address != u64::from(register.address)
            || *width != u32::from(register.width)
        {
            return Err(crate::Error::invalid(format!(
                "revision register {:?} does not match its canonical address {:#010x}/{}",
                register.id, register.address, register.width
            )));
        }
    }
    validate_unique(
        "interface",
        snapshot.interfaces.iter().map(|entity| &entity.id),
    )?;
    for interface in &snapshot.interfaces {
        validate_revision_features(
            "interface",
            &interface.id,
            &interface.features,
            &interface.fingerprint,
        )?;
    }
    if snapshot
        .assertions
        .iter()
        .any(|record| !matches!(&record.anchor, RevisionReviewedAnchor::Assertion { .. }))
    {
        return Err(crate::Error::invalid(
            "revision snapshot assertions require assertion anchors",
        ));
    }
    if snapshot
        .vendor_bugs
        .iter()
        .any(|record| !matches!(&record.anchor, RevisionReviewedAnchor::VendorBug { .. }))
    {
        return Err(crate::Error::invalid(
            "revision snapshot vendor bugs require vendor-bug anchors",
        ));
    }
    if snapshot
        .bindings
        .iter()
        .any(|record| !matches!(&record.anchor, RevisionReviewedAnchor::EntityBinding { .. }))
    {
        return Err(crate::Error::invalid(
            "revision snapshot entity bindings require entity-binding anchors",
        ));
    }
    validate_unique(
        "reviewed record",
        snapshot
            .assertions
            .iter()
            .chain(&snapshot.vendor_bugs)
            .chain(&snapshot.bindings)
            .map(|record| &record.id),
    )?;
    for record in snapshot
        .assertions
        .iter()
        .chain(&snapshot.vendor_bugs)
        .chain(&snapshot.bindings)
    {
        validate_reviewed_record_anchor(record, &snapshot.applicability)?;
    }
    Ok(())
}

fn validate_reviewed_record_anchor(
    record: &RevisionReviewedRecord,
    context: &ApplicabilityContext,
) -> Result<()> {
    let field = |name: &str| record.record.get(name).and_then(serde_json::Value::as_str);
    if field("id") != Some(record.id.as_str()) {
        return Err(crate::Error::invalid(format!(
            "revision reviewed record {:?} payload id does not match its typed record id",
            record.id
        )));
    }
    let (classification, applicability) = match &record.anchor {
        RevisionReviewedAnchor::Assertion { subject } => {
            let value: open_radio_vendor_review::EffectiveAssertion =
                serde_json::from_value(record.record.clone()).map_err(|error| {
                    crate::Error::invalid(format!(
                        "revision reviewed assertion {:?} has invalid payload: {error}",
                        record.id
                    ))
                })?;
            if value.subject != *subject {
                return Err(crate::Error::invalid(format!(
                    "revision reviewed assertion {:?} payload does not match its typed anchor",
                    record.id
                )));
            }
            (value.metadata.classification, value.metadata.applies_to)
        }
        RevisionReviewedAnchor::VendorBug { function, register } => {
            let value: open_radio_vendor_review::EffectiveVendorBug =
                serde_json::from_value(record.record.clone()).map_err(|error| {
                    crate::Error::invalid(format!(
                        "revision vendor bug {:?} has invalid payload: {error}",
                        record.id
                    ))
                })?;
            if value.function != *function || value.register != *register {
                return Err(crate::Error::invalid(format!(
                    "revision vendor bug {:?} payload does not match its typed anchor",
                    record.id
                )));
            }
            (value.metadata.classification, value.metadata.applies_to)
        }
        RevisionReviewedAnchor::EntityBinding {
            occurrence,
            semantic,
        } => {
            let value: open_radio_vendor_review::EffectiveEntityBinding =
                serde_json::from_value(record.record.clone()).map_err(|error| {
                    crate::Error::invalid(format!(
                        "revision entity binding {:?} has invalid payload: {error}",
                        record.id
                    ))
                })?;
            if value.occurrence != *occurrence || value.semantic != *semantic {
                return Err(crate::Error::invalid(format!(
                    "revision entity binding {:?} payload does not match its typed anchor",
                    record.id
                )));
            }
            if !value.has_authentic_occurrence_evidence() {
                return Err(crate::Error::invalid(format!(
                    "revision entity binding {:?} occurrence is not derived from its exact artifact identity and evidence locator",
                    record.id
                )));
            }
            (value.metadata.classification, value.metadata.applies_to)
        }
    };
    if classification.provenance == FactProvenance::Hint {
        return Err(crate::Error::invalid(format!(
            "revision reviewed record {:?} is a hint, not accepted knowledge",
            record.id
        )));
    }
    if !applicability.matches_context(context).map_err(|error| {
        crate::Error::invalid(format!(
            "revision reviewed record {:?} has invalid applicability: {error}",
            record.id
        ))
    })? {
        return Err(crate::Error::invalid(format!(
            "revision reviewed record {:?} does not apply to the snapshot context",
            record.id
        )));
    }
    Ok(())
}

fn validate_revision_features(
    domain: &str,
    id: &str,
    features: &[String],
    actual_fingerprint: &str,
) -> Result<()> {
    if features.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(crate::Error::invalid(format!(
            "revision {domain} {id:?} features must be sorted and unique"
        )));
    }
    validate_sha256("revision entity fingerprint", actual_fingerprint)?;
    let expected = fingerprint(features)?;
    if actual_fingerprint != expected {
        return Err(crate::Error::invalid(format!(
            "revision {domain} {id:?} fingerprint does not match its canonical features"
        )));
    }
    Ok(())
}

pub(crate) fn validate_operand_pair(
    project: &str,
    from: &RevisionSnapshot,
    to: &RevisionSnapshot,
) -> Result<()> {
    if from.project != project || to.project != project {
        return Err(crate::Error::invalid(format!(
            "revision operands must belong to active project {project:?}, got {:?} and {:?}",
            from.project, to.project
        )));
    }
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
    let functions = function_delta(&changes);
    let invalidated_research = research_invalidations(from, to, &changes);
    RevisionDiffReport {
        schema_version: REVISION_DIFF_REPORT_SCHEMA,
        command: "revision diff".to_owned(),
        from: from.name.clone(),
        to: to.name.clone(),
        artifacts_changed: from.artifacts != to.artifacts,
        summary,
        functions,
        invalidated_research,
        changes,
    }
}

fn function_delta(changes: &[RevisionEntityChange]) -> RevisionFunctionDelta {
    let mut delta = RevisionFunctionDelta::default();
    for change in changes.iter().filter(|change| change.domain == "function") {
        match change.classification {
            RevisionChangeClass::Modified => delta.changed.extend(change.before.iter().cloned()),
            RevisionChangeClass::Added => delta.added.extend(change.after.iter().cloned()),
            RevisionChangeClass::Removed => delta.removed.extend(change.before.iter().cloned()),
            RevisionChangeClass::Moved => {
                delta
                    .remapped
                    .extend(
                        change
                            .before
                            .iter()
                            .zip(&change.after)
                            .map(|(before, after)| RevisionFunctionRemap {
                                before: before.clone(),
                                after: after.clone(),
                            }),
                    )
            }
            RevisionChangeClass::Split
            | RevisionChangeClass::Merged
            | RevisionChangeClass::Ambiguous => {
                delta.uncertain.extend(change.before.iter().cloned());
                delta.uncertain.extend(change.after.iter().cloned());
            }
            RevisionChangeClass::Unchanged => {}
        }
    }
    for values in [
        &mut delta.changed,
        &mut delta.added,
        &mut delta.removed,
        &mut delta.uncertain,
    ] {
        values.sort();
        values.dedup();
    }
    delta.remapped.sort();
    delta.remapped.dedup();
    delta
}

struct ResearchInvalidationAccumulator {
    reason: &'static str,
    subjects: BTreeSet<String>,
    reviewed_records: BTreeSet<String>,
}

fn research_invalidations(
    from: &RevisionSnapshot,
    to: &RevisionSnapshot,
    changes: &[RevisionEntityChange],
) -> Vec<RevisionResearchInvalidation> {
    let mut areas = BTreeMap::<&'static str, ResearchInvalidationAccumulator>::new();
    let mut add = |area: &'static str, reason: &'static str, subjects: &[String]| {
        let entry = areas
            .entry(area)
            .or_insert_with(|| ResearchInvalidationAccumulator {
                reason,
                subjects: BTreeSet::new(),
                reviewed_records: BTreeSet::new(),
            });
        entry.subjects.extend(subjects.iter().cloned());
    };
    for change in changes {
        let mut subjects = change.before.clone();
        subjects.extend(change.after.iter().cloned());
        subjects.sort();
        subjects.dedup();
        match (change.domain.as_str(), change.classification) {
            ("function", RevisionChangeClass::Added) => add(
                "function-coverage",
                "new vendor functions have no inherited analysis or reviewed coverage",
                &subjects,
            ),
            ("function", RevisionChangeClass::Moved) => add(
                "evidence-locations",
                "function semantics correlate exactly, but address-bound evidence must be revalidated",
                &subjects,
            ),
            ("function", RevisionChangeClass::Modified)
            | ("function", RevisionChangeClass::Removed)
            | ("function", RevisionChangeClass::Split)
            | ("function", RevisionChangeClass::Merged)
            | ("function", RevisionChangeClass::Ambiguous) => add(
                "function-semantics",
                "changed or unresolved function identity invalidates semantic conclusions and dependent evidence",
                &subjects,
            ),
            ("register", classification) if classification != RevisionChangeClass::Unchanged => {
                add(
                    "register-model",
                    "register observations changed and reviewed names/access semantics require revalidation",
                    &subjects,
                )
            }
            ("interface", classification) if classification != RevisionChangeClass::Unchanged => {
                add(
                    "interface-contracts",
                    "interface observations changed and dependent call/event contracts require revalidation",
                    &subjects,
                )
            }
            _ => {}
        }
    }

    let before_functions = from
        .functions
        .iter()
        .map(|function| (function.id.as_str(), function))
        .collect::<BTreeMap<_, _>>();
    for function in &to.functions {
        if let Some(before) = before_functions.get(function.id.as_str())
            && (before.completeness != function.completeness
                || before.blocker_roots != function.blocker_roots)
        {
            add(
                "analysis-completeness",
                "analysis completeness or blocker roots changed; prior research priorities must be recalculated",
                std::slice::from_ref(&function.id),
            );
        }
    }

    if from.applicability != to.applicability {
        let mut subjects = from
            .applicability
            .artifacts
            .iter()
            .chain(&to.applicability.artifacts)
            .map(|artifact| artifact.source().to_owned())
            .collect::<BTreeSet<_>>();
        subjects.extend(
            from.applicability
                .ecosystems
                .iter()
                .chain(&to.applicability.ecosystems)
                .cloned(),
        );
        subjects.extend(
            from.applicability
                .chips
                .iter()
                .chain(&to.applicability.chips)
                .cloned(),
        );
        subjects.extend(
            from.applicability
                .chip_revisions
                .iter()
                .chain(&to.applicability.chip_revisions)
                .cloned(),
        );
        subjects.extend(
            from.applicability
                .artifact_lineages
                .iter()
                .chain(&to.applicability.artifact_lineages)
                .cloned(),
        );
        add(
            "artifact-applicability",
            "the active revision applicability context changed; bounded reviewed facts must be revalidated",
            &subjects.into_iter().collect::<Vec<_>>(),
        );
    }

    let reviewed = from
        .assertions
        .iter()
        .chain(&from.vendor_bugs)
        .chain(&from.bindings)
        .collect::<Vec<_>>();
    for area in areas.values_mut() {
        for record in &reviewed {
            let subject_matches = record.anchor.semantic_entities().any(|subject| {
                let encoded = subject.to_string();
                let (subject, _) = split_subject_suffix(&encoded);
                area.subjects.contains(subject)
            });
            let applicability_bounded =
                reviewed_record_applicability(record)
                    .ok()
                    .is_some_and(|applicability| {
                        applicability
                            .ecosystems
                            .iter()
                            .chain(&applicability.chips)
                            .chain(&applicability.chip_revisions)
                            .chain(&applicability.artifact_lineages)
                            .any(|item| area.subjects.contains(item))
                            || applicability
                                .artifacts
                                .iter()
                                .any(|artifact| area.subjects.contains(artifact.source()))
                    });
            if subject_matches || applicability_bounded {
                area.reviewed_records.insert(record.id.clone());
            }
        }
    }
    areas
        .into_iter()
        .map(|(area, invalidation)| RevisionResearchInvalidation {
            area: area.to_owned(),
            subjects: invalidation.subjects.into_iter().collect(),
            reviewed_records: invalidation.reviewed_records.into_iter().collect(),
            reason: invalidation.reason.to_owned(),
        })
        .collect()
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

pub(crate) fn rebase(
    from: &RevisionSnapshot,
    to: &RevisionSnapshot,
) -> Result<RevisionRebaseReport> {
    let diff = diff(from, to);
    let mappings = automatic_mappings(&diff);
    let unchanged_subjects = unchanged_subjects(from, to);
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
    let current_bindings = to
        .bindings
        .iter()
        .map(|record| (record.id.as_str(), record))
        .collect::<BTreeMap<_, _>>();
    let mut records = from
        .assertions
        .iter()
        .map(|record| {
            rebase_record(
                "assertion",
                record,
                &current,
                &unchanged_subjects,
                &target_subjects,
                &to.applicability,
                &mappings,
            )
        })
        .chain(from.vendor_bugs.iter().map(|record| {
            rebase_record(
                "vendor-bug",
                record,
                &current,
                &unchanged_subjects,
                &target_subjects,
                &to.applicability,
                &mappings,
            )
        }))
        .chain(
            from.bindings
                .iter()
                .map(|record| rebase_binding(record, &current_bindings, &to.applicability)),
        )
        .collect::<Result<Vec<_>>>()?;
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
    Ok(RevisionRebaseReport {
        schema_version: REVISION_REBASE_REPORT_SCHEMA,
        command: "revision rebase".to_owned(),
        from: from.name.clone(),
        to: to.name.clone(),
        summary,
        records,
    })
}

fn unchanged_subjects<'a>(
    from: &'a RevisionSnapshot,
    to: &'a RevisionSnapshot,
) -> BTreeSet<&'a str> {
    let from = subject_fingerprints(from);
    let to = subject_fingerprints(to);
    to.iter()
        .filter_map(|(id, fingerprints)| {
            (fingerprints.len() == 1
                && from
                    .get(id)
                    .is_some_and(|old| old.len() == 1 && old == fingerprints))
            .then_some(*id)
        })
        .collect()
}

fn subject_fingerprints(snapshot: &RevisionSnapshot) -> BTreeMap<&str, Vec<&str>> {
    let mut fingerprints = BTreeMap::<_, Vec<_>>::new();
    for entity in snapshot
        .functions
        .iter()
        .map(|entity| EntityView {
            id: &entity.id,
            fingerprint: &entity.fingerprint,
        })
        .chain(snapshot.registers.iter().map(|entity| EntityView {
            id: &entity.id,
            fingerprint: &entity.fingerprint,
        }))
        .chain(snapshot.interfaces.iter().map(|entity| EntityView {
            id: &entity.id,
            fingerprint: &entity.fingerprint,
        }))
    {
        fingerprints
            .entry(entity.id)
            .or_default()
            .push(entity.fingerprint);
    }
    fingerprints
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
    unchanged_subjects: &BTreeSet<&str>,
    target_subjects: &BTreeSet<&str>,
    target_context: &ApplicabilityContext,
    mappings: &BTreeMap<&str, &str>,
) -> Result<RevisionRebaseRecord> {
    let semantic = record.anchor.primary_semantic();
    let old_subject = semantic.to_string();
    let subject_prefix = if old_subject.starts_with("function:") {
        "function:"
    } else {
        ""
    };
    let base = rebase_subject_base(semantic);
    let dependent_bases = record
        .anchor
        .semantic_entities()
        .skip(1)
        .map(rebase_subject_base)
        .collect::<Vec<_>>();
    let dependents_target = dependent_bases
        .iter()
        .all(|subject| target_subjects.contains(subject.as_str()));
    let dependents_unchanged = dependent_bases
        .iter()
        .all(|subject| unchanged_subjects.contains(subject.as_str()));
    let exact_target = target_subjects.contains(base.as_str()) && dependents_target;
    let exact_unchanged = unchanged_subjects.contains(base.as_str()) && dependents_unchanged;
    let mapped = (!matches!(semantic, SemanticEntityId::RegisterField { .. })
        && dependents_unchanged)
        .then(|| mappings.get(base.as_str()).copied())
        .flatten();
    let proposed_subject = mapped
        .map(|target| format!("{subject_prefix}{target}"))
        .or_else(|| exact_target.then(|| old_subject.clone()));
    let applicability_current = applicability_matches(record, target_context)?;
    let (status, reason) = if !applicability_current {
        (
            RevisionRebaseStatus::ReviewRequired,
            "record applicability does not match the target revision context",
        )
    } else if exact_unchanged
        && current
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
    } else if exact_unchanged {
        (
            RevisionRebaseStatus::CarryExact,
            "the stable subject exists unchanged in the target revision",
        )
    } else if exact_target {
        (
            RevisionRebaseStatus::ReviewRequired,
            "the stable subject exists but its normalized features changed",
        )
    } else {
        (
            RevisionRebaseStatus::ReviewRequired,
            "no unique unchanged or moved target subject exists",
        )
    };
    Ok(RevisionRebaseRecord {
        id: record.id.clone(),
        kind: kind.to_owned(),
        status,
        old_subject: Some(old_subject),
        proposed_subject,
        reason: reason.to_owned(),
        record: record.record.clone(),
    })
}

fn rebase_subject_base(subject: &SemanticEntityId) -> String {
    match subject {
        SemanticEntityId::Function(path) => path.to_string(),
        SemanticEntityId::RegisterField {
            chip,
            address_space,
            address,
            register_width,
            ..
        } => SemanticEntityId::register(chip, address_space, *address, *register_width)
            .expect("a validated register-field identity has a valid parent register")
            .to_string(),
        _ => subject.to_string(),
    }
}

fn rebase_binding(
    record: &RevisionReviewedRecord,
    current: &BTreeMap<&str, &RevisionReviewedRecord>,
    target_context: &ApplicabilityContext,
) -> Result<RevisionRebaseRecord> {
    let old_subject = record.anchor.primary_semantic().to_string();
    let applicability_current = applicability_matches(record, target_context)?;
    let already_present = current
        .get(record.id.as_str())
        .is_some_and(|current| *current == record);
    let (status, proposed_subject, reason) = if !applicability_current {
        (
            RevisionRebaseStatus::ReviewRequired,
            None,
            "binding applicability does not match the target revision context",
        )
    } else if already_present {
        (
            RevisionRebaseStatus::AlreadyPresent,
            Some(old_subject.clone()),
            "the target snapshot already contains the identical occurrence-to-semantic binding",
        )
    } else {
        (
            RevisionRebaseStatus::ReviewRequired,
            None,
            "occurrence correspondence is not proven; revalidate the binding against the target revision",
        )
    };
    Ok(RevisionRebaseRecord {
        id: record.id.clone(),
        kind: "entity-binding".to_owned(),
        status,
        old_subject: Some(old_subject),
        proposed_subject,
        reason: reason.to_owned(),
        record: record.record.clone(),
    })
}

fn split_subject_suffix(subject: &str) -> (&str, &str) {
    let subject = subject.strip_prefix("function:").unwrap_or(subject);
    subject
        .find('#')
        .map_or((subject, ""), |index| subject.split_at(index))
}

fn applicability_matches(
    record: &RevisionReviewedRecord,
    target_context: &ApplicabilityContext,
) -> Result<bool> {
    reviewed_record_applicability(record)?
        .matches_context(target_context)
        .map_err(|error| crate::Error::invalid(error.to_string()))
}

fn reviewed_record_applicability(record: &RevisionReviewedRecord) -> Result<Applicability> {
    match &record.anchor {
        RevisionReviewedAnchor::Assertion { .. } => serde_json::from_value::<
            open_radio_vendor_review::EffectiveAssertion,
        >(record.record.clone())
        .map(|record| record.metadata.applies_to),
        RevisionReviewedAnchor::VendorBug { .. } => serde_json::from_value::<
            open_radio_vendor_review::EffectiveVendorBug,
        >(record.record.clone())
        .map(|record| record.metadata.applies_to),
        RevisionReviewedAnchor::EntityBinding { .. } => serde_json::from_value::<
            open_radio_vendor_review::EffectiveEntityBinding,
        >(record.record.clone())
        .map(|record| record.metadata.applies_to),
    }
    .map_err(|error| {
        crate::Error::invalid(format!(
            "revision reviewed record {:?} has invalid typed payload: {error}",
            record.id
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn function(id: &str, feature: &str) -> RevisionFunction {
        let features = vec![format!("fixture:{feature}")];
        RevisionFunction {
            id: id.to_owned(),
            source: "vendor".to_owned(),
            member: None,
            symbol: id.to_owned(),
            profiles: vec!["all".to_owned()],
            fingerprint: fingerprint(&features).unwrap(),
            features,
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
            artifact_scope: Some(RevisionArtifactScope::VendorInputs),
            artifacts: Vec::new(),
            applicability: ApplicabilityContext::default(),
            functions,
            registers: Vec::new(),
            interfaces: Vec::new(),
            assertions: Vec::new(),
            vendor_bugs: Vec::new(),
            bindings: Vec::new(),
        }
    }

    fn sync_artifact_context(snapshot: &mut RevisionSnapshot) {
        snapshot.applicability.artifacts = snapshot
            .artifacts
            .iter()
            .map(|artifact| ArtifactIdentity::new(&artifact.source, &artifact.sha256).unwrap())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
    }

    fn assertion_record(
        id: &str,
        function: &str,
        record: serde_json::Value,
    ) -> RevisionReviewedRecord {
        let subject = SemanticEntityId::function(function).unwrap();
        let mut payload = serde_json::json!({
            "pack": "fixture",
            "id": id,
            "subject": subject.to_string(),
            "kind": "fixture-fact",
            "value": true,
            "classification": {
                "provenance": "reviewed",
                "accuracy": "exact",
                "completeness": "partial"
            },
            "evidence": [{"source": "fixture", "locator": "manual"}]
        });
        if let (Some(payload), Some(overrides)) = (payload.as_object_mut(), record.as_object()) {
            payload.extend(overrides.clone());
        }
        RevisionReviewedRecord {
            id: id.to_owned(),
            anchor: RevisionReviewedAnchor::Assertion { subject },
            record: payload,
        }
    }

    fn binding_record(id: &str, function: &str, digest: char) -> RevisionReviewedRecord {
        let semantic = SemanticEntityId::function(function).unwrap();
        let artifact = ArtifactIdentity::new("fixture", digest.to_string().repeat(64)).unwrap();
        let occurrence = RevisionOccurrenceId::derive(
            semantic.domain(),
            std::slice::from_ref(&artifact),
            "manual",
        )
        .unwrap();
        RevisionReviewedRecord {
            id: id.to_owned(),
            anchor: RevisionReviewedAnchor::EntityBinding {
                occurrence: occurrence.clone(),
                semantic: semantic.clone(),
            },
            record: serde_json::json!({
                "pack": "fixture",
                "id": id,
                "occurrence": occurrence.to_string(),
                "semantic": semantic.to_string(),
                "classification": {
                    "provenance": "reviewed",
                    "accuracy": "exact",
                    "completeness": "partial"
                },
                "applies-to": {"artifacts": [{
                    "source": artifact.source(),
                    "sha256": artifact.sha256()
                }]},
                "evidence": [{
                    "source": "fixture",
                    "locator": "manual",
                    "occurrence": occurrence.to_string()
                }]
            }),
        }
    }

    fn vendor_bug_record(
        id: &str,
        function: &str,
        register: SemanticEntityId,
    ) -> RevisionReviewedRecord {
        let function = SemanticEntityId::function(function).unwrap();
        RevisionReviewedRecord {
            id: id.to_owned(),
            anchor: RevisionReviewedAnchor::VendorBug {
                function: function.clone(),
                register: Some(register.clone()),
            },
            record: serde_json::json!({
                "pack": "fixture",
                "id": id,
                "function": function.to_string(),
                "register": register.to_string(),
                "kind": "incorrect-register-access",
                "status": "reviewed",
                "observed": "fixture observed behavior",
                "expected": "fixture expected behavior",
                "classification": {
                    "provenance": "reviewed",
                    "accuracy": "exact",
                    "completeness": "partial"
                },
                "evidence": [{"source": "fixture", "locator": "manual"}]
            }),
        }
    }

    fn register(id: &str, address: u32, width: u8) -> RevisionRegister {
        let features = vec!["fixture:register".to_owned()];
        RevisionRegister {
            id: id.to_owned(),
            address,
            width,
            name: None,
            fingerprint: fingerprint(&features).unwrap(),
            features,
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

        assert_eq!(before.schema_version, REVISION_SCHEMA);
        assert_eq!(report.schema_version, REVISION_DIFF_REPORT_SCHEMA);
        assert_ne!(report.schema_version, before.schema_version);
        assert_eq!(report.summary.unchanged, 1);
        assert_eq!(report.summary.modified, 1);
        assert_eq!(report.summary.moved, 1);
        assert_eq!(report.summary.ambiguous, 1);
        assert_eq!(report.summary.removed, 1);
        assert_eq!(report.summary.added, 1);
        assert_eq!(report.functions.changed, ["changed"]);
        assert_eq!(report.functions.added, ["added"]);
        assert_eq!(report.functions.removed, ["removed"]);
        assert_eq!(
            report.functions.remapped,
            [RevisionFunctionRemap {
                before: "old-moved".to_owned(),
                after: "new-moved".to_owned(),
            }]
        );
        assert_eq!(
            serde_json::to_value(&report.functions.remapped[0]).unwrap(),
            serde_json::json!({"before":"old-moved","after":"new-moved"})
        );
        assert!(
            report
                .invalidated_research
                .iter()
                .any(|area| area.area == "function-semantics")
        );
    }

    #[test]
    fn research_invalidation_ignores_unchanged_entities_and_tracks_blocker_drift() {
        let mut before = snapshot("old", vec![function("stable", "same")]);
        before.assertions.push(assertion_record(
            "fact.stable",
            "stable",
            serde_json::json!({"id":"fact.stable","subject":"function:stable"}),
        ));
        let mut after = snapshot("new", vec![function("stable", "same")]);
        after.functions[0].blocker_roots = vec!["decode:unsupported:16".to_owned()];
        let unchanged = ["register", "interface"].map(|domain| RevisionEntityChange {
            domain: domain.to_owned(),
            classification: RevisionChangeClass::Unchanged,
            before: vec![format!("{domain}:stable")],
            after: vec![format!("{domain}:stable")],
            confidence: "high".to_owned(),
            reason: "fixture".to_owned(),
        });

        let areas = research_invalidations(&before, &after, &unchanged);

        assert!(!areas.iter().any(|area| area.area == "register-model"));
        assert!(!areas.iter().any(|area| area.area == "interface-contracts"));
        let completeness = areas
            .iter()
            .find(|area| area.area == "analysis-completeness")
            .unwrap();
        assert_eq!(completeness.subjects, ["stable"]);
        assert_eq!(completeness.reviewed_records, ["fact.stable"]);
    }

    #[test]
    fn rebase_carries_only_exact_or_uniquely_moved_subjects() {
        let mut before = snapshot("old", vec![function("old", "a"), function("gone", "b")]);
        before.assertions = vec![
            assertion_record(
                "fact.moved",
                "old",
                serde_json::json!({"id":"fact.moved","subject":"function:old"}),
            ),
            assertion_record(
                "fact.gone",
                "gone",
                serde_json::json!({"id":"fact.gone","subject":"function:gone"}),
            ),
        ];
        let after = snapshot("new", vec![function("new", "a")]);

        let report = rebase(&before, &after).unwrap();

        assert_eq!(report.summary.carry_remapped, 1);
        assert_eq!(report.summary.review_required, 1);
        assert_eq!(
            report.records[1].proposed_subject.as_deref(),
            Some("function:new")
        );
    }

    #[test]
    fn rebase_requires_review_when_an_identical_fact_targets_modified_semantics() {
        let record = assertion_record(
            "fact.modified",
            "stable",
            serde_json::json!({"id":"fact.modified","subject":"function:stable"}),
        );
        let mut before = snapshot("old", vec![function("stable", "a")]);
        before.assertions = vec![record.clone()];
        let mut after = snapshot("new", vec![function("stable", "changed")]);
        after.assertions = vec![record];

        let report = rebase(&before, &after).unwrap();

        assert_eq!(report.summary.already_present, 0);
        assert_eq!(report.summary.carry_exact, 0);
        assert_eq!(report.summary.review_required, 1);
        assert_eq!(
            report.records[0].proposed_subject.as_deref(),
            Some("function:stable")
        );
        assert!(
            report.records[0]
                .reason
                .contains("normalized features changed")
        );
    }

    #[test]
    fn rebase_requires_review_when_an_identical_fact_targets_a_removed_subject() {
        let record = assertion_record(
            "fact.removed",
            "removed",
            serde_json::json!({"id":"fact.removed","subject":"function:removed"}),
        );
        let mut before = snapshot("old", vec![function("removed", "a")]);
        before.assertions = vec![record.clone()];
        let mut after = snapshot("new", Vec::new());
        after.assertions = vec![record];

        let report = rebase(&before, &after).unwrap();

        assert_eq!(report.summary.already_present, 0);
        assert_eq!(report.summary.review_required, 1);
        assert_eq!(report.records[0].proposed_subject, None);
    }

    #[test]
    fn vendor_bug_rebase_requires_unchanged_function_and_register() {
        let register_id = SemanticEntityId::register("fixture-chip", "cpu", 0x1000, 32).unwrap();
        let record = vendor_bug_record("bug.register", "stable", register_id.clone());
        let mut before = snapshot("old", vec![function("stable", "a")]);
        before.registers = vec![register(&register_id.to_string(), 0x1000, 32)];
        before.vendor_bugs = vec![record.clone()];
        let mut after = snapshot("new", vec![function("stable", "a")]);
        after.vendor_bugs = vec![record];

        let report = rebase(&before, &after).unwrap();

        assert_eq!(report.summary.already_present, 0);
        assert_eq!(report.summary.carry_exact, 0);
        assert_eq!(report.summary.review_required, 1);
        assert!(report.records[0].reason.contains("target subject"));
    }

    #[test]
    fn rebase_rejects_an_artifact_guard_from_the_old_revision() {
        let old_digest = "aa".repeat(32);
        let new_digest = "bb".repeat(32);
        let mut before = snapshot("old", vec![function("stable", "a")]);
        before.artifacts = vec![RevisionArtifact {
            role: Some("vendor-artifact".to_owned()),
            source: "vendor".to_owned(),
            sha256: old_digest.clone(),
        }];
        sync_artifact_context(&mut before);
        before.assertions = vec![assertion_record(
            "fact.guarded",
            "stable",
            serde_json::json!({
                "id": "fact.guarded",
                "subject": "function:stable",
                "applies-to": {"artifacts": [{
                    "source": "vendor",
                    "sha256": old_digest
                }]}
            }),
        )];
        let mut after = snapshot("new", vec![function("stable", "a")]);
        after.artifacts = vec![RevisionArtifact {
            role: Some("vendor-artifact".to_owned()),
            source: "vendor".to_owned(),
            sha256: new_digest,
        }];
        sync_artifact_context(&mut after);

        let report = rebase(&before, &after).unwrap();

        assert_eq!(report.summary.review_required, 1);
        assert!(report.records[0].reason.contains("target revision context"));
    }

    #[test]
    fn reviewed_applicability_uses_the_full_target_context() {
        let first = "a".repeat(64);
        let second = "b".repeat(64);
        let record = assertion_record(
            "fact.context",
            "stable",
            serde_json::json!({
                "applies-to": {
                    "chips": ["fixture-chip"],
                    "artifacts": [
                        {"source": "vendor-a", "sha256": first.clone()},
                        {"source": "vendor-b", "sha256": second.clone()}
                    ]
                }
            }),
        );
        let target = ApplicabilityContext::new(
            Vec::new(),
            vec!["fixture-chip".to_owned()],
            Vec::new(),
            Vec::new(),
            vec![ArtifactIdentity::new("vendor-b", second).unwrap()],
        )
        .unwrap();

        assert!(applicability_matches(&record, &target).unwrap());

        let wrong_chip = ApplicabilityContext::new(
            Vec::new(),
            vec!["other-chip".to_owned()],
            Vec::new(),
            Vec::new(),
            target.artifacts.clone(),
        )
        .unwrap();
        assert!(!applicability_matches(&record, &wrong_chip).unwrap());
    }

    #[test]
    fn entity_bindings_are_never_carried_without_occurrence_correspondence() {
        let binding = binding_record("binding.radio", "radio/status", 'a');
        let mut before = snapshot("old", Vec::new());
        before.artifacts = vec![RevisionArtifact {
            role: Some("source-artifact:fixture".to_owned()),
            source: "fixture".to_owned(),
            sha256: "a".repeat(64),
        }];
        sync_artifact_context(&mut before);
        before.bindings = vec![binding.clone()];
        let mut identical = snapshot("identical", Vec::new());
        identical.artifacts = before.artifacts.clone();
        sync_artifact_context(&mut identical);
        identical.bindings = vec![binding];

        let present = rebase(&before, &identical).unwrap();
        assert_eq!(present.schema_version, REVISION_REBASE_REPORT_SCHEMA);
        assert_eq!(present.summary.already_present, 1);
        assert_eq!(present.records[0].kind, "entity-binding");

        let mut changed_snapshot = snapshot("changed", Vec::new());
        changed_snapshot.artifacts = before.artifacts.clone();
        sync_artifact_context(&mut changed_snapshot);
        let changed = rebase(&before, &changed_snapshot).unwrap();
        assert_eq!(changed.summary.review_required, 1);
        assert_eq!(changed.summary.carry_exact, 0);
        assert_eq!(changed.summary.carry_remapped, 0);
        assert!(changed.records[0].reason.contains("not proven"));
    }

    #[test]
    fn snapshot_rejects_cross_kind_ids_and_anchor_payload_mismatches() {
        let mut duplicate = snapshot("duplicate", Vec::new());
        duplicate.assertions = vec![assertion_record(
            "shared.id",
            "radio/status",
            serde_json::json!({"id":"shared.id","subject":"function:radio/status"}),
        )];
        duplicate.bindings = vec![binding_record("shared.id", "radio/status", 'a')];
        assert!(
            validate_snapshot(&duplicate)
                .unwrap_err()
                .to_string()
                .contains("duplicate reviewed record")
        );

        let mut mismatched = snapshot("mismatched", Vec::new());
        mismatched.assertions = vec![assertion_record(
            "fact.anchor",
            "radio/status",
            serde_json::json!({"id":"fact.anchor","subject":"function:radio/other"}),
        )];
        assert!(
            validate_snapshot(&mismatched)
                .unwrap_err()
                .to_string()
                .contains("payload does not match its typed anchor")
        );

        let mut malformed = snapshot("malformed-payload", Vec::new());
        let mut record = assertion_record("fact.malformed", "radio/status", serde_json::json!({}));
        record.record["applies-to"] = serde_json::json!("not-an-applicability-object");
        malformed.assertions = vec![record];
        assert!(
            validate_snapshot(&malformed)
                .unwrap_err()
                .to_string()
                .contains("invalid payload")
        );
    }

    #[test]
    fn snapshot_rejects_a_consistently_forged_entity_binding() {
        let mut forged = snapshot("forged-binding", Vec::new());
        forged.artifacts = vec![RevisionArtifact {
            role: Some("source-artifact:fixture".to_owned()),
            source: "fixture".to_owned(),
            sha256: "a".repeat(64),
        }];
        sync_artifact_context(&mut forged);
        let mut binding = binding_record("binding.forged", "radio/status", 'a');
        binding.record["evidence"][0]["locator"] = serde_json::json!("forged-locator");
        forged.bindings = vec![binding];

        assert!(
            validate_snapshot(&forged)
                .unwrap_err()
                .to_string()
                .contains("occurrence is not derived")
        );

        let mut cross_domain = snapshot("cross-domain-binding", Vec::new());
        cross_domain.artifacts = forged.artifacts.clone();
        sync_artifact_context(&mut cross_domain);
        let mut binding = binding_record("binding.cross-domain", "radio/status", 'a');
        let semantic = SemanticEntityId::interface("radio/status").unwrap();
        let RevisionReviewedAnchor::EntityBinding {
            semantic: anchor_semantic,
            ..
        } = &mut binding.anchor
        else {
            unreachable!();
        };
        *anchor_semantic = semantic.clone();
        binding.record["semantic"] = serde_json::json!(semantic.to_string());
        cross_domain.bindings = vec![binding];

        assert!(
            validate_snapshot(&cross_domain)
                .unwrap_err()
                .to_string()
                .contains("occurrence is not derived")
        );
    }

    #[test]
    fn snapshot_rejects_forged_feature_fingerprints_and_cross_project_operands() {
        let mut forged = snapshot("forged", vec![function("stable", "a")]);
        forged.functions[0]
            .features
            .push("fixture:tampered".to_owned());
        assert!(
            validate_snapshot(&forged)
                .unwrap_err()
                .to_string()
                .contains("fingerprint does not match")
        );

        let from = snapshot("from", Vec::new());
        let mut to = snapshot("to", Vec::new());
        to.project = "other-project".to_owned();
        assert!(
            validate_operand_pair("fixture", &from, &to)
                .unwrap_err()
                .to_string()
                .contains("must belong to active project")
        );
    }

    #[test]
    fn schema_four_registers_require_canonical_matching_coordinates() {
        let mut valid = snapshot("valid-register", Vec::new());
        valid.registers = vec![register(
            "register:esp32s31/cpu/0x20103064/32",
            0x2010_3064,
            32,
        )];
        validate_snapshot(&valid).unwrap();

        let mut legacy = valid.clone();
        legacy.registers[0].id = "mmio:cpu:0x20103064/32".to_owned();
        assert!(
            validate_snapshot(&legacy)
                .unwrap_err()
                .to_string()
                .contains("not a canonical semantic identity")
        );

        let mut mismatched = valid;
        mismatched.registers[0].address = 0x2010_3068;
        assert!(
            validate_snapshot(&mismatched)
                .unwrap_err()
                .to_string()
                .contains("does not match its canonical address")
        );
    }

    #[test]
    fn register_field_fact_uses_unchanged_parent_geometry() {
        let register_id = "register:esp32s31/cpu/0x20103064/32";
        let field =
            SemanticEntityId::register_field("esp32s31", "cpu", 0x2010_3064, 32, 3, 1).unwrap();
        let mut record = assertion_record("field.pending", "placeholder", serde_json::json!({}));
        record.anchor = RevisionReviewedAnchor::Assertion {
            subject: field.clone(),
        };
        record.record["subject"] = serde_json::json!(field.to_string());
        let mut before = snapshot("field-old", Vec::new());
        before.registers = vec![register(register_id, 0x2010_3064, 32)];
        before.assertions = vec![record];
        let mut after = snapshot("field-new", Vec::new());
        after.registers = vec![register(register_id, 0x2010_3064, 32)];

        let report = rebase(&before, &after).unwrap();
        assert_eq!(report.summary.carry_exact, 1);
        assert_eq!(report.records[0].proposed_subject, Some(field.to_string()));
    }

    #[test]
    fn durable_snapshot_initializes_content_addressed_state() {
        let (directory, manifest) = temporary_manifest("state-init");
        let snapshot = snapshot("vendor-1", vec![function("stable", "a")]);
        let output = default_path(&manifest, &snapshot.name).unwrap();

        persist_snapshot(&manifest, &snapshot, &output, false).unwrap();
        persist_snapshot(&manifest, &snapshot, &output, true).unwrap();

        let state = load_state_optional(&manifest, Some("fixture"))
            .unwrap()
            .unwrap();
        assert_eq!(state.baseline.as_deref(), Some("vendor-1"));
        assert_eq!(state.current.as_deref(), Some("vendor-1"));
        assert_eq!(state.entries.len(), 1);
        assert_eq!(state.entries[0].snapshot, "snapshots/vendor-1.json.gz");
        assert_eq!(state.entries[0].snapshot_sha256.len(), 64);
        assert_eq!(state.entries[0].artifacts_sha256.len(), 64);
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
            inspect_state(&manifest, "fixture", true).health,
            RevisionStateHealth::Ready
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn unaccepted_ordinary_revision_is_reported_as_review_pending() {
        let (directory, manifest) = temporary_manifest("state-review-pending");
        let baseline = snapshot("vendor-1", vec![function("stable", "a")]);
        persist_snapshot(
            &manifest,
            &baseline,
            &default_path(&manifest, &baseline.name).unwrap(),
            false,
        )
        .unwrap();
        let current = snapshot("vendor-2", vec![function("stable", "changed")]);
        persist_snapshot(
            &manifest,
            &current,
            &default_path(&manifest, &current.name).unwrap(),
            false,
        )
        .unwrap();

        for deep in [false, true] {
            let inspection = inspect_state(&manifest, "fixture", deep);
            assert_eq!(
                inspection.health,
                RevisionStateHealth::RevisionReviewPending
            );
            assert_eq!(inspection.baseline.as_deref(), Some("vendor-1"));
            assert_eq!(inspection.current.as_deref(), Some("vendor-2"));
            assert!(
                inspection
                    .diagnostic
                    .as_deref()
                    .is_some_and(|diagnostic| diagnostic.contains("diff and rebase"))
            );
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn gzip_snapshots_are_deterministic_and_loadable() {
        let (directory, manifest) = temporary_manifest("gzip-roundtrip");
        let snapshot = snapshot("vendor-1", vec![function("stable", "a")]);
        let path = default_path(&manifest, &snapshot.name).unwrap();

        let first = encode_snapshot(&snapshot, &path).unwrap();
        let second = encode_snapshot(&snapshot, &path).unwrap();
        assert_eq!(first, second);
        assert_eq!(&first[..2], &[0x1f, 0x8b]);
        assert_eq!(&first[4..8], &[0, 0, 0, 0]);

        persist_snapshot(&manifest, &snapshot, &path, false).unwrap();
        assert_eq!(load(&path).unwrap(), snapshot);

        // A different legal gzip representation must not change the durable
        // identity or make `--check` fail: the state owns logical content,
        // while gzip is only its replaceable storage codec.
        let mut json = serde_json::to_vec_pretty(&snapshot).unwrap();
        json.push(b'\n');
        let mut alternate = GzBuilder::new()
            .mtime(1)
            .write(Vec::new(), Compression::fast());
        alternate.write_all(&json).unwrap();
        let alternate = alternate.finish().unwrap();
        assert_ne!(alternate, first);
        std::fs::write(&path, alternate).unwrap();
        persist_snapshot(&manifest, &snapshot, &path, true).unwrap();
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn corrupt_gzip_snapshot_fails_closed() {
        let (directory, manifest) = temporary_manifest("gzip-corrupt");
        let path = default_path(&manifest, "vendor-1").unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"not gzip").unwrap();

        let error = load(&path).unwrap_err();
        assert!(error.to_string().contains("cannot decompress"));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn immutable_revision_name_cannot_be_reused_for_different_content() {
        let (directory, manifest) = temporary_manifest("state-immutable");
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
        let (directory, manifest) = temporary_manifest("state-preflight");
        let mut old = snapshot("vendor-1", vec![function("stable", "a")]);
        old.artifacts.push(RevisionArtifact {
            role: Some("vendor-artifact".to_owned()),
            source: "vendor".to_owned(),
            sha256: "aa".repeat(32),
        });
        sync_artifact_context(&mut old);
        persist_snapshot(
            &manifest,
            &old,
            &default_path(&manifest, "vendor-1").unwrap(),
            false,
        )
        .unwrap();
        let mut new = snapshot("vendor-2", vec![function("stable", "a")]);
        new.artifacts.push(RevisionArtifact {
            role: Some("vendor-artifact".to_owned()),
            source: "vendor".to_owned(),
            sha256: "bb".repeat(32),
        });
        sync_artifact_context(&mut new);
        let output = default_path(&manifest, "vendor-2").unwrap();
        let error = persist_snapshot(&manifest, &new, &output, false).unwrap_err();
        assert!(error.to_string().contains("prepare-update"));
        assert!(!output.exists());

        let mut state = load_state_optional(&manifest, Some("fixture"))
            .unwrap()
            .unwrap();
        let current = state.entries[0].clone();
        state.prepared_update = Some(PreparedRevisionUpdate {
            from: current.name.clone(),
            snapshot_sha256: current.snapshot_sha256,
            artifacts_sha256: current.artifacts_sha256,
        });
        write_state_atomic(&manifest, &state).unwrap();
        persist_snapshot(&manifest, &new, &output, false).unwrap();
        let state = load_state_optional(&manifest, Some("fixture"))
            .unwrap()
            .unwrap();
        assert_eq!(state.baseline.as_deref(), Some("vendor-1"));
        assert_eq!(state.current.as_deref(), Some("vendor-2"));
        assert!(state.prepared_update.is_none());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn generated_directory_cannot_hold_a_durable_baseline() {
        let (directory, manifest) = temporary_manifest("state-generated");
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
            role: Some("vendor-artifact".to_owned()),
            source: "vendor".to_owned(),
            sha256: crate::artifact_path_sha256(&artifact).unwrap(),
        });
        sync_artifact_context(&mut current_snapshot);
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
        sync_artifact_context(&mut next);
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

    #[test]
    fn obsolete_snapshot_and_non_dsl_state_require_a_new_current_state() {
        let (directory, manifest) = temporary_manifest("revision-cutover");
        let obsolete_snapshot_path = directory.join("obsolete.json");
        let mut obsolete_snapshot = serde_json::to_value(snapshot("obsolete", Vec::new())).unwrap();
        obsolete_snapshot["schema_version"] = serde_json::json!(1);
        obsolete_snapshot
            .as_object_mut()
            .unwrap()
            .remove("artifact_scope");
        std::fs::write(
            &obsolete_snapshot_path,
            serde_json::to_vec_pretty(&obsolete_snapshot).unwrap(),
        )
        .unwrap();

        let snapshot_error = load(&obsolete_snapshot_path).unwrap_err().to_string();
        assert!(snapshot_error.contains(&format!("not current schema {REVISION_SCHEMA}")));
        assert!(snapshot_error.contains("create a new current state"));
        assert!(snapshot_error.contains("project revision snapshot CURRENT"));

        let state = state_path(&manifest);
        std::fs::create_dir_all(state.parent().unwrap()).unwrap();
        std::fs::write(&state, "schema = 1\nproject = \"fixture\"\n").unwrap();
        let inspection = inspect_state(&manifest, "fixture", true);
        assert_eq!(inspection.health, RevisionStateHealth::Invalid);
        let diagnostic = inspection.diagnostic.unwrap();
        assert!(diagnostic.contains("must start with \"blobray-revision-state 1\""));
        assert!(diagnostic.contains("create a new current state"));
        assert!(diagnostic.contains("project revision snapshot CURRENT"));

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn revision_artifacts_require_canonical_vendor_roles() {
        let mut value = snapshot("vendor", Vec::new());
        value.artifacts.push(RevisionArtifact {
            role: Some("rust-artifact".to_owned()),
            source: "rust".to_owned(),
            sha256: "11".repeat(32),
        });
        sync_artifact_context(&mut value);
        assert!(
            validate_snapshot(&value)
                .unwrap_err()
                .to_string()
                .contains("vendor source")
        );

        value.artifacts[0] = RevisionArtifact {
            role: Some("source-artifact:rom".to_owned()),
            source: "archive".to_owned(),
            sha256: "11".repeat(32),
        };
        sync_artifact_context(&mut value);
        assert!(
            validate_snapshot(&value)
                .unwrap_err()
                .to_string()
                .contains("canonically")
        );

        value.artifacts[0] = RevisionArtifact {
            role: Some("source-companion:rom".to_owned()),
            source: "rom".to_owned(),
            sha256: "11".repeat(32),
        };
        sync_artifact_context(&mut value);
        validate_snapshot(&value).unwrap();
    }

    #[test]
    fn linked_ir_dependencies_match_inventory_and_companion_bindings_exactly() {
        let (directory, _) = temporary_manifest("linked-ir-dependencies");
        let primary = directory.join("wifi.elf");
        let inventory = directory.join("libwifi.a");
        let companion = directory.join("rom.elf");
        std::fs::write(&primary, b"linked vendor image").unwrap();
        std::fs::write(&inventory, b"vendor archive").unwrap();
        std::fs::write(&companion, b"vendor rom").unwrap();
        let run_path = directory.join("local.toml");
        std::fs::write(
            &run_path,
            format!(
                "schema = 1\n\n[[inputs]]\nrole = \"source-artifact:wifi\"\npath = {:?}\n\n[[inputs]]\nrole = \"source-inventory:wifi\"\npath = {:?}\n\n[[inputs]]\nrole = \"source-companion:wifi\"\npath = {:?}\n",
                primary, inventory, companion
            ),
        )
        .unwrap();
        let run_spec = crate::run_spec::RunSpec::load(&run_path).unwrap();
        let (_, digests) = current_revision_artifacts_with_digests(&run_spec).unwrap();
        let profile = crate::project_ir::ProjectIrProfile {
            id: "wifi".to_owned(),
            sources: vec!["wifi".to_owned()],
            roots: crate::project_ir::ProjectIrRoots::All,
            include_reachable: true,
            entry_contract: "none".to_owned(),
            output: directory.join("ir"),
        };
        let inventory_identity =
            vec![("wifi".to_owned(), digests.get(&inventory).unwrap().clone())];
        let companion_identity = vec![(
            companion.display().to_string(),
            digests.get(&companion).unwrap().clone(),
        )];
        validate_linked_ir_inventories(&profile, &inventory_identity, &run_spec, &digests).unwrap();
        validate_linked_ir_companions(
            &profile,
            &BTreeSet::from(["wifi".to_owned()]),
            &companion_identity,
            &run_spec,
            &digests,
        )
        .unwrap();
        assert!(
            validate_linked_ir_inventories(&profile, &[], &run_spec, &digests)
                .unwrap_err()
                .to_string()
                .contains("inventory provenance")
        );
        assert!(
            validate_linked_ir_companions(
                &profile,
                &BTreeSet::from(["wifi".to_owned()]),
                &[],
                &run_spec,
                &digests,
            )
            .unwrap_err()
            .to_string()
            .contains("companion provenance")
        );
        std::fs::remove_dir_all(directory).unwrap();
    }
}
