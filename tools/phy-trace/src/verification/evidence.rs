//! Reproducible oracle provenance and verification evidence.

use std::{collections::BTreeMap, fmt::Write as _, fs, path::Path};

use sha2::{Digest, Sha256};

use crate::{Result, VerificationGate, VerifySummary, dispositions, profiles};

pub(crate) type EvidenceSet = BTreeMap<(String, String), String>;

pub(crate) const ESP32S31_LIBPHY_SHA256: &str =
    "51497819736295c9b33d6775495dade4c6fb39db887edfe095608c670d9ae223";
pub(crate) const ESP32S31_LIBPP_SHA256: &str =
    "f863c65c3ed89cf5d2a2cbe0d6bca3b783ca35788a704bb68e13958e4b94958e";
pub(crate) const ESP32S31_REV0_ROM_LOCAL_SHA256: &str =
    "d01bde81d9b3806e37ef1d9ac3b58af4f5b3d91eeef4f44d20e79d6a9f227542";
pub(crate) const ESP32S31_REV0_ROM_CANONICAL_SHA256: &str =
    "a52ad7513deb656a910a5740125f1cce2c7941f11ce57213b7b43aea93d5ab87";
pub(crate) const ESP32S31_LINKED_LIBPHY_SHA256: &str =
    "a38df8f225107786bbb77c03cdc2ec62d8aa68178d8412279745073c4a991524";

pub(crate) fn is_pinned_vendor_digest(digest: &str) -> bool {
    matches!(
        digest,
        ESP32S31_LIBPHY_SHA256
            | ESP32S31_LIBPP_SHA256
            | ESP32S31_REV0_ROM_LOCAL_SHA256
            | ESP32S31_REV0_ROM_CANONICAL_SHA256
            | ESP32S31_LINKED_LIBPHY_SHA256
    )
}

pub(crate) fn artifact_sha256(path: &Path) -> Result<String> {
    let bytes = fs::read(path)?;
    Ok(format!("{:x}", Sha256::digest(&bytes)))
}

pub(crate) fn pinned_vendor_digest(path: &Path) -> Result<String> {
    let digest = artifact_sha256(path)?;
    if !is_pinned_vendor_digest(&digest) {
        return Err(
            format!("vendor artifact is not a pinned ESP32-S31 oracle: sha256 {digest}").into(),
        );
    }
    Ok(digest)
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
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"open-esp-radio-effect-contract-v1\0");
    digest.update(policy.canonical().as_bytes());
    digest.update(b"\0binding\0");
    digest.update(binding.canonical().as_bytes());
    digest.update(b"\0comparator\0");
    digest.update(include_str!("effect_contract.rs").as_bytes());
    digest.update(b"\0binding-validator\0");
    digest.update(include_str!("bindings.rs").as_bytes());
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
pub(crate) fn write_verification_json_report(
    path: &Path,
    gate: VerificationGate,
    summary: VerifySummary,
    orphan_probes: usize,
    evidence_baseline_passed: bool,
    passed: bool,
    evidence: &EvidenceSet,
    artifacts: &[(&str, &Path)],
    qualification_gaps: &[&dispositions::Entry],
) -> Result<()> {
    let mut output = String::new();
    output.push_str("{\n  \"schema_version\": 1,\n  \"command\": \"verify-all\",\n");
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
        write_json_string(&mut output, role);
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
    // `Profile` is composed only of ordered vectors and ordered maps. Hashing
    // its parsed canonical Debug form binds evidence to every scenario input,
    // observation and response without making comments or whitespace part of
    // the contract identity. The repository pins the Rust toolchain that
    // defines this representation.
    let canonical = format!("{profile:#?}");
    format!(
        "{}/profile:{}/sha256:{:x}",
        profile.contract.evidence(),
        profile.name,
        Sha256::digest(canonical.as_bytes())
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

pub(crate) fn semantic_contract_evidence(label: &str) -> String {
    // Bind composition evidence to every implementation unit that can change
    // the corresponding semantic verdict. Facade modules alone are not
    // sufficient: moving the implementation into submodules must not silently
    // stop evidence hashes from tracking validator behavior.
    let mut sources = vec![
        (
            "qualification/mod.rs",
            include_str!("../qualification/mod.rs"),
        ),
        (
            "qualification/state.rs",
            include_str!("../qualification/state.rs"),
        ),
        (
            "qualification/runner.rs",
            include_str!("../qualification/runner.rs"),
        ),
        ("verification/execution.rs", include_str!("execution.rs")),
        ("execution/image.rs", include_str!("../execution/image.rs")),
        ("execution/model.rs", include_str!("../execution/model.rs")),
        (
            "execution/machine.rs",
            include_str!("../execution/machine.rs"),
        ),
    ];
    sources.push(match label {
        "esp32s31-channel" => (
            "qualification/channel.rs",
            include_str!("../qualification/channel.rs"),
        ),
        "esp32s31-rf-init" => (
            "qualification/rf_init.rs",
            include_str!("../qualification/rf_init.rs"),
        ),
        "esp32s31-bluetooth-txdc" => (
            "qualification/bluetooth_txdc.rs",
            include_str!("../qualification/bluetooth_txdc.rs"),
        ),
        "esp32s31-bluetooth-txdc-pwdet" => (
            "qualification/bluetooth_txdc_pwdet.rs",
            include_str!("../qualification/bluetooth_txdc_pwdet.rs"),
        ),
        "esp32s31-bluetooth-tx-power" => (
            "qualification/bluetooth_tx_power.rs",
            include_str!("../qualification/bluetooth_tx_power.rs"),
        ),
        _ => ("qualification/unknown", "unknown semantic contract"),
    });
    let digest = semantic_contract_digest_from_sources(label, &sources);
    format!("composition-state-scenario/{label}/sha256:{digest}")
}
