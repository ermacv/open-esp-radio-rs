//! Reproducible oracle provenance and verification evidence.

use std::collections::BTreeMap;

use crate::{Result, profiles};

mod baseline;
mod identity;
mod report;

pub(crate) use baseline::*;
pub(crate) use identity::*;
pub(crate) use report::*;

pub(crate) type EvidenceSet = BTreeMap<(String, String), EvidenceIdentity>;

fn reference_codegen_component() -> (String, String) {
    combined_component(
        "reference-code-generator",
        [
            (
                "codegen/mod.rs",
                include_str!("../../crates/backend-riscv/src/codegen/mod.rs"),
            ),
            (
                "codegen/events.rs",
                include_str!("../../crates/backend-riscv/src/codegen/events.rs"),
            ),
            (
                "codegen/flow.rs",
                include_str!("../../crates/backend-riscv/src/codegen/flow.rs"),
            ),
            (
                "codegen/value.rs",
                include_str!("../../crates/backend-riscv/src/codegen/value.rs"),
            ),
        ],
    )
}

const EXECUTION_IMAGE_SOURCES: [(&str, &str); 5] = [
    (
        "execution/image.rs",
        include_str!("../../crates/backend-riscv/src/execution/image.rs"),
    ),
    (
        "execution/image/access.rs",
        include_str!("../../crates/backend-riscv/src/execution/image/access.rs"),
    ),
    (
        "execution/image/closure_identity.rs",
        include_str!("../../crates/backend-riscv/src/execution/image/closure_identity.rs"),
    ),
    (
        "execution/image/coverage.rs",
        include_str!("../../crates/backend-riscv/src/execution/image/coverage.rs"),
    ),
    (
        "execution/image/loader.rs",
        include_str!("../../crates/backend-riscv/src/execution/image/loader.rs"),
    ),
];

const EXECUTION_MACHINE_SOURCES: [(&str, &str); 5] = [
    (
        "execution/mod.rs",
        include_str!("../../crates/backend-riscv/src/execution/mod.rs"),
    ),
    (
        "execution/machine.rs",
        include_str!("../../crates/backend-riscv/src/execution/machine.rs"),
    ),
    (
        "execution/machine/events.rs",
        include_str!("../../crates/backend-riscv/src/execution/machine/events.rs"),
    ),
    (
        "execution/machine/memory.rs",
        include_str!("../../crates/backend-riscv/src/execution/machine/memory.rs"),
    ),
    (
        "execution/machine/step.rs",
        include_str!("../../crates/backend-riscv/src/execution/machine/step.rs"),
    ),
];

const EXECUTION_MODEL_SOURCE: (&str, &str) = (
    "execution/model.rs",
    include_str!("../../crates/backend-riscv/src/execution/model.rs"),
);

fn execution_image_component() -> (String, String) {
    combined_component("execution-image", EXECUTION_IMAGE_SOURCES)
}

fn execution_machine_component() -> (String, String) {
    combined_component("execution-machine", EXECUTION_MACHINE_SOURCES)
}

fn execution_engine_component() -> (String, String) {
    combined_component(
        "execution-engine",
        EXECUTION_IMAGE_SOURCES
            .into_iter()
            .chain(EXECUTION_MACHINE_SOURCES)
            .chain([EXECUTION_MODEL_SOURCE]),
    )
}

pub(crate) fn record_evidence(
    evidence: &mut EvidenceSet,
    source: &str,
    symbol: &str,
    identity: EvidenceIdentity,
) -> Result<()> {
    let key = (source.to_owned(), symbol.to_owned());
    identity.validate()?;
    if let Some(previous) = evidence.insert(key, identity.clone())
        && previous != identity
    {
        return Err(crate::Error::invalid(format!(
            "conflicting evidence for {source} {symbol}: {previous} and {identity}"
        )));
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn effect_contract_evidence(
    policy: &super::effect_contract::EffectPolicy,
    binding: &super::bindings::Binding,
    generated_reference_proof: &str,
) -> EvidenceIdentity {
    EvidenceIdentity::composed(
        format!("effect-contract/{}", policy.comparison.label()),
        "open-esp-radio-effect-contract-v2",
        [
            component("policy", policy.canonical()),
            component("binding", binding.canonical()),
            component(
                "effect-comparator",
                include_str!("../../crates/semantics/src/effect_contract.rs"),
            ),
            component("binding-verifier", include_str!("bindings.rs")),
            component("generated-reference-proof", generated_reference_proof),
            component(
                "generated-reference-verifier",
                include_str!("../orchestration/generated_reference.rs"),
            ),
            reference_codegen_component(),
        ],
    )
    .expect("static effect-contract evidence components are valid")
}

pub(crate) fn driver_adapter_effect_evidence(
    harness: &str,
    policy: &super::effect_contract::EffectPolicy,
    binding: &super::bindings::Binding,
    adapter_proof: &str,
) -> EvidenceIdentity {
    let adapter = binding
        .driver_adapter
        .as_ref()
        .expect("driver adapter evidence requires a registered adapter");
    let sources = crate::harnesses::driver_adapter_evidence_sources(harness, adapter.label())
        .expect("binding adapter must be registered by the selected harness");
    EvidenceIdentity::composed(
        format!("effect-contract/{}", policy.comparison.label()),
        "open-esp-radio-driver-adapter-effect-contract-v2",
        [
            component("policy", policy.canonical()),
            component("binding", binding.canonical()),
            component("adapter-proof", adapter_proof),
            component(
                "effect-comparator",
                include_str!("../../crates/semantics/src/effect_contract.rs"),
            ),
            component("binding-verifier", include_str!("bindings.rs")),
            combined_component(
                "driver-adapter",
                sources
                    .adapter
                    .iter()
                    .map(|source| (source.name, source.contents)),
            ),
            execution_engine_component(),
            component(
                "reviewed-summary",
                format!(
                    "{}\0{}",
                    sources.reviewed_summary.name, sources.reviewed_summary.contents
                ),
            ),
            reference_codegen_component(),
        ],
    )
    .expect("static driver-adapter evidence components are valid")
}

pub(crate) fn driver_adapter_limited_claim_evidence(
    harness: &str,
    policy: &super::effect_contract::EffectPolicy,
    binding: &super::bindings::Binding,
    claim: open_radio_vendor_semantics::DriverAdapterClaim,
    adapter_proof: &str,
) -> EvidenceIdentity {
    debug_assert_ne!(
        claim,
        open_radio_vendor_semantics::DriverAdapterClaim::WholeFunctionEquivalence
    );
    let adapter = binding
        .driver_adapter
        .as_ref()
        .expect("driver adapter evidence requires a registered adapter");
    let sources = crate::harnesses::driver_adapter_evidence_sources(harness, adapter.label())
        .expect("binding adapter must be registered by the selected harness");
    EvidenceIdentity::composed(
        format!(
            "driver-adapter/{}/{}",
            claim.label(),
            policy.comparison.label()
        ),
        "open-esp-radio-driver-adapter-limited-claim-v1",
        [
            component("claim", claim.label()),
            component("policy", policy.canonical()),
            component("binding", binding.canonical()),
            component("adapter-proof", adapter_proof),
            component(
                "effect-comparator",
                include_str!("../../crates/semantics/src/effect_contract.rs"),
            ),
            component("binding-verifier", include_str!("bindings.rs")),
            combined_component(
                "driver-adapter",
                sources
                    .adapter
                    .iter()
                    .map(|source| (source.name, source.contents)),
            ),
            execution_engine_component(),
            component(
                "reviewed-summary",
                format!(
                    "{}\0{}",
                    sources.reviewed_summary.name, sources.reviewed_summary.contents
                ),
            ),
            reference_codegen_component(),
        ],
    )
    .expect("static limited driver-adapter evidence components are valid")
}

pub(crate) fn profile_evidence(profile: &profiles::Profile) -> EvidenceIdentity {
    let canonical = format!("{profile:#?}");
    EvidenceIdentity::composed(
        format!(
            "{}/profile:{}/{}",
            profile.claim.label(),
            profile.contract.evidence(),
            profile.name
        ),
        "open-esp-radio-execution-profile-v4",
        [
            component("profile", canonical),
            component("profile-parser", include_str!("profiles.rs")),
            combined_component(
                "comparison-orchestrator",
                [
                    ("verification/execution.rs", include_str!("execution.rs")),
                    (
                        "verification/execution/scenario.rs",
                        include_str!("execution/scenario.rs"),
                    ),
                ],
            ),
            execution_image_component(),
            execution_machine_component(),
            component(
                "execution-model",
                include_str!("../../crates/backend-riscv/src/execution/model.rs"),
            ),
        ],
    )
    .expect("static execution-profile evidence components are valid")
}

pub(crate) fn semantic_contract_evidence(harness_id: &str, label: &str) -> EvidenceIdentity {
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
    let components = sources
        .into_iter()
        .map(|(name, contents)| component(name, contents))
        .collect::<Vec<_>>();
    EvidenceIdentity::composed(
        format!("composition-state-scenario/{label}"),
        "open-esp-radio-semantic-contract-v2",
        components,
    )
    .expect("registered semantic-contract evidence components are valid")
}
