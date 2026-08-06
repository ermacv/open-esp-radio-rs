//! Multi-source vendor inventory verification.

use std::path::Path;

use super::super::*;

#[derive(Default)]
struct SourceInput {
    artifact: Option<PathBuf>,
    inventory: Option<PathBuf>,
    companion: Option<PathBuf>,
    prefix: Option<String>,
}

struct ResolvedSourceInput {
    id: String,
    artifact: PathBuf,
    inventory: Option<PathBuf>,
    companion: Option<PathBuf>,
    prefix: String,
}

fn source_option<'a>(argument: &'a str, name: &str) -> Option<&'a str> {
    argument
        .strip_prefix(name)
        .and_then(|suffix| suffix.strip_prefix(':'))
        .filter(|source| !source.is_empty())
}

fn source_mut<'a>(
    sources: &'a mut BTreeMap<String, SourceInput>,
    id: &str,
) -> Result<&'a mut SourceInput> {
    dispositions::validate_source_id(id, 0)?;
    Ok(sources.entry(id.to_owned()).or_default())
}

fn set_path(slot: &mut Option<PathBuf>, value: String, option: &str) -> Result<()> {
    if slot.replace(PathBuf::from(value)).is_some() {
        return Err(format!("duplicate {option}").into());
    }
    Ok(())
}

fn set_string(slot: &mut Option<String>, value: String, option: &str) -> Result<()> {
    if slot.replace(value).is_some() {
        return Err(format!("duplicate {option}").into());
    }
    Ok(())
}

pub(super) fn run(
    filtered: Vec<String>,
    svd: &MmioRegisterMap,
    target: &TargetSpec,
) -> Result<bool> {
    let mut source_inputs = BTreeMap::<String, SourceInput>::new();
    let mut rust_artifact = None;
    let mut rust_companion = None;
    let mut profile_path = None;
    let mut disposition_path = None;
    let mut rust_prefix = "open_phy_trace_".to_owned();
    let mut gate_name = "completion".to_owned();
    let mut match_floor = None;
    let mut evidence_baseline = None;
    let mut json_report = None;
    let mut arguments = filtered.into_iter();
    while let Some(argument) = arguments.next() {
        if let Some(source) = source_option(&argument, "--source-artifact") {
            let value = take_value(&mut arguments, &argument)?;
            set_path(
                &mut source_mut(&mut source_inputs, source)?.artifact,
                value,
                &argument,
            )?;
            continue;
        }
        if let Some(source) = source_option(&argument, "--source-inventory") {
            let value = take_value(&mut arguments, &argument)?;
            set_path(
                &mut source_mut(&mut source_inputs, source)?.inventory,
                value,
                &argument,
            )?;
            continue;
        }
        if let Some(source) = source_option(&argument, "--source-companion") {
            let value = take_value(&mut arguments, &argument)?;
            set_path(
                &mut source_mut(&mut source_inputs, source)?.companion,
                value,
                &argument,
            )?;
            continue;
        }
        if let Some(source) = source_option(&argument, "--source-prefix") {
            let value = take_value(&mut arguments, &argument)?;
            set_string(
                &mut source_mut(&mut source_inputs, source)?.prefix,
                value,
                &argument,
            )?;
            continue;
        }

        match argument.as_str() {
            "--rust-artifact" => set_path(
                &mut rust_artifact,
                take_value(&mut arguments, "--rust-artifact")?,
                "--rust-artifact",
            )?,
            "--rust-companion" => set_path(
                &mut rust_companion,
                take_value(&mut arguments, "--rust-companion")?,
                "--rust-companion",
            )?,
            "--profiles" => set_path(
                &mut profile_path,
                take_value(&mut arguments, "--profiles")?,
                "--profiles",
            )?,
            "--dispositions" => set_path(
                &mut disposition_path,
                take_value(&mut arguments, "--dispositions")?,
                "--dispositions",
            )?,
            "--rust-prefix" => {
                rust_prefix = take_value(&mut arguments, "--rust-prefix")?;
            }
            "--gate" => gate_name = take_value(&mut arguments, "--gate")?,
            "--match-floor" => {
                match_floor = Some(take_value(&mut arguments, "--match-floor")?.parse::<usize>()?);
            }
            "--evidence-baseline" => set_path(
                &mut evidence_baseline,
                take_value(&mut arguments, "--evidence-baseline")?,
                "--evidence-baseline",
            )?,
            "--json-report" => set_path(
                &mut json_report,
                take_value(&mut arguments, "--json-report")?,
                "--json-report",
            )?,
            "--no-profiles" | "--no-dispositions" | "--no-evidence-baseline" => {}
            _ => return Err(format!("unknown verify inventory option: {argument}").into()),
        }
    }

    if source_inputs.is_empty() {
        return Err("verify inventory requires at least one --source-artifact:SOURCE".into());
    }
    let sources = source_inputs
        .into_iter()
        .map(|(id, input)| {
            let artifact = input
                .artifact
                .ok_or_else(|| format!("source {id} has no artifact"))?;
            let prefix = input.prefix.unwrap_or_else(|| {
                if id == "rom" {
                    "phy_".to_owned()
                } else {
                    String::new()
                }
            });
            Ok(ResolvedSourceInput {
                id,
                artifact,
                inventory: input.inventory,
                companion: input.companion,
                prefix,
            })
        })
        .collect::<Result<Vec<_>>>()?;
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

    let verify_sources = sources
        .iter()
        .map(|source| VerifySource {
            name: &source.id,
            artifact: &source.artifact,
            inventory: source.inventory.as_deref(),
            companion: source.companion.as_deref(),
            prefix: &source.prefix,
        })
        .collect::<Vec<_>>();
    let symbol_sets = verify_sources
        .iter()
        .copied()
        .map(vendor_symbols)
        .collect::<Result<Vec<_>>>()?;
    let inventory = verify_sources
        .iter()
        .zip(&symbol_sets)
        .map(|(source, symbols)| (source.name, symbols.as_slice()))
        .collect::<Vec<_>>();
    if let Some(manifest) = disposition_manifest.as_ref() {
        manifest.validate(&inventory)?;
        print_protocol_inventory(manifest, &inventory);
    }

    let mut profiles_by_source = BTreeMap::<String, Vec<profiles::Profile>>::new();
    for profile in execution_profiles {
        let source_index = verify_sources
            .iter()
            .position(|source| source.name == profile.vendor_source)
            .ok_or_else(|| {
                format!(
                    "profile {} refers to unconfigured vendor source {}",
                    profile.name, profile.vendor_source
                )
            })?;
        if !symbol_sets[source_index]
            .iter()
            .any(|symbol| symbol.name == profile.vendor_symbol)
        {
            return Err(format!(
                "profile {} refers to {} symbol {} which does not exist",
                profile.name, profile.vendor_source, profile.vendor_symbol
            )
            .into());
        }
        profiles_by_source
            .entry(profile.vendor_source.clone())
            .or_default()
            .push(profile);
    }

    let total_symbols = symbol_sets.iter().map(Vec::len).sum::<usize>();
    print!("INVENTORY");
    for (source, symbols) in verify_sources.iter().zip(&symbol_sets) {
        print!("\t{}={}", source.name, symbols.len());
    }
    println!("\ttotal={total_symbols}");

    let mut total = VerifySummary::default();
    let mut evidence = EvidenceSet::new();
    let harness = target.require_available_harness()?;
    for source in &verify_sources {
        let source_profiles = profiles_by_source
            .get(source.name)
            .map(Vec::as_slice)
            .unwrap_or_default();
        total.add(verify_source(
            svd,
            harness,
            &target.rust_target,
            *source,
            &rust_artifact,
            rust_companion.as_deref(),
            &rust_prefix,
            source_profiles,
            disposition_manifest.as_ref(),
            &mut evidence,
        )?);
    }
    let orphan_sources = verify_sources
        .iter()
        .copied()
        .zip(symbol_sets.iter().map(Vec::as_slice))
        .collect::<Vec<_>>();
    let explicitly_bound_probes = disposition_manifest
        .iter()
        .flat_map(dispositions::Manifest::entries)
        .filter_map(|entry| entry.binding.as_ref())
        .map(|binding| binding.rust_probe.clone())
        .collect::<BTreeSet<_>>();
    let orphan_probes = orphan_probe_count(
        &rust_artifact,
        &rust_prefix,
        &orphan_sources,
        &explicitly_bound_probes,
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
        let mut artifacts = Vec::<(String, &Path)>::new();
        for source in &sources {
            artifacts.push((format!("source:{}:artifact", source.id), &source.artifact));
            if let Some(inventory) = source.inventory.as_deref() {
                artifacts.push((format!("source:{}:inventory", source.id), inventory));
            }
            if let Some(companion) = source.companion.as_deref() {
                artifacts.push((format!("source:{}:companion", source.id), companion));
            }
        }
        artifacts.push(("rust-probes".to_owned(), &rust_artifact));
        if let Some(companion) = rust_companion.as_deref() {
            artifacts.push(("rust-companion".to_owned(), companion));
        }
        if let Some(profiles) = profile_path.as_deref() {
            artifacts.push(("profiles".to_owned(), profiles));
        }
        if let Some(dispositions) = disposition_path.as_deref() {
            artifacts.push(("dispositions".to_owned(), dispositions));
        }
        if let Some(baseline) = evidence_baseline.as_deref() {
            artifacts.push(("evidence-baseline".to_owned(), baseline));
        }
        write_verification_json_report(
            path,
            target,
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
