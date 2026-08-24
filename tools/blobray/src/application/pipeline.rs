//! Frontend-neutral stage outcomes shared by project workflows.

use std::time::Instant;

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

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct StageExecution {
    outcome: StageOutcome,
    duration_ms: Option<u64>,
}

impl StageExecution {
    pub(crate) const fn unmeasured(outcome: StageOutcome) -> Self {
        Self {
            outcome,
            duration_ms: None,
        }
    }

    pub(crate) const fn blocks_dependants(&self) -> bool {
        self.outcome.blocks_dependants()
    }

    pub(crate) fn failed(reason: impl Into<String>) -> Self {
        Self::unmeasured(StageOutcome::Failed(reason.into()))
    }

    pub(crate) fn blocked(reason: impl Into<String>) -> Self {
        Self::unmeasured(StageOutcome::Blocked(reason.into()))
    }

    pub(crate) fn not_configured(reason: impl Into<String>) -> Self {
        Self::unmeasured(StageOutcome::NotConfigured(reason.into()))
    }
}

impl From<StageOutcome> for StageExecution {
    fn from(outcome: StageOutcome) -> Self {
        Self::unmeasured(outcome)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct StageReport {
    pub(crate) name: String,
    pub(crate) status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) duration_ms: Option<u64>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) duration_ms: Option<u64>,
}

impl PipelineSummary {
    pub(crate) fn record(&mut self, name: &str, execution: &StageExecution) {
        let outcome = &execution.outcome;
        let (status, reason) = outcome_fields(outcome);
        self.stages.push(StageReport {
            name: name.to_owned(),
            status,
            duration_ms: execution.duration_ms,
            reason: reason.map(str::to_owned),
        });
        if let Some(duration_ms) = execution.duration_ms {
            self.duration_ms = Some(
                self.duration_ms
                    .unwrap_or_default()
                    .saturating_add(duration_ms),
            );
        }
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
) -> StageExecution {
    execute_with_timer(name, success, action, &MonotonicTimer)
}

trait StageTimer {
    type Mark;

    fn start(&self) -> Self::Mark;
    fn elapsed_ms(&self, mark: Self::Mark) -> u64;
}

struct MonotonicTimer;

impl StageTimer for MonotonicTimer {
    type Mark = Instant;

    fn start(&self) -> Self::Mark {
        Instant::now()
    }

    fn elapsed_ms(&self, mark: Self::Mark) -> u64 {
        mark.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
    }
}

fn execute_with_timer<T: StageActionResult>(
    name: &str,
    success: StageSuccess,
    action: impl FnOnce() -> Result<T>,
    timer: &impl StageTimer,
) -> StageExecution {
    let span = tracing::info_span!(
        "project_stage",
        indicatif.pb_show = tracing::field::Empty,
        stage = name
    );
    span.pb_set_message(name);
    let _entered = span.enter();
    tracing::info!("started");
    let started = timer.start();
    let result = action();
    let duration_ms = timer.elapsed_ms(started);
    let outcome = match result {
        Ok(value) => match value.stage_run() {
            Some(StageRun::Executed) => StageOutcome::Complete(success),
            Some(StageRun::Current) => StageOutcome::Complete(StageSuccess::Current),
            None => StageOutcome::Failed(format!("{name} reported an unsuccessful result")),
        },
        Err(error) => StageOutcome::Failed(error.to_string()),
    };
    match &outcome {
        StageOutcome::Complete(_) => tracing::info!(duration_ms, "completed"),
        StageOutcome::Failed(reason) => tracing::warn!(duration_ms, %reason, "failed"),
        StageOutcome::Blocked(reason) => tracing::warn!(duration_ms, %reason, "blocked"),
        StageOutcome::NotConfigured(reason) => {
            tracing::debug!(duration_ms, %reason, "not configured")
        }
    }
    StageExecution {
        outcome,
        duration_ms: Some(duration_ms),
    }
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
    use std::cell::Cell;

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
        summary.record(
            "generated",
            &StageExecution::unmeasured(StageOutcome::Complete(StageSuccess::Written)),
        );
        summary.record(
            "current",
            &StageExecution::unmeasured(StageOutcome::Complete(StageSuccess::Current)),
        );
        summary.record(
            "blocked",
            &StageExecution::unmeasured(StageOutcome::Blocked("missing".to_owned())),
        );
        assert_eq!(summary.written, 1);
        assert_eq!(summary.current, 1);
        assert_eq!(summary.blocked, 1);
        assert!(!summary.succeeded());
    }

    struct RecordingTimer<'a> {
        duration_ms: u64,
        started: Cell<bool>,
        action_completed: &'a Cell<bool>,
        elapsed: Cell<bool>,
    }

    impl StageTimer for RecordingTimer<'_> {
        type Mark = ();

        fn start(&self) -> Self::Mark {
            assert!(!self.action_completed.get());
            self.started.set(true);
        }

        fn elapsed_ms(&self, (): Self::Mark) -> u64 {
            assert!(self.action_completed.get());
            self.elapsed.set(true);
            self.duration_ms
        }
    }

    #[test]
    fn failed_actions_keep_their_measured_duration() {
        let action_completed = Cell::new(false);
        let timer = RecordingTimer {
            duration_ms: 37,
            started: Cell::new(false),
            action_completed: &action_completed,
            elapsed: Cell::new(false),
        };
        let execution = execute_with_timer::<StageRun>(
            "failed-stage",
            StageSuccess::Written,
            || {
                assert!(timer.started.get());
                action_completed.set(true);
                Err(crate::Error::invalid("boom"))
            },
            &timer,
        );
        assert!(timer.elapsed.get());
        assert_eq!(
            execution,
            StageExecution {
                outcome: StageOutcome::Failed("boom".to_owned()),
                duration_ms: Some(37),
            }
        );

        let mut summary = PipelineSummary::default();
        summary.record("failed-stage", &execution);
        assert_eq!(summary.duration_ms, Some(37));
        assert_eq!(summary.stages()[0].duration_ms, Some(37));
    }
}
