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

#[cfg(test)]
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

fn execution_image_component() -> (String, String) {
    combined_component("execution-image", EXECUTION_IMAGE_SOURCES)
}

fn execution_machine_component() -> (String, String) {
    combined_component("execution-machine", EXECUTION_MACHINE_SOURCES)
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

pub(crate) fn profile_evidence(
    profile: &profiles::Profile,
    diagnostic_contracts: &crate::DiagnosticContractsReport,
) -> EvidenceIdentity {
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
            component("diagnostic-contracts", diagnostic_contracts.canonical()),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_profile() -> profiles::Profile {
        profiles::Profile {
            name: "diagnostic-provenance".to_owned(),
            vendor_source: "vendor".to_owned(),
            vendor_symbol: "vendor_entry".to_owned(),
            rust_symbol: "rust_entry".to_owned(),
            claim: open_radio_vendor_semantics::VerificationClaim::WholeFunctionEquivalence,
            precondition: None,
            contract: profiles::ProfileContract::Scenario,
            compare_return: false,
            case_execution: profiles::CaseExecution::Independent,
            transaction_comparison: profiles::TransactionComparison::Observables,
            call_equivalences: Vec::new(),
            argument_ranges: Vec::new(),
            argument_values: Vec::new(),
            mmio_domains: Vec::new(),
            mmio_images: Vec::new(),
            vendor_setup: Vec::new(),
            scenarios: vec![crate::NamedScenario::new("default".to_owned())],
        }
    }

    #[test]
    fn profile_fingerprint_tracks_canonical_provider_diagnostic_contracts() {
        let profile = fixture_profile();
        let first = crate::DiagnosticContractsReport::from_calls(
            Some("fixture-provider@7".to_owned()),
            [("z_log", 2), ("a_assert", 4)],
        )
        .unwrap();
        let reordered = crate::DiagnosticContractsReport::from_calls(
            Some("fixture-provider@7".to_owned()),
            [("a_assert", 4), ("z_log", 2)],
        )
        .unwrap();
        let revised_contract = crate::DiagnosticContractsReport::from_calls(
            Some("fixture-provider@7".to_owned()),
            [("a_assert", 3), ("z_log", 2)],
        )
        .unwrap();
        let revised_provider = crate::DiagnosticContractsReport::from_calls(
            Some("fixture-provider@8".to_owned()),
            [("a_assert", 4), ("z_log", 2)],
        )
        .unwrap();

        assert_eq!(first.calls[0].symbol, "a_assert");
        assert_eq!(
            profile_evidence(&profile, &first),
            profile_evidence(&profile, &reordered)
        );
        assert_ne!(
            profile_evidence(&profile, &first).digest,
            profile_evidence(&profile, &revised_contract).digest
        );
        assert_ne!(
            profile_evidence(&profile, &first).digest,
            profile_evidence(&profile, &revised_provider).digest
        );
    }
}
