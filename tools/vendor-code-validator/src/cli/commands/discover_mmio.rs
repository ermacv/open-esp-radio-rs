//! Artifact-wide best-effort MMIO register discovery.

use std::{fmt::Write as _, path::Path};

use super::super::json::{write_artifact, write_string, write_strings};
use super::super::*;

fn stable_id(value: &str, kind: &str) -> Result<String> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(format!("invalid {kind} id {value:?}").into());
    }
    Ok(value.to_owned())
}

fn parse_artifact(value: &str) -> Result<(String, PathBuf)> {
    let (source, path) = value
        .split_once('=')
        .filter(|(source, path)| !source.is_empty() && !path.is_empty())
        .ok_or("--artifact requires SOURCE=PATH")?;
    Ok((stable_id(source, "artifact source")?, PathBuf::from(path)))
}

fn source_artifact_option(argument: &str) -> Option<&str> {
    argument
        .strip_prefix("--source-artifact:")
        .filter(|source| !source.is_empty())
}

fn parse_range(value: &str) -> Result<DiscoveryRange> {
    let (name, bounds) = value
        .split_once('=')
        .filter(|(name, bounds)| !name.is_empty() && !bounds.is_empty())
        .ok_or("--range requires NAME=START..END")?;
    let (start, end) = bounds
        .split_once("..")
        .filter(|(start, end)| !start.is_empty() && !end.is_empty())
        .ok_or("--range requires a half-open START..END interval")?;
    let start = parse_u32(start).ok_or("invalid --range start")?;
    let end = parse_u32(end).ok_or("invalid --range end")?;
    if start >= end {
        return Err("--range start must be less than its exclusive end".into());
    }
    Ok(DiscoveryRange {
        name: stable_id(name, "MMIO range")?,
        start,
        end,
    })
}

fn function_names(functions: &BTreeSet<DiscoveryFunction>) -> Vec<String> {
    functions.iter().map(DiscoveryFunction::canonical).collect()
}

fn mask_ranges(mask: u32) -> String {
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
        println!(
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
        println!(
            "REGISTER\t{:#010x}\twidth={}\t{}\treads={}\twrites={}\tfunctions={}",
            register.address,
            register.width,
            register.name,
            register.read_count,
            register.write_count,
            users.len(),
        );
        if !register.read_functions.is_empty() {
            println!(
                "READ-USERS\t{:#010x}\t{}",
                register.address,
                function_names(&register.read_functions).join(",")
            );
        }
        if !register.write_functions.is_empty() {
            println!(
                "WRITE-USERS\t{:#010x}\t{}",
                register.address,
                function_names(&register.write_functions).join(",")
            );
        }
        for pattern in &register.write_patterns {
            println!(
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
        println!(
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
    println!(
        "SUMMARY\tartifacts={}\tranges={}\tregister-widths={}\taccesses={}\tdiagnostics={}",
        report.artifacts.len(),
        report.ranges.len(),
        report.registers.len(),
        accesses,
        report.diagnostics.len(),
    );
}

fn write_functions_json(output: &mut String, functions: &BTreeSet<DiscoveryFunction>) {
    write_strings(output, function_names(functions));
}

fn write_json_report(path: &Path, report: &MmioDiscoveryReport) -> Result<()> {
    let mut output = String::new();
    output.push_str("{\n  \"schema_version\": 1,\n  \"command\": \"mmio-discover\",\n");
    output.push_str("  \"analysis_mode\": \"best-effort\",\n");
    output.push_str("  \"access_count_mode\": \"maximum-per-path\",\n");
    output.push_str("  \"completeness_claim\": false,\n  \"ranges\": [\n");
    for (index, range) in report.ranges.iter().enumerate() {
        output.push_str("    {\"name\": ");
        write_string(&mut output, &range.name);
        writeln!(
            output,
            ", \"start\": \"{:#010x}\", \"end_exclusive\": \"{:#010x}\"}}{}",
            range.start,
            range.end,
            if index + 1 == report.ranges.len() {
                ""
            } else {
                ","
            }
        )
        .expect("writing to String cannot fail");
    }
    output.push_str("  ],\n  \"artifacts\": [\n");
    for (index, artifact) in report.artifacts.iter().enumerate() {
        output.push_str("    {\"source\": ");
        write_string(&mut output, &artifact.source);
        output.push_str(", \"artifact\": ");
        write_artifact(&mut output, &artifact.path)?;
        writeln!(
            output,
            ", \"functions\": {}, \"functions_with_mmio\": {}, \"functions_with_diagnostics\": {}, \"explored_states\": {}, \"terminal_paths\": {}, \"branch_sites\": {}}}{}",
            artifact.functions,
            artifact.functions_with_mmio,
            artifact.functions_with_diagnostics,
            artifact.explored_states,
            artifact.terminal_paths,
            artifact.branch_sites,
            if index + 1 == report.artifacts.len() {
                ""
            } else {
                ","
            }
        )
        .expect("writing to String cannot fail");
    }
    output.push_str("  ],\n  \"registers\": [\n");
    for (index, register) in report.registers.iter().enumerate() {
        write!(
            output,
            "    {{\"address\": \"{:#010x}\", \"width\": {}, \"name\": ",
            register.address, register.width
        )
        .expect("writing to String cannot fail");
        write_string(&mut output, &register.name);
        write!(
            output,
            ", \"reads\": {}, \"writes\": {}, \"read_functions\": ",
            register.read_count, register.write_count
        )
        .expect("writing to String cannot fail");
        write_functions_json(&mut output, &register.read_functions);
        output.push_str(", \"write_functions\": ");
        write_functions_json(&mut output, &register.write_functions);
        output.push_str(", \"write_patterns\": [");
        for (pattern_index, finding) in register.write_patterns.iter().enumerate() {
            if pattern_index != 0 {
                output.push_str(", ");
            }
            let pattern = &finding.pattern;
            write!(
                output,
                "{{\"occurrences\": {}, \"modified_mask\": \"{:#010x}\", \"candidate_bit_ranges\": ",
                finding.occurrences,
                pattern.modified_mask(register.width),
            )
            .expect("writing to String cannot fail");
            write_string(
                &mut output,
                &mask_ranges(pattern.modified_mask(register.width)),
            );
            write!(
                output,
                ", \"preserved_mask\": \"{:#010x}\", \"inverted_mask\": \"{:#010x}\", \"forced_zero_mask\": \"{:#010x}\", \"forced_one_mask\": \"{:#010x}\", \"read_derived_mask\": \"{:#010x}\", \"dynamic_mask\": \"{:#010x}\", \"functions\": ",
                pattern.preserved_mask,
                pattern.inverted_mask,
                pattern.forced_zero_mask,
                pattern.forced_one_mask,
                pattern.read_derived_mask,
                pattern.dynamic_mask,
            )
            .expect("writing to String cannot fail");
            write_functions_json(&mut output, &finding.functions);
            output.push('}');
        }
        output.push_str("]}");
        output.push_str(if index + 1 == report.registers.len() {
            "\n"
        } else {
            ",\n"
        });
    }
    output.push_str("  ],\n  \"diagnostics\": [\n");
    for (index, diagnostic) in report.diagnostics.iter().enumerate() {
        output.push_str("    {\"function\": ");
        write_string(&mut output, &diagnostic.function.canonical());
        output.push_str(", \"scope\": ");
        write_string(&mut output, diagnostic.scope);
        output.push_str(", \"message\": ");
        write_string(&mut output, &diagnostic.message);
        output.push('}');
        output.push_str(if index + 1 == report.diagnostics.len() {
            "\n"
        } else {
            ",\n"
        });
    }
    output.push_str("  ]\n}\n");
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, output)?;
    println!("JSON-REPORT\t{}", path.display());
    Ok(())
}

pub(super) fn run(filtered: Vec<String>, svd: &MmioRegisterMap) -> Result<bool> {
    let mut artifacts = Vec::new();
    let mut ranges = Vec::new();
    let mut symbol_prefix = String::new();
    let mut json_report = None;
    let mut arguments = filtered.into_iter();
    while let Some(argument) = arguments.next() {
        if let Some(source) = source_artifact_option(&argument) {
            artifacts.push((
                stable_id(source, "artifact source")?,
                PathBuf::from(take_value(&mut arguments, &argument)?),
            ));
            continue;
        }
        match argument.as_str() {
            "--artifact" => {
                artifacts.push(parse_artifact(&take_value(&mut arguments, "--artifact")?)?);
            }
            "--range" => {
                ranges.push(parse_range(&take_value(&mut arguments, "--range")?)?);
            }
            "--symbol-prefix" => {
                symbol_prefix = take_value(&mut arguments, "--symbol-prefix")?;
            }
            "--json-report" => {
                json_report = Some(PathBuf::from(take_value(&mut arguments, "--json-report")?));
            }
            _ => return Err(format!("unknown mmio discover option: {argument}").into()),
        }
    }
    if artifacts.is_empty() {
        return Err("mmio discover requires at least one --artifact SOURCE=PATH".into());
    }
    if ranges.is_empty() {
        return Err("mmio discover requires at least one --range NAME=START..END".into());
    }
    let mut sources = BTreeSet::new();
    for (source, _) in &artifacts {
        if !sources.insert(source.clone()) {
            return Err(format!("duplicate artifact source {source:?}").into());
        }
    }
    let mut range_names = BTreeSet::new();
    for range in &ranges {
        if !range_names.insert(range.name.clone()) {
            return Err(format!("duplicate MMIO range name {:?}", range.name).into());
        }
    }

    let report = discover_mmio(&artifacts, &ranges, &symbol_prefix, svd)?;
    print_report(&report);
    if let Some(path) = json_report.as_deref() {
        write_json_report(path, &report)?;
    }
    // Discovery is intentionally best-effort. Diagnostics qualify individual
    // findings but do not turn a useful partial inventory into a failed run.
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_named_half_open_ranges() {
        assert_eq!(
            parse_range("radio=0x20100000..0x20110000").unwrap(),
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
        assert_eq!(
            parse_artifact("libpp=/tmp/vendor=linked.elf").unwrap(),
            ("libpp".to_owned(), PathBuf::from("/tmp/vendor=linked.elf"))
        );
    }
}
