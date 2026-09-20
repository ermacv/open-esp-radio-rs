//! Navigation and work classification for the engineering map.
//!
//! Links describe reviewed associations, not test execution or readiness.
//! Work annotations refine existing gaps; they do not create a second backlog.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use super::{GapDocument, catalog::validate_regular_reference};
use crate::Result;

#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) struct Development {
    /// Canonical research, register-model or semantic-contract files.
    #[serde(default)]
    pub(crate) knowledge: Vec<PathBuf>,
    /// Reviewed test selectors. Existence of a selector is not a recorded PASS.
    #[serde(default)]
    pub(crate) host_tests: Vec<HostTest>,
    /// Optional classification of an existing gap, with an engineering reason.
    #[serde(default)]
    pub(crate) gap_work: Vec<GapWork>,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) struct HostTest {
    pub(crate) manifest: PathBuf,
    pub(crate) filter: String,
    pub(crate) source: PathBuf,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq, Ord, PartialOrd)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum WorkKind {
    Research,
    Implement,
    HostTest,
    Experiment,
    MeasurementMethod,
    InspectVendor,
    ReviewGap,
    InspectEvidence,
    AssessApplicability,
    Recheck,
    InvestigateFailure,
    LinkOwners,
    LinkKnowledge,
    LinkHostTests,
}

impl WorkKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Research => "research",
            Self::Implement => "implement",
            Self::HostTest => "host-test",
            Self::Experiment => "experiment",
            Self::MeasurementMethod => "measurement-method",
            Self::InspectVendor => "inspect-vendor",
            Self::ReviewGap => "review-gap",
            Self::InspectEvidence => "inspect-evidence",
            Self::AssessApplicability => "assess-applicability",
            Self::Recheck => "recheck",
            Self::InvestigateFailure => "investigate-failure",
            Self::LinkOwners => "link-owners",
            Self::LinkKnowledge => "link-knowledge",
            Self::LinkHostTests => "link-host-tests",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) struct GapWork {
    pub(crate) gap: String,
    pub(crate) kind: WorkKind,
    pub(crate) reason: String,
}

impl Development {
    pub(super) fn validate(&self, gaps: &[GapDocument], root: &Path) -> Result<()> {
        let mut paths = BTreeSet::new();
        for path in &self.knowledge {
            if !paths.insert(path) {
                return Err(format!("duplicate knowledge link {}", path.display()).into());
            }
            validate_regular_reference(root, path, "knowledge link")?;
        }
        let mut selectors = BTreeSet::new();
        for test in &self.host_tests {
            validate_regular_reference(root, &test.manifest, "host test manifest")?;
            validate_regular_reference(root, &test.source, "host test source")?;
            if test
                .manifest
                .file_name()
                .is_none_or(|name| name != "Cargo.toml")
            {
                return Err("host test manifest must name Cargo.toml".into());
            }
            let manifest: toml_edit::DocumentMut =
                fs::read_to_string(root.join(&test.manifest))?.parse()?;
            if manifest
                .get("package")
                .and_then(|p| p.get("name"))
                .and_then(|v| v.as_str())
                .is_none()
            {
                return Err("host test manifest must declare a package name".into());
            }
            if test.filter.trim().is_empty() || test.filter.chars().any(char::is_control) {
                return Err(
                    "host test filter must be nonempty and contain no control characters".into(),
                );
            }
            if !selectors.insert((&test.manifest, &test.filter, &test.source)) {
                return Err("duplicate host test selector".into());
            }
        }
        let mut annotated = BTreeSet::new();
        for work in &self.gap_work {
            if gaps.iter().filter(|gap| gap.id == work.gap).count() != 1 {
                return Err(format!("gap-work must name one existing gap: {}", work.gap).into());
            }
            if !annotated.insert(&work.gap) || work.reason.trim().is_empty() {
                return Err("gap-work requires a unique gap and nonempty reason".into());
            }
            if !matches!(
                work.kind,
                WorkKind::Research
                    | WorkKind::Implement
                    | WorkKind::HostTest
                    | WorkKind::Experiment
                    | WorkKind::MeasurementMethod
                    | WorkKind::InspectVendor
                    | WorkKind::ReviewGap
            ) {
                return Err(
                    "gap-work kind must describe declared work, not an evidence decision".into(),
                );
            }
        }
        Ok(())
    }
}
