//! Stage states, aggregation, and stable machine-readable reporting.

use super::{Command, Result};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Mode {
    Build,
    Check,
}

impl Mode {
    pub(super) fn parse(command: Command) -> Self {
        match command {
            Command::ProjectBuild => Self::Build,
            Command::ProjectCheck => Self::Check,
            _ => unreachable!("project pipeline received another command"),
        }
    }

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Build => "build",
            Self::Check => "check",
        }
    }

    pub(super) const fn generated_success(self) -> StageSuccess {
        match self {
            Self::Build => StageSuccess::Written,
            Self::Check => StageSuccess::Verified,
        }
    }

    pub(super) fn check_argument(self) -> Vec<String> {
        match self {
            Self::Build => Vec::new(),
            Self::Check => vec!["--check".to_owned()],
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StageSuccess {
    Written,
    Verified,
}

impl StageSuccess {
    const fn label(self) -> &'static str {
        match self {
            Self::Written => "written",
            Self::Verified => "verified",
        }
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

#[derive(Default)]
pub(crate) struct PipelineSummary {
    pub(crate) written: usize,
    pub(crate) verified: usize,
    pub(crate) failed: usize,
    pub(crate) blocked: usize,
    pub(crate) not_configured: usize,
}

impl PipelineSummary {
    fn record(&mut self, outcome: &StageOutcome) {
        match outcome {
            StageOutcome::Complete(StageSuccess::Written) => self.written += 1,
            StageOutcome::Complete(StageSuccess::Verified) => self.verified += 1,
            StageOutcome::Failed(_) => self.failed += 1,
            StageOutcome::Blocked(_) => self.blocked += 1,
            StageOutcome::NotConfigured(_) => self.not_configured += 1,
        }
    }

    pub(crate) const fn succeeded(&self) -> bool {
        self.failed == 0 && self.blocked == 0
    }
}

pub(crate) fn execute(
    name: &str,
    success: StageSuccess,
    action: impl FnOnce() -> Result<bool>,
) -> StageOutcome {
    match action() {
        Ok(true) => StageOutcome::Complete(success),
        Ok(false) => StageOutcome::Failed(format!("{name} reported an unsuccessful result")),
        Err(error) => StageOutcome::Failed(error.to_string()),
    }
}

pub(crate) fn report(name: &str, outcome: &StageOutcome, summary: &mut PipelineSummary) {
    summary.record(outcome);
    let (status, reason) = match outcome {
        StageOutcome::Complete(success) => (success.label(), None),
        StageOutcome::Failed(reason) => ("failed", Some(reason)),
        StageOutcome::Blocked(reason) => ("blocked", Some(reason)),
        StageOutcome::NotConfigured(reason) => ("not-configured", Some(reason)),
    };
    println!(
        "PROJECT-STAGE\tname={name}\tstatus={status}\treason={}",
        reason.map_or_else(|| "-".to_owned(), |reason| sanitize(reason))
    );
}

fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            '\t' | '\r' | '\n' => ' ',
            character => character,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dependency_states_do_not_treat_optional_absence_as_failure() {
        assert_eq!(
            StageOutcome::Complete(StageSuccess::Verified),
            StageOutcome::Complete(StageSuccess::Verified)
        );
        assert!(!StageOutcome::NotConfigured("optional".to_owned()).blocks_dependants());
        assert!(StageOutcome::Failed("stale".to_owned()).blocks_dependants());
        assert!(StageOutcome::Blocked("input".to_owned()).blocks_dependants());
    }

    #[test]
    fn diagnostic_reasons_remain_single_line_tsv() {
        assert_eq!(sanitize("first\tsecond\nthird"), "first second third");
    }
}
