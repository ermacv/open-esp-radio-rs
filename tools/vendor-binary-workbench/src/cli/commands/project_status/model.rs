//! Small stable status model shared by text and JSON renderers.

use serde_json::{Map, Value};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
    pub(super) details: Map<String, Value>,
    pub(super) diagnostic: Option<String>,
}

impl Component {
    pub(super) fn new(name: &'static str, status: Readiness) -> Self {
        Self {
            name,
            status,
            details: Map::new(),
            diagnostic: None,
        }
    }

    pub(super) fn detail(mut self, key: &str, value: impl Into<Value>) -> Self {
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

#[derive(Debug, Eq, PartialEq)]
pub(super) struct TargetIdentity {
    pub(super) id: String,
    pub(super) architecture: String,
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
