//! Typed projection of project verification evidence for one vendor function.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{ProjectSpec, Result};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReplacementEvidence {
    pub association: &'static str,
    pub report: String,
    pub report_complete_project_run: bool,
    pub report_passed: bool,
    pub freshness_claim: bool,
    pub vendor_source: String,
    pub vendor_symbol: String,
    pub status: String,
    pub reviewed: bool,
    pub disposition: Option<String>,
    pub protocol: Option<String>,
    pub binding_scope: Option<String>,
    pub production_component: Option<String>,
    pub production_component_evidence: Option<ProductionComponentEvidence>,
    pub verification_probes: Vec<String>,
    pub proofs: Vec<ReplacementProofEvidence>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct ProductionComponentEvidence {
    pub component_id: String,
    pub source_status: String,
    pub compiled_status: String,
    pub freshness_status: String,
    #[serde(default)]
    pub source_items: Vec<ProductionSourceItemEvidence>,
    #[serde(default)]
    pub compiled_symbols: Vec<ProductionCompiledSymbolEvidence>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct ProductionSourceItemEvidence {
    pub path: String,
    pub line: usize,
    pub kind: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct ProductionCompiledSymbolEvidence {
    pub artifact: String,
    pub demangled: String,
    pub address: String,
    pub size: u64,
    pub source_file: Option<String>,
    pub source_line: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct ReplacementProofEvidence {
    pub suite: String,
    pub status: String,
    pub claim: Option<String>,
    pub probe_symbol: Option<String>,
    pub evidence: Option<String>,
    pub contract: Option<String>,
    pub profile: Option<String>,
    pub hil_evidence: Option<String>,
    #[serde(default)]
    pub blockers: Vec<String>,
    #[serde(default)]
    pub adapter_cases: Vec<ReplacementCaseEvidence>,
    #[serde(default)]
    pub execution_cases: Vec<ReplacementExecutionCaseEvidence>,
    #[serde(default)]
    pub effects: Option<usize>,
    #[serde(default)]
    pub return_compared: Option<bool>,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct ReplacementCaseEvidence {
    pub name: String,
    pub matched: bool,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct ReplacementExecutionCaseEvidence {
    pub name: String,
    pub verdict: String,
    pub events: Option<usize>,
    pub memory_changes: Option<usize>,
    pub return_compared: Option<bool>,
    pub first_difference: Option<usize>,
    pub difference_kind: Option<String>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct ReviewedEffectRuleEvidence {
    pub suite: String,
    pub selector: String,
    pub disposition: String,
}

#[derive(Deserialize)]
struct StoredVerificationProjection {
    complete_project_run: bool,
    passed: bool,
    replacement_graph: StoredReplacementGraphProjection,
    rust_component_index: StoredRustComponentIndexProjection,
}

#[derive(Deserialize)]
struct StoredSuiteProjection {
    id: String,
    #[serde(default)]
    sources: Vec<StoredSourceProjection>,
}

#[derive(Deserialize)]
struct StoredSourceProjection {
    source: String,
    #[serde(default)]
    functions: Vec<StoredFunctionDetailProjection>,
}

#[derive(Deserialize)]
struct StoredFunctionDetailProjection {
    vendor_symbol: String,
    #[serde(default)]
    adapter_cases: Vec<ReplacementCaseEvidence>,
    effects: Option<usize>,
    return_compared: Option<bool>,
    reason: Option<String>,
    execution: Option<StoredExecutionProjection>,
}

#[derive(Deserialize)]
struct StoredExecutionProjection {
    #[serde(default)]
    cases: Vec<StoredExecutionCaseProjection>,
}

#[derive(Deserialize)]
#[serde(tag = "verdict", rename_all = "kebab-case")]
enum StoredExecutionCaseProjection {
    Match {
        name: String,
        events: usize,
        memory_changes: usize,
        return_compared: bool,
    },
    Diff {
        name: String,
        difference: StoredDifferenceProjection,
    },
    Incomplete {
        name: String,
    },
}

#[derive(Deserialize)]
struct StoredDifferenceProjection {
    first_difference: usize,
    kind: String,
}

#[derive(Deserialize)]
struct StoredRustComponentIndexProjection {
    components: Vec<ProductionComponentEvidence>,
}

#[derive(Deserialize)]
struct StoredReplacementGraphProjection {
    replacements: Vec<StoredReplacementProjection>,
}

#[derive(Deserialize)]
struct StoredReplacementProjection {
    vendor: StoredVendorFunctionProjection,
    reviewed: bool,
    disposition: Option<String>,
    protocol: Option<String>,
    rust: Option<StoredRustReplacementProjection>,
    status: String,
    proofs: Vec<ReplacementProofEvidence>,
}

#[derive(Deserialize)]
struct StoredVendorFunctionProjection {
    source: String,
    symbol: String,
}

#[derive(Deserialize)]
struct StoredRustReplacementProjection {
    binding_scope: String,
    production_component: Option<String>,
    #[serde(default)]
    verification_probes: Vec<String>,
}

pub(crate) fn replacement_evidence(
    source: &str,
    symbol: &str,
    project: &ProjectSpec,
) -> Result<Vec<ReplacementEvidence>> {
    let Some(workspace) = project.verification.as_ref() else {
        return Ok(Vec::new());
    };
    if !workspace.report.is_file() {
        return Ok(Vec::new());
    }
    let input = std::fs::read_to_string(&workspace.report)
        .map_err(|source| crate::Error::read("verification report", &workspace.report, source))?;
    let mut evidence = parse_replacement_evidence(
        source,
        symbol,
        &workspace.report,
        serde_json::from_str(&input)?,
    )?;
    let suites = evidence
        .iter()
        .flat_map(|replacement| {
            replacement
                .proofs
                .iter()
                .filter(|proof| proof.status != "uncovered")
                .map(|proof| proof.suite.clone())
        })
        .collect::<std::collections::BTreeSet<_>>();
    for suite in suites {
        let path = crate::qualification::suite_report_path(&workspace.report, &suite);
        let input = std::fs::read_to_string(&path)
            .map_err(|source| crate::Error::read("verification suite report", &path, source))?;
        let report: StoredSuiteProjection = serde_json::from_str(&input)?;
        if report.id != suite {
            return Err(crate::Error::invalid(format!(
                "verification suite report {} has id {:?}, expected {suite:?}",
                path.display(),
                report.id
            )));
        }
        join_proof_details(&mut evidence, report);
    }
    Ok(evidence)
}

/// Load the reviewed effect boundary for one exact project function.
///
/// These rows are policy, not an observed execution trace. Currency remains
/// owned by `project check`, just like the other editable project inputs.
pub(crate) fn reviewed_effect_rules(
    source: &str,
    symbol: &str,
    project: &ProjectSpec,
) -> Result<Vec<ReviewedEffectRuleEvidence>> {
    let Some(workspace) = project.verification.as_ref() else {
        return Ok(Vec::new());
    };
    let mut rules = std::collections::BTreeSet::new();
    for suite in &workspace.suites {
        if !suite
            .vendor
            .iter()
            .any(|vendor| vendor.source.as_str() == source && vendor.selection.includes(symbol))
        {
            continue;
        }
        let Some(manifest) =
            crate::verification::dispositions::Manifest::load_all(&suite.dispositions)?
        else {
            continue;
        };
        let resolved = manifest.resolve(source, symbol);
        let Some(policy) = resolved
            .entry
            .and_then(|entry| entry.effect_contract.as_ref())
        else {
            continue;
        };
        rules.extend(
            policy
                .rules()
                .map(|(selector, disposition)| ReviewedEffectRuleEvidence {
                    suite: suite.id.clone(),
                    selector: selector.canonical(),
                    disposition: disposition.canonical(),
                }),
        );
    }
    Ok(rules.into_iter().collect())
}

fn parse_replacement_evidence(
    source: &str,
    symbol: &str,
    report: &Path,
    document: StoredVerificationProjection,
) -> Result<Vec<ReplacementEvidence>> {
    let components = document
        .rust_component_index
        .components
        .into_iter()
        .map(|component| (component.component_id.clone(), component))
        .collect::<std::collections::BTreeMap<_, _>>();
    let candidates = document
        .replacement_graph
        .replacements
        .into_iter()
        .filter(|edge| edge.vendor.symbol == symbol)
        .collect::<Vec<_>>();
    let has_exact = candidates.iter().any(|edge| edge.vendor.source == source);
    let selected = if has_exact {
        candidates
            .into_iter()
            .filter(|edge| edge.vendor.source == source)
            .collect()
    } else if candidates.len() == 1 {
        candidates
    } else {
        Vec::new()
    };
    selected
        .into_iter()
        .map(|edge| {
            let vendor_source = edge.vendor.source;
            let association = if vendor_source == source {
                "exact-source-symbol"
            } else {
                "unique-symbol-across-replacement-graph"
            };
            Ok(ReplacementEvidence {
                association,
                report: report.display().to_string(),
                report_complete_project_run: document.complete_project_run,
                report_passed: document.passed,
                freshness_claim: false,
                vendor_source,
                vendor_symbol: edge.vendor.symbol,
                status: edge.status,
                reviewed: edge.reviewed,
                disposition: edge.disposition,
                protocol: edge.protocol,
                binding_scope: edge.rust.as_ref().map(|rust| rust.binding_scope.clone()),
                production_component: edge
                    .rust
                    .as_ref()
                    .and_then(|rust| rust.production_component.clone()),
                production_component_evidence: edge
                    .rust
                    .as_ref()
                    .and_then(|rust| rust.production_component.as_ref())
                    .and_then(|component| components.get(component))
                    .cloned(),
                verification_probes: edge
                    .rust
                    .map(|rust| rust.verification_probes)
                    .unwrap_or_default(),
                proofs: edge.proofs,
            })
        })
        .collect()
}

fn join_proof_details(evidence: &mut [ReplacementEvidence], suite: StoredSuiteProjection) {
    let suite_id = suite.id;
    let details = suite
        .sources
        .into_iter()
        .flat_map(|source| {
            source.functions.into_iter().map(move |function| {
                (
                    (source.source.clone(), function.vendor_symbol.clone()),
                    function,
                )
            })
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    for replacement in evidence {
        let Some(detail) = details.get(&(
            replacement.vendor_source.clone(),
            replacement.vendor_symbol.clone(),
        )) else {
            continue;
        };
        for proof in replacement
            .proofs
            .iter_mut()
            .filter(|proof| proof.suite == suite_id)
        {
            proof.adapter_cases.clone_from(&detail.adapter_cases);
            proof.effects = detail.effects;
            proof.return_compared = detail.return_compared;
            proof.reason.clone_from(&detail.reason);
            proof.execution_cases = detail
                .execution
                .as_ref()
                .map(|execution| {
                    execution
                        .cases
                        .iter()
                        .map(ReplacementExecutionCaseEvidence::from)
                        .collect()
                })
                .unwrap_or_default();
        }
    }
}

impl From<&StoredExecutionCaseProjection> for ReplacementExecutionCaseEvidence {
    fn from(case: &StoredExecutionCaseProjection) -> Self {
        match case {
            StoredExecutionCaseProjection::Match {
                name,
                events,
                memory_changes,
                return_compared,
            } => Self {
                name: name.clone(),
                verdict: "match".to_owned(),
                events: Some(*events),
                memory_changes: Some(*memory_changes),
                return_compared: Some(*return_compared),
                first_difference: None,
                difference_kind: None,
            },
            StoredExecutionCaseProjection::Diff { name, difference } => Self {
                name: name.clone(),
                verdict: "diff".to_owned(),
                events: None,
                memory_changes: None,
                return_compared: None,
                first_difference: Some(difference.first_difference),
                difference_kind: Some(difference.kind.clone()),
            },
            StoredExecutionCaseProjection::Incomplete { name } => Self {
                name: name.clone(),
                verdict: "incomplete".to_owned(),
                events: None,
                memory_changes: None,
                return_compared: None,
                first_difference: None,
                difference_kind: None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focused_projection_keeps_bounded_scope_and_production_source() {
        let document = serde_json::from_value(serde_json::json!({
            "complete_project_run": true,
            "passed": true,
            "replacement_graph": {
                "replacements": [{
                    "vendor": {"source": "wifi", "symbol": "set_key"},
                    "reviewed": true,
                    "disposition": "bounded-feature",
                    "protocol": "wifi",
                    "rust": {
                        "binding_scope": "production-feature",
                        "production_component": "radio::KeyRole",
                        "verification_probes": ["probe_set_key"]
                    },
                    "status": "bounded-match",
                    "proofs": [{
                        "suite": "key-role",
                        "status": "bounded-match",
                        "claim": "rust-conformance"
                    }]
                }]
            },
            "rust_component_index": {
                "components": [{
                    "component_id": "radio::KeyRole",
                    "source_status": "resolved",
                    "compiled_status": "missing",
                    "freshness_status": "unknown",
                    "source_items": [{
                        "path": "src/key.rs",
                        "line": 12,
                        "kind": "enum"
                    }],
                    "compiled_symbols": []
                }]
            }
        }))
        .unwrap();
        let mut evidence =
            parse_replacement_evidence("wifi", "set_key", Path::new("verification.json"), document)
                .unwrap();
        join_proof_details(
            &mut evidence,
            serde_json::from_value(serde_json::json!({
                "id": "key-role",
                "sources": [{
                    "source": "wifi",
                    "functions": [{
                        "vendor_symbol": "set_key",
                        "adapter_cases": [{"name": "ap", "matched": true}],
                        "effects": 1,
                        "return_compared": false
                    }]
                }]
            }))
            .unwrap(),
        );

        assert_eq!(
            evidence[0].binding_scope.as_deref(),
            Some("production-feature")
        );
        assert_eq!(
            evidence[0]
                .production_component_evidence
                .as_ref()
                .unwrap()
                .source_items[0]
                .line,
            12
        );
        assert_eq!(evidence[0].proofs[0].adapter_cases[0].name, "ap");
        assert_eq!(evidence[0].proofs[0].effects, Some(1));
    }
}
