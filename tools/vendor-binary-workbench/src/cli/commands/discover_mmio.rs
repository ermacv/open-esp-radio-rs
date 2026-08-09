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

pub(super) fn function_names(functions: &BTreeSet<DiscoveryFunction>) -> Vec<String> {
    functions.iter().map(DiscoveryFunction::canonical).collect()
}

pub(super) fn mask_ranges(mask: u32) -> String {
    let mut ranges = Vec::new();
    let mut bit = 0_u8;
    while bit < 32 {
        if mask & (1_u32 << bit) == 0 {
            bit += 1;
            continue;
        }
        let start = bit;
        while bit < 31 && mask & (1_u32 << (bit + 1)) != 0 {
            bit += 1;
        }
        if start == bit {
            ranges.push(start.to_string());
        } else {
            ranges.push(format!("{start}-{bit}"));
        }
        bit += 1;
    }
    if ranges.is_empty() {
        "-".to_owned()
    } else {
        ranges.join(",")
    }
}

fn print_report(report: &MmioDiscoveryReport) {
    for artifact in &report.artifacts {
        outputln!(
            "ARTIFACT\t{}\t{}\tfunctions={}\tfunctions-with-mmio={}\tfunctions-with-diagnostics={}\texplored-states={}\tterminal-paths={}\tbranch-sites={}",
            artifact.source,
            artifact.path.display(),
            artifact.functions,
            artifact.functions_with_mmio,
            artifact.functions_with_diagnostics,
            artifact.explored_states,
            artifact.terminal_paths,
            artifact.branch_sites,
        );
    }
    for register in &report.registers {
        let users = register
            .read_functions
            .union(&register.write_functions)
            .cloned()
            .collect::<BTreeSet<_>>();
        outputln!(
            "REGISTER\t{:#010x}\twidth={}\t{}\treads={}\twrites={}\tfunctions={}",
            register.address,
            register.width,
            register.name,
            register.read_count,
            register.write_count,
            users.len(),
        );
        if !register.read_functions.is_empty() {
            outputln!(
                "READ-USERS\t{:#010x}\t{}",
                register.address,
                function_names(&register.read_functions).join(",")
            );
        }
        if !register.write_functions.is_empty() {
            outputln!(
                "WRITE-USERS\t{:#010x}\t{}",
                register.address,
                function_names(&register.write_functions).join(",")
            );
        }
        for pattern in &register.write_patterns {
            outputln!(
                "WRITE-PATTERN\t{:#010x}\twidth={}\toccurrences={}\tmodified={:#010x}\tfields={}\tpreserved={:#010x}\tinverted={:#010x}\tforced-zero={:#010x}\tforced-one={:#010x}\tread-derived={:#010x}\tdynamic={:#010x}\tfunctions={}",
                register.address,
                register.width,
                pattern.occurrences,
                pattern.pattern.modified_mask(register.width),
                mask_ranges(pattern.pattern.modified_mask(register.width)),
                pattern.pattern.preserved_mask,
                pattern.pattern.inverted_mask,
                pattern.pattern.forced_zero_mask,
                pattern.pattern.forced_one_mask,
                pattern.pattern.read_derived_mask,
                pattern.pattern.dynamic_mask,
                function_names(&pattern.functions).join(","),
            );
        }
    }
    for diagnostic in &report.diagnostics {
        outputln!(
            "DIAGNOSTIC\t{}\t{}\t{}",
            diagnostic.function.canonical(),
            diagnostic.scope,
            diagnostic.message
        );
    }
    let accesses = report
        .registers
        .iter()
        .map(|register| register.read_count + register.write_count)
        .sum::<usize>();
    outputln!(
        "SUMMARY\tartifacts={}\tranges={}\tregister-widths={}\taccesses={}\tdiagnostics={}",
        report.artifacts.len(),
        report.ranges.len(),
        report.registers.len(),
        accesses,
        report.diagnostics.len(),
    );
}

pub(super) fn run(arguments: MmioDiscoverArgs, svd: &MmioMap) -> Result<bool> {
    if arguments.check && arguments.json_report.is_none() {
        return Err(crate::Error::invalid(
            "mmio discover --check requires --json-report PATH",
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
    let report = discover_mmio(
        &artifacts,
        &ranges,
        &arguments.symbol_prefix,
        code_symbol_selection,
        svd,
    )?;
    let publication = arguments.json_report.as_deref().map(|path| {
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
    if let Some(path) = arguments.json_report.as_deref() {
        let output = crate::artifacts::render_mmio_facts(&artifact)?;
        crate::application::generated_file::write_or_check(
            path,
            &output,
            arguments.check,
            "MMIO discovery report",
        )?;
    }
    let document = CommandDocument {
        artifact: &artifact,
        publication: publication.clone(),
    };
    if !crate::cli::output::structured(&document) {
        print_report(&report);
        if let Some(publication) = publication {
            outputln!(
                "PUBLICATION\tstatus={}\tpath={}",
                publication.status,
                publication.path
            );
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
    fn formats_candidate_bit_ranges() {
        assert_eq!(mask_ranges(0b1011_1110), "1-5,7");
        assert_eq!(mask_ranges(0), "-");
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
