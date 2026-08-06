//! Batch generation of all fail-closed references currently supported by an artifact.

use std::{fmt::Write as _, path::Path};

use super::super::json::{write_artifact, write_string, write_strings};
use super::super::*;

#[derive(Debug)]
struct GeneratedCandidate {
    symbol: String,
    owner: Option<String>,
    reference_file: String,
    probe_symbol: String,
    exit_a0_modeled: bool,
    dependencies: Vec<String>,
    source: String,
}

#[derive(Debug)]
struct BlockedCandidate {
    symbol: String,
    owner: Option<String>,
    reasons: Vec<String>,
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

#[allow(
    clippy::too_many_arguments,
    reason = "the manifest boundary receives a complete immutable generation record"
)]
fn write_manifest(
    path: &Path,
    artifact: &Path,
    companions: &[PathBuf],
    symbol_prefix: &str,
    probe_prefix: &str,
    source_name: Option<&str>,
    entry_contract: EntryContractRef,
    inventory_count: usize,
    generated: &[GeneratedCandidate],
    blocked: &[BlockedCandidate],
) -> Result<()> {
    let mut output = String::new();
    output.push_str("{\n  \"schema_version\": 2,\n  \"command\": \"reference generate-batch\",\n");
    output.push_str("  \"artifact\": ");
    write_artifact(&mut output, artifact)?;
    output.push_str(",\n  \"companions\": [");
    for (index, companion) in companions.iter().enumerate() {
        if index != 0 {
            output.push_str(", ");
        }
        write_artifact(&mut output, companion)?;
    }
    output.push_str("],\n  \"symbol_prefix\": ");
    write_string(&mut output, symbol_prefix);
    output.push_str(",\n  \"probe_prefix\": ");
    write_string(&mut output, probe_prefix);
    output.push_str(",\n  \"source_name\": ");
    if let Some(source_name) = source_name {
        write_string(&mut output, source_name);
    } else {
        output.push_str("null");
    }
    output.push_str(",\n  \"entry_contract\": ");
    write_string(&mut output, entry_contract.id());
    writeln!(
        output,
        ",\n  \"summary\": {{\"functions\": {inventory_count}, \"generated\": {}, \"blocked\": {}}},",
        generated.len(),
        blocked.len()
    )
    .expect("writing to String cannot fail");
    output.push_str("  \"generated\": [\n");
    for (index, candidate) in generated.iter().enumerate() {
        output.push_str("    {\"symbol\": ");
        write_string(&mut output, &candidate.symbol);
        output.push_str(", \"owner\": ");
        if let Some(owner) = candidate.owner.as_deref() {
            write_string(&mut output, owner);
        } else {
            output.push_str("null");
        }
        output.push_str(", \"reference_file\": ");
        write_string(&mut output, &candidate.reference_file);
        output.push_str(", \"probe_symbol\": ");
        write_string(&mut output, &candidate.probe_symbol);
        write!(
            output,
            ", \"exit_a0_modeled\": {}, \"dependencies\": ",
            candidate.exit_a0_modeled
        )
        .expect("writing to String cannot fail");
        write_strings(&mut output, &candidate.dependencies);
        output.push('}');
        output.push_str(if index + 1 == generated.len() {
            "\n"
        } else {
            ",\n"
        });
    }
    output.push_str("  ],\n  \"blocked\": [\n");
    for (index, candidate) in blocked.iter().enumerate() {
        output.push_str("    {\"symbol\": ");
        write_string(&mut output, &candidate.symbol);
        output.push_str(", \"owner\": ");
        if let Some(owner) = candidate.owner.as_deref() {
            write_string(&mut output, owner);
        } else {
            output.push_str("null");
        }
        output.push_str(", \"reasons\": ");
        write_strings(&mut output, &candidate.reasons);
        output.push('}');
        output.push_str(if index + 1 == blocked.len() {
            "\n"
        } else {
            ",\n"
        });
    }
    output.push_str("  ]\n}\n");
    fs::write(path, output)?;
    Ok(())
}

pub(super) fn run(
    filtered: Vec<String>,
    svd: &MmioRegisterMap,
    target: &TargetSpec,
) -> Result<bool> {
    let mut artifact = None;
    let mut companions = Vec::new();
    let mut symbol_prefix = "phy_".to_owned();
    let mut probe_prefix = "open_phy_trace_".to_owned();
    let mut source_name = None;
    let harness = target.require_available_harness()?;
    let riscv_harness = harnesses::riscv(harness)?;
    let mut entry_contract = harnesses::entry_contract(harness, "none")?;
    let mut output_dir = None;
    let mut manifest = None;
    let mut force = false;
    let mut arguments = filtered.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--artifact" => {
                artifact = Some(PathBuf::from(take_value(&mut arguments, "--artifact")?));
            }
            "--companion" => {
                companions.push(PathBuf::from(take_value(&mut arguments, "--companion")?));
            }
            "--symbol-prefix" => {
                symbol_prefix = take_value(&mut arguments, "--symbol-prefix")?;
            }
            "--probe-prefix" => {
                probe_prefix = take_value(&mut arguments, "--probe-prefix")?;
            }
            "--source-name" => {
                source_name = Some(take_value(&mut arguments, "--source-name")?);
            }
            "--entry-contract" => {
                entry_contract = harnesses::entry_contract(
                    harness,
                    &take_value(&mut arguments, "--entry-contract")?,
                )?;
            }
            "--output-dir" => {
                output_dir = Some(PathBuf::from(take_value(&mut arguments, "--output-dir")?));
            }
            "--manifest" => {
                manifest = Some(PathBuf::from(take_value(&mut arguments, "--manifest")?));
            }
            "--force" => force = true,
            _ => {
                return Err(format!("unknown reference generate-batch option: {argument}").into());
            }
        }
    }
    let artifact = artifact.ok_or("missing --artifact")?;
    let output_dir = output_dir.ok_or("missing --output-dir")?;
    let manifest = manifest.unwrap_or_else(|| output_dir.join("manifest.json"));
    let symbols = list_code_symbols(&artifact, &symbol_prefix)?;
    if symbols.is_empty() {
        return Err(format!("no external code symbols start with {symbol_prefix:?}").into());
    }

    let resolver = ReferenceResolver::load_with_entry_contract(
        &artifact,
        &companions,
        riscv_harness,
        entry_contract,
    )?;
    let digest = artifact_sha256(&artifact)?;
    let artifact_display = artifact.display().to_string();
    let companion_provenance = companions
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
            .map_err(|error| -> Error { error.into() })?;
        let reference_file =
            candidate_file_name(symbol.member.as_deref(), &symbol.name, &mut used_names);
        let generated_reference = codegen::generate(
            &resolved,
            &artifact_display,
            &digest,
            symbol.member.as_deref(),
            &companion_provenance,
        )
        .map_err(|error| -> Error { error.into() })?;
        generated.push(GeneratedCandidate {
            symbol: symbol.name.clone(),
            owner: symbol.member.clone(),
            reference_file,
            probe_symbol: probe_symbol(
                &symbol.name,
                &symbol_prefix,
                &probe_prefix,
                source_name.as_deref(),
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
    if !force && let Some(existing) = destinations.iter().find(|path| path.exists()) {
        return Err(format!(
            "refusing to overwrite {}; pass --force to replace generated output",
            existing.display()
        )
        .into());
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
        println!(
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
    write_manifest(
        &manifest,
        &artifact,
        &companions,
        &symbol_prefix,
        &probe_prefix,
        source_name.as_deref(),
        entry_contract,
        symbols.len(),
        &generated,
        &blocked,
    )?;
    println!("MANIFEST\t{}", manifest.display());
    println!(
        "SUMMARY\tfunctions={}\tgenerated={}\tblocked={}",
        symbols.len(),
        generated.len(),
        blocked.len()
    );
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
