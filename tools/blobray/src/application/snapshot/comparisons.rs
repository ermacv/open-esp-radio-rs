//! Verification-profile projection for the read-only workspace snapshot.

use std::collections::BTreeSet;

use super::{ProjectSession, push_error};
use crate::application::model::{ComparisonProfileSummary, DiagnosticRecord, DiagnosticSeverity};

pub(super) fn collect(
    resolved: &ProjectSession,
    diagnostics: &mut Vec<DiagnosticRecord>,
) -> Vec<ComparisonProfileSummary> {
    let Some(workspace) = resolved.project.verification.as_ref() else {
        return Vec::new();
    };
    let mut output = Vec::new();
    let mut names = BTreeSet::new();
    for suite in &workspace.suites {
        for path in &suite.profiles {
            let profiles = match crate::verification::profiles::load(path) {
                Ok(profiles) => profiles,
                Err(error) => {
                    push_error(
                        diagnostics,
                        "verification.profiles",
                        error,
                        Some(path.clone()),
                    );
                    continue;
                }
            };
            for profile in profiles {
                if !names.insert(profile.name.clone()) {
                    diagnostics.push(DiagnosticRecord {
                        severity: DiagnosticSeverity::Error,
                        component: "verification.profiles".to_owned(),
                        message: format!("duplicate comparison profile {:?}", profile.name),
                        path: Some(path.clone()),
                    });
                    continue;
                }
                output.push(ComparisonProfileSummary {
                    name: profile.name,
                    path: path.clone(),
                    vendor_source: profile.vendor_source,
                    vendor_symbol: profile.vendor_symbol,
                    rust_symbol: profile.rust_symbol,
                    scenarios: profile.scenarios.len(),
                });
            }
        }
    }
    output.sort_by(|left, right| left.name.cmp(&right.name));
    output
}
