//! Artifact-wide best-effort MMIO register discovery.

use serde::Serialize;

use super::super::*;

#[derive(Serialize)]
struct CommandDocument<'a> {
    #[serde(flatten)]
    artifact: &'a crate::artifacts::MmioFactsDocument,
    #[serde(skip_serializing_if = "Option::is_none")]
    publication: Option<crate::cli::output::Publication>,
}

fn print_report(report: &MmioDiscoveryReport) {
    const MAX_HOT_REGISTERS: usize = 32;

    outputln!("MMIO discovery");
    outputln!(
        "Artifacts:\n{}",
        crate::cli::table::render(
            [
                "Source",
                "Functions",
                "MMIO users",
                "Diagnostics",
                "States",
                "Branches",
                "Path",
            ],
            report.artifacts.iter().map(|artifact| [
                artifact.source.clone(),
                artifact.functions.to_string(),
                artifact.functions_with_mmio.to_string(),
                artifact.functions_with_diagnostics.to_string(),
                artifact.explored_states.to_string(),
                artifact.branch_sites.to_string(),
                artifact.path.display().to_string(),
            ]),
        )
    );
    outputln!(
        "Ranges:\n{}",
        crate::cli::table::render(
            ["Range", "Addresses", "Registers", "Named", "Accesses"],
            report.ranges.iter().map(|range| {
                let registers = report
                    .registers
                    .iter()
                    .filter(|register| {
                        register.address >= range.start && register.address < range.end
                    })
                    .collect::<Vec<_>>();
                let named = registers
                    .iter()
                    .filter(|register| {
                        register.name != format!("{}.REG_{:08X}", range.name, register.address)
                    })
                    .count();
                let accesses = registers
                    .iter()
                    .map(|register| register.read_count + register.write_count)
                    .sum::<usize>();
                [
                    range.name.clone(),
                    format!("{:#010x}..{:#010x}", range.start, range.end),
                    registers.len().to_string(),
                    named.to_string(),
                    accesses.to_string(),
                ]
            }),
        )
    );
    let mut hot_registers = report.registers.iter().collect::<Vec<_>>();
    hot_registers.sort_by_key(|register| {
        (
            std::cmp::Reverse(register.read_count + register.write_count),
            register.address,
            register.width,
        )
    });
    hot_registers.truncate(MAX_HOT_REGISTERS);
    outputln!(
        "Most active registers ({} of {}):\n{}",
        hot_registers.len(),
        report.registers.len(),
        crate::cli::table::render(
            [
                "Address", "Width", "Name", "Reads", "Writes", "Users", "Patterns"
            ],
            hot_registers.into_iter().map(|register| {
                let users = register
                    .read_functions
                    .union(&register.write_functions)
                    .count();
                [
                    format!("{:#010x}", register.address),
                    register.width.to_string(),
                    register.name.clone(),
                    register.read_count.to_string(),
                    register.write_count.to_string(),
                    users.to_string(),
                    register.write_patterns.len().to_string(),
                ]
            }),
        )
    );
    let diagnostic_scopes =
        report
            .diagnostics
            .iter()
            .fold(BTreeMap::<&str, usize>::new(), |mut scopes, diagnostic| {
                *scopes.entry(diagnostic.scope).or_default() += 1;
                scopes
            });
    let accesses = report
        .registers
        .iter()
        .map(|register| register.read_count + register.write_count)
        .sum::<usize>();
    outputln!(
        "Summary: artifacts={} ranges={} register-widths={} accesses={} diagnostics={} ({})",
        report.artifacts.len(),
        report.ranges.len(),
        report.registers.len(),
        accesses,
        report.diagnostics.len(),
        diagnostic_scopes
            .into_iter()
            .map(|(scope, count)| format!("{scope}={count}"))
            .collect::<Vec<_>>()
            .join(", "),
    );
    outputln!(
        "All register findings, users, bit patterns and blockers are available in the JSON report."
    );
}

pub(super) fn run(
    arguments: MmioDiscoverArgs,
    svd: &MmioMap,
    project: Option<&crate::project::ProjectSpec>,
) -> Result<bool> {
    if !(1..=8).contains(&arguments.jobs) {
        return Err(crate::Error::invalid("mmio discover --jobs accepts 1..=8"));
    }
    if arguments.check && arguments.output.is_none() {
        return Err(crate::Error::invalid(
            "mmio discover --check requires --output PATH",
        ));
    }
    let artifacts = arguments
        .artifact
        .into_iter()
        .map(|artifact| (artifact.source.into_string(), artifact.path))
        .collect::<Vec<_>>();
    let ranges = arguments
        .range
        .into_iter()
        .map(|range| DiscoveryRange {
            name: range.name,
            start: range.start,
            end: range.end,
        })
        .collect::<Vec<_>>();
    if artifacts.is_empty() {
        return Err(crate::Error::invalid(
            "mmio discover requires at least one --artifact SOURCE=PATH",
        ));
    }
    if ranges.is_empty() {
        return Err(crate::Error::invalid(
            "mmio discover requires at least one --range NAME=START..END",
        ));
    }
    let mut sources = BTreeSet::new();
    for (source, _) in &artifacts {
        if !sources.insert(source.clone()) {
            return Err(crate::Error::invalid(format!(
                "duplicate artifact source {source:?}"
            )));
        }
    }
    let mut range_names = BTreeSet::new();
    for range in &ranges {
        if !range_names.insert(range.name.clone()) {
            return Err(crate::Error::invalid(format!(
                "duplicate MMIO range name {:?}",
                range.name
            )));
        }
    }

    let code_symbol_selection = match arguments.code_symbols {
        crate::cli::CodeSymbolSelectionArg::All => artifact::CodeSymbolSelection::All,
        crate::cli::CodeSymbolSelectionArg::Exported => artifact::CodeSymbolSelection::Exported,
    };
    let effective_code = project
        .map(crate::analysis::EffectiveCodeCatalog::load)
        .transpose()?;
    let report = discover_mmio(
        &artifacts,
        &ranges,
        &arguments.symbol_prefix,
        code_symbol_selection,
        svd,
        effective_code.as_ref(),
        crate::analysis::MmioDiscoveryOptions {
            jobs: usize::from(arguments.jobs),
        },
    )?;
    let publication = arguments.output.as_deref().map(|path| {
        crate::cli::output::Publication::new(
            path,
            if arguments.check {
                "verified"
            } else {
                "written"
            },
        )
    });
    let artifact = crate::artifacts::build_mmio_facts(&report)?;
    if let Some(path) = arguments.output.as_deref() {
        crate::application::generated_file::write_or_check_json(
            path,
            &artifact,
            arguments.check,
            "MMIO discovery report",
            false,
        )?;
    }
    let document = CommandDocument {
        artifact: &artifact,
        publication: publication.clone(),
    };
    if !crate::cli::output::structured(&document) {
        print_report(&report);
        if let Some(publication) = publication {
            outputln!("\nReport {}: {}", publication.status, publication.path);
        }
    }
    // Discovery is intentionally best-effort. Diagnostics scope individual
    // findings but do not turn a useful partial inventory into a failed run.
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_named_half_open_ranges() {
        assert_eq!(
            "radio=0x20100000..0x20110000"
                .parse::<NamedAddressRange>()
                .map(|range| DiscoveryRange {
                    name: range.name,
                    start: range.start,
                    end: range.end,
                })
                .unwrap(),
            DiscoveryRange {
                name: "radio".to_owned(),
                start: 0x2010_0000,
                end: 0x2011_0000,
            }
        );
    }

    #[test]
    fn artifact_input_keeps_equals_signs_in_paths() {
        let artifact = "libpp=/tmp/vendor=linked.elf"
            .parse::<SourcePath>()
            .unwrap();
        assert_eq!(artifact.source.as_str(), "libpp");
        assert_eq!(
            artifact.path,
            std::path::PathBuf::from("/tmp/vendor=linked.elf")
        );
    }
}
