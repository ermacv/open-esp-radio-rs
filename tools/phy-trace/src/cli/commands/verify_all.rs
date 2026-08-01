//! Combined ROM/archive verification command.

use super::super::*;

pub(super) fn run(filtered: Vec<String>, svd: &MmioRegisterMap) -> Result<bool> {
    let mut rom_artifact = None;
    let mut rom_companion = None;
    let mut archive_artifact = None;
    let mut archive_inventory = None;
    let mut archive_companion = None;
    let mut rust_artifact = None;
    let mut rust_companion = None;
    let mut profile_path = None;
    let mut disposition_path = None;
    let mut rom_prefix = "phy_".to_owned();
    let mut archive_prefix = String::new();
    let mut rust_prefix = "open_phy_trace_".to_owned();
    let mut gate_name = "completion".to_owned();
    let mut match_floor = None;
    let mut evidence_baseline = None;
    let mut json_report = None;
    let mut arguments = filtered.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--rom-artifact" => {
                rom_artifact = Some(PathBuf::from(take_value(&mut arguments, "--rom-artifact")?));
            }
            "--rom-companion" => {
                rom_companion = Some(PathBuf::from(take_value(
                    &mut arguments,
                    "--rom-companion",
                )?));
            }
            "--archive-artifact" => {
                archive_artifact = Some(PathBuf::from(take_value(
                    &mut arguments,
                    "--archive-artifact",
                )?));
            }
            "--archive-inventory" => {
                archive_inventory = Some(PathBuf::from(take_value(
                    &mut arguments,
                    "--archive-inventory",
                )?));
            }
            "--archive-companion" => {
                archive_companion = Some(PathBuf::from(take_value(
                    &mut arguments,
                    "--archive-companion",
                )?));
            }
            "--rust-artifact" => {
                rust_artifact = Some(PathBuf::from(take_value(
                    &mut arguments,
                    "--rust-artifact",
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
            "--dispositions" => {
                disposition_path =
                    Some(PathBuf::from(take_value(&mut arguments, "--dispositions")?));
            }
            "--rom-prefix" => {
                rom_prefix = take_value(&mut arguments, "--rom-prefix")?;
            }
            "--archive-prefix" => {
                archive_prefix = take_value(&mut arguments, "--archive-prefix")?;
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
            "--json-report" => {
                json_report = Some(PathBuf::from(take_value(&mut arguments, "--json-report")?));
            }
            _ => return Err(format!("unknown verify-all option: {argument}").into()),
        }
    }
    let rom_artifact = rom_artifact.ok_or("missing --rom-artifact")?;
    let archive_artifact = archive_artifact.ok_or("missing --archive-artifact")?;
    let archive_inventory = archive_inventory.ok_or("missing --archive-inventory")?;
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
    let disposition_manifest = disposition_path
        .as_deref()
        .map(dispositions::Manifest::load)
        .transpose()?;
    let rom = VerifySource {
        name: "rom",
        artifact: &rom_artifact,
        inventory: None,
        companion: rom_companion.as_deref(),
        prefix: &rom_prefix,
    };
    let archive = VerifySource {
        name: "archive",
        artifact: &archive_artifact,
        inventory: Some(&archive_inventory),
        companion: archive_companion.as_deref(),
        prefix: &archive_prefix,
    };
    let rom_symbols = vendor_symbols(rom)?;
    let archive_symbols = vendor_symbols(archive)?;
    if let Some(manifest) = disposition_manifest.as_ref() {
        manifest.validate(&[
            ("rom", rom_symbols.as_slice()),
            ("archive", archive_symbols.as_slice()),
        ])?;
        print_protocol_inventory(
            manifest,
            &[
                ("rom", rom_symbols.as_slice()),
                ("archive", archive_symbols.as_slice()),
            ],
        );
    }
    let mut rom_profiles = Vec::new();
    let mut archive_profiles = Vec::new();
    for profile in execution_profiles {
        let in_rom = rom_symbols
            .iter()
            .any(|symbol| symbol.name == profile.vendor_symbol);
        let in_archive = archive_symbols
            .iter()
            .any(|symbol| symbol.name == profile.vendor_symbol);
        match profile.vendor_source.as_str() {
            "rom" if in_rom => rom_profiles.push(profile),
            "archive" if in_archive => archive_profiles.push(profile),
            source @ ("rom" | "archive") => {
                return Err(format!(
                    "profile {} refers to {} symbol {} which does not exist",
                    profile.name, source, profile.vendor_symbol
                )
                .into());
            }
            source => {
                return Err(format!(
                    "profile {} has unsupported vendor source {source}",
                    profile.name
                )
                .into());
            }
        }
    }
    println!(
        "INVENTORY\trom={}\tarchive={}\ttotal={}",
        rom_symbols.len(),
        archive_symbols.len(),
        rom_symbols.len() + archive_symbols.len()
    );
    let mut total = VerifySummary::default();
    let mut evidence = EvidenceSet::new();
    total.add(verify_source(
        svd,
        rom,
        &rust_artifact,
        rust_companion.as_deref(),
        &rust_prefix,
        &rom_profiles,
        disposition_manifest.as_ref(),
        &mut evidence,
    )?);
    total.add(verify_source(
        svd,
        archive,
        &rust_artifact,
        rust_companion.as_deref(),
        &rust_prefix,
        &archive_profiles,
        disposition_manifest.as_ref(),
        &mut evidence,
    )?);
    let orphan_probes = orphan_probe_count(
        &rust_artifact,
        &rust_prefix,
        &[(rom, &rom_symbols), (archive, &archive_symbols)],
    )?;
    println!(
        "TOTAL-SUMMARY\tvendor-functions={}\tmatch={}\tsymbolic-match={}\teffect-contract-match={}\tscenario-match={}\tstate-match={}\tcomposition-match={}\tmismatch={}\tincomplete={}\tmissing-rust-probe={}\timplemented-unqualified={}\tnot-yet-ported={}\torphan-rust-probe={orphan_probes}",
        total.vendor_functions,
        total.matched,
        total.symbolic_matches,
        total.effect_contract_matches,
        total.scenario_matches,
        total.state_matches,
        total.composition_matches,
        total.mismatched,
        total.incomplete,
        total.missing,
        total.implemented_unqualified,
        total.not_yet_ported,
    );
    print_evidence(&evidence);
    let evidence_passed = evidence_baseline
        .as_deref()
        .map(load_evidence_baseline)
        .transpose()?
        .is_none_or(|baseline| check_evidence_baseline(&baseline, &evidence));
    let passed = gate.passes(total, orphan_probes) && evidence_passed;
    if let Some(path) = json_report.as_deref() {
        let mut artifacts = vec![
            ("rom", rom_artifact.as_path()),
            ("archive", archive_artifact.as_path()),
            ("archive-inventory", archive_inventory.as_path()),
            ("rust-probes", rust_artifact.as_path()),
        ];
        if let Some(companion) = rom_companion.as_deref() {
            artifacts.push(("rom-companion", companion));
        }
        if let Some(companion) = archive_companion.as_deref() {
            artifacts.push(("archive-companion", companion));
        }
        if let Some(companion) = rust_companion.as_deref() {
            artifacts.push(("rust-companion", companion));
        }
        if let Some(profiles) = profile_path.as_deref() {
            artifacts.push(("profiles", profiles));
        }
        if let Some(dispositions) = disposition_path.as_deref() {
            artifacts.push(("dispositions", dispositions));
        }
        if let Some(baseline) = evidence_baseline.as_deref() {
            artifacts.push(("evidence-baseline", baseline));
        }
        write_verification_json_report(
            path,
            gate,
            total,
            orphan_probes,
            evidence_passed,
            passed,
            &evidence,
            &artifacts,
            &disposition_manifest
                .as_ref()
                .map(|manifest| {
                    manifest
                        .entries()
                        .filter(|entry| {
                            entry.disposition.is_implemented()
                                && entry.semantic_contract.is_none()
                                && entry.effect_contract.is_none()
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
        )?;
    }
    gate.report(passed);
    Ok(passed)
}
