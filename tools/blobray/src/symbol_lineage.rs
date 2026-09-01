//! Multi-revision composition of exact symbol-correspondence evidence.

use std::{collections::BTreeMap, path::Path};

use serde::Serialize;

use crate::{
    Result,
    symbol_correspondence::{
        self, DataObjectCorrespondence, DataObjectCorrespondenceObject,
        DataObjectCorrespondenceSummary, ObfuscationEpochEvidence, SymbolCorrespondence,
        SymbolCorrespondenceArtifact, SymbolCorrespondenceFunction, SymbolCorrespondenceRequest,
        SymbolCorrespondenceStatus, SymbolCorrespondenceSummary,
    },
};

pub(crate) const SYMBOL_LINEAGE_SCHEMA: u32 = 1;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
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
    pub(crate) from: SymbolCorrespondenceArtifact,
    pub(crate) to: SymbolCorrespondenceArtifact,
    pub(crate) obfuscation_epoch: ObfuscationEpochEvidence,
    pub(crate) functions: SymbolCorrespondenceSummary,
    pub(crate) data_objects: DataObjectCorrespondenceSummary,
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
pub(crate) struct SymbolLineageRecord<T> {
    pub(crate) source: T,
    pub(crate) status: SymbolLineageStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) direct_basis: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) direct: Option<T>,
    pub(crate) chain: Vec<SymbolLineageHop<T>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) chain_blocker: Option<SymbolLineageBlocker>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) resolved: Option<T>,
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
    pub(crate) artifacts: Vec<SymbolCorrespondenceArtifact>,
    pub(crate) edges: Vec<SymbolLineageEdgeSummary>,
    pub(crate) direct: SymbolLineageEdgeSummary,
    pub(crate) function_summary: SymbolLineageSummary,
    pub(crate) functions: Vec<SymbolLineageRecord<SymbolCorrespondenceFunction>>,
    pub(crate) data_summary: SymbolLineageSummary,
    pub(crate) data_objects: Vec<SymbolLineageRecord<DataObjectCorrespondenceObject>>,
    pub(crate) pin_candidates: Vec<SymbolLineagePinCandidate>,
}

pub(crate) struct SymbolLineageRevision<'a> {
    pub(crate) source: &'a str,
    pub(crate) path: &'a Path,
}

pub(crate) fn build(revisions: &[SymbolLineageRevision<'_>]) -> Result<SymbolLineageReport> {
    if revisions.len() < 3 {
        return Err(crate::Error::invalid(
            "symbols lineage requires at least three ordered --revision artifacts",
        ));
    }
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
    let mut pin_candidates = function_pin_candidates(&functions);
    pin_candidates.extend(data_pin_candidates(&data_objects));
    pin_candidates.sort_by(|left, right| {
        (&left.domain, &left.suggested_name, &left.target_occurrence).cmp(&(
            &right.domain,
            &right.suggested_name,
            &right.target_occurrence,
        ))
    });
    let mut artifacts = Vec::with_capacity(edges.len() + 1);
    artifacts.push(edges[0].from.clone());
    artifacts.extend(edges.iter().map(|edge| edge.to.clone()));
    let edge_summaries = edges
        .iter()
        .enumerate()
        .map(|(index, report)| edge_summary(Some(index), report))
        .collect();
    let direct_summary = edge_summary(None, &direct);

    Ok(SymbolLineageReport {
        schema_version: SYMBOL_LINEAGE_SCHEMA,
        command: "symbols lineage",
        method: "direct-and-ordered-one-to-one-correspondence-composition-v1",
        artifacts,
        edges: edge_summaries,
        direct: direct_summary,
        function_summary,
        functions,
        data_summary,
        data_objects,
        pin_candidates,
    })
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
    report: &symbol_correspondence::SymbolCorrespondenceReport,
) -> SymbolLineageEdgeSummary {
    SymbolLineageEdgeSummary {
        index,
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
    }

    #[test]
    fn lineage_requires_an_independent_direct_path() {
        let error = build(&[]).unwrap_err();
        assert!(error.to_string().contains("at least three ordered"));
    }
}
