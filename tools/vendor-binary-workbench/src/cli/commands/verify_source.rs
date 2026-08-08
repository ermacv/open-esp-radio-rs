//! Single-source verification command.

use super::super::*;

pub(super) fn run(arguments: VerifySourceArgs, svd: &MmioMap, target: &TargetSpec) -> Result<bool> {
    let vendor_artifact = arguments
        .vendor_artifact
        .ok_or("missing --vendor-artifact")
        .map_err(crate::Error::invalid)?;
    let rust_artifact = arguments
        .rust_artifact
        .ok_or("missing --rust-artifact")
        .map_err(crate::Error::invalid)?;
    let gate = VerificationGate::parse(&arguments.gate, arguments.match_floor)?;
    if matches!(gate, VerificationGate::Regression { .. }) && arguments.evidence_baseline.is_none()
    {
        return Err(crate::Error::invalid(
            "--gate regression requires --evidence-baseline",
        ));
    }
    let execution_profiles = arguments
        .profiles
        .as_deref()
        .map(profiles::load)
        .transpose()?
        .unwrap_or_default();
    let source = VerifySource {
        name: "vendor",
        artifact: &vendor_artifact,
        inventory: arguments.vendor_inventory.as_deref(),
        companion: arguments.vendor_companion.as_deref(),
        prefix: &arguments.vendor_prefix,
    };
    let symbols = vendor_symbols(source)?;
    let mut evidence = EvidenceSet::new();
    let harness = target.require_available_harness()?;
    let source_report = verify_source(
        svd,
        harness,
        &target.rust_target,
        source,
        &rust_artifact,
        arguments.rust_companion.as_deref(),
        &arguments.rust_prefix,
        &execution_profiles,
        None,
        &mut evidence,
    )?;
    let orphan_probes = orphan_probe_count(
        &rust_artifact,
        &arguments.rust_prefix,
        &[(source, &symbols)],
        &BTreeSet::new(),
    )?;
    let evidence_comparison = arguments
        .evidence_baseline
        .as_deref()
        .map(load_evidence_baseline)
        .transpose()?
        .map(|baseline| compare_evidence_baseline(&baseline, &evidence));
    let evidence_passed = evidence_comparison
        .as_ref()
        .is_none_or(|comparison| comparison.passed);
    let passed = gate.passes(source_report.summary, orphan_probes) && evidence_passed;
    let mut artifacts = vec![
        ("vendor-artifact", vendor_artifact.as_path()),
        ("rust-probes", rust_artifact.as_path()),
    ];
    if let Some(path) = arguments.vendor_inventory.as_deref() {
        artifacts.push(("vendor-inventory", path));
    }
    if let Some(path) = arguments.vendor_companion.as_deref() {
        artifacts.push(("vendor-companion", path));
    }
    if let Some(path) = arguments.rust_companion.as_deref() {
        artifacts.push(("rust-companion", path));
    }
    if let Some(path) = arguments.profiles.as_deref() {
        artifacts.push(("profiles", path));
    }
    if let Some(path) = arguments.evidence_baseline.as_deref() {
        artifacts.push(("evidence-baseline", path));
    }
    let verification = verification_core_report(VerificationCoreInputs {
        target,
        gate,
        summary: source_report.summary,
        orphan_probes,
        evidence_baseline_passed: evidence_passed,
        passed,
        evidence: &evidence,
        artifacts: &artifacts,
        qualification_gaps: &[],
    })?;
    let sources = [source_report];
    let report = VerificationCommandReport {
        schema_version: VERIFICATION_REPORT_SCHEMA,
        command: "verify source",
        verification: &verification,
        sources: &sources,
        inventory: vec![SourceInventoryReport {
            source: "vendor".to_owned(),
            symbols: symbols.len(),
        }],
        protocols: None,
        evidence_comparison: evidence_comparison.as_ref(),
        report: None,
    };
    crate::cli::output::render_report(
        &report,
        || render_verification_human(&report),
        || render_verification_tsv(&report),
    );
    Ok(passed)
}
