//! Stage states, aggregation, and stable machine-readable reporting.

use serde::Serialize;

use super::Result;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Mode {
    Write,
    Check,
}

impl Mode {
    pub(super) const fn from_check(check: bool) -> Self {
        if check { Self::Check } else { Self::Write }
    }

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Write => "write",
            Self::Check => "check",
        }
    }

    pub(super) const fn generated_success(self) -> StageSuccess {
        match self {
            Self::Write => StageSuccess::Written,
            Self::Check => StageSuccess::Verified,
        }
    }

    pub(super) const fn is_check(self) -> bool {
        matches!(self, Self::Check)
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

#[derive(Serialize)]
pub(crate) struct StageReport {
    pub(crate) name: String,
    pub(crate) status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<String>,
}

#[derive(Default)]
pub(crate) struct PipelineSummary {
    stages: Vec<StageReport>,
    pub(crate) written: usize,
    pub(crate) verified: usize,
    pub(crate) failed: usize,
    pub(crate) blocked: usize,
    pub(crate) not_configured: usize,
}

impl PipelineSummary {
    fn record(&mut self, name: &str, outcome: &StageOutcome) {
        let (status, reason) = outcome_fields(outcome);
        self.stages.push(StageReport {
            name: name.to_owned(),
            status,
            reason: reason.map(str::to_owned),
        });
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

    pub(crate) fn stages(&self) -> &[StageReport] {
        &self.stages
    }
}

pub(crate) fn execute(
    name: &str,
    success: StageSuccess,
    action: impl FnOnce() -> Result<bool>,
) -> StageOutcome {
    let span = crate::cli::progress::stage_span(name);
    let _entered = span.enter();
    tracing::info!("started");
    let outcome = match crate::cli::output::suppress(action) {
        Ok(true) => StageOutcome::Complete(success),
        Ok(false) => StageOutcome::Failed(format!("{name} reported an unsuccessful result")),
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

pub(crate) fn record(name: &str, outcome: &StageOutcome, summary: &mut PipelineSummary) {
    summary.record(name, outcome);
}

pub(super) fn render(mode: Mode, summary: &PipelineSummary) {
    let document = AnalysisDocument {
        schema: 1,
        command: "project analyze",
        mode: mode.label(),
        status: if summary.succeeded() { "ok" } else { "failed" },
        stages: &summary.stages,
        written: summary.written,
        verified: summary.verified,
        failed: summary.failed,
        blocked: summary.blocked,
        not_configured: summary.not_configured,
    };
    crate::cli::output::render_report(
        &document,
        || print_human(&document),
        || print_tsv(&document),
    );
}

fn print_human(document: &AnalysisDocument<'_>) {
    outputln!("Project analysis: {} ({})", document.status, document.mode);
    for stage in document.stages {
        outputln!(
            "  {:<24} {:<14} {}",
            stage.name,
            stage.status,
            stage.reason.as_deref().unwrap_or("")
        );
    }
    outputln!(
        "  written={} verified={} failed={} blocked={} not-configured={}",
        document.written,
        document.verified,
        document.failed,
        document.blocked,
        document.not_configured
    );
}

fn print_tsv(document: &AnalysisDocument<'_>) {
    for stage in document.stages {
        print_stage_tsv(stage);
    }
    outputln!(
        "PROJECT-ANALYSIS\tmode={}\tstatus={}\twritten={}\tverified={}\tfailed={}\tblocked={}\tnot-configured={}",
        document.mode,
        document.status,
        document.written,
        document.verified,
        document.failed,
        document.blocked,
        document.not_configured
    );
}

pub(crate) fn print_stage_tsv(stage: &StageReport) {
    outputln!(
        "PROJECT-STAGE\tname={}\tstatus={}\treason={}",
        stage.name,
        stage.status,
        stage
            .reason
            .as_deref()
            .map_or_else(|| "-".to_owned(), sanitize)
    );
}

fn outcome_fields(outcome: &StageOutcome) -> (&'static str, Option<&str>) {
    match outcome {
        StageOutcome::Complete(success) => (success.label(), None),
        StageOutcome::Failed(reason) => ("failed", Some(reason.as_str())),
        StageOutcome::Blocked(reason) => ("blocked", Some(reason.as_str())),
        StageOutcome::NotConfigured(reason) => ("not-configured", Some(reason.as_str())),
    }
}

#[derive(Serialize)]
struct AnalysisDocument<'a> {
    schema: u32,
    command: &'static str,
    mode: &'static str,
    status: &'static str,
    stages: &'a [StageReport],
    written: usize,
    verified: usize,
    failed: usize,
    blocked: usize,
    #[serde(rename = "not-configured")]
    not_configured: usize,
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

    #[test]
    fn analysis_document_keeps_stage_states_and_counts_typed() {
        let mut summary = PipelineSummary::default();
        record(
            "linked-ir",
            &StageOutcome::Complete(StageSuccess::Written),
            &mut summary,
        );
        record(
            "function-review",
            &StageOutcome::Blocked("linked IR unavailable".to_owned()),
            &mut summary,
        );
        let document = AnalysisDocument {
            schema: 1,
            command: "project analyze",
            mode: Mode::Write.label(),
            status: "failed",
            stages: &summary.stages,
            written: summary.written,
            verified: summary.verified,
            failed: summary.failed,
            blocked: summary.blocked,
            not_configured: summary.not_configured,
        };
        let value = serde_json::to_value(document).unwrap();
        assert_eq!(value["command"], "project analyze");
        assert_eq!(value["mode"], "write");
        assert_eq!(value["written"], 1);
        assert_eq!(value["blocked"], 1);
        assert_eq!(value["stages"][1]["status"], "blocked");
        assert_eq!(value["stages"][1]["reason"], "linked IR unavailable");
    }
}
