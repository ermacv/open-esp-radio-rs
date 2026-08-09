//! Stable project-status model shared by application frontends.

use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LinkedIrProfileDetail {
    pub id: String,
    pub sources: Vec<String>,
    pub missing_sources: Vec<String>,
    pub entry_contract: String,
    pub contract_status: &'static str,
    pub contract_error: Option<String>,
    pub output: String,
    pub output_status: &'static str,
    pub output_error: Option<String>,
    pub functions: usize,
    pub registers: usize,
    pub field_candidates: usize,
    pub pseudo_rust: Option<String>,
    pub pseudo_status: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MmioRegionDetail {
    pub name: String,
    pub address_space: String,
    pub start: u64,
    pub end_exclusive: u64,
    pub permissions: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ArtifactDetail {
    pub role: String,
    pub status: &'static str,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub objects: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped_members: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol_facts: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum DetailValue {
    String(String),
    Unsigned(u64),
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
        Self::Unsigned(value as u64)
    }
}

impl From<u64> for DetailValue {
    fn from(value: u64) -> Self {
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
pub enum Readiness {
    Ready,
    Incomplete,
    NotConfigured,
    Invalid,
}

impl Readiness {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Incomplete => "incomplete",
            Self::NotConfigured => "not-configured",
            Self::Invalid => "invalid",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Component {
    pub name: String,
    pub status: Readiness,
    pub details: BTreeMap<String, DetailValue>,
    pub diagnostic: Option<String>,
}

impl Component {
    pub(crate) fn new(name: &'static str, status: Readiness) -> Self {
        Self {
            name: name.to_owned(),
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Phase {
    pub name: String,
    pub status: Readiness,
    pub components: Vec<Component>,
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
            name: name.to_owned(),
            status,
            components,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TargetIdentity {
    pub id: String,
    pub architecture: String,
    #[serde(rename = "calling_convention")]
    pub calling_convention: String,
    pub harness: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProjectStatusReport {
    pub project_id: String,
    pub manifest: String,
    pub target: TargetIdentity,
    pub overall: Readiness,
    pub phases: Vec<Phase>,
}

impl ProjectStatusReport {
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
