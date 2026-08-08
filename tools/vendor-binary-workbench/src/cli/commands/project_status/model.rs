//! Small stable status model shared by text and JSON renderers.

use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Eq, PartialEq, Serialize)]
pub(super) struct LinkedIrProfileDetail {
    pub(super) id: String,
    pub(super) sources: Vec<String>,
    pub(super) missing_sources: Vec<String>,
    pub(super) entry_contract: String,
    pub(super) contract_status: &'static str,
    pub(super) contract_error: Option<String>,
    pub(super) output: String,
    pub(super) output_status: &'static str,
    pub(super) output_error: Option<String>,
    pub(super) functions: usize,
    pub(super) registers: usize,
    pub(super) field_candidates: usize,
    pub(super) pseudo_rust: Option<String>,
    pub(super) pseudo_status: &'static str,
}

#[derive(Debug, Eq, PartialEq, Serialize)]
pub(super) struct MmioRegionDetail {
    pub(super) name: String,
    pub(super) address_space: String,
    pub(super) start: u64,
    pub(super) end_exclusive: u64,
    pub(super) permissions: String,
}

#[derive(Debug, Eq, PartialEq, Serialize)]
pub(super) struct ArtifactDetail {
    pub(super) role: String,
    pub(super) status: &'static str,
    pub(super) path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) container: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) objects: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) skipped_members: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) symbol_facts: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) error: Option<String>,
}

#[derive(Debug, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub(super) enum DetailValue {
    String(String),
    Unsigned(usize),
    Bool(bool),
    Strings(Vec<String>),
    LinkedIrProfiles(Vec<LinkedIrProfileDetail>),
    MmioRegions(Vec<MmioRegionDetail>),
    Artifacts(Vec<ArtifactDetail>),
}

impl From<String> for DetailValue {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<&str> for DetailValue {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}

impl From<usize> for DetailValue {
    fn from(value: usize) -> Self {
        Self::Unsigned(value)
    }
}

impl From<bool> for DetailValue {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<Vec<String>> for DetailValue {
    fn from(value: Vec<String>) -> Self {
        Self::Strings(value)
    }
}

impl From<Vec<LinkedIrProfileDetail>> for DetailValue {
    fn from(value: Vec<LinkedIrProfileDetail>) -> Self {
        Self::LinkedIrProfiles(value)
    }
}

impl From<Vec<MmioRegionDetail>> for DetailValue {
    fn from(value: Vec<MmioRegionDetail>) -> Self {
        Self::MmioRegions(value)
    }
}

impl From<Vec<ArtifactDetail>> for DetailValue {
    fn from(value: Vec<ArtifactDetail>) -> Self {
        Self::Artifacts(value)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum Readiness {
    Ready,
    Incomplete,
    NotConfigured,
    Invalid,
}

impl Readiness {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Incomplete => "incomplete",
            Self::NotConfigured => "not-configured",
            Self::Invalid => "invalid",
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct Component {
    pub(super) name: &'static str,
    pub(super) status: Readiness,
    pub(super) details: BTreeMap<String, DetailValue>,
    pub(super) diagnostic: Option<String>,
}

impl Component {
    pub(super) fn new(name: &'static str, status: Readiness) -> Self {
        Self {
            name,
            status,
            details: BTreeMap::new(),
            diagnostic: None,
        }
    }

    pub(super) fn detail(mut self, key: &str, value: impl Into<DetailValue>) -> Self {
        self.details.insert(key.to_owned(), value.into());
        self
    }

    pub(super) fn diagnostic(mut self, value: impl ToString) -> Self {
        self.diagnostic = Some(value.to_string());
        self
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct Phase {
    pub(super) name: &'static str,
    pub(super) status: Readiness,
    pub(super) components: Vec<Component>,
}

impl Phase {
    pub(super) fn collect(name: &'static str, components: Vec<Component>) -> Self {
        let status = if components
            .iter()
            .any(|component| component.status == Readiness::Invalid)
        {
            Readiness::Invalid
        } else if components
            .iter()
            .any(|component| component.status == Readiness::Incomplete)
        {
            Readiness::Incomplete
        } else if components
            .iter()
            .any(|component| component.status == Readiness::Ready)
        {
            Readiness::Ready
        } else {
            Readiness::NotConfigured
        };
        Self {
            name,
            status,
            components,
        }
    }
}

#[derive(Debug, Eq, PartialEq, Serialize)]
pub(super) struct TargetIdentity {
    pub(super) id: String,
    pub(super) architecture: String,
    #[serde(rename = "calling_convention")]
    pub(super) calling_convention: String,
    pub(super) harness: Option<String>,
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct StatusReport {
    pub(super) project_id: String,
    pub(super) manifest: String,
    pub(super) target: TargetIdentity,
    pub(super) overall: Readiness,
    pub(super) phases: Vec<Phase>,
}

impl StatusReport {
    pub(super) fn new(
        project_id: String,
        manifest: String,
        target: TargetIdentity,
        phases: Vec<Phase>,
    ) -> Self {
        let overall = if phases
            .iter()
            .any(|phase| phase.status == Readiness::Invalid)
        {
            Readiness::Invalid
        } else if phases
            .iter()
            .all(|phase| matches!(phase.status, Readiness::Ready | Readiness::NotConfigured))
        {
            Readiness::Ready
        } else {
            Readiness::Incomplete
        };
        Self {
            project_id,
            manifest,
            target,
            overall,
            phases,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optional_components_do_not_hide_incomplete_or_invalid_phases() {
        let ready = Component::new("ready", Readiness::Ready);
        let absent = Component::new("optional", Readiness::NotConfigured);
        assert_eq!(
            Phase::collect("phase", vec![ready, absent]).status,
            Readiness::Ready
        );
        assert_eq!(
            Phase::collect(
                "phase",
                vec![Component::new("missing", Readiness::Incomplete)]
            )
            .status,
            Readiness::Incomplete
        );
    }
}
