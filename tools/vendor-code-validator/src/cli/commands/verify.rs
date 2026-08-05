//! Single-source verification command.

use super::super::*;

pub(super) fn run(
    filtered: Vec<String>,
    svd: &MmioRegisterMap,
    target: &TargetSpec,
) -> Result<bool> {
    let mut vendor_artifact = None;
    let mut vendor_inventory = None;
    let mut vendor_companion = None;
    let mut rust_artifact = None;
    let mut rust_companion = None;
    let mut profile_path = None;
    let mut vendor_prefix = "phy_".to_owned();
    let mut rust_prefix = "open_phy_trace_".to_owned();
    let mut gate_name = "completion".to_owned();
    let mut match_floor = None;
    let mut evidence_baseline = None;
    let mut arguments = filtered.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--vendor-artifact" => {
                vendor_artifact = Some(PathBuf::from(take_value(
                    &mut arguments,
                    "--vendor-artifact",
                )?));
            }
            "--rust-artifact" => {
                rust_artifact = Some(PathBuf::from(take_value(
                    &mut arguments,
                    "--rust-artifact",
                )?));
            }
            "--vendor-inventory" => {
                vendor_inventory = Some(PathBuf::from(take_value(
                    &mut arguments,
                    "--vendor-inventory",
                )?));
            }
            "--vendor-companion" => {
                vendor_companion = Some(PathBuf::from(take_value(
                    &mut arguments,
                    "--vendor-companion",
                )?));
            }
            "--rust-companion" => {
                rust_companion = Some(PathBuf::from(take_value(
                    &mut arguments,
                    "--rust-companion",
                )?));
            }
            "--profiles" => {
                profile_path = Some(PathBuf::from(take_value(&mut arguments, "--profiles")?));
            }
            "--vendor-prefix" => {
                vendor_prefix = take_value(&mut arguments, "--vendor-prefix")?;
            }
            "--rust-prefix" => {
                rust_prefix = take_value(&mut arguments, "--rust-prefix")?;
            }
            "--gate" => gate_name = take_value(&mut arguments, "--gate")?,
            "--match-floor" => {
                match_floor = Some(take_value(&mut arguments, "--match-floor")?.parse::<usize>()?);
            }
            "--evidence-baseline" => {
                evidence_baseline = Some(PathBuf::from(take_value(
                    &mut arguments,
                    "--evidence-baseline",
                )?));
            }
            _ => return Err(format!("unknown verify option: {argument}").into()),
        }
    }
    let vendor_artifact = vendor_artifact.ok_or("missing --vendor-artifact")?;
    let rust_artifact = rust_artifact.ok_or("missing --rust-artifact")?;
    let gate = VerificationGate::parse(&gate_name, match_floor)?;
    if matches!(gate, VerificationGate::Regression { .. }) && evidence_baseline.is_none() {
        return Err("--gate regression requires --evidence-baseline".into());
    }
    let execution_profiles = profile_path
        .as_deref()
        .map(profiles::load)
        .transpose()?
        .unwrap_or_default();
    let source = VerifySource {
        name: "vendor",
        artifact: &vendor_artifact,
        inventory: vendor_inventory.as_deref(),
        companion: vendor_companion.as_deref(),
        prefix: &vendor_prefix,
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
        rust_companion.as_deref(),
        &rust_prefix,
        &execution_profiles,
        None,
        &mut evidence,
    )?;
    let orphan_probes = orphan_probe_count(
        &rust_artifact,
        &rust_prefix,
        &[(source, &symbols)],
        &BTreeSet::new(),
    )?;
    println!(
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
    let evidence_passed = evidence_baseline
        .as_deref()
        .map(load_evidence_baseline)
        .transpose()?
        .is_none_or(|baseline| check_evidence_baseline(&baseline, &evidence));
    let passed = gate.passes(summary, orphan_probes) && evidence_passed;
    gate.report(passed);
    Ok(passed)
}
