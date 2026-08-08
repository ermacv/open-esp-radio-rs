//! Stable JSON projection of artifact-wide MMIO evidence.

use serde::Serialize;

use super::{
    super::Result,
    discover_mmio::{function_names, mask_ranges},
};
use crate::{analysis::MmioDiscoveryReport, artifact_sha256};

#[derive(Serialize)]
struct ArtifactIdentity {
    path: String,
    sha256: String,
}

#[derive(Serialize)]
struct RangeDocument<'a> {
    name: &'a str,
    start: String,
    end_exclusive: String,
}

#[derive(Serialize)]
struct ArtifactDocument<'a> {
    source: &'a str,
    artifact: ArtifactIdentity,
    functions: usize,
    functions_with_mmio: usize,
    functions_with_diagnostics: usize,
    explored_states: usize,
    terminal_paths: usize,
    branch_sites: usize,
}

#[derive(Serialize)]
struct WritePatternDocument {
    occurrences: usize,
    modified_mask: String,
    candidate_bit_ranges: String,
    preserved_mask: String,
    inverted_mask: String,
    forced_zero_mask: String,
    forced_one_mask: String,
    read_derived_mask: String,
    dynamic_mask: String,
    functions: Vec<String>,
}

#[derive(Serialize)]
struct RegisterDocument<'a> {
    address: String,
    width: u8,
    name: &'a str,
    reads: usize,
    writes: usize,
    read_functions: Vec<String>,
    write_functions: Vec<String>,
    write_patterns: Vec<WritePatternDocument>,
}

#[derive(Serialize)]
struct DiagnosticDocument<'a> {
    function: String,
    scope: &'a str,
    message: &'a str,
}

#[derive(Serialize)]
pub(super) struct DiscoveryDocument<'a> {
    schema_version: u32,
    command: &'static str,
    analysis_mode: &'static str,
    access_count_mode: &'static str,
    completeness_claim: bool,
    ranges: Vec<RangeDocument<'a>>,
    artifacts: Vec<ArtifactDocument<'a>>,
    registers: Vec<RegisterDocument<'a>>,
    diagnostics: Vec<DiagnosticDocument<'a>>,
}

pub(super) fn document(report: &MmioDiscoveryReport) -> Result<DiscoveryDocument<'_>> {
    Ok(DiscoveryDocument {
        schema_version: 2,
        command: "mmio discover",
        analysis_mode: "best-effort",
        access_count_mode: "maximum-per-path",
        completeness_claim: false,
        ranges: report
            .ranges
            .iter()
            .map(|range| RangeDocument {
                name: &range.name,
                start: format!("{:#010x}", range.start),
                end_exclusive: format!("{:#010x}", range.end),
            })
            .collect(),
        artifacts: report
            .artifacts
            .iter()
            .map(|artifact| {
                Ok(ArtifactDocument {
                    source: &artifact.source,
                    artifact: ArtifactIdentity {
                        path: artifact.path.display().to_string(),
                        sha256: artifact_sha256(&artifact.path)?,
                    },
                    functions: artifact.functions,
                    functions_with_mmio: artifact.functions_with_mmio,
                    functions_with_diagnostics: artifact.functions_with_diagnostics,
                    explored_states: artifact.explored_states,
                    terminal_paths: artifact.terminal_paths,
                    branch_sites: artifact.branch_sites,
                })
            })
            .collect::<Result<Vec<_>>>()?,
        registers: report
            .registers
            .iter()
            .map(|register| RegisterDocument {
                address: format!("{:#010x}", register.address),
                width: register.width,
                name: &register.name,
                reads: register.read_count,
                writes: register.write_count,
                read_functions: function_names(&register.read_functions),
                write_functions: function_names(&register.write_functions),
                write_patterns: register
                    .write_patterns
                    .iter()
                    .map(|finding| {
                        let pattern = &finding.pattern;
                        let modified_mask = pattern.modified_mask(register.width);
                        WritePatternDocument {
                            occurrences: finding.occurrences,
                            modified_mask: format!("{modified_mask:#010x}"),
                            candidate_bit_ranges: mask_ranges(modified_mask),
                            preserved_mask: format!("{:#010x}", pattern.preserved_mask),
                            inverted_mask: format!("{:#010x}", pattern.inverted_mask),
                            forced_zero_mask: format!("{:#010x}", pattern.forced_zero_mask),
                            forced_one_mask: format!("{:#010x}", pattern.forced_one_mask),
                            read_derived_mask: format!("{:#010x}", pattern.read_derived_mask),
                            dynamic_mask: format!("{:#010x}", pattern.dynamic_mask),
                            functions: function_names(&finding.functions),
                        }
                    })
                    .collect(),
            })
            .collect(),
        diagnostics: report
            .diagnostics
            .iter()
            .map(|diagnostic| DiagnosticDocument {
                function: diagnostic.function.canonical(),
                scope: diagnostic.scope,
                message: &diagnostic.message,
            })
            .collect(),
    })
}

pub(super) fn render_document(document: &DiscoveryDocument<'_>) -> Result<String> {
    let mut output = serde_json::to_string_pretty(&document)?;
    output.push('\n');
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
        let rendered = render_document(&document(&report).unwrap()).unwrap();
        let parsed = serde_json::from_str::<serde_json::Value>(&rendered).unwrap();
        assert_eq!(parsed["schema_version"], 2);
        assert_eq!(parsed["command"], "mmio discover");
    }
}
