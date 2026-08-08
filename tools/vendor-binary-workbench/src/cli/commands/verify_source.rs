//! Single-source verification command.

use super::super::*;

pub(super) fn run(
    arguments: VerifySourceArgs,
    svd: &MmioRegisterMap,
    target: &TargetSpec,
) -> Result<bool> {
    let vendor_artifact = arguments
        .vendor_artifact
        .ok_or("missing --vendor-artifact")?;
    let rust_artifact = arguments.rust_artifact.ok_or("missing --rust-artifact")?;
    let gate = VerificationGate::parse(&arguments.gate, arguments.match_floor)?;
    if matches!(gate, VerificationGate::Regression { .. }) && arguments.evidence_baseline.is_none()
    {
        return Err("--gate regression requires --evidence-baseline".into());
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
    let summary = verify_source(
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
    outputln!(
        "SUMMARY\tvendor-functions={}\tmatch={}\tsymbolic-match={}\teffect-contract-match={}\tscenario-match={}\tstate-match={}\tcomposition-match={}\tmismatch={}\tincomplete={}\tmissing-rust-probe={}\torphan-rust-probe={orphan_probes}",
        summary.vendor_functions,
        summary.matched,
        summary.symbolic_matches,
        summary.effect_contract_matches,
        summary.scenario_matches,
        summary.state_matches,
        summary.composition_matches,
        summary.mismatched,
        summary.incomplete,
        summary.missing
    );
    print_evidence(&evidence);
    let evidence_comparison = arguments
        .evidence_baseline
        .as_deref()
        .map(load_evidence_baseline)
        .transpose()?
        .map(|baseline| compare_evidence_baseline(&baseline, &evidence));
    if let Some(comparison) = &evidence_comparison
        && !crate::cli::output::structured("evidence-comparison", comparison)
    {
        print_evidence_comparison(comparison);
    }
    let evidence_passed = evidence_comparison
        .as_ref()
        .is_none_or(|comparison| comparison.passed);
    let passed = gate.passes(summary, orphan_probes) && evidence_passed;
    gate.report(passed);
    Ok(passed)
}
