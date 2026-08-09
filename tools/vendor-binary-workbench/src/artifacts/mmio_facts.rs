//! Stored schema-v4 projection of artifact-wide MMIO evidence.

use serde::Serialize;

use super::MMIO_FACTS;
use crate::{analysis::MmioDiscoveryReport, artifact_sha256};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct ArtifactIdentity {
    path: String,
    sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct RangeDocument {
    name: String,
    start: String,
    end_exclusive: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct ArtifactDocument {
    source: String,
    artifact: ArtifactIdentity,
    functions: usize,
    reviewed_boundaries: usize,
    functions_with_mmio: usize,
    functions_with_diagnostics: usize,
    explored_states: usize,
    terminal_paths: usize,
    branch_sites: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct RegisterDocument {
    address: String,
    width: u8,
    name: String,
    reads: usize,
    writes: usize,
    read_functions: Vec<String>,
    write_functions: Vec<String>,
    write_patterns: Vec<WritePatternDocument>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct DiagnosticDocument {
    function: String,
    scope: &'static str,
    message: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct CodeSelectionDocument {
    symbols: &'static str,
    symbol_prefix: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct MmioFactsDocument {
    schema_version: u32,
    command: &'static str,
    analysis_mode: &'static str,
    access_count_mode: &'static str,
    completeness_claim: bool,
    code_selection: CodeSelectionDocument,
    ranges: Vec<RangeDocument>,
    artifacts: Vec<ArtifactDocument>,
    registers: Vec<RegisterDocument>,
    diagnostics: Vec<DiagnosticDocument>,
}

pub(crate) fn build_mmio_facts(report: &MmioDiscoveryReport) -> crate::Result<MmioFactsDocument> {
    Ok(MmioFactsDocument {
        schema_version: MMIO_FACTS.version,
        command: MMIO_FACTS.command,
        analysis_mode: "best-effort",
        access_count_mode: "maximum-per-path",
        completeness_claim: false,
        code_selection: CodeSelectionDocument {
            symbols: report.code_symbol_selection.label(),
            symbol_prefix: report.symbol_prefix.clone(),
        },
        ranges: report
            .ranges
            .iter()
            .map(|range| RangeDocument {
                name: range.name.clone(),
                start: format!("{:#010x}", range.start),
                end_exclusive: format!("{:#010x}", range.end),
            })
            .collect(),
        artifacts: report
            .artifacts
            .iter()
            .map(|artifact| {
                Ok(ArtifactDocument {
                    source: artifact.source.clone(),
                    artifact: ArtifactIdentity {
                        path: artifact.path.display().to_string(),
                        sha256: artifact_sha256(&artifact.path)?,
                    },
                    functions: artifact.functions,
                    reviewed_boundaries: artifact.reviewed_boundaries,
                    functions_with_mmio: artifact.functions_with_mmio,
                    functions_with_diagnostics: artifact.functions_with_diagnostics,
                    explored_states: artifact.explored_states,
                    terminal_paths: artifact.terminal_paths,
                    branch_sites: artifact.branch_sites,
                })
            })
            .collect::<crate::Result<Vec<_>>>()?,
        registers: report
            .registers
            .iter()
            .map(|register| RegisterDocument {
                address: format!("{:#010x}", register.address),
                width: register.width,
                name: register.name.clone(),
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
                message: diagnostic.message.clone(),
            })
            .collect(),
    })
}

pub(crate) fn render_mmio_facts(document: &MmioFactsDocument) -> crate::Result<String> {
    let mut output = serde_json::to_string_pretty(document)?;
    output.push('\n');
    Ok(output)
}

fn function_names(
    functions: &std::collections::BTreeSet<crate::analysis::DiscoveryFunction>,
) -> Vec<String> {
    functions
        .iter()
        .map(|function| function.canonical())
        .collect()
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
        ranges.push(if start == bit {
            start.to_string()
        } else {
            format!("{start}-{bit}")
        });
        bit += 1;
    }
    if ranges.is_empty() {
        "-".to_owned()
    } else {
        ranges.join(",")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rendered_report_has_the_canonical_identity() {
        let report = MmioDiscoveryReport {
            code_symbol_selection: crate::artifact::CodeSymbolSelection::All,
            symbol_prefix: String::new(),
            artifacts: Vec::new(),
            ranges: Vec::new(),
            registers: Vec::new(),
            diagnostics: Vec::new(),
        };
        let rendered = render_mmio_facts(&build_mmio_facts(&report).unwrap()).unwrap();
        let parsed = serde_json::from_str::<serde_json::Value>(&rendered).unwrap();
        assert_eq!(parsed["schema_version"], 4);
        assert_eq!(parsed["command"], "mmio discover");
        assert_eq!(parsed["code_selection"]["symbols"], "all");
        assert_eq!(parsed["code_selection"]["symbol_prefix"], "");
    }

    #[test]
    fn stored_mmio_facts_reject_unknown_and_missing_fields() {
        let report = MmioDiscoveryReport {
            code_symbol_selection: crate::artifact::CodeSymbolSelection::All,
            symbol_prefix: String::new(),
            artifacts: Vec::new(),
            ranges: Vec::new(),
            registers: Vec::new(),
            diagnostics: Vec::new(),
        };
        let rendered = render_mmio_facts(&build_mmio_facts(&report).unwrap()).unwrap();
        let mut unknown: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        unknown["legacy_field"] = serde_json::json!(true);
        let error = super::super::parse_mmio_facts(&unknown.to_string()).unwrap_err();
        assert!(error.to_string().contains("unknown field `legacy_field`"));

        let mut missing: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        missing.as_object_mut().unwrap().remove("diagnostics");
        let error = super::super::parse_mmio_facts(&missing.to_string()).unwrap_err();
        assert!(error.to_string().contains("missing field `diagnostics`"));
    }
}
