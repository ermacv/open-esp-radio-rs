//! Human and machine view of vendor-to-production binding trust.

use crate::{Result, application::ProjectSession, cli::output, verification};

pub(super) fn run(session: &ProjectSession) -> Result<bool> {
    let provider = session
        .project
        .verification
        .as_ref()
        .map(|workspace| workspace.provider.as_str());
    let report = verification::audit(&session.project, provider)?;
    let passed = report.passed;
    output::render_report(&report, || render_human(&report));
    Ok(passed)
}

fn render_human(report: &verification::BindingAuditReport) {
    outputln!("{}", output::heading("Binding trust audit"));
    outputln!("Project:  {}", report.project);
    outputln!("Bindings: {}", report.bindings.len());
    let outcome = if report.passed {
        output::success(format!(
            "PASS — all {} required bindings are verification-ready; {} research-only",
            report.verification_required, report.research_only
        ))
    } else {
        output::failure(format!(
            "BLOCKED — {} required binding(s) lack qualifying verification proof; {} invalid declaration(s)",
            report.verification_blocked, report.invalid
        ))
    };
    outputln!("\n{outcome}");

    let blocked = report
        .bindings
        .iter()
        .filter(|binding| matches!(binding.status, "invalid" | "verification-blocked"))
        .collect::<Vec<_>>();
    if !blocked.is_empty() {
        outputln!("\n{}", output::heading("Trust blockers"));
        for (index, binding) in blocked.iter().enumerate() {
            outputln!(
                "{}. {}:{}",
                index + 1,
                binding.source,
                binding.vendor_symbol
            );
            outputln!(
                "   Status: {} · Oracle: {} · Rust: {} · Ceiling: {}",
                binding.status,
                binding.vendor_oracle.label(),
                binding.rust_binding.label(),
                binding.maximum_claim.label(),
            );
            outputln!(
                "   {}",
                binding
                    .blocker
                    .as_deref()
                    .unwrap_or("binding is not verification eligible")
            );
            if !binding.required_by.is_empty() {
                outputln!("   Required by: {}", binding.required_by.join(", "));
            }
        }
    }

    if output::details() {
        outputln!("\n{}", output::heading("All bindings"));
        for binding in &report.bindings {
            outputln!(
                "{}:{} [{}]",
                binding.source,
                binding.vendor_symbol,
                binding.suite
            );
            outputln!("   Production: {}", binding.rust_component);
            outputln!("   Probe:      {}", binding.rust_probe);
            outputln!("   Claim:      {}", binding.maximum_claim.label());
            outputln!("   Status:     {}", binding.status);
        }
    }

    if !report.passed {
        outputln!("\n{}", output::heading("Next"));
        outputln!(
            "1. Fix invalid claims/dispositions; their declared policy must match the registered trust ceiling."
        );
        outputln!(
            "2. For required bindings, replace verification projections with production entry/core and concrete vendor replay."
        );
        outputln!("3. Rerun `project audit bindings` before accepting evidence baselines.");
    }
}
