//! Stable project-status model shared by application frontends.

use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Eq, PartialEq, Serialize)]
pub(crate) struct LinkedIrProfileDetail {
    pub(crate) id: String,
    pub(crate) sources: Vec<String>,
    pub(crate) missing_sources: Vec<String>,
    pub(crate) entry_contract: String,
    pub(crate) contract_status: &'static str,
    pub(crate) contract_error: Option<String>,
    pub(crate) output: String,
    pub(crate) output_status: &'static str,
    pub(crate) output_error: Option<String>,
    pub(crate) functions: usize,
    pub(crate) registers: usize,
    pub(crate) field_candidates: usize,
    pub(crate) pseudo_rust: Option<String>,
    pub(crate) pseudo_status: &'static str,
}

#[derive(Debug, Eq, PartialEq, Serialize)]
pub(crate) struct MmioRegionDetail {
    pub(crate) name: String,
    pub(crate) address_space: String,
    pub(crate) start: u64,
    pub(crate) end_exclusive: u64,
    pub(crate) permissions: String,
}

#[derive(Debug, Eq, PartialEq, Serialize)]
pub(crate) struct ArtifactDetail {
    pub(crate) role: String,
    pub(crate) status: &'static str,
    pub(crate) path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) container: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) objects: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) skipped_members: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) symbol_facts: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
}

#[derive(Debug, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub(crate) enum DetailValue {
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
pub(crate) enum Readiness {
    Ready,
    Incomplete,
    NotConfigured,
    Invalid,
}

impl Readiness {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Incomplete => "incomplete",
            Self::NotConfigured => "not-configured",
            Self::Invalid => "invalid",
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct Component {
    pub(crate) name: &'static str,
    pub(crate) status: Readiness,
    pub(crate) details: BTreeMap<String, DetailValue>,
    pub(crate) diagnostic: Option<String>,
}

impl Component {
    pub(crate) fn new(name: &'static str, status: Readiness) -> Self {
        Self {
            name,
            status,
            details: BTreeMap::new(),
            diagnostic: None,
        }
    }

    pub(crate) fn detail(mut self, key: &str, value: impl Into<DetailValue>) -> Self {
        self.details.insert(key.to_owned(), value.into());
        self
    }

    pub(crate) fn diagnostic(mut self, value: impl ToString) -> Self {
        self.diagnostic = Some(value.to_string());
        self
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct Phase {
    pub(crate) name: &'static str,
    pub(crate) status: Readiness,
    pub(crate) components: Vec<Component>,
}

impl Phase {
    pub(crate) fn collect(name: &'static str, components: Vec<Component>) -> Self {
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
pub(crate) struct TargetIdentity {
    pub(crate) id: String,
    pub(crate) architecture: String,
    #[serde(rename = "calling_convention")]
    pub(crate) calling_convention: String,
    pub(crate) harness: Option<String>,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct StatusReport {
    pub(crate) project_id: String,
    pub(crate) manifest: String,
    pub(crate) target: TargetIdentity,
    pub(crate) overall: Readiness,
    pub(crate) phases: Vec<Phase>,
}

impl StatusReport {
    pub(crate) fn new(
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
