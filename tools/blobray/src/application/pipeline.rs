//! Frontend-neutral stage outcomes shared by project workflows.

use serde::Serialize;
use tracing_indicatif::span_ext::IndicatifSpanExt;

use crate::Result;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WorkflowMode {
    Write,
    Check,
}

impl WorkflowMode {
    pub(crate) const fn from_check(check: bool) -> Self {
        if check { Self::Check } else { Self::Write }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Write => "write",
            Self::Check => "check",
        }
    }

    pub(crate) const fn generated_success(self) -> StageSuccess {
        match self {
            Self::Write => StageSuccess::Written,
            Self::Check => StageSuccess::Verified,
        }
    }

    pub(crate) const fn is_check(self) -> bool {
        matches!(self, Self::Check)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StageSuccess {
    Written,
    Verified,
    Current,
}

impl StageSuccess {
    const fn label(self) -> &'static str {
        match self {
            Self::Written => "written",
            Self::Verified => "verified",
            Self::Current => "up-to-date",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StageRun {
    Executed,
    Current,
}

pub(crate) trait StageActionResult {
    fn stage_run(self) -> Option<StageRun>;
}

impl StageActionResult for StageRun {
    fn stage_run(self) -> Option<StageRun> {
        Some(self)
    }
}

impl StageActionResult for bool {
    fn stage_run(self) -> Option<StageRun> {
        self.then_some(StageRun::Executed)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum StageOutcome {
    Complete(StageSuccess),
    Failed(String),
    Blocked(String),
    NotConfigured(String),
}

impl StageOutcome {
    pub(crate) const fn blocks_dependants(&self) -> bool {
        matches!(self, Self::Failed(_) | Self::Blocked(_))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct StageReport {
    pub(crate) name: String,
    pub(crate) status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub(crate) struct PipelineSummary {
    stages: Vec<StageReport>,
    pub(crate) written: usize,
    pub(crate) verified: usize,
    #[serde(rename = "up-to-date")]
    pub(crate) current: usize,
    pub(crate) failed: usize,
    pub(crate) blocked: usize,
    #[serde(rename = "not-configured")]
    pub(crate) not_configured: usize,
}

impl PipelineSummary {
    pub(crate) fn record(&mut self, name: &str, outcome: &StageOutcome) {
        let (status, reason) = outcome_fields(outcome);
        self.stages.push(StageReport {
            name: name.to_owned(),
            status,
            reason: reason.map(str::to_owned),
        });
        match outcome {
            StageOutcome::Complete(StageSuccess::Written) => self.written += 1,
            StageOutcome::Complete(StageSuccess::Verified) => self.verified += 1,
            StageOutcome::Complete(StageSuccess::Current) => self.current += 1,
            StageOutcome::Failed(_) => self.failed += 1,
            StageOutcome::Blocked(_) => self.blocked += 1,
            StageOutcome::NotConfigured(_) => self.not_configured += 1,
        }
    }

    pub(crate) const fn succeeded(&self) -> bool {
        self.failed == 0 && self.blocked == 0
    }

    pub(crate) fn stages(&self) -> &[StageReport] {
        &self.stages
    }
}

pub(crate) fn execute<T: StageActionResult>(
    name: &str,
    success: StageSuccess,
    action: impl FnOnce() -> Result<T>,
) -> StageOutcome {
    let span = tracing::info_span!(
        "project_stage",
        indicatif.pb_show = tracing::field::Empty,
        stage = name
    );
    span.pb_set_message(name);
    let _entered = span.enter();
    tracing::info!("started");
    let outcome = match action() {
        Ok(value) => match value.stage_run() {
            Some(StageRun::Executed) => StageOutcome::Complete(success),
            Some(StageRun::Current) => StageOutcome::Complete(StageSuccess::Current),
            None => StageOutcome::Failed(format!("{name} reported an unsuccessful result")),
        },
        Err(error) => StageOutcome::Failed(error.to_string()),
    };
    match &outcome {
        StageOutcome::Complete(_) => tracing::info!("completed"),
        StageOutcome::Failed(reason) => tracing::warn!(%reason, "failed"),
        StageOutcome::Blocked(reason) => tracing::warn!(%reason, "blocked"),
        StageOutcome::NotConfigured(reason) => tracing::debug!(%reason, "not configured"),
    }
    outcome
}

fn outcome_fields(outcome: &StageOutcome) -> (&'static str, Option<&str>) {
    match outcome {
        StageOutcome::Complete(success) => (success.label(), None),
        StageOutcome::Failed(reason) => ("failed", Some(reason.as_str())),
        StageOutcome::Blocked(reason) => ("blocked", Some(reason.as_str())),
        StageOutcome::NotConfigured(reason) => ("not-configured", Some(reason.as_str())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dependency_states_keep_optional_absence_distinct_from_failure() {
        assert!(!StageOutcome::NotConfigured("optional".to_owned()).blocks_dependants());
        assert!(StageOutcome::Failed("stale".to_owned()).blocks_dependants());
        assert!(StageOutcome::Blocked("input".to_owned()).blocks_dependants());
    }

    #[test]
    fn summary_preserves_typed_stage_counts() {
        let mut summary = PipelineSummary::default();
        summary.record("generated", &StageOutcome::Complete(StageSuccess::Written));
        summary.record("current", &StageOutcome::Complete(StageSuccess::Current));
        summary.record("blocked", &StageOutcome::Blocked("missing".to_owned()));
        assert_eq!(summary.written, 1);
        assert_eq!(summary.current, 1);
        assert_eq!(summary.blocked, 1);
        assert!(!summary.succeeded());
    }
}
