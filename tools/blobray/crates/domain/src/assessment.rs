//! Result meaning is independent of operation termination and output delivery.
use crate::*;
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CoverageStatus {
    Complete,
    Partial,
    Unknown,
}
impl CoverageStatus {
    pub fn from_complete(complete: bool) -> Self {
        if complete {
            Self::Complete
        } else {
            Self::Partial
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "id",
    rename_all = "kebab-case",
    deny_unknown_fields
)]
pub enum CoverageSubject {
    Inventory(RevisionId),
    Function(FunctionAnalysisId),
    Investigation(PublicationId),
    Execution(ArtifactId),
    StaticTargetAudit(ArtifactId),
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultCoverage {
    pub subject: CoverageSubject,
    pub status: CoverageStatus,
    pub scope: CoverageScope,
}
/// The universe to which completeness applies, never an unqualified proof.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CoverageScope {
    InventoryOccurrences,
    SelectedFunctionExtents,
    FunctionExtent,
    ExecutionScenario,
    StaticResolvedTransfers,
}
impl CoverageSubject {
    pub fn scope(&self) -> CoverageScope {
        match self {
            Self::Inventory(_) => CoverageScope::InventoryOccurrences,
            Self::Investigation(_) => CoverageScope::SelectedFunctionExtents,
            Self::Function(_) => CoverageScope::FunctionExtent,
            Self::Execution(_) => CoverageScope::ExecutionScenario,
            Self::StaticTargetAudit(_) => CoverageScope::StaticResolvedTransfers,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CheckVerdict {
    Pass,
    Fail,
    Inconclusive,
}
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultAssessment {
    pub coverage: Option<ResultCoverage>,
    pub check: Option<CheckVerdict>,
    pub comparison: Option<ComparisonVerdict>,
}
impl ResultAssessment {
    pub fn covered(subject: CoverageSubject, complete: bool) -> Self {
        Self {
            coverage: Some(ResultCoverage {
                scope: subject.scope(),
                subject,
                status: CoverageStatus::from_complete(complete),
            }),
            ..Self::default()
        }
    }
    pub fn checked(pass: bool) -> Self {
        Self {
            check: Some(if pass {
                CheckVerdict::Pass
            } else {
                CheckVerdict::Fail
            }),
            ..Self::default()
        }
    }
    pub fn execution(
        id: ArtifactId,
        complete: bool,
        comparison: Option<ComparisonVerdict>,
    ) -> Self {
        Self {
            comparison,
            ..Self::covered(CoverageSubject::Execution(id), complete)
        }
    }
    pub fn function(id: FunctionAnalysisId, manifest: &FunctionManifest) -> Self {
        let status = if !manifest.coverage.complete() {
            CoverageStatus::Partial
        } else {
            manifest.semantics.map_or(CoverageStatus::Unknown, |s| {
                CoverageStatus::from_complete(s.complete)
            })
        };
        Self {
            coverage: Some(ResultCoverage {
                subject: CoverageSubject::Function(id),
                status,
                scope: CoverageScope::FunctionExtent,
            }),
            ..Self::default()
        }
    }
    pub fn is_complete(&self) -> bool {
        self.coverage
            .as_ref()
            .is_some_and(|c| c.status == CoverageStatus::Complete)
    }
    /// CLI check policy only; partial research and comparison verdicts remain data.
    pub fn check_passed(&self) -> bool {
        matches!(self.check, None | Some(CheckVerdict::Pass))
    }
}
