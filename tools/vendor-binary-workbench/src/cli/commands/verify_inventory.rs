//! Multi-source vendor inventory verification.

use std::path::{Path, PathBuf};

use super::super::*;

#[derive(Default)]
struct SourceInput {
    artifact: Option<PathBuf>,
    inventory: Option<PathBuf>,
    companion: Option<PathBuf>,
    prefix: Option<String>,
    symbols: Vec<String>,
}

struct ResolvedSourceInput {
    id: String,
    artifact: PathBuf,
    inventory: Option<PathBuf>,
    companion: Option<PathBuf>,
    selection: ResolvedVendorSelection,
}

enum ResolvedVendorSelection {
    All,
    Prefix(String),
    Symbols(Vec<String>),
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

pub(super) fn execute(
    arguments: VerifyInventoryArgs,
    svd: &MmioMap,
    target: &TargetSpec,
) -> Result<VerificationCommandReport> {
    let mut source_inputs = BTreeMap::<String, SourceInput>::new();
    let mut auxiliary_artifacts = BTreeMap::<String, PathBuf>::new();
    for value in arguments.auxiliary_artifact {
        dispositions::validate_source_id(value.source.as_str(), 0)?;
        if auxiliary_artifacts
            .insert(value.source.to_string(), value.path)
            .is_some()
        {
            return Err(crate::Error::invalid(format!(
                "duplicate --auxiliary-artifact {}",
                value.source
            )));
        }
    }
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
    for value in arguments.source_symbol {
        let input = source_mut(&mut source_inputs, value.source.as_str())?;
        if input.symbols.contains(&value.value) {
            return Err(crate::Error::invalid(format!(
                "duplicate --source-symbol {}={}",
                value.source, value.value
            )));
        }
        input.symbols.push(value.value);
    }

    let rust_artifact = arguments.rust_artifact;
    let rust_companion = arguments.rust_companion;
    let profile_paths = arguments.profiles;
    let disposition_paths = arguments.dispositions;
    let rust_prefix = arguments
        .rust_prefix
        .ok_or("verify inventory requires --rust-prefix or project verification.rust-prefix")
        .map_err(crate::Error::invalid)?;
    let gate_name = arguments.gate;
    let match_floor = arguments.match_floor;
    let evidence_baselines = arguments.evidence_baseline;
    let output = arguments.output;

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
            if input.prefix.is_some() && !input.symbols.is_empty() {
                return Err(crate::Error::invalid(format!(
                    "source {id} cannot combine --source-prefix and --source-symbol"
                )));
            }
            let selection = match (input.prefix, input.symbols) {
                (Some(prefix), _) => ResolvedVendorSelection::Prefix(prefix),
                (None, symbols) if !symbols.is_empty() => ResolvedVendorSelection::Symbols(symbols),
                (None, _) => ResolvedVendorSelection::All,
            };
            Ok(ResolvedSourceInput {
                id,
                artifact,
                inventory: input.inventory,
                companion: input.companion,
                selection,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let rust_artifact = rust_artifact
        .ok_or("missing --rust-artifact")
        .map_err(crate::Error::invalid)?;
    let gate = VerificationGate::parse(&gate_name, match_floor)?;
    if matches!(gate, VerificationGate::Regression { .. }) && evidence_baselines.is_empty() {
        return Err(crate::Error::invalid(
            "--gate regression requires --evidence-baseline",
        ));
    }
    let mut execution_profiles = Vec::new();
    let mut profile_names = BTreeSet::new();
    for path in &profile_paths {
        for profile in profiles::load(path)? {
            if !profile_names.insert(profile.name.clone()) {
                return Err(crate::Error::invalid(format!(
                    "verification profile name {:?} is repeated across fragments",
                    profile.name
                )));
            }
            execution_profiles.push(profile);
        }
    }
    let disposition_manifest = dispositions::Manifest::load_all(&disposition_paths)?;

    let verify_sources = sources
        .iter()
        .map(|source| VerifySource {
            name: &source.id,
            artifact: &source.artifact,
            inventory: source.inventory.as_deref(),
            companion: source.companion.as_deref(),
            selection: match &source.selection {
                ResolvedVendorSelection::All => VendorSymbolSelection::All,
                ResolvedVendorSelection::Prefix(prefix) => VendorSymbolSelection::Prefix(prefix),
                ResolvedVendorSelection::Symbols(symbols) => {
                    VendorSymbolSelection::Symbols(symbols)
                }
            },
        })
        .collect::<Vec<_>>();
    let adapter_artifacts = auxiliary_artifacts
        .iter()
        .map(|(id, artifact)| open_radio_vendor_semantics::DriverAdapterArtifact { id, artifact })
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
    for source in &verify_sources {
        let source_profiles = profiles_by_source
            .get(source.name)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let report = verify_source(
            svd,
            target.harness.as_deref(),
            &target.rust_target,
            *source,
            &rust_artifact,
            rust_companion.as_deref(),
            &rust_prefix,
            source_profiles,
            disposition_manifest.as_ref(),
            &adapter_artifacts,
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
        .chain(
            profiles_by_source
                .values()
                .flatten()
                .map(|profile| profile.rust_symbol.clone()),
        )
        .collect::<BTreeSet<_>>();
    let orphan_probes = orphan_probe_count(
        &rust_artifact,
        &rust_prefix,
        &orphan_sources,
        &explicitly_bound_probes,
    )?;
    let mut baseline = EvidenceSet::new();
    for path in &evidence_baselines {
        for ((source, symbol), kind) in load_evidence_baseline(path)? {
            record_evidence(&mut baseline, &source, &symbol, kind)?;
        }
    }
    let evidence_comparison =
        (!evidence_baselines.is_empty()).then(|| compare_evidence_baseline(&baseline, &evidence));
    let evidence_passed = evidence_comparison
        .as_ref()
        .is_none_or(|comparison| comparison.passed);
    let passed = gate.passes(total, orphan_probes) && evidence_passed;
    let profile_contracts = profiles_by_source
        .values()
        .flatten()
        .map(|profile| {
            (
                profile.vendor_source.as_str(),
                profile.vendor_symbol.as_str(),
            )
        })
        .collect::<BTreeSet<_>>();
    let qualification_gaps = disposition_manifest
        .as_ref()
        .map(|manifest| {
            manifest
                .entries()
                .filter(|entry| {
                    entry.disposition.is_implemented()
                        && entry.semantic_contract.is_none()
                        && entry.effect_contract.is_none()
                        && !profile_contracts
                            .contains(&(entry.source.as_str(), entry.symbol.as_str()))
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
    for (id, artifact) in &auxiliary_artifacts {
        artifacts.push((format!("auxiliary:{id}"), artifact));
    }
    artifacts.push(("rust-probes".to_owned(), &rust_artifact));
    if let Some(companion) = rust_companion.as_deref() {
        artifacts.push(("rust-companion".to_owned(), companion));
    }
    for (index, profiles) in profile_paths.iter().enumerate() {
        artifacts.push((format!("profiles:{index}"), profiles));
    }
    for (index, dispositions) in disposition_paths.iter().enumerate() {
        artifacts.push((format!("dispositions:{index}"), dispositions));
    }
    for (index, baseline) in evidence_baselines.iter().enumerate() {
        artifacts.push((format!("evidence-baseline:{index}"), baseline));
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
    let publication = output.as_deref().map(PublishedVerificationReport::written);
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
        verification,
        sources: source_reports,
        inventory,
        protocols,
        evidence_comparison,
        report: publication,
    };
    if let Some(path) = output.as_deref() {
        write_verification_json_report(path, &report)?;
    }
    Ok(report)
}

pub(super) fn run(
    arguments: VerifyInventoryArgs,
    svd: &MmioMap,
    target: &TargetSpec,
) -> Result<bool> {
    let report = execute(arguments, svd, target)?;
    let passed = report.verification.passed;
    crate::cli::output::render_report(&report, || crate::cli::render::verification_human(&report));
    Ok(passed)
}
