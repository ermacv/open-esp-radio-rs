//! Reproducible oracle provenance and verification evidence.

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

use crate::{Result, profiles};

mod baseline;
mod report;

pub(crate) use baseline::*;
pub(crate) use report::*;

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

pub(crate) fn profile_evidence(profile: &profiles::Profile) -> String {
    let canonical = format!("{profile:#?}");
    let mut digest = Sha256::new();
    digest.update(b"open-esp-radio-execution-profile-v2\0");
    digest.update(canonical.as_bytes());
    digest.update(b"\0profile-parser\0");
    digest.update(include_str!("profiles.rs").as_bytes());
    digest.update(b"\0comparison-orchestrator\0");
    digest.update(include_str!("execution.rs").as_bytes());
    digest.update(include_str!("execution/scenario.rs").as_bytes());
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
            "verification/execution/scenario.rs",
            include_str!("execution/scenario.rs"),
        ),
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
