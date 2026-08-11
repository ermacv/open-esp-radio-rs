//! Function/context workspace readiness diagnostics.

use std::path::PathBuf;

use serde::Serialize;

use crate::{
    function_workspace::{FunctionFacts, FunctionWorkspace},
    project::ProjectSpec,
};

#[derive(Serialize)]
pub(super) struct FunctionDoctorReport {
    status: &'static str,
    profiles: usize,
    missing: usize,
    functions: usize,
    root_functions: usize,
    context_fields: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    review: Option<FunctionReviewCounts>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pack: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    review_output: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    errors: usize,
    warnings: usize,
}

impl FunctionDoctorReport {
    pub(super) const fn counts(&self) -> (usize, usize) {
        (self.errors, self.warnings)
    }

    pub(super) fn render_human(&self) {
        outputln!(
            "Function workspace: {} — profiles={} roots={} errors={} warnings={}",
            self.status,
            self.profiles,
            self.root_functions,
            self.errors,
            self.warnings
        );
        if let Some(error) = self.error.as_deref() {
            outputln!("  error: {error}");
        }
        if let Some(pack) = self.pack.as_deref() {
            outputln!("  pack: {}", pack.display());
        }
    }

    pub(super) fn issues(&self) -> Vec<String> {
        let mut issues = Vec::new();
        if self.missing != 0 {
            issues.push(format!(
                "function workspace is missing {} linked IR profile output(s)",
                self.missing
            ));
        }
        if let Some(error) = &self.error {
            issues.push(format!("function workspace: {error}"));
        } else if self.status == "pack-not-initialized" {
            issues.push("function review pack has not been initialized".to_owned());
        }
        issues
    }
}

#[derive(Serialize)]
struct FunctionReviewCounts {
    reviewed_functions: usize,
    ignored_functions: usize,
    unreviewed_functions: usize,
    reviewed_contexts: usize,
    ignored_contexts: usize,
    unreviewed_contexts: usize,
    reviewed_fields: usize,
    ignored_fields: usize,
    unreviewed_fields: usize,
    logical_types: usize,
    type_bindings: usize,
    reviewed_type_fields: usize,
    ignored_type_fields: usize,
    unreviewed_type_fields: usize,
    accepted_incomplete: usize,
}

impl FunctionDoctorReport {
    fn new(status: &'static str) -> Self {
        Self {
            status,
            profiles: 0,
            missing: 0,
            functions: 0,
            root_functions: 0,
            context_fields: 0,
            review: None,
            pack: None,
            review_output: None,
            error: None,
            errors: 0,
            warnings: 0,
        }
    }
}

pub(super) fn inspect(project: &ProjectSpec) -> FunctionDoctorReport {
    let Some(paths) = &project.functions else {
        return FunctionDoctorReport::new("not-configured");
    };
    let reports = match project.function_ir_reports() {
        Ok(reports) => reports,
        Err(error) => {
            let mut report = FunctionDoctorReport::new("invalid-config");
            report.error = Some(error.to_string());
            report.errors = 1;
            return report;
        }
    };
    let missing = reports
        .iter()
        .filter(|(_, report)| !report.is_dir())
        .count();
    if missing != 0 {
        let mut report = FunctionDoctorReport::new("not-generated");
        report.profiles = reports.len();
        report.missing = missing;
        report.pack = Some(paths.pack.clone());
        report.review_output = paths.review_output.clone();
        report.warnings = 1;
        return report;
    }
    // Doctor is a readiness query. The indexed review projection contains
    // every field needed for pack validation and avoids decoding the complete
    // provenance-heavy function stream.
    let facts = match FunctionFacts::load_summary(&reports) {
        Ok(facts) => facts,
        Err(error) => {
            let mut report = FunctionDoctorReport::new("invalid-facts");
            report.profiles = reports.len();
            report.error = Some(error.to_string());
            report.errors = 1;
            return report;
        }
    };
    if !paths.pack.is_file() {
        let root_functions = facts.root_functions().count();
        let context_fields = facts
            .root_functions()
            .map(|function| function.context_fields.len())
            .sum::<usize>();
        let mut report = FunctionDoctorReport::new("pack-not-initialized");
        report.profiles = reports.len();
        report.functions = facts.functions.len();
        report.root_functions = root_functions;
        report.context_fields = context_fields;
        report.pack = Some(paths.pack.clone());
        report.review_output = paths.review_output.clone();
        report.warnings = 1;
        return report;
    }
    match FunctionWorkspace::load_summary(&reports, &paths.pack) {
        Ok(workspace) => {
            let summary = workspace.summary();
            let mut report = FunctionDoctorReport::new("available");
            report.profiles = reports.len();
            report.root_functions = summary.observed_functions;
            report.review = Some(FunctionReviewCounts {
                reviewed_functions: summary.reviewed_functions,
                ignored_functions: summary.ignored_functions,
                unreviewed_functions: summary.unreviewed_functions,
                reviewed_contexts: summary.reviewed_contexts,
                ignored_contexts: summary.ignored_contexts,
                unreviewed_contexts: summary.unreviewed_contexts,
                reviewed_fields: summary.reviewed_fields,
                ignored_fields: summary.ignored_fields,
                unreviewed_fields: summary.unreviewed_fields,
                logical_types: summary.logical_types,
                type_bindings: summary.type_bindings,
                reviewed_type_fields: summary.reviewed_type_fields,
                ignored_type_fields: summary.ignored_type_fields,
                unreviewed_type_fields: summary.unreviewed_type_fields,
                accepted_incomplete: summary.accepted_incomplete,
            });
            report.pack = Some(paths.pack.clone());
            report.review_output = paths.review_output.clone();
            report
        }
        Err(error) => {
            let mut report = FunctionDoctorReport::new("invalid");
            report.profiles = reports.len();
            report.pack = Some(paths.pack.clone());
            report.error = Some(error.to_string());
            report.errors = 1;
            report
        }
    }
}
