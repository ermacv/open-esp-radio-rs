//! Batch generation of all fail-closed references currently supported by an artifact.

use std::path::{Path, PathBuf};

use serde::Serialize;

use super::super::*;

#[derive(Debug, Serialize)]
struct GeneratedCandidate {
    symbol: String,
    owner: Option<String>,
    reference_file: String,
    probe_symbol: String,
    exit_a0_modeled: bool,
    dependencies: Vec<String>,
    source: String,
}

#[derive(Debug, Serialize)]
struct BlockedCandidate {
    symbol: String,
    owner: Option<String>,
    reasons: Vec<String>,
}

#[derive(Serialize)]
struct ArtifactIdentity {
    path: String,
    sha256: String,
}

#[derive(Serialize)]
struct GenerationSummary {
    functions: usize,
    generated: usize,
    blocked: usize,
}

#[derive(Serialize)]
struct GenerationManifest<'a> {
    schema_version: u32,
    command: &'static str,
    artifact: ArtifactIdentity,
    companions: Vec<ArtifactIdentity>,
    symbol_prefix: &'a str,
    probe_prefix: &'a str,
    source_name: Option<&'a str>,
    entry_contract: &'a str,
    summary: GenerationSummary,
    generated: &'a [GeneratedCandidate],
    blocked: &'a [BlockedCandidate],
}

fn sanitize_file_component(value: &str) -> String {
    let mut output = String::new();
    for character in value.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
            output.push(character);
        } else {
            output.push('_');
        }
    }
    if output.is_empty() {
        "unnamed".to_owned()
    } else {
        output
    }
}

fn probe_symbol(
    symbol: &str,
    symbol_prefix: &str,
    probe_prefix: &str,
    source: Option<&str>,
) -> String {
    let suffix = symbol.strip_prefix(symbol_prefix).unwrap_or(symbol);
    source.map_or_else(
        || format!("{probe_prefix}{suffix}"),
        |source| format!("{probe_prefix}{source}_{suffix}"),
    )
}

fn candidate_file_name(
    owner: Option<&str>,
    symbol: &str,
    used_names: &mut BTreeSet<String>,
) -> String {
    let stem = owner.map_or_else(
        || sanitize_file_component(symbol),
        |owner| {
            format!(
                "{}__{}",
                sanitize_file_component(owner),
                sanitize_file_component(symbol)
            )
        },
    );
    let mut name = format!("{stem}.rs");
    let mut discriminator = 2usize;
    while !used_names.insert(name.clone()) {
        name = format!("{stem}__{discriminator}.rs");
        discriminator += 1;
    }
    name
}

struct ManifestInputs<'a> {
    artifact: &'a Path,
    companions: &'a [PathBuf],
    symbol_prefix: &'a str,
    probe_prefix: &'a str,
    source_name: Option<&'a str>,
    entry_contract: EntryContractRef,
    inventory_count: usize,
    generated: &'a [GeneratedCandidate],
    blocked: &'a [BlockedCandidate],
}

fn manifest_document(inputs: ManifestInputs<'_>) -> Result<GenerationManifest<'_>> {
    Ok(GenerationManifest {
        schema_version: 2,
        command: "reference generate-batch",
        artifact: ArtifactIdentity {
            path: inputs.artifact.display().to_string(),
            sha256: artifact_sha256(inputs.artifact)?,
        },
        companions: inputs
            .companions
            .iter()
            .map(|companion| {
                Ok(ArtifactIdentity {
                    path: companion.display().to_string(),
                    sha256: artifact_sha256(companion)?,
                })
            })
            .collect::<Result<Vec<_>>>()?,
        symbol_prefix: inputs.symbol_prefix,
        probe_prefix: inputs.probe_prefix,
        source_name: inputs.source_name,
        entry_contract: inputs.entry_contract.id(),
        summary: GenerationSummary {
            functions: inputs.inventory_count,
            generated: inputs.generated.len(),
            blocked: inputs.blocked.len(),
        },
        generated: inputs.generated,
        blocked: inputs.blocked,
    })
}

fn write_manifest(path: &Path, document: &GenerationManifest<'_>) -> Result<()> {
    let mut output = serde_json::to_string_pretty(&document)?;
    output.push('\n');
    fs::write(path, output)?;
    Ok(())
}

fn print_generation_report(
    output_dir: &Path,
    manifest: &Path,
    functions: usize,
    generated: &[GeneratedCandidate],
    blocked: &[BlockedCandidate],
) {
    for candidate in generated {
        outputln!(
            "GENERATED\t{}\t{}\texit-a0={}",
            candidate.symbol,
            output_dir.join(&candidate.reference_file).display(),
            if candidate.exit_a0_modeled {
                "modeled"
            } else {
                "unresolved"
            }
        );
    }
    outputln!("MANIFEST\t{}", manifest.display());
    outputln!(
        "SUMMARY\tfunctions={functions}\tgenerated={}\tblocked={}",
        generated.len(),
        blocked.len()
    );
}

#[tracing::instrument(
    name = "generate_reference_batch",
    skip_all,
    fields(artifact = tracing::field::Empty, symbol_prefix = %arguments.symbol_prefix)
)]
pub(super) fn run(
    arguments: ReferenceBatchArgs,
    svd: &MmioMap,
    target: &TargetSpec,
) -> Result<bool> {
    let harness = target.require_available_harness()?;
    let riscv_harness = harnesses::riscv(harness)?;
    let entry_contract = harnesses::entry_contract(harness, &arguments.entry_contract)?;
    let artifact = arguments
        .artifact
        .ok_or("missing --artifact")
        .map_err(crate::Error::invalid)?;
    tracing::Span::current().record("artifact", tracing::field::display(artifact.display()));
    let output_dir = arguments
        .output_dir
        .ok_or("missing --output-dir")
        .map_err(crate::Error::invalid)?;
    let manifest = arguments
        .manifest
        .unwrap_or_else(|| output_dir.join("manifest.json"));
    let symbols = list_code_symbols(&artifact, &arguments.symbol_prefix)?;
    if symbols.is_empty() {
        return Err(crate::Error::invalid(format!(
            "no external code symbols start with {:?}",
            arguments.symbol_prefix
        )));
    }

    let resolver = ReferenceResolver::load_with_entry_contract(
        &artifact,
        &arguments.companion,
        riscv_harness,
        entry_contract,
    )?;
    let digest = artifact_sha256(&artifact)?;
    let artifact_display = artifact.display().to_string();
    let companion_provenance = arguments
        .companion
        .iter()
        .map(|companion| Ok((companion.display().to_string(), artifact_sha256(companion)?)))
        .collect::<Result<Vec<_>>>()?;
    let mut generated = Vec::new();
    let mut blocked = Vec::new();
    let mut used_names = BTreeSet::new();

    for symbol in &symbols {
        let trace = resolver.trace(symbol.member.as_deref(), &symbol.name, svd)?;
        if !trace.is_reference_eligible() {
            blocked.push(BlockedCandidate {
                symbol: symbol.name.clone(),
                owner: symbol.member.clone(),
                reasons: trace.reference_failure_reasons(),
            });
            continue;
        }
        let resolved = ResolvedReferenceProgram::try_from(&trace)
            .map_err(|error| -> Error { crate::Error::invalid(error) })?;
        let reference_file =
            candidate_file_name(symbol.member.as_deref(), &symbol.name, &mut used_names);
        let generated_reference = codegen::generate(
            &resolved,
            &artifact_display,
            &digest,
            symbol.member.as_deref(),
            &companion_provenance,
        )
        .map_err(|error| -> Error { crate::Error::invalid(error) })?;
        generated.push(GeneratedCandidate {
            symbol: symbol.name.clone(),
            owner: symbol.member.clone(),
            reference_file,
            probe_symbol: probe_symbol(
                &symbol.name,
                &arguments.symbol_prefix,
                &arguments.probe_prefix,
                arguments.source_name.as_deref(),
            ),
            exit_a0_modeled: generated_reference.exit_a0_modeled,
            dependencies: resolved.dependencies,
            source: generated_reference.source,
        });
    }

    let mut destinations = generated
        .iter()
        .map(|candidate| output_dir.join(&candidate.reference_file))
        .collect::<Vec<_>>();
    destinations.push(manifest.clone());
    if !arguments.force
        && let Some(existing) = destinations.iter().find(|path| path.exists())
    {
        return Err(crate::Error::invalid(format!(
            "refusing to overwrite {}; pass --force to replace generated output",
            existing.display()
        )));
    }
    fs::create_dir_all(&output_dir)?;
    if let Some(parent) = manifest
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    for candidate in &generated {
        fs::write(
            output_dir.join(&candidate.reference_file),
            &candidate.source,
        )?;
    }
    let document = manifest_document(ManifestInputs {
        artifact: &artifact,
        companions: &arguments.companion,
        symbol_prefix: &arguments.symbol_prefix,
        probe_prefix: &arguments.probe_prefix,
        source_name: arguments.source_name.as_deref(),
        entry_contract,
        inventory_count: symbols.len(),
        generated: &generated,
        blocked: &blocked,
    })?;
    write_manifest(&manifest, &document)?;
    if !crate::cli::output::structured(&document) {
        print_generation_report(&output_dir, &manifest, symbols.len(), &generated, &blocked);
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_names_are_stable_and_collision_safe() {
        let mut used = BTreeSet::new();
        assert_eq!(
            candidate_file_name(Some("phy/init.o"), "phy_x", &mut used),
            "phy_init_o__phy_x.rs"
        );
        assert_eq!(
            candidate_file_name(Some("phy:init.o"), "phy_x", &mut used),
            "phy_init_o__phy_x__2.rs"
        );
    }

    #[test]
    fn probe_names_follow_verifier_prefix_rules() {
        assert_eq!(
            probe_symbol("phy_disable_agc", "phy_", "open_phy_trace_", None),
            "open_phy_trace_disable_agc"
        );
        assert_eq!(
            probe_symbol("phy_disable_agc", "phy_", "open_phy_trace_", Some("rom")),
            "open_phy_trace_rom_disable_agc"
        );
    }
}
