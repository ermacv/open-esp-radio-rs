//! Focused feature-assurance report over current generated evidence.

use std::{fmt::Write as _, path::Path};

use serde::Serialize;

use super::Result;
use crate::{
    ProjectSpec,
    cli::{ProjectFeatureArgs, output, table},
    qualification::{
        FeatureQualificationReport, FeatureQualificationStatus, FeatureTransactionReport,
    },
};

#[derive(Serialize)]
struct ProjectFeatureReport {
    schema: u32,
    command: &'static str,
    project: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    selected_phase: Option<String>,
    feature: FeatureQualificationReport,
    #[serde(skip_serializing_if = "Option::is_none")]
    review_draft: Option<String>,
}

pub(super) fn run(arguments: ProjectFeatureArgs, project: &ProjectSpec) -> Result<bool> {
    let mut feature = crate::qualification::evaluate(project)?
        .into_iter()
        .find(|feature| feature.id == arguments.feature)
        .ok_or_else(|| {
            crate::Error::invalid(format!(
                "unknown qualification feature {:?}",
                arguments.feature
            ))
        })?;
    if let Some(phase) = arguments.phase.as_deref() {
        select_phase(&mut feature, phase)?;
    }
    let review_draft = arguments
        .write_review_draft
        .as_deref()
        .map(|path| write_review_draft(path, &feature).map(|()| path.display().to_string()))
        .transpose()?;
    let passed = feature.status != FeatureQualificationStatus::Blocked;
    let report = ProjectFeatureReport {
        schema: 2,
        command: "project feature",
        project: project.id.clone(),
        selected_phase: arguments.phase,
        feature,
        review_draft,
    };
    output::render_report(&report, || render_human(&report));
    Ok(passed)
}

fn render_human(report: &ProjectFeatureReport) {
    let feature = &report.feature;
    outputln!("{}", output::heading("Feature assurance"));
    outputln!("Project: {}", report.project);
    outputln!("Feature: {}", feature.id);
    if let Some(phase) = &report.selected_phase {
        outputln!("Phase:   {phase}");
    }
    outputln!("Claim:   {}", feature.description);
    let outcome = match feature.status {
        FeatureQualificationStatus::Qualified => output::success(
            "QUALIFIED — every discovered transaction has a current reviewed disposition and proof",
        ),
        FeatureQualificationStatus::HardwareQualified => output::success(
            "HARDWARE-QUALIFIED — static assurance and current hardware evidence both pass",
        ),
        FeatureQualificationStatus::Blocked => {
            output::failure("BLOCKED — the configured feature boundary is incomplete or stale")
        }
    };
    outputln!("\n{outcome}");
    outputln!(
        "Surface: {}/{} transaction(s), {} proof requirement(s)",
        feature.covered_effects,
        feature.surface_effects,
        feature.requirements
    );

    if let Some(hardware) = &feature.hardware {
        outputln!("\n{}", output::heading("Hardware evidence"));
        outputln!(
            "Status: {} — {}/{} successful run(s)",
            hardware.status,
            hardware.successful_runs,
            hardware.minimum_successful_runs
        );
        for blocker in &hardware.blockers {
            outputln!("- {blocker}");
        }
    }

    if !feature.blockers.is_empty() {
        outputln!("\n{}", output::heading("Problems"));
        let limit = if output::details() {
            feature.blockers.len()
        } else {
            5
        };
        for (index, blocker) in feature.blockers.iter().take(limit).enumerate() {
            outputln!("{}. {blocker}", index + 1);
        }
        if feature.blockers.len() > limit {
            outputln!(
                "{} more problem(s); use --details or --write-review-draft PATH.",
                feature.blockers.len() - limit
            );
        }
    }

    outputln!("\n{}", output::heading("Lifecycle"));
    outputln!(
        "{}",
        table::render(
            ["Phase", "Status", "Transactions", "Proofs"],
            feature.phases.iter().map(|phase| [
                phase.id.clone(),
                if phase.blockers.is_empty() {
                    "ready".to_owned()
                } else {
                    "blocked".to_owned()
                },
                format!("{}/{}", phase.covered_transactions, phase.transactions),
                phase.requirements.to_string(),
            ]),
        )
    );

    outputln!("\n{}", output::heading("Vendor → Rust transactions"));
    if feature.transactions.is_empty() {
        outputln!("No transactions were discovered for this feature.");
    } else {
        let limit = if output::details() {
            feature.transactions.len()
        } else {
            12
        };
        outputln!(
            "{}",
            table::render(
                ["Phase", "Vendor", "Disposition"],
                feature.transactions.iter().take(limit).map(|transaction| [
                    transaction.phase.clone(),
                    format!("{}:{}", transaction.source, transaction.symbol),
                    format!(
                        "{} ({})",
                        transaction.disposition,
                        if transaction.current {
                            "current"
                        } else {
                            "stale"
                        }
                    ),
                ]),
            )
        );
        if feature.transactions.len() > limit {
            outputln!(
                "Showing {limit} of {} transactions; use --details for the complete list.",
                feature.transactions.len()
            );
        }
    }

    if output::details() {
        for transaction in &feature.transactions {
            render_transaction(transaction);
        }
    }

    if !feature.blockers.is_empty() {
        outputln!("\n{}", output::heading("Next"));
        outputln!("1. {}", next_action(&feature.blockers[0]));
    }
    if let Some(path) = &report.review_draft {
        outputln!("\nReview draft written: {path}");
    }
}

fn select_phase(feature: &mut FeatureQualificationReport, selected: &str) -> Result<()> {
    let phase = feature
        .phases
        .iter()
        .find(|phase| phase.id == selected)
        .cloned()
        .ok_or_else(|| {
            let known = feature
                .phases
                .iter()
                .map(|phase| phase.id.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            crate::Error::invalid(format!(
                "qualification feature {:?} has no phase {selected:?}; expected one of: {known}",
                feature.id
            ))
        })?;
    feature.scopes = phase.scopes.clone();
    feature.requirements = phase.requirements;
    feature.surface_effects = phase.transactions;
    feature.covered_effects = phase.covered_transactions;
    feature
        .transactions
        .retain(|transaction| transaction.phase == selected);
    feature.blockers = phase.blockers.clone();
    feature.phases = vec![phase];
    feature.hardware = None;
    feature.status = if feature.blockers.is_empty() {
        FeatureQualificationStatus::Qualified
    } else {
        FeatureQualificationStatus::Blocked
    };
    Ok(())
}

fn next_action(blocker: &str) -> String {
    if blocker.contains("cannot load suite") || blocker.contains("has no result in suite") {
        "Run `project verify`, then inspect the referenced suite and production binding.".to_owned()
    } else if blocker.contains("new transaction")
        || blocker.contains("changed:")
        || blocker.contains("is stale")
    {
        "Write a phase-scoped review candidate with `--write-review-draft PATH`, then review its fingerprint and effects.".to_owned()
    } else if blocker.contains("analysis scope") {
        "Run `project analyze`, then inspect the named review scope and its first semantic blocker."
            .to_owned()
    } else {
        "Inspect the first blocker above, repair its production proof, and rerun this phase."
            .to_owned()
    }
}

fn render_transaction(transaction: &FeatureTransactionReport) {
    outputln!(
        "\n{}",
        output::heading(format!("Transaction {}", transaction.id))
    );
    outputln!("Fingerprint: {}", transaction.fingerprint);
    if let Some(requirement) = &transaction.requirement {
        outputln!("Proof:       {requirement}");
    }
    if !transaction.rationale.is_empty() {
        outputln!("Rationale:   {}", transaction.rationale);
    }
    for path in &transaction.paths {
        outputln!("Path:        {}", path.join(" → "));
    }
    for effect in &transaction.effects {
        let site = effect
            .site
            .map_or_else(|| "-".to_owned(), |site| format!("{site:#010x}"));
        let value = effect.value.as_deref().unwrap_or("-");
        outputln!(
            "  {site} {:<13} {:<18} {} value={value}",
            effect.kind,
            effect.operation,
            effect.target,
        );
    }
}

fn write_review_draft(path: &Path, feature: &FeatureQualificationReport) -> Result<()> {
    let mut draft = String::new();
    writeln!(draft, "# Review candidate for feature {}", feature.id).unwrap();
    writeln!(
        draft,
        "# Copy reviewed entries into the schema-4 feature pack. REVIEW values are intentionally invalid."
    )
    .unwrap();
    for transaction in &feature.transactions {
        writeln!(draft, "\n[[features.effects]]").unwrap();
        writeln!(draft, "id = {}", toml_string(&draft_id(transaction))).unwrap();
        writeln!(draft, "phase = {}", toml_string(&transaction.phase)).unwrap();
        write!(
            draft,
            "vendor = {{ source = {}, symbol = {}",
            toml_string(&transaction.source),
            toml_string(&transaction.symbol)
        )
        .unwrap();
        if let Some(identity) = &transaction.identity {
            write!(draft, ", identity = {}", toml_string(identity)).unwrap();
        }
        writeln!(
            draft,
            ", fingerprint = {} }}",
            toml_string(&transaction.fingerprint)
        )
        .unwrap();
        writeln!(
            draft,
            "disposition = {}",
            toml_string(if transaction.disposition == "missing" {
                "REVIEW"
            } else {
                &transaction.disposition
            })
        )
        .unwrap();
        if let Some(requirement) = &transaction.requirement {
            writeln!(draft, "requirement = {}", toml_string(requirement)).unwrap();
        }
        writeln!(
            draft,
            "rationale = {}",
            toml_string(if transaction.rationale.is_empty() {
                "REVIEW: explain why this transaction is preserved or intentionally excluded"
            } else {
                &transaction.rationale
            })
        )
        .unwrap();
    }
    crate::application::generated_file::write_or_check(path, &draft, false, "feature review draft")
}

fn draft_id(transaction: &FeatureTransactionReport) -> String {
    let identity = transaction.identity.as_deref().unwrap_or(&transaction.id);
    encode_id(identity)
}

fn encode_id(identity: &str) -> String {
    let mut id = String::new();
    for byte in identity.bytes() {
        if byte.is_ascii_alphanumeric() {
            id.push(char::from(byte).to_ascii_lowercase());
        } else {
            write!(id, "-{byte:02x}").expect("writing to a String cannot fail");
        }
    }
    id
}

fn toml_string(value: &str) -> String {
    toml_edit::Value::from(value).to_string()
}

#[cfg(test)]
mod tests {
    use super::{encode_id, select_phase};
    use crate::qualification::{
        FeatureCoverage, FeaturePhaseReport, FeatureQualificationReport, FeatureQualificationStatus,
    };

    #[test]
    fn review_ids_escape_punctuation_without_collisions() {
        assert_ne!(encode_id("wifi::foo-a"), encode_id("wifi::foo_a"));
        assert_ne!(encode_id("wifi::foo@0x10"), encode_id("wifi::foo@0x20"));
    }

    #[test]
    fn phase_selection_reduces_the_report_to_that_lifecycle_slice() {
        let mut report = qualification_report();

        select_phase(&mut report, "stop").unwrap();

        assert_eq!(report.scopes, ["stop-scope"]);
        assert_eq!(report.requirements, 1);
        assert_eq!(report.surface_effects, 1);
        assert_eq!(report.covered_effects, 1);
        assert_eq!(report.phases.len(), 1);
        assert_eq!(report.phases[0].id, "stop");
        assert!(report.hardware.is_none());
        assert_eq!(report.status, FeatureQualificationStatus::Qualified);
    }

    #[test]
    fn phase_selection_reports_the_known_phase_ids() {
        let mut report = qualification_report();

        let error = select_phase(&mut report, "missing").unwrap_err();

        assert!(error.to_string().contains("expected one of: start, stop"));
    }

    fn qualification_report() -> FeatureQualificationReport {
        FeatureQualificationReport {
            id: "feature".to_owned(),
            description: "feature".to_owned(),
            required: true,
            status: FeatureQualificationStatus::Blocked,
            coverage: FeatureCoverage::ReviewScopes,
            scopes: vec!["start-scope".to_owned(), "stop-scope".to_owned()],
            requirements: 2,
            surface_effects: 2,
            covered_effects: 1,
            phases: vec![
                FeaturePhaseReport {
                    id: "start".to_owned(),
                    description: "start".to_owned(),
                    scopes: vec!["start-scope".to_owned()],
                    requirements: 1,
                    transactions: 1,
                    covered_transactions: 0,
                    blockers: vec!["start blocker".to_owned()],
                },
                FeaturePhaseReport {
                    id: "stop".to_owned(),
                    description: "stop".to_owned(),
                    scopes: vec!["stop-scope".to_owned()],
                    requirements: 1,
                    transactions: 1,
                    covered_transactions: 1,
                    blockers: Vec::new(),
                },
            ],
            transactions: Vec::new(),
            hardware: None,
            blockers: vec!["start blocker".to_owned()],
        }
    }
}
