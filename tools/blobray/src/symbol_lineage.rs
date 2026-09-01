//! Multi-revision composition of exact symbol-correspondence evidence.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use open_radio_vendor_contracts::{ArtifactIdentity, EntityDomain, RevisionOccurrenceId};
use serde::{Deserialize, Serialize};

use crate::{
    Result,
    source_id::validate_source_id,
    symbol_correspondence::{
        self, DataObjectCorrespondence, DataObjectCorrespondenceObject,
        DataObjectCorrespondenceSummary, ObfuscationEpochEvidence, SymbolCorrespondence,
        SymbolCorrespondenceArtifact, SymbolCorrespondenceFunction, SymbolCorrespondenceRequest,
        SymbolCorrespondenceStatus, SymbolCorrespondenceSummary,
    },
};

pub(crate) const SYMBOL_LINEAGE_SCHEMA: u32 = 5;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum SymbolLineageStatus {
    Confirmed,
    DirectOnly,
    ChainOnly,
    Conflict,
    #[default]
    Unresolved,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct SymbolLineageEdgeSummary {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) index: Option<usize>,
    pub(crate) from_label: String,
    pub(crate) to_label: String,
    pub(crate) from: SymbolCorrespondenceArtifact,
    pub(crate) to: SymbolCorrespondenceArtifact,
    pub(crate) obfuscation_epoch: ObfuscationEpochEvidence,
    pub(crate) functions: SymbolCorrespondenceSummary,
    pub(crate) data_objects: DataObjectCorrespondenceSummary,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct SymbolLineageArtifact {
    pub(crate) label: String,
    pub(crate) source: String,
    pub(crate) path: String,
    pub(crate) sha256: String,
    pub(crate) functions: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct SymbolLineageHop<T> {
    pub(crate) edge: usize,
    pub(crate) basis: &'static str,
    pub(crate) from: T,
    pub(crate) to: T,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct SymbolLineageBlocker {
    pub(crate) edge: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) status: Option<SymbolCorrespondenceStatus>,
    pub(crate) basis: &'static str,
    pub(crate) candidates: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct SymbolLineageDirectBlocker {
    pub(crate) status: SymbolCorrespondenceStatus,
    pub(crate) basis: &'static str,
    pub(crate) candidates: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct SymbolLineageRecord<T> {
    pub(crate) source: T,
    pub(crate) status: SymbolLineageStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) direct_basis: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) direct: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) direct_blocker: Option<SymbolLineageDirectBlocker>,
    pub(crate) chain: Vec<SymbolLineageHop<T>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) chain_blocker: Option<SymbolLineageBlocker>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) resolved: Option<T>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum SymbolLineageFrontierRoute {
    AdjacentChain,
    DirectEndpoint,
    EndpointConflict,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct SymbolLineageReviewFrontier {
    pub(crate) domain: &'static str,
    pub(crate) affected_status: SymbolLineageStatus,
    pub(crate) resolution_blocked: bool,
    pub(crate) route: SymbolLineageFrontierRoute,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) edge: Option<usize>,
    pub(crate) from: String,
    pub(crate) to: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) correspondence_status: Option<SymbolCorrespondenceStatus>,
    pub(crate) basis: &'static str,
    pub(crate) candidate_min: usize,
    pub(crate) candidate_max: usize,
    pub(crate) reviewable_records: usize,
    pub(crate) records: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub(crate) struct SymbolLineageSummary {
    pub(crate) source: usize,
    pub(crate) resolved: usize,
    pub(crate) confirmed: usize,
    pub(crate) direct_only: usize,
    pub(crate) chain_only: usize,
    pub(crate) conflict: usize,
    pub(crate) unresolved: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct SymbolLineagePinCandidate {
    pub(crate) domain: &'static str,
    pub(crate) review: &'static str,
    pub(crate) suggested_name: String,
    pub(crate) target_occurrence: String,
    pub(crate) target_locator: String,
    pub(crate) lineage_status: SymbolLineageStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) direct_basis: Option<&'static str>,
    pub(crate) chain_bases: Vec<&'static str>,
    pub(crate) source_occurrence: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct SymbolLineageReport {
    pub(crate) schema_version: u32,
    pub(crate) command: &'static str,
    pub(crate) method: &'static str,
    pub(crate) artifacts: Vec<SymbolLineageArtifact>,
    pub(crate) edges: Vec<SymbolLineageEdgeSummary>,
    pub(crate) direct: SymbolLineageEdgeSummary,
    pub(crate) function_summary: SymbolLineageSummary,
    pub(crate) functions: Vec<SymbolLineageRecord<SymbolCorrespondenceFunction>>,
    pub(crate) data_summary: SymbolLineageSummary,
    pub(crate) data_objects: Vec<SymbolLineageRecord<DataObjectCorrespondenceObject>>,
    pub(crate) review_frontiers: Vec<SymbolLineageReviewFrontier>,
    pub(crate) pin_candidates: Vec<SymbolLineagePinCandidate>,
}

pub(crate) struct SymbolLineageRevision<'a> {
    pub(crate) label: &'a str,
    pub(crate) source: &'a str,
    pub(crate) path: &'a Path,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SymbolLineageRebaseMapping {
    pub(crate) target_occurrence: RevisionOccurrenceId,
    pub(crate) target_locator: String,
    pub(crate) status: SymbolLineageStatus,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SymbolLineageRebaseEvidence {
    pub(crate) report_sha256: String,
    pub(crate) source_artifact: ArtifactIdentity,
    pub(crate) target_artifact: ArtifactIdentity,
    pub(crate) mappings: BTreeMap<RevisionOccurrenceId, SymbolLineageRebaseMapping>,
}

#[derive(Deserialize)]
struct RebaseLineageInput {
    schema_version: u32,
    command: String,
    artifacts: Vec<RebaseLineageArtifact>,
    functions: Vec<RebaseLineageRecord>,
    data_objects: Vec<RebaseLineageRecord>,
}

#[derive(Deserialize)]
struct RebaseLineageArtifact {
    label: String,
    source: String,
    path: String,
    sha256: String,
}

#[derive(Deserialize)]
struct RebaseLineageRecord {
    source: RebaseLineageOccurrence,
    status: SymbolLineageStatus,
    resolved: Option<RebaseLineageOccurrence>,
}

#[derive(Deserialize)]
struct RebaseLineageOccurrence {
    locator: String,
    occurrence: RevisionOccurrenceId,
}

pub(crate) fn load_rebase_evidence(path: &Path) -> Result<SymbolLineageRebaseEvidence> {
    const MAX_LINEAGE_BYTES: usize = 64 * 1024 * 1024;
    let bytes = fs::read(path).map_err(|error| {
        crate::Error::invalid(format!(
            "cannot read symbol lineage {}: {error}",
            path.display()
        ))
    })?;
    if bytes.len() > MAX_LINEAGE_BYTES {
        return Err(crate::Error::invalid(format!(
            "symbol lineage {} is {} bytes; limit is {MAX_LINEAGE_BYTES}",
            path.display(),
            bytes.len()
        )));
    }
    let input: RebaseLineageInput = serde_json::from_slice(&bytes).map_err(|error| {
        crate::Error::invalid(format!(
            "cannot parse symbol lineage {}: {error}",
            path.display()
        ))
    })?;
    if input.schema_version != SYMBOL_LINEAGE_SCHEMA || input.command != "symbols lineage" {
        return Err(crate::Error::invalid(format!(
            "symbol lineage {} must be current schema {SYMBOL_LINEAGE_SCHEMA}",
            path.display()
        )));
    }
    if input.artifacts.len() < 3 {
        return Err(crate::Error::invalid(format!(
            "symbol lineage {} has fewer than three ordered artifacts",
            path.display()
        )));
    }
    let revisions = input
        .artifacts
        .iter()
        .map(|artifact| SymbolLineageRevision {
            label: &artifact.label,
            source: &artifact.source,
            path: Path::new(&artifact.path),
        })
        .collect::<Vec<_>>();
    let rebuilt = build(&revisions)?;
    let mut expected = serde_json::to_vec(&rebuilt)?;
    expected.push(b'\n');
    if bytes != expected {
        return Err(crate::Error::invalid(format!(
            "symbol lineage {} is not the current generated report for its artifact paths; rerun symbols lineage",
            path.display()
        )));
    }
    let artifacts = input
        .artifacts
        .into_iter()
        .map(|artifact| {
            ArtifactIdentity::new(artifact.source, artifact.sha256)
                .map_err(|error| crate::Error::invalid(error.to_string()))
        })
        .collect::<Result<Vec<_>>>()?;
    let source_artifact = artifacts
        .first()
        .expect("three lineage artifacts have a first item")
        .clone();
    let target_artifact = artifacts
        .last()
        .expect("three lineage artifacts have a last item")
        .clone();
    let mut mappings = BTreeMap::new();
    let mut reverse = BTreeMap::<RevisionOccurrenceId, RevisionOccurrenceId>::new();
    for (domain, records) in [
        (EntityDomain::Function, input.functions),
        (EntityDomain::MemoryObject, input.data_objects),
    ] {
        for record in records {
            validate_lineage_occurrence(&record.source, domain, &source_artifact, "source")?;
            let resolved_status = matches!(
                record.status,
                SymbolLineageStatus::Confirmed
                    | SymbolLineageStatus::DirectOnly
                    | SymbolLineageStatus::ChainOnly
            );
            if resolved_status != record.resolved.is_some() {
                return Err(crate::Error::invalid(format!(
                    "symbol lineage record {} has status {:?} inconsistent with its resolved occurrence",
                    record.source.occurrence, record.status
                )));
            }
            let Some(target) = record.resolved else {
                continue;
            };
            validate_lineage_occurrence(&target, domain, &target_artifact, "target")?;
            if let Some(previous) =
                reverse.insert(target.occurrence.clone(), record.source.occurrence.clone())
                && previous != record.source.occurrence
            {
                return Err(crate::Error::invalid(format!(
                    "symbol lineage maps both {previous} and {} to {}",
                    record.source.occurrence, target.occurrence
                )));
            }
            let mapping = SymbolLineageRebaseMapping {
                target_occurrence: target.occurrence,
                target_locator: target.locator,
                status: record.status,
            };
            if let Some(previous) =
                mappings.insert(record.source.occurrence.clone(), mapping.clone())
                && previous != mapping
            {
                return Err(crate::Error::invalid(format!(
                    "symbol lineage contains conflicting mappings for {}",
                    record.source.occurrence
                )));
            }
        }
    }
    Ok(SymbolLineageRebaseEvidence {
        report_sha256: crate::bytes_sha256(&bytes),
        source_artifact,
        target_artifact,
        mappings,
    })
}

fn validate_lineage_occurrence(
    occurrence: &RebaseLineageOccurrence,
    domain: EntityDomain,
    artifact: &ArtifactIdentity,
    role: &str,
) -> Result<()> {
    if occurrence.occurrence.domain() != domain {
        return Err(crate::Error::invalid(format!(
            "symbol lineage {role} occurrence {} is in the wrong domain",
            occurrence.occurrence
        )));
    }
    let derived =
        RevisionOccurrenceId::derive(domain, std::slice::from_ref(artifact), &occurrence.locator)
            .map_err(|error| crate::Error::invalid(error.to_string()))?;
    if derived != occurrence.occurrence {
        return Err(crate::Error::invalid(format!(
            "symbol lineage {role} occurrence {} is not derived from {}@{} and locator {:?}",
            occurrence.occurrence,
            artifact.source(),
            artifact.sha256(),
            occurrence.locator
        )));
    }
    Ok(())
}

pub(crate) fn build(revisions: &[SymbolLineageRevision<'_>]) -> Result<SymbolLineageReport> {
    if revisions.len() < 3 {
        return Err(crate::Error::invalid(
            "symbols lineage requires at least three ordered --revision artifacts",
        ));
    }
    validate_revisions(revisions)?;
    let edges = revisions
        .windows(2)
        .map(|window| correlate(&window[0], &window[1]))
        .collect::<Result<Vec<_>>>()?;
    let direct = correlate(
        revisions
            .first()
            .expect("three lineage revisions have a first item"),
        revisions
            .last()
            .expect("three lineage revisions have a last item"),
    )?;

    let functions = compose(&direct.correspondences, &edges, |report| {
        &report.correspondences
    });
    let data_objects = compose(&direct.data_correspondences, &edges, |report| {
        &report.data_correspondences
    });
    let function_summary = summarize(&functions);
    let data_summary = summarize(&data_objects);
    let review_frontiers = review_frontiers(&functions, &data_objects, revisions);
    let mut pin_candidates = function_pin_candidates(&functions);
    pin_candidates.extend(data_pin_candidates(&data_objects));
    pin_candidates.sort_by(|left, right| {
        (&left.domain, &left.suggested_name, &left.target_occurrence).cmp(&(
            &right.domain,
            &right.suggested_name,
            &right.target_occurrence,
        ))
    });
    let correspondence_artifacts =
        std::iter::once(&edges[0].from).chain(edges.iter().map(|edge| &edge.to));
    let artifacts = revisions
        .iter()
        .zip(correspondence_artifacts)
        .map(|(revision, artifact)| SymbolLineageArtifact {
            label: revision.label.to_owned(),
            source: artifact.source.clone(),
            path: artifact.path.clone(),
            sha256: artifact.sha256.clone(),
            functions: artifact.functions,
        })
        .collect();
    let edge_summaries = edges
        .iter()
        .enumerate()
        .map(|(index, report)| {
            edge_summary(
                Some(index),
                revisions[index].label,
                revisions[index + 1].label,
                report,
            )
        })
        .collect();
    let direct_summary = edge_summary(
        None,
        revisions.first().expect("validated revisions").label,
        revisions.last().expect("validated revisions").label,
        &direct,
    );

    Ok(SymbolLineageReport {
        schema_version: SYMBOL_LINEAGE_SCHEMA,
        command: "symbols lineage",
        method: "direct-and-ordered-one-to-one-correspondence-composition-v5",
        artifacts,
        edges: edge_summaries,
        direct: direct_summary,
        function_summary,
        functions,
        data_summary,
        data_objects,
        review_frontiers,
        pin_candidates,
    })
}

fn validate_revisions(revisions: &[SymbolLineageRevision<'_>]) -> Result<()> {
    let expected_source = revisions.first().expect("three revisions").source;
    validate_source_id(expected_source)?;
    let mut labels = BTreeSet::new();
    for revision in revisions {
        if revision.source != expected_source {
            return Err(crate::Error::invalid(format!(
                "symbols lineage revisions must share logical source {expected_source:?}; label {:?} uses {:?}",
                revision.label, revision.source
            )));
        }
        if revision.label.is_empty()
            || !revision
                .label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            return Err(crate::Error::invalid(format!(
                "invalid symbol lineage revision label {:?}",
                revision.label
            )));
        }
        if !labels.insert(revision.label) {
            return Err(crate::Error::invalid(format!(
                "duplicate symbol lineage revision label {:?}",
                revision.label
            )));
        }
    }
    Ok(())
}

fn correlate(
    from: &SymbolLineageRevision<'_>,
    to: &SymbolLineageRevision<'_>,
) -> Result<symbol_correspondence::SymbolCorrespondenceReport> {
    symbol_correspondence::correlate(SymbolCorrespondenceRequest {
        from_source: from.source,
        from_path: from.path,
        from_prefix: "",
        to_source: to.source,
        to_path: to.path,
        to_prefix: "",
    })
}

fn edge_summary(
    index: Option<usize>,
    from_label: &str,
    to_label: &str,
    report: &symbol_correspondence::SymbolCorrespondenceReport,
) -> SymbolLineageEdgeSummary {
    SymbolLineageEdgeSummary {
        index,
        from_label: from_label.to_owned(),
        to_label: to_label.to_owned(),
        from: report.from.clone(),
        to: report.to.clone(),
        obfuscation_epoch: report.obfuscation_epoch.clone(),
        functions: report.summary.clone(),
        data_objects: report.data_summary.clone(),
    }
}

trait LineageEntity: Clone + Eq {
    fn occurrence(&self) -> &str;
    fn locator(&self) -> &str;
    fn symbol(&self) -> &str;
}

impl LineageEntity for SymbolCorrespondenceFunction {
    fn occurrence(&self) -> &str {
        &self.occurrence
    }

    fn locator(&self) -> &str {
        &self.locator
    }

    fn symbol(&self) -> &str {
        &self.symbol
    }
}

impl LineageEntity for DataObjectCorrespondenceObject {
    fn occurrence(&self) -> &str {
        &self.occurrence
    }

    fn locator(&self) -> &str {
        &self.locator
    }

    fn symbol(&self) -> &str {
        &self.symbol
    }
}

trait LineageCorrespondence {
    type Entity: LineageEntity;

    fn source(&self) -> &Self::Entity;
    fn status(&self) -> SymbolCorrespondenceStatus;
    fn basis(&self) -> &'static str;
    fn candidates(&self) -> &[Self::Entity];
}

impl LineageCorrespondence for SymbolCorrespondence {
    type Entity = SymbolCorrespondenceFunction;

    fn source(&self) -> &Self::Entity {
        &self.from
    }

    fn status(&self) -> SymbolCorrespondenceStatus {
        self.status
    }

    fn basis(&self) -> &'static str {
        self.basis
    }

    fn candidates(&self) -> &[Self::Entity] {
        &self.candidates
    }
}

impl LineageCorrespondence for DataObjectCorrespondence {
    type Entity = DataObjectCorrespondenceObject;

    fn source(&self) -> &Self::Entity {
        &self.from
    }

    fn status(&self) -> SymbolCorrespondenceStatus {
        self.status
    }

    fn basis(&self) -> &'static str {
        self.basis
    }

    fn candidates(&self) -> &[Self::Entity] {
        &self.candidates
    }
}

fn unique_target<C: LineageCorrespondence>(correspondence: &C) -> Option<&C::Entity> {
    (correspondence.status() == SymbolCorrespondenceStatus::Unique
        && correspondence.candidates().len() == 1)
        .then(|| &correspondence.candidates()[0])
}

fn compose<C, F>(
    direct: &[C],
    edges: &[symbol_correspondence::SymbolCorrespondenceReport],
    correspondences: F,
) -> Vec<SymbolLineageRecord<C::Entity>>
where
    C: LineageCorrespondence,
    F: Fn(&symbol_correspondence::SymbolCorrespondenceReport) -> &[C],
{
    let indexes = edges
        .iter()
        .map(|edge| {
            correspondences(edge)
                .iter()
                .map(|correspondence| {
                    (
                        correspondence.source().occurrence().to_owned(),
                        correspondence,
                    )
                })
                .collect::<BTreeMap<_, _>>()
        })
        .collect::<Vec<_>>();
    direct
        .iter()
        .map(|direct| {
            let direct_target = unique_target(direct).cloned();
            let direct_blocker = direct_target
                .is_none()
                .then_some(SymbolLineageDirectBlocker {
                    status: direct.status(),
                    basis: direct.basis(),
                    candidates: direct.candidates().len(),
                });
            let mut current = direct.source().clone();
            let mut chain = Vec::with_capacity(indexes.len());
            let mut chain_blocker = None;
            for (edge, index) in indexes.iter().enumerate() {
                let Some(correspondence) = index.get(current.occurrence()) else {
                    chain_blocker = Some(SymbolLineageBlocker {
                        edge,
                        status: None,
                        basis: "intermediate-occurrence-not-indexed",
                        candidates: 0,
                    });
                    break;
                };
                let Some(target) = unique_target(*correspondence) else {
                    chain_blocker = Some(SymbolLineageBlocker {
                        edge,
                        status: Some(correspondence.status()),
                        basis: correspondence.basis(),
                        candidates: correspondence.candidates().len(),
                    });
                    break;
                };
                chain.push(SymbolLineageHop {
                    edge,
                    basis: correspondence.basis(),
                    from: correspondence.source().clone(),
                    to: target.clone(),
                });
                current = target.clone();
            }
            let chain_target = chain_blocker.is_none().then_some(current);
            let (status, resolved) = match (&direct_target, &chain_target) {
                (Some(direct), Some(chain)) if direct.occurrence() == chain.occurrence() => {
                    (SymbolLineageStatus::Confirmed, Some(direct.clone()))
                }
                (Some(_), Some(_)) => (SymbolLineageStatus::Conflict, None),
                (Some(direct), None) => (SymbolLineageStatus::DirectOnly, Some(direct.clone())),
                (None, Some(chain)) => (SymbolLineageStatus::ChainOnly, Some(chain.clone())),
                (None, None) => (SymbolLineageStatus::Unresolved, None),
            };
            SymbolLineageRecord {
                source: direct.source().clone(),
                status,
                direct_basis: direct_target.as_ref().map(|_| direct.basis()),
                direct: direct_target,
                direct_blocker,
                chain,
                chain_blocker,
                resolved,
            }
        })
        .collect()
}

fn summarize<T>(records: &[SymbolLineageRecord<T>]) -> SymbolLineageSummary {
    let mut summary = SymbolLineageSummary {
        source: records.len(),
        ..SymbolLineageSummary::default()
    };
    for record in records {
        summary.resolved += usize::from(record.resolved.is_some());
        match record.status {
            SymbolLineageStatus::Confirmed => summary.confirmed += 1,
            SymbolLineageStatus::DirectOnly => summary.direct_only += 1,
            SymbolLineageStatus::ChainOnly => summary.chain_only += 1,
            SymbolLineageStatus::Conflict => summary.conflict += 1,
            SymbolLineageStatus::Unresolved => summary.unresolved += 1,
        }
    }
    summary
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ReviewFrontierKey {
    domain: &'static str,
    affected_status: SymbolLineageStatus,
    route: SymbolLineageFrontierRoute,
    edge: Option<usize>,
    from: String,
    to: String,
    correspondence_status: Option<SymbolCorrespondenceStatus>,
    basis: &'static str,
}

#[derive(Clone, Copy, Debug)]
struct ReviewFrontierCount {
    candidate_min: usize,
    candidate_max: usize,
    reviewable_records: usize,
    records: usize,
}

fn review_frontiers(
    functions: &[SymbolLineageRecord<SymbolCorrespondenceFunction>],
    data_objects: &[SymbolLineageRecord<DataObjectCorrespondenceObject>],
    revisions: &[SymbolLineageRevision<'_>],
) -> Vec<SymbolLineageReviewFrontier> {
    let mut counts = BTreeMap::new();
    collect_review_frontiers("function", functions, revisions, &mut counts);
    collect_review_frontiers("memory-object", data_objects, revisions, &mut counts);
    let mut frontiers = counts
        .into_iter()
        .map(|(key, count)| SymbolLineageReviewFrontier {
            domain: key.domain,
            affected_status: key.affected_status,
            resolution_blocked: matches!(
                key.affected_status,
                SymbolLineageStatus::Conflict | SymbolLineageStatus::Unresolved
            ),
            route: key.route,
            edge: key.edge,
            from: key.from,
            to: key.to,
            correspondence_status: key.correspondence_status,
            basis: key.basis,
            candidate_min: count.candidate_min,
            candidate_max: count.candidate_max,
            reviewable_records: count.reviewable_records,
            records: count.records,
        })
        .collect::<Vec<_>>();
    frontiers.sort_by(|left, right| {
        right
            .resolution_blocked
            .cmp(&left.resolution_blocked)
            .then_with(|| right.reviewable_records.cmp(&left.reviewable_records))
            .then_with(|| right.records.cmp(&left.records))
            .then_with(|| {
                (
                    left.domain,
                    left.route,
                    left.edge,
                    left.affected_status,
                    &left.from,
                    &left.to,
                    left.correspondence_status,
                    left.basis,
                )
                    .cmp(&(
                        right.domain,
                        right.route,
                        right.edge,
                        right.affected_status,
                        &right.from,
                        &right.to,
                        right.correspondence_status,
                        right.basis,
                    ))
            })
    });
    frontiers
}

fn collect_review_frontiers<T: LineageEntity>(
    domain: &'static str,
    records: &[SymbolLineageRecord<T>],
    revisions: &[SymbolLineageRevision<'_>],
    counts: &mut BTreeMap<ReviewFrontierKey, ReviewFrontierCount>,
) {
    for record in records {
        let (key, candidates) = match record.status {
            SymbolLineageStatus::Confirmed => continue,
            SymbolLineageStatus::DirectOnly | SymbolLineageStatus::Unresolved => {
                let blocker = record
                    .chain_blocker
                    .as_ref()
                    .expect("an incomplete adjacent route records its exact blocker");
                (
                    ReviewFrontierKey {
                        domain,
                        affected_status: record.status,
                        route: SymbolLineageFrontierRoute::AdjacentChain,
                        edge: Some(blocker.edge),
                        from: revisions[blocker.edge].label.to_owned(),
                        to: revisions[blocker.edge + 1].label.to_owned(),
                        correspondence_status: blocker.status,
                        basis: blocker.basis,
                    },
                    blocker.candidates,
                )
            }
            SymbolLineageStatus::ChainOnly => {
                let blocker = record
                    .direct_blocker
                    .as_ref()
                    .expect("a missing direct route records its exact blocker");
                (
                    ReviewFrontierKey {
                        domain,
                        affected_status: record.status,
                        route: SymbolLineageFrontierRoute::DirectEndpoint,
                        edge: None,
                        from: revisions
                            .first()
                            .expect("validated revisions")
                            .label
                            .to_owned(),
                        to: revisions
                            .last()
                            .expect("validated revisions")
                            .label
                            .to_owned(),
                        correspondence_status: Some(blocker.status),
                        basis: blocker.basis,
                    },
                    blocker.candidates,
                )
            }
            SymbolLineageStatus::Conflict => (
                ReviewFrontierKey {
                    domain,
                    affected_status: record.status,
                    route: SymbolLineageFrontierRoute::EndpointConflict,
                    edge: None,
                    from: revisions
                        .first()
                        .expect("validated revisions")
                        .label
                        .to_owned(),
                    to: revisions
                        .last()
                        .expect("validated revisions")
                        .label
                        .to_owned(),
                    correspondence_status: None,
                    basis: "direct-and-adjacent-targets-disagree",
                },
                2,
            ),
        };
        counts
            .entry(key)
            .and_modify(|count| {
                count.candidate_min = count.candidate_min.min(candidates);
                count.candidate_max = count.candidate_max.max(candidates);
                count.reviewable_records += usize::from(
                    symbol_correspondence::is_reviewable_source_name(record.source.symbol()),
                );
                count.records += 1;
            })
            .or_insert(ReviewFrontierCount {
                candidate_min: candidates,
                candidate_max: candidates,
                reviewable_records: usize::from(symbol_correspondence::is_reviewable_source_name(
                    record.source.symbol(),
                )),
                records: 1,
            });
    }
}

fn function_pin_candidates(
    records: &[SymbolLineageRecord<SymbolCorrespondenceFunction>],
) -> Vec<SymbolLineagePinCandidate> {
    records
        .iter()
        .filter(|record| symbol_correspondence::is_reviewable_source_name(&record.source.symbol))
        .filter_map(|record| pin_candidate("function", record))
        .collect()
}

fn data_pin_candidates(
    records: &[SymbolLineageRecord<DataObjectCorrespondenceObject>],
) -> Vec<SymbolLineagePinCandidate> {
    records
        .iter()
        .filter(|record| symbol_correspondence::is_reviewable_source_name(&record.source.symbol))
        .filter_map(|record| pin_candidate("memory-object", record))
        .collect()
}

fn pin_candidate<T: LineageEntity>(
    domain: &'static str,
    record: &SymbolLineageRecord<T>,
) -> Option<SymbolLineagePinCandidate> {
    let target = record.resolved.as_ref()?;
    Some(SymbolLineagePinCandidate {
        domain,
        review: "required",
        suggested_name: record.source.symbol().to_owned(),
        target_occurrence: target.occurrence().to_owned(),
        target_locator: target.locator().to_owned(),
        lineage_status: record.status,
        direct_basis: record.direct_basis,
        chain_bases: record.chain.iter().map(|hop| hop.basis).collect(),
        source_occurrence: record.source.occurrence().to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity(name: &str, occurrence: &str) -> SymbolCorrespondenceFunction {
        SymbolCorrespondenceFunction {
            member: Some("radio.o".to_owned()),
            symbol: name.to_owned(),
            locator: format!("archive-member:radio.o/symbol:{name}"),
            occurrence: occurrence.to_owned(),
            size: 4,
            fingerprint: "sha256:body".to_owned(),
        }
    }

    fn correspondence(
        from: SymbolCorrespondenceFunction,
        to: Option<SymbolCorrespondenceFunction>,
    ) -> SymbolCorrespondence {
        SymbolCorrespondence {
            from,
            status: if to.is_some() {
                SymbolCorrespondenceStatus::Unique
            } else {
                SymbolCorrespondenceStatus::Unmatched
            },
            basis: "exact-normalized-body",
            candidates: to.into_iter().collect(),
        }
    }

    fn empty_report(
        correspondences: Vec<SymbolCorrespondence>,
    ) -> symbol_correspondence::SymbolCorrespondenceReport {
        let artifact = |source: &str| SymbolCorrespondenceArtifact {
            source: source.to_owned(),
            path: format!("/{source}.a"),
            sha256: source.chars().next().unwrap_or('0').to_string().repeat(64),
            functions: correspondences.len(),
        };
        let overlap = symbol_correspondence::ObfuscationTokenOverlap {
            status: symbol_correspondence::ObfuscationEpochStatus::Inconclusive,
            from_tokens: 0,
            to_tokens: 0,
            common_tokens: 0,
            smaller_set_retention_parts_per_million: 0,
            automatic_matches: false,
        };
        symbol_correspondence::SymbolCorrespondenceReport {
            schema_version: symbol_correspondence::SYMBOL_CORRESPONDENCE_SCHEMA,
            command: "symbols correlate",
            method: "test",
            from: artifact("a"),
            to: artifact("b"),
            obfuscation_epoch: ObfuscationEpochEvidence {
                status: symbol_correspondence::ObfuscationEpochStatus::Inconclusive,
                basis: "test",
                minimum_common_tokens: 64,
                minimum_smaller_set_retention_parts_per_million: 900_000,
                automatic_matches: false,
                functions: overlap.clone(),
                data_objects: overlap,
            },
            member_order: None,
            summary: SymbolCorrespondenceSummary::default(),
            correspondences,
            data_summary: DataObjectCorrespondenceSummary::default(),
            data_correspondences: Vec::new(),
            pin_candidates: Vec::new(),
        }
    }

    #[test]
    fn direct_and_chain_agreement_is_confirmed() {
        let source = entity("named", "source");
        let middle = entity("token-old", "middle");
        let target = entity("token-new", "target");
        let edges = vec![
            empty_report(vec![correspondence(source.clone(), Some(middle.clone()))]),
            empty_report(vec![correspondence(middle, Some(target.clone()))]),
        ];
        let direct = vec![correspondence(source, Some(target))];

        let records = compose(&direct, &edges, |report| &report.correspondences);

        assert_eq!(records[0].status, SymbolLineageStatus::Confirmed);
        assert_eq!(records[0].chain.len(), 2);
        assert_eq!(records[0].resolved.as_ref().unwrap().occurrence, "target");
    }

    #[test]
    fn direct_and_chain_disagreement_is_a_conflict() {
        let source = entity("named", "source");
        let middle = entity("token-old", "middle");
        let chained = entity("token-new", "chained");
        let direct_target = entity("other", "direct");
        let edges = vec![
            empty_report(vec![correspondence(source.clone(), Some(middle.clone()))]),
            empty_report(vec![correspondence(middle, Some(chained))]),
        ];
        let direct = vec![correspondence(source, Some(direct_target))];

        let records = compose(&direct, &edges, |report| &report.correspondences);

        assert_eq!(records[0].status, SymbolLineageStatus::Conflict);
        assert!(records[0].resolved.is_none());
    }

    #[test]
    fn a_partial_chain_keeps_its_exact_blocker() {
        let source = entity("named", "source");
        let middle = entity("token-old", "middle");
        let edges = vec![
            empty_report(vec![correspondence(source.clone(), Some(middle.clone()))]),
            empty_report(vec![correspondence(middle, None)]),
        ];
        let direct = vec![correspondence(source, None)];

        let records = compose(&direct, &edges, |report| &report.correspondences);

        assert_eq!(records[0].status, SymbolLineageStatus::Unresolved);
        assert_eq!(records[0].chain.len(), 1);
        assert_eq!(records[0].chain_blocker.as_ref().unwrap().edge, 1);
        assert_eq!(
            records[0].direct_blocker.as_ref().unwrap().status,
            SymbolCorrespondenceStatus::Unmatched
        );
    }

    #[test]
    fn review_frontiers_rank_the_exact_route_that_blocks_confirmation() {
        let source = entity("named", "source");
        let middle = entity("token-old", "middle");
        let edges = vec![
            empty_report(vec![correspondence(source.clone(), Some(middle.clone()))]),
            empty_report(vec![correspondence(middle, None)]),
        ];
        let direct = empty_report(vec![correspondence(source, None)]);
        let functions = compose(&direct.correspondences, &edges, |report| {
            &report.correspondences
        });
        let revisions = ["named", "old", "current"].map(|label| SymbolLineageRevision {
            label,
            source: "ble-controller",
            path: Path::new("unused"),
        });

        let frontiers = review_frontiers(&functions, &[], &revisions);

        assert_eq!(frontiers.len(), 1);
        assert_eq!(frontiers[0].domain, "function");
        assert_eq!(
            frontiers[0].affected_status,
            SymbolLineageStatus::Unresolved
        );
        assert_eq!(
            frontiers[0].route,
            SymbolLineageFrontierRoute::AdjacentChain
        );
        assert_eq!(frontiers[0].edge, Some(1));
        assert!(frontiers[0].resolution_blocked);
        assert_eq!(frontiers[0].reviewable_records, 1);
        assert_eq!(frontiers[0].records, 1);
    }

    #[test]
    fn review_frontiers_rank_semantic_work_before_compiler_artifacts() {
        let named = entity("controller_state", "named-source");
        let middle = entity("token-old", "named-middle");
        let compiler = entity(".LC0", "compiler-source");
        let edges = vec![
            empty_report(vec![
                correspondence(named.clone(), Some(middle.clone())),
                correspondence(compiler.clone(), None),
            ]),
            empty_report(vec![correspondence(middle, None)]),
        ];
        let direct = empty_report(vec![
            correspondence(named, None),
            correspondence(compiler, None),
        ]);
        let functions = compose(&direct.correspondences, &edges, |report| {
            &report.correspondences
        });
        let revisions = ["named", "old", "current"].map(|label| SymbolLineageRevision {
            label,
            source: "ble-controller",
            path: Path::new("unused"),
        });

        let frontiers = review_frontiers(&functions, &[], &revisions);

        assert_eq!(frontiers.len(), 2);
        assert_eq!(frontiers[0].edge, Some(1));
        assert_eq!(frontiers[0].reviewable_records, 1);
        assert_eq!(frontiers[1].edge, Some(0));
        assert_eq!(frontiers[1].reviewable_records, 0);
    }

    #[test]
    fn unresolved_work_ranks_before_larger_corroboration_frontiers() {
        let unresolved = entity("unresolved", "unresolved-source");
        let mut direct = vec![correspondence(unresolved.clone(), None)];
        let mut first_edge = vec![correspondence(unresolved, None)];
        let mut second_edge = Vec::new();
        for index in 0..3 {
            let source = entity(&format!("chain_{index}"), &format!("source-{index}"));
            let middle = entity(&format!("token_{index}"), &format!("middle-{index}"));
            let target = entity(&format!("target_{index}"), &format!("target-{index}"));
            direct.push(correspondence(source.clone(), None));
            first_edge.push(correspondence(source, Some(middle.clone())));
            second_edge.push(correspondence(middle, Some(target)));
        }
        let edges = vec![empty_report(first_edge), empty_report(second_edge)];
        let direct = empty_report(direct);
        let functions = compose(&direct.correspondences, &edges, |report| {
            &report.correspondences
        });
        let revisions = ["named", "old", "current"].map(|label| SymbolLineageRevision {
            label,
            source: "ble-controller",
            path: Path::new("unused"),
        });

        let frontiers = review_frontiers(&functions, &[], &revisions);

        assert_eq!(frontiers.len(), 2);
        assert!(frontiers[0].resolution_blocked);
        assert_eq!(
            frontiers[0].affected_status,
            SymbolLineageStatus::Unresolved
        );
        assert_eq!(frontiers[0].records, 1);
        assert!(!frontiers[1].resolution_blocked);
        assert_eq!(frontiers[1].affected_status, SymbolLineageStatus::ChainOnly);
        assert_eq!(frontiers[1].records, 3);
    }

    #[test]
    fn lineage_requires_an_independent_direct_path() {
        let error = build(&[]).unwrap_err();
        assert!(error.to_string().contains("at least three ordered"));
    }

    #[test]
    fn lineage_requires_unique_labels_and_one_logical_source() {
        let duplicate_labels = [
            SymbolLineageRevision {
                label: "named",
                source: "btdm",
                path: Path::new("missing-named.a"),
            },
            SymbolLineageRevision {
                label: "named",
                source: "btdm",
                path: Path::new("missing-middle.a"),
            },
            SymbolLineageRevision {
                label: "current",
                source: "btdm",
                path: Path::new("missing-current.a"),
            },
        ];
        assert!(
            build(&duplicate_labels)
                .unwrap_err()
                .to_string()
                .contains("duplicate symbol lineage revision label")
        );

        let mixed_sources = [
            SymbolLineageRevision {
                label: "named",
                source: "btdm",
                path: Path::new("missing-named.a"),
            },
            SymbolLineageRevision {
                label: "middle",
                source: "ble",
                path: Path::new("missing-middle.a"),
            },
            SymbolLineageRevision {
                label: "current",
                source: "btdm",
                path: Path::new("missing-current.a"),
            },
        ];
        assert!(
            build(&mixed_sources)
                .unwrap_err()
                .to_string()
                .contains("must share logical source")
        );
    }

    #[test]
    fn rebase_loader_rebuilds_the_report_before_trusting_confirmed_status() {
        let fixture = std::env::temp_dir().join(format!(
            "blobray-symbol-lineage-artifact-{}.elf",
            std::process::id()
        ));
        let bytes = include_str!("../tests/fixtures/symbols-rv32.hex")
            .split_ascii_whitespace()
            .map(|octet| u8::from_str_radix(octet, 16).unwrap())
            .collect::<Vec<_>>();
        std::fs::write(&fixture, bytes).unwrap();
        let revisions = ["old", "middle", "new"].map(|label| SymbolLineageRevision {
            label,
            source: "fixture",
            path: &fixture,
        });
        let report = build(&revisions).unwrap();
        assert_eq!(
            report
                .artifacts
                .iter()
                .map(|artifact| artifact.label.as_str())
                .collect::<Vec<_>>(),
            ["old", "middle", "new"]
        );
        assert!(
            report
                .artifacts
                .iter()
                .all(|artifact| artifact.source == "fixture")
        );
        assert_eq!(report.edges[0].from_label, "old");
        assert_eq!(report.edges[0].to_label, "middle");
        assert_eq!(report.direct.from_label, "old");
        assert_eq!(report.direct.to_label, "new");
        let path = std::env::temp_dir().join(format!(
            "blobray-symbol-lineage-rebase-{}.json",
            std::process::id()
        ));
        crate::application::generated_file::write_or_check_json(
            &path,
            &report,
            false,
            "symbol lineage fixture",
            false,
        )
        .unwrap();

        let evidence = load_rebase_evidence(&path).unwrap();
        let confirmed = report
            .functions
            .iter()
            .find(|record| record.status == SymbolLineageStatus::Confirmed)
            .unwrap();
        let source_occurrence = confirmed.source.occurrence.parse().unwrap();
        let target_occurrence = confirmed
            .resolved
            .as_ref()
            .unwrap()
            .occurrence
            .parse()
            .unwrap();
        assert_eq!(
            evidence.mappings[&source_occurrence].target_occurrence,
            target_occurrence
        );

        let authentic = std::fs::read(&path).unwrap();
        let mut obsolete: serde_json::Value = serde_json::from_slice(&authentic).unwrap();
        obsolete["schema_version"] = serde_json::json!(1);
        std::fs::write(&path, serde_json::to_vec(&obsolete).unwrap()).unwrap();
        assert!(
            load_rebase_evidence(&path)
                .unwrap_err()
                .to_string()
                .contains("current schema 5")
        );

        let mut forged: serde_json::Value = serde_json::from_slice(&authentic).unwrap();
        let confirmed_index = forged["functions"]
            .as_array()
            .unwrap()
            .iter()
            .position(|record| record["status"] == "confirmed")
            .unwrap();
        forged["functions"][confirmed_index]["status"] = serde_json::json!("chain-only");
        let mut forged_bytes = serde_json::to_vec(&forged).unwrap();
        forged_bytes.push(b'\n');
        std::fs::write(&path, forged_bytes).unwrap();
        assert!(
            load_rebase_evidence(&path)
                .unwrap_err()
                .to_string()
                .contains("not the current generated report")
        );
        std::fs::remove_file(path).unwrap();
        std::fs::remove_file(fixture).unwrap();
    }
}
