//! Durable revision ledger integrity and current-binding diagnostics.

use crate::application::{ProjectContext, revision};

use super::model::{CapabilityReport, DoctorReport};

pub(super) fn collect(context: &ProjectContext<'_>, report: &mut DoctorReport) {
    let inspection = revision::inspect_ledger(context.project_path, &context.project.id, true);
    use revision::RevisionLedgerHealth as Health;
    match inspection.health {
        Health::Missing | Health::BaselineMissing => {
            report.warning(
                inspection
                    .diagnostic
                    .clone()
                    .unwrap_or_else(|| "durable revision baseline is missing".to_owned()),
            );
            report.capability(
                CapabilityReport::new("revision-workflow", "baseline-missing")
                    .field("ledger", inspection.path)
                    .field("revisions", inspection.revisions)
                    .field(
                        "baseline",
                        inspection.baseline.unwrap_or_else(|| "-".to_owned()),
                    )
                    .field(
                        "current",
                        inspection.current.unwrap_or_else(|| "-".to_owned()),
                    ),
            );
        }
        Health::LegacyScope | Health::MigrationReviewPending | Health::RevisionReviewPending => {
            let status = match inspection.health {
                Health::MigrationReviewPending => "migration-review-pending",
                Health::RevisionReviewPending => "revision-review-pending",
                _ => "legacy-scope",
            };
            report.warning(
                inspection
                    .diagnostic
                    .clone()
                    .unwrap_or_else(|| "legacy revision scope requires migration".to_owned()),
            );
            report.capability(
                CapabilityReport::new("revision-workflow", status)
                    .field("ledger", inspection.path)
                    .field("revisions", inspection.revisions)
                    .field(
                        "baseline",
                        inspection.baseline.unwrap_or_else(|| "-".to_owned()),
                    )
                    .field(
                        "current",
                        inspection.current.unwrap_or_else(|| "-".to_owned()),
                    ),
            );
        }
        Health::Invalid => {
            report.error();
            report.capability(
                CapabilityReport::new("revision-workflow", "invalid")
                    .field("ledger", inspection.path)
                    .field(
                        "error",
                        inspection
                            .diagnostic
                            .unwrap_or_else(|| "invalid revision ledger".to_owned()),
                    ),
            );
        }
        Health::Ready => {
            let binding_status = if context.run_spec.is_some() {
                match revision::verify_ledger_bindings_from_context(context) {
                    Ok(verified) => ("available", verified, None),
                    Err(error) => {
                        report.warning(format!(
                            "current artifact bindings differ from the revision ledger: {error}"
                        ));
                        ("binding-drift", 0, Some(error.to_string()))
                    }
                }
            } else {
                ("available", 0, None)
            };
            let mut capability = CapabilityReport::new("revision-workflow", binding_status.0)
                .field("ledger", inspection.path)
                .field("revisions", inspection.revisions)
                .field(
                    "baseline",
                    inspection.baseline.unwrap_or_else(|| "-".to_owned()),
                )
                .field(
                    "current",
                    inspection.current.unwrap_or_else(|| "-".to_owned()),
                )
                .field("update-prepared", inspection.update_prepared)
                .field("artifact-identities-verified", binding_status.1);
            if let Some(error) = binding_status.2 {
                capability = capability.field("error", error);
            }
            report.capability(capability);
        }
    }
}
