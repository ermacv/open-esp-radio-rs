//! Human and machine view of vendor-to-production binding declarations.

use crate::{Result, application::ProjectSession, cli::output, verification};

pub(super) fn run(session: &ProjectSession) -> Result<bool> {
    let report = verification::audit(&session.project)?;
    let passed = report.passed;
    output::render_report(&report, || render_human(&report));
    Ok(passed)
}

fn render_human(report: &verification::BindingAuditReport) {
    outputln!("{}", output::heading("Binding declaration audit"));
    outputln!("Project:  {}", report.project);
    outputln!("Bindings: {}", report.bindings.len());
    let outcome = if report.passed {
        output::success(format!(
            "PASS — {} binding declaration(s) are structurally valid; execution is not evaluated",
            report.declared
        ))
    } else {
        output::failure(format!(
            "BLOCKED — {} invalid binding declaration(s)",
            report.invalid
        ))
    };
    outputln!("\n{outcome}");

    let blocked = report
        .bindings
        .iter()
        .filter(|binding| binding.status == "invalid")
        .collect::<Vec<_>>();
    if !blocked.is_empty() {
        outputln!("\n{}", output::heading("Declaration blockers"));
        for (index, binding) in blocked.iter().enumerate() {
            outputln!(
                "{}. {}:{}",
                index + 1,
                binding.source,
                binding.vendor_symbol
            );
            outputln!(
                "   Status: {} · Rust: {} · Disposition: {}",
                binding.status,
                binding.rust_binding.label(),
                binding.disposition,
            );
            outputln!(
                "   {}",
                binding
                    .blocker
                    .as_deref()
                    .unwrap_or("binding declaration is invalid")
            );
            if !binding.required_by.is_empty() {
                outputln!("   Required by: {}", binding.required_by.join(", "));
            }
        }
    }
    if !report.unbound_requirements.is_empty() {
        outputln!(
            "\n{}",
            output::heading("Policy requirements without bindings")
        );
        for (index, requirement) in report.unbound_requirements.iter().enumerate() {
            outputln!(
                "{}. {}:{} [{}]",
                index + 1,
                requirement.source,
                requirement.vendor_symbol,
                requirement.suite,
            );
            outputln!("   Required by: {}", requirement.required_by.join(", "));
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
            outputln!(
                "   Claims:     {}",
                binding
                    .declared_claims
                    .iter()
                    .map(|claim| claim.label())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            outputln!("   Status:     {}", binding.status);
        }
    }

    if !report.passed {
        outputln!("\n{}", output::heading("Next"));
        outputln!("1. Fix invalid binding, claim, and disposition declarations.");
        outputln!("2. Run `project verify` to obtain execution observations and verdicts.");
        outputln!("3. Rerun `project audit bindings` before accepting evidence baselines.");
    }
}
