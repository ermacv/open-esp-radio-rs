//! Reproducible oracle provenance and verification evidence.

use std::{
    collections::BTreeMap,
    env,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

use serde::Serialize;
use serde_json::{Map, Value};

use sha2::{Digest, Sha256};

use crate::{Result, TargetSpec, VerificationGate, VerifySummary, dispositions, profiles};

pub(crate) type EvidenceSet = BTreeMap<(String, String), String>;

fn update_reference_codegen_sources(digest: &mut Sha256) {
    for source in [
        include_str!("../../crates/backend-riscv/src/codegen/mod.rs"),
        include_str!("../../crates/backend-riscv/src/codegen/events.rs"),
        include_str!("../../crates/backend-riscv/src/codegen/flow.rs"),
        include_str!("../../crates/backend-riscv/src/codegen/value.rs"),
    ] {
        digest.update(source.as_bytes());
    }
}

pub(crate) fn record_evidence(
    evidence: &mut EvidenceSet,
    source: &str,
    symbol: &str,
    kind: impl Into<String>,
) -> Result<()> {
    let key = (source.to_owned(), symbol.to_owned());
    let kind = kind.into();
    if let Some(previous) = evidence.insert(key, kind.clone())
        && previous != kind
    {
        return Err(
            format!("conflicting evidence for {source} {symbol}: {previous} and {kind}").into(),
        );
    }
    Ok(())
}

pub(crate) fn effect_contract_evidence(
    policy: &super::effect_contract::EffectPolicy,
    binding: &super::bindings::Binding,
    generated_reference_proof: &str,
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"open-esp-radio-effect-contract-v1\0");
    digest.update(policy.canonical().as_bytes());
    digest.update(b"\0binding\0");
    digest.update(binding.canonical().as_bytes());
    digest.update(b"\0comparator\0");
    digest.update(include_str!("../../crates/semantics/src/effect_contract.rs").as_bytes());
    digest.update(b"\0binding-verifier\0");
    digest.update(include_str!("bindings.rs").as_bytes());
    digest.update(b"\0generated-reference-proof\0");
    digest.update(generated_reference_proof.as_bytes());
    digest.update(b"\0generated-reference-verifier\0");
    digest.update(include_str!("../orchestration/generated_reference.rs").as_bytes());
    digest.update(b"\0reference-code-generator\0");
    update_reference_codegen_sources(&mut digest);
    format!(
        "effect-contract/{}/sha256:{:x}",
        policy.comparison.label(),
        digest.finalize()
    )
}

pub(crate) fn driver_adapter_effect_evidence(
    harness: &str,
    policy: &super::effect_contract::EffectPolicy,
    binding: &super::bindings::Binding,
    adapter_proof: &str,
) -> String {
    let adapter = binding
        .driver_adapter
        .as_ref()
        .expect("driver adapter evidence requires a registered adapter");
    let sources = crate::harnesses::driver_adapter_evidence_sources(harness, adapter.label())
        .expect("binding adapter must be registered by the selected harness");
    let mut digest = Sha256::new();
    digest.update(b"open-esp-radio-driver-adapter-effect-contract-v1\0");
    digest.update(policy.canonical().as_bytes());
    digest.update(b"\0binding\0");
    digest.update(binding.canonical().as_bytes());
    digest.update(b"\0adapter-proof\0");
    digest.update(adapter_proof.as_bytes());
    digest.update(b"\0effect-comparator\0");
    digest.update(include_str!("../../crates/semantics/src/effect_contract.rs").as_bytes());
    digest.update(b"\0binding-verifier\0");
    digest.update(include_str!("bindings.rs").as_bytes());
    digest.update(b"\0iq-driver-adapter\0");
    for source in sources.adapter {
        digest.update(source.name.as_bytes());
        digest.update(source.contents.as_bytes());
    }
    digest.update(b"\0execution-engine\0");
    digest.update(include_str!("../../crates/backend-riscv/src/execution/image.rs").as_bytes());
    digest.update(include_str!("../../crates/backend-riscv/src/execution/machine.rs").as_bytes());
    digest.update(include_str!("../../crates/backend-riscv/src/execution/model.rs").as_bytes());
    digest.update(b"\0reference-generator\0");
    digest.update(sources.reviewed_summary.name.as_bytes());
    digest.update(sources.reviewed_summary.contents.as_bytes());
    update_reference_codegen_sources(&mut digest);
    format!(
        "effect-contract/{}/sha256:{:x}",
        policy.comparison.label(),
        digest.finalize()
    )
}

pub(crate) fn load_evidence_baseline(path: &Path) -> Result<EvidenceSet> {
    let text = fs::read_to_string(path)?;
    let mut evidence = EvidenceSet::new();
    for (index, raw) in text.lines().enumerate() {
        let line_number = index + 1;
        let line = raw.split_once('#').map_or(raw, |(before, _)| before).trim();
        if line.is_empty() {
            continue;
        }
        let fields = line.split_whitespace().collect::<Vec<_>>();
        let ["evidence", source, symbol, kind] = fields.as_slice() else {
            return Err(format!(
                "invalid evidence baseline line {line_number}; expected: evidence SOURCE SYMBOL KIND"
            )
            .into());
        };
        record_evidence(&mut evidence, source, symbol, *kind)?;
    }
    if evidence.is_empty() {
        return Err(format!("evidence baseline {} is empty", path.display()).into());
    }
    Ok(evidence)
}

#[derive(Debug, Serialize)]
pub(crate) struct EvidenceRegression {
    pub(crate) source: String,
    pub(crate) symbol: String,
    pub(crate) expected: String,
    pub(crate) actual: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct EvidenceAddition {
    pub(crate) source: String,
    pub(crate) symbol: String,
    pub(crate) kind: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct EvidenceComparison {
    pub(crate) passed: bool,
    pub(crate) expected: usize,
    pub(crate) actual: usize,
    pub(crate) regressions: Vec<EvidenceRegression>,
    pub(crate) additions: Vec<EvidenceAddition>,
}

pub(crate) fn compare_evidence_baseline(
    expected: &EvidenceSet,
    actual: &EvidenceSet,
) -> EvidenceComparison {
    let mut regressions = Vec::new();
    for ((source, symbol), expected_kind) in expected {
        match actual.get(&(source.clone(), symbol.clone())) {
            Some(actual_kind) if actual_kind == expected_kind => {}
            Some(actual_kind) => {
                regressions.push(EvidenceRegression {
                    source: source.clone(),
                    symbol: symbol.clone(),
                    expected: expected_kind.clone(),
                    actual: Some(actual_kind.clone()),
                });
            }
            None => {
                regressions.push(EvidenceRegression {
                    source: source.clone(),
                    symbol: symbol.clone(),
                    expected: expected_kind.clone(),
                    actual: None,
                });
            }
        }
    }
    let additions = actual
        .iter()
        .filter(|((source, symbol), _)| !expected.contains_key(&(source.clone(), symbol.clone())))
        .map(|((source, symbol), kind)| EvidenceAddition {
            source: source.clone(),
            symbol: symbol.clone(),
            kind: kind.clone(),
        })
        .collect::<Vec<_>>();
    EvidenceComparison {
        passed: regressions.is_empty(),
        expected: expected.len(),
        actual: actual.len(),
        regressions,
        additions,
    }
}

pub(crate) fn print_evidence_comparison(comparison: &EvidenceComparison) {
    for regression in &comparison.regressions {
        outputln!(
            "EVIDENCE-REGRESSION\t{}\t{}\texpected={}\tactual={}",
            regression.source,
            regression.symbol,
            regression.expected,
            regression.actual.as_deref().unwrap_or("missing")
        );
    }
    for addition in &comparison.additions {
        outputln!(
            "EVIDENCE-ADDITION\t{}\t{}\t{}",
            addition.source,
            addition.symbol,
            addition.kind
        );
    }
    outputln!(
        "EVIDENCE-BASELINE\t{}\texpected={}\tactual={}",
        if comparison.passed { "PASS" } else { "FAIL" },
        comparison.expected,
        comparison.actual
    );
}

fn evidence_path_identity(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return Ok(fs::canonicalize(path)?);
    }
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        env::current_dir()?.join(path)
    };
    let parent = absolute.parent().ok_or_else(|| {
        format!(
            "evidence candidate path {} has no parent directory",
            path.display()
        )
    })?;
    let file_name = absolute.file_name().ok_or_else(|| {
        format!(
            "evidence candidate path {} has no file name",
            path.display()
        )
    })?;
    Ok(fs::canonicalize(parent)?.join(file_name))
}

pub(crate) fn write_evidence_candidate(
    path: &Path,
    protected_inputs: &[(&str, &Path)],
    evidence: &EvidenceSet,
) -> Result<()> {
    if evidence.is_empty() {
        return Err("refusing to write an empty evidence candidate".into());
    }
    let candidate_identity = evidence_path_identity(path)?;
    for (role, protected) in protected_inputs {
        if evidence_path_identity(protected)? == candidate_identity {
            return Err(format!(
                "evidence candidate must not overwrite {role} {}; choose a separate candidate path",
                protected.display()
            )
            .into());
        }
    }
    let mut output = String::new();
    for ((source, symbol), kind) in evidence {
        writeln!(output, "evidence {source} {symbol} {kind}")
            .expect("writing to String cannot fail");
    }
    fs::write(path, output)?;
    Ok(())
}

fn json_object<'a>(value: &'a Value, context: &str) -> Result<&'a Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| format!("{context} must be a JSON object").into())
}

fn json_string<'a>(object: &'a Map<String, Value>, field: &str, context: &str) -> Result<&'a str> {
    object
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{context}.{field} must be a JSON string").into())
}

pub(crate) fn load_evidence_report(path: &Path) -> Result<EvidenceSet> {
    let text = fs::read_to_string(path)?;
    let document: Value = serde_json::from_str(&text)?;
    let root = json_object(&document, "verification report")?;
    if root.get("schema_version").and_then(Value::as_u64) != Some(3) {
        return Err("verification report schema_version must be 3".into());
    }
    if json_string(root, "command", "verification report")? != "verify inventory" {
        return Err("evidence review requires a verify inventory JSON report".into());
    }
    let entries = root
        .get("evidence")
        .and_then(Value::as_array)
        .ok_or("verification report.evidence must be a JSON array")?;
    let mut evidence = EvidenceSet::new();
    for (index, value) in entries.iter().enumerate() {
        let context = format!("verification report.evidence[{index}]");
        let entry = json_object(value, &context)?;
        record_evidence(
            &mut evidence,
            json_string(entry, "source", &context)?,
            json_string(entry, "symbol", &context)?,
            json_string(entry, "kind", &context)?,
        )?;
    }
    if evidence.is_empty() {
        return Err(format!("verification report {} has no evidence", path.display()).into());
    }
    Ok(evidence)
}

pub(crate) fn print_evidence(evidence: &EvidenceSet) {
    for ((source, symbol), kind) in evidence {
        outputln!("EVIDENCE\t{source}\t{symbol}\t{kind}");
    }
}

#[derive(Serialize)]
pub(crate) struct VerificationTargetDocument {
    id: String,
    harness: String,
    architecture: &'static str,
    calling_convention: &'static str,
    endianness: &'static str,
    pointer_width: u8,
    rust_target: String,
}

#[derive(Serialize)]
#[serde(tag = "mode", rename_all = "kebab-case")]
pub(crate) enum VerificationGateDocument {
    Completion,
    Regression { match_floor: usize },
}

#[derive(Serialize)]
pub(crate) struct VerificationSummaryDocument {
    vendor_functions: usize,
    matched: usize,
    symbolic_matches: usize,
    effect_contract_matches: usize,
    scenario_matches: usize,
    state_matches: usize,
    composition_matches: usize,
    mismatched: usize,
    incomplete: usize,
    missing: usize,
    implemented_unqualified: usize,
    not_yet_ported: usize,
    orphan_rust_probes: usize,
}

#[derive(Serialize)]
pub(crate) struct QualificationBlockerDocument {
    source: String,
    symbol: String,
}

#[derive(Serialize)]
pub(crate) struct QualificationGapDocument {
    source: String,
    symbol: String,
    rust_component: String,
    blocked_by: Vec<QualificationBlockerDocument>,
}

#[derive(Serialize)]
pub(crate) struct VerificationArtifactDocument {
    role: String,
    path: String,
    sha256: String,
}

#[derive(Serialize)]
pub(crate) struct VerificationEvidenceDocument {
    source: String,
    symbol: String,
    kind: String,
}

#[derive(Serialize)]
pub(crate) struct VerificationDocument {
    schema_version: u32,
    command: &'static str,
    target: VerificationTargetDocument,
    gate: VerificationGateDocument,
    passed: bool,
    evidence_baseline_passed: bool,
    summary: VerificationSummaryDocument,
    qualification_gaps: Vec<QualificationGapDocument>,
    artifacts: Vec<VerificationArtifactDocument>,
    evidence: Vec<VerificationEvidenceDocument>,
}

pub(crate) struct VerificationDocumentInputs<'a, S> {
    pub(crate) target: &'a TargetSpec,
    pub(crate) gate: VerificationGate,
    pub(crate) summary: VerifySummary,
    pub(crate) orphan_probes: usize,
    pub(crate) evidence_baseline_passed: bool,
    pub(crate) passed: bool,
    pub(crate) evidence: &'a EvidenceSet,
    pub(crate) artifacts: &'a [(S, &'a Path)],
    pub(crate) qualification_gaps: &'a [&'a dispositions::Entry],
}

pub(crate) fn verification_document<S: AsRef<str>>(
    inputs: VerificationDocumentInputs<'_, S>,
) -> Result<VerificationDocument> {
    let VerificationDocumentInputs {
        target,
        gate,
        summary,
        orphan_probes,
        evidence_baseline_passed,
        passed,
        evidence,
        artifacts,
        qualification_gaps,
    } = inputs;
    Ok(VerificationDocument {
        schema_version: 3,
        command: "verify inventory",
        target: VerificationTargetDocument {
            id: target.id.clone(),
            harness: target.require_available_harness()?.to_owned(),
            architecture: target.architecture.label(),
            calling_convention: target.calling_convention.label(),
            endianness: target.endianness.label(),
            pointer_width: target.pointer_width,
            rust_target: target.rust_target.clone(),
        },
        gate: match gate {
            VerificationGate::Completion => VerificationGateDocument::Completion,
            VerificationGate::Regression { match_floor } => {
                VerificationGateDocument::Regression { match_floor }
            }
        },
        passed,
        evidence_baseline_passed,
        summary: VerificationSummaryDocument {
            vendor_functions: summary.vendor_functions,
            matched: summary.matched,
            symbolic_matches: summary.symbolic_matches,
            effect_contract_matches: summary.effect_contract_matches,
            scenario_matches: summary.scenario_matches,
            state_matches: summary.state_matches,
            composition_matches: summary.composition_matches,
            mismatched: summary.mismatched,
            incomplete: summary.incomplete,
            missing: summary.missing,
            implemented_unqualified: summary.implemented_unqualified,
            not_yet_ported: summary.not_yet_ported,
            orphan_rust_probes: orphan_probes,
        },
        qualification_gaps: qualification_gaps
            .iter()
            .map(|gap| QualificationGapDocument {
                source: gap.source.clone(),
                symbol: gap.symbol.clone(),
                rust_component: gap
                    .rust_component
                    .clone()
                    .unwrap_or_else(|| "missing".to_owned()),
                blocked_by: gap
                    .qualification_blockers
                    .iter()
                    .map(|(source, symbol)| QualificationBlockerDocument {
                        source: source.clone(),
                        symbol: symbol.clone(),
                    })
                    .collect(),
            })
            .collect(),
        artifacts: artifacts
            .iter()
            .map(|(role, artifact)| {
                Ok(VerificationArtifactDocument {
                    role: role.as_ref().to_owned(),
                    path: artifact.display().to_string(),
                    sha256: format!("{:x}", Sha256::digest(fs::read(artifact)?)),
                })
            })
            .collect::<Result<Vec<_>>>()?,
        evidence: evidence
            .iter()
            .map(|((source, symbol), kind)| VerificationEvidenceDocument {
                source: source.clone(),
                symbol: symbol.clone(),
                kind: kind.clone(),
            })
            .collect(),
    })
}

pub(crate) fn write_verification_json_report(
    path: &Path,
    document: &VerificationDocument,
) -> Result<()> {
    fs::write(path, serde_json::to_string_pretty(document)? + "\n")?;
    Ok(())
}

pub(crate) fn profile_evidence(profile: &profiles::Profile) -> String {
    // `Profile` is composed only of ordered vectors and ordered maps. Its
    // parsed Debug form binds every scenario input, domain, observation and
    // response without making comments or whitespace part of the identity.
    // The verifier sources are equally part of the proof: a weaker executor
    // or reachability pass must invalidate a previously accepted baseline.
    let canonical = format!("{profile:#?}");
    let mut digest = Sha256::new();
    digest.update(b"open-esp-radio-execution-profile-v2\0");
    digest.update(canonical.as_bytes());
    digest.update(b"\0profile-parser\0");
    digest.update(include_str!("profiles.rs").as_bytes());
    digest.update(b"\0comparison-orchestrator\0");
    digest.update(include_str!("execution.rs").as_bytes());
    digest.update(b"\0execution-image\0");
    digest.update(include_str!("../../crates/backend-riscv/src/execution/image.rs").as_bytes());
    digest.update(b"\0execution-machine\0");
    digest.update(include_str!("../../crates/backend-riscv/src/execution/machine.rs").as_bytes());
    digest.update(b"\0execution-model\0");
    digest.update(include_str!("../../crates/backend-riscv/src/execution/model.rs").as_bytes());
    format!(
        "{}/profile:{}/sha256:{:x}",
        profile.contract.evidence(),
        profile.name,
        digest.finalize()
    )
}

pub(crate) fn semantic_contract_digest_from_sources(
    label: &str,
    sources: &[(&str, &str)],
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"open-esp-radio semantic contract\0");
    digest.update(label.as_bytes());
    for (name, source) in sources {
        digest.update([0]);
        digest.update(name.as_bytes());
        digest.update([0]);
        digest.update(source.as_bytes());
    }
    format!("{:x}", digest.finalize())
}

pub(crate) fn semantic_contract_evidence(harness_id: &str, label: &str) -> String {
    // Bind composition evidence to every implementation unit that can change
    // the corresponding semantic verdict. Facade modules alone are not
    // sufficient: moving the implementation into submodules must not silently
    // stop evidence hashes from tracking verifier behavior.
    let harness = crate::harnesses::semantic_contract_evidence_sources(harness_id, label)
        .expect("semantic contract must be registered by the selected harness");
    let mut sources = harness
        .common
        .iter()
        .map(|source| (source.name, source.contents))
        .collect::<Vec<_>>();
    sources.extend([
        ("verification/execution.rs", include_str!("execution.rs")),
        (
            "execution/image.rs",
            include_str!("../../crates/backend-riscv/src/execution/image.rs"),
        ),
        (
            "execution/model.rs",
            include_str!("../../crates/backend-riscv/src/execution/model.rs"),
        ),
        (
            "execution/machine.rs",
            include_str!("../../crates/backend-riscv/src/execution/machine.rs"),
        ),
    ]);
    sources.push((harness.contract.name, harness.contract.contents));
    let digest = semantic_contract_digest_from_sources(label, &sources);
    format!("composition-state-scenario/{label}/sha256:{digest}")
}
