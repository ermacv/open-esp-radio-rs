//! Stable JSON projection of artifact-wide MMIO evidence.

use std::{collections::BTreeSet, fmt::Write as _};

use super::{
    super::{
        Result,
        json::{write_artifact, write_string, write_strings},
    },
    discover_mmio::{function_names, mask_ranges},
};
use crate::analysis::{DiscoveryFunction, MmioDiscoveryReport};

fn write_functions(output: &mut String, functions: &BTreeSet<DiscoveryFunction>) {
    write_strings(output, function_names(functions));
}

pub(super) fn render_json_report(report: &MmioDiscoveryReport) -> Result<String> {
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
        write_functions(&mut output, &register.read_functions);
        output.push_str(", \"write_functions\": ");
        write_functions(&mut output, &register.write_functions);
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
            write_functions(&mut output, &finding.functions);
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
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rendered_report_is_stable_json() {
        let report = MmioDiscoveryReport {
            artifacts: Vec::new(),
            ranges: Vec::new(),
            registers: Vec::new(),
            diagnostics: Vec::new(),
        };
        let rendered = render_json_report(&report).unwrap();
        let parsed = serde_json::from_str::<serde_json::Value>(&rendered).unwrap();
        assert_eq!(parsed["command"], "mmio-discover");
    }
}
