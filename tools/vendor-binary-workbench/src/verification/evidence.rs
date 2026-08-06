//! Reproducible oracle provenance and verification evidence.

use std::{
    collections::BTreeMap,
    env,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

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

pub(crate) fn check_evidence_baseline(expected: &EvidenceSet, actual: &EvidenceSet) -> bool {
    let mut passed = true;
    for ((source, symbol), expected_kind) in expected {
        match actual.get(&(source.clone(), symbol.clone())) {
            Some(actual_kind) if actual_kind == expected_kind => {}
            Some(actual_kind) => {
                passed = false;
                println!(
                    "EVIDENCE-REGRESSION\t{source}\t{symbol}\texpected={expected_kind}\tactual={actual_kind}"
                );
            }
            None => {
                passed = false;
                println!(
                    "EVIDENCE-REGRESSION\t{source}\t{symbol}\texpected={expected_kind}\tactual=missing"
                );
            }
        }
    }
    for ((source, symbol), kind) in actual {
        if !expected.contains_key(&(source.clone(), symbol.clone())) {
            println!("EVIDENCE-ADDITION\t{source}\t{symbol}\t{kind}");
        }
    }
    println!(
        "EVIDENCE-BASELINE\t{}\texpected={}\tactual={}",
        if passed { "PASS" } else { "FAIL" },
        expected.len(),
        actual.len()
    );
    passed
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
    println!(
        "EVIDENCE-CANDIDATE\t{}\tentries={}",
        path.display(),
        evidence.len()
    );
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
        println!("EVIDENCE\t{source}\t{symbol}\t{kind}");
    }
}

pub(crate) fn write_json_string(output: &mut String, value: &str) {
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character.is_control() => {
                write!(output, "\\u{:04x}", character as u32)
                    .expect("writing to String cannot fail");
            }
            character => output.push(character),
        }
    }
    output.push('"');
}

#[allow(
    clippy::too_many_arguments,
    reason = "the report boundary deliberately receives a complete immutable proof record"
)]
pub(crate) fn write_verification_json_report<S: AsRef<str>>(
    path: &Path,
    target: &TargetSpec,
    gate: VerificationGate,
    summary: VerifySummary,
    orphan_probes: usize,
    evidence_baseline_passed: bool,
    passed: bool,
    evidence: &EvidenceSet,
    artifacts: &[(S, &Path)],
    qualification_gaps: &[&dispositions::Entry],
) -> Result<()> {
    let mut output = String::new();
    output.push_str("{\n  \"schema_version\": 3,\n  \"command\": \"verify inventory\",\n");
    output.push_str("  \"target\": {\"id\": ");
    write_json_string(&mut output, &target.id);
    output.push_str(", \"harness\": ");
    write_json_string(&mut output, target.require_available_harness()?);
    output.push_str(", \"architecture\": ");
    write_json_string(&mut output, target.architecture.label());
    output.push_str(", \"calling_convention\": ");
    write_json_string(&mut output, target.calling_convention.label());
    output.push_str(", \"endianness\": ");
    write_json_string(&mut output, target.endianness.label());
    write!(
        output,
        ", \"pointer_width\": {}, \"rust_target\": ",
        target.pointer_width
    )
    .expect("writing to String cannot fail");
    write_json_string(&mut output, &target.rust_target);
    output.push_str("},\n");
    output.push_str("  \"gate\": ");
    write_json_string(
        &mut output,
        match gate {
            VerificationGate::Completion => "completion",
            VerificationGate::Regression { .. } => "regression",
        },
    );
    writeln!(output, ",\n  \"passed\": {passed},").expect("writing to String cannot fail");
    writeln!(
        output,
        "  \"evidence_baseline_passed\": {evidence_baseline_passed},"
    )
    .expect("writing to String cannot fail");
    output.push_str("  \"summary\": {\n");
    for (name, value, trailing) in [
        ("vendor_functions", summary.vendor_functions, true),
        ("matched", summary.matched, true),
        ("symbolic_matches", summary.symbolic_matches, true),
        (
            "effect_contract_matches",
            summary.effect_contract_matches,
            true,
        ),
        ("scenario_matches", summary.scenario_matches, true),
        ("state_matches", summary.state_matches, true),
        ("composition_matches", summary.composition_matches, true),
        ("mismatched", summary.mismatched, true),
        ("incomplete", summary.incomplete, true),
        ("missing", summary.missing, true),
        (
            "implemented_unqualified",
            summary.implemented_unqualified,
            true,
        ),
        ("not_yet_ported", summary.not_yet_ported, true),
        ("orphan_rust_probes", orphan_probes, false),
    ] {
        writeln!(
            output,
            "    \"{name}\": {value}{}",
            if trailing { "," } else { "" }
        )
        .expect("writing to String cannot fail");
    }
    output.push_str("  },\n  \"qualification_gaps\": [\n");
    for (index, gap) in qualification_gaps.iter().enumerate() {
        output.push_str("    {\"source\": ");
        write_json_string(&mut output, &gap.source);
        output.push_str(", \"symbol\": ");
        write_json_string(&mut output, &gap.symbol);
        output.push_str(", \"rust_component\": ");
        write_json_string(
            &mut output,
            gap.rust_component.as_deref().unwrap_or("missing"),
        );
        output.push_str(", \"blocked_by\": [");
        for (blocker_index, (source, symbol)) in gap.qualification_blockers.iter().enumerate() {
            if blocker_index != 0 {
                output.push_str(", ");
            }
            output.push_str("{\"source\": ");
            write_json_string(&mut output, source);
            output.push_str(", \"symbol\": ");
            write_json_string(&mut output, symbol);
            output.push('}');
        }
        output.push_str("]}");
        output.push_str(if index + 1 == qualification_gaps.len() {
            "\n"
        } else {
            ",\n"
        });
    }
    output.push_str("  ],\n  \"artifacts\": [\n");
    for (index, (role, artifact)) in artifacts.iter().enumerate() {
        let bytes = fs::read(artifact)?;
        let digest = format!("{:x}", Sha256::digest(&bytes));
        output.push_str("    {\"role\": ");
        write_json_string(&mut output, role.as_ref());
        output.push_str(", \"path\": ");
        write_json_string(&mut output, &artifact.display().to_string());
        output.push_str(", \"sha256\": ");
        write_json_string(&mut output, &digest);
        output.push('}');
        output.push_str(if index + 1 == artifacts.len() {
            "\n"
        } else {
            ",\n"
        });
    }
    output.push_str("  ],\n  \"evidence\": [\n");
    for (index, ((source, symbol), kind)) in evidence.iter().enumerate() {
        output.push_str("    {\"source\": ");
        write_json_string(&mut output, source);
        output.push_str(", \"symbol\": ");
        write_json_string(&mut output, symbol);
        output.push_str(", \"kind\": ");
        write_json_string(&mut output, kind);
        output.push('}');
        output.push_str(if index + 1 == evidence.len() {
            "\n"
        } else {
            ",\n"
        });
    }
    output.push_str("  ]\n}\n");
    fs::write(path, output)?;
    println!("JSON-REPORT\t{}", path.display());
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
