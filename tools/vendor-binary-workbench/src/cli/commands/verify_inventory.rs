//! Multi-source vendor inventory verification.

use std::path::{Path, PathBuf};

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

fn source_mut<'a>(
    sources: &'a mut BTreeMap<String, SourceInput>,
    id: &str,
) -> Result<&'a mut SourceInput> {
    dispositions::validate_source_id(id, 0)?;
    Ok(sources.entry(id.to_owned()).or_default())
}

fn set_path(slot: &mut Option<PathBuf>, value: PathBuf, option: &str) -> Result<()> {
    if slot.replace(value).is_some() {
        return Err(crate::Error::invalid(format!("duplicate {option}")));
    }
    Ok(())
}

fn set_string(slot: &mut Option<String>, value: String, option: &str) -> Result<()> {
    if slot.replace(value).is_some() {
        return Err(crate::Error::invalid(format!("duplicate {option}")));
    }
    Ok(())
}

pub(super) fn run(
    arguments: VerifyInventoryArgs,
    svd: &MmioRegisterMap,
    target: &TargetSpec,
) -> Result<bool> {
    let mut source_inputs = BTreeMap::<String, SourceInput>::new();
    for value in arguments.source_artifact {
        set_path(
            &mut source_mut(&mut source_inputs, value.source.as_str())?.artifact,
            value.path,
            "--source-artifact",
        )?;
    }
    for value in arguments.source_inventory {
        set_path(
            &mut source_mut(&mut source_inputs, value.source.as_str())?.inventory,
            value.path,
            "--source-inventory",
        )?;
    }
    for value in arguments.source_companion {
        set_path(
            &mut source_mut(&mut source_inputs, value.source.as_str())?.companion,
            value.path,
            "--source-companion",
        )?;
    }
    for value in arguments.source_prefix {
        set_string(
            &mut source_mut(&mut source_inputs, value.source.as_str())?.prefix,
            value.value,
            "--source-prefix",
        )?;
    }

    let rust_artifact = arguments.rust_artifact;
    let rust_companion = arguments.rust_companion;
    let profile_path = arguments.profiles;
    let disposition_path = arguments.dispositions;
    let rust_prefix = arguments.rust_prefix;
    let gate_name = arguments.gate;
    let match_floor = arguments.match_floor;
    let evidence_baseline = arguments.evidence_baseline;
    let json_report = arguments.json_report;

    if source_inputs.is_empty() {
        return Err(crate::Error::invalid(
            "verify inventory requires at least one --source-artifact SOURCE=PATH",
        ));
    }
    let sources = source_inputs
        .into_iter()
        .map(|(id, input)| {
            let artifact = input
                .artifact
                .ok_or_else(|| format!("source {id} has no artifact"))
                .map_err(crate::Error::invalid)?;
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
    let rust_artifact = rust_artifact
        .ok_or("missing --rust-artifact")
        .map_err(crate::Error::invalid)?;
    let gate = VerificationGate::parse(&gate_name, match_floor)?;
    if matches!(gate, VerificationGate::Regression { .. }) && evidence_baseline.is_none() {
        return Err(crate::Error::invalid(
            "--gate regression requires --evidence-baseline",
        ));
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
    let protocols = if let Some(manifest) = disposition_manifest.as_ref() {
        manifest.validate(&inventory)?;
        Some(protocol_inventory(manifest, &inventory))
    } else {
        None
    };

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
            })
            .map_err(crate::Error::invalid)?;
        if !symbol_sets[source_index]
            .iter()
            .any(|symbol| symbol.name == profile.vendor_symbol)
        {
            return Err(crate::Error::invalid(format!(
                "profile {} refers to {} symbol {} which does not exist",
                profile.name, profile.vendor_source, profile.vendor_symbol
            )));
        }
        profiles_by_source
            .entry(profile.vendor_source.clone())
            .or_default()
            .push(profile);
    }

    let mut total = VerifySummary::default();
    let mut source_reports = Vec::with_capacity(verify_sources.len());
    let mut evidence = EvidenceSet::new();
    let harness = target.require_available_harness()?;
    for source in &verify_sources {
        let source_profiles = profiles_by_source
            .get(source.name)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let report = verify_source(
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
        )?;
        total.add(report.summary);
        source_reports.push(report);
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
    let evidence_comparison = evidence_baseline
        .as_deref()
        .map(load_evidence_baseline)
        .transpose()?
        .map(|baseline| compare_evidence_baseline(&baseline, &evidence));
    let evidence_passed = evidence_comparison
        .as_ref()
        .is_none_or(|comparison| comparison.passed);
    let passed = gate.passes(total, orphan_probes) && evidence_passed;
    let qualification_gaps = disposition_manifest
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
        .unwrap_or_default();
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
    let verification = verification_core_report(VerificationCoreInputs {
        target,
        gate,
        summary: total,
        orphan_probes,
        evidence_baseline_passed: evidence_passed,
        passed,
        evidence: &evidence,
        artifacts: &artifacts,
        qualification_gaps: &qualification_gaps,
    })?;
    let publication = json_report
        .as_deref()
        .map(PublishedVerificationReport::written);
    let inventory = verify_sources
        .iter()
        .zip(&symbol_sets)
        .map(|(source, symbols)| SourceInventoryReport {
            source: source.name.to_owned(),
            symbols: symbols.len(),
        })
        .collect();
    let report = VerificationCommandReport {
        schema_version: VERIFICATION_REPORT_SCHEMA,
        command: "verify inventory",
        verification: &verification,
        sources: &source_reports,
        inventory,
        protocols,
        evidence_comparison: evidence_comparison.as_ref(),
        report: publication,
    };
    if let Some(path) = json_report.as_deref() {
        write_verification_json_report(path, &report)?;
    }
    crate::cli::output::render_report(
        &report,
        || render_verification_human(&report),
        || render_verification_tsv(&report),
    );
    Ok(passed)
}
