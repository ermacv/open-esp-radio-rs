//! Effect Contract parsing and comparison tests.

use super::*;
use crate::{EquivalenceMode, EquivalenceOutcome, EquivalenceVerdict};

fn register() -> RegisterId {
    RegisterId {
        address: 0x2010_7030,
        width: 32,
        name: "PHY_AGC_ORACLE.AGC_ANTENNA_CONTROL".to_owned(),
    }
}

fn read() -> ContractEffect {
    ContractEffect::MmioRead {
        register: register(),
        value: ContractValue::ReadResult { ordinal: 0 },
    }
}

#[test]
fn exact_policy_rejects_an_unclassified_vendor_effect() {
    let policy = EffectPolicy::new(
        EffectComparison::ExactEffectsV1,
        [(EffectSelector::Delay, EffectDisposition::Required)],
    )
    .unwrap();
    let outcome = compare_effects(&[read()], &[read()], &policy).unwrap();
    assert_eq!(outcome.mode, EquivalenceMode::Semantic);
    assert_eq!(outcome.verdict, EquivalenceVerdict::Incomplete);
    assert!(
        outcome
            .reason
            .unwrap()
            .contains("unclassified vendor effect")
    );
}

#[test]
fn exact_policy_rejects_an_extra_rust_effect() {
    let selector = read().selector();
    let policy = EffectPolicy::new(
        EffectComparison::ExactEffectsV1,
        [(selector, EffectDisposition::Required)],
    )
    .unwrap();
    let outcome = compare_effects(&[read()], &[read(), read()], &policy).unwrap();
    assert_eq!(outcome.verdict, EquivalenceVerdict::Incomplete);
    assert!(
        outcome
            .reason
            .unwrap()
            .contains("unclassified extra Rust effect")
    );
}

#[test]
fn blocking_effect_requires_an_explicit_await_ready_replacement() {
    let policy = EffectPolicy::new(
        EffectComparison::ExactEffectsV1,
        [(
            EffectSelector::Delay,
            EffectDisposition::ReplacedByAsync {
                condition: "iq-estimator-ready".to_owned(),
                timeout: Timeout::Attempts(100),
            },
        )],
    )
    .unwrap();
    let vendor = [ContractEffect::Delay {
        micros: ContractValue::Concrete(1),
    }];
    assert_eq!(
        compare_effects(&vendor, &[], &policy).unwrap().verdict,
        EquivalenceVerdict::Diff
    );
    let rust = [ContractEffect::AwaitReady {
        condition: ReadyCondition::Named("iq-estimator-ready".to_owned()),
        timeout: Timeout::Attempts(100),
    }];
    assert_eq!(
        compare_effects(&vendor, &rust, &policy).unwrap(),
        EquivalenceOutcome::matched(EquivalenceMode::Semantic)
    );
}

#[test]
fn omission_reason_and_platform_operation_vocabularies_are_closed() {
    assert!(OmissionReason::parse("debug-diagnostic", 1).is_ok());
    assert!(OmissionReason::parse("whatever", 1).is_err());
    assert!(PlatformOperation::parse("nvs-calibration-cache", 1).is_ok());
    assert!(PlatformOperation::parse("vendor-magic", 1).is_err());
}

#[test]
fn effect_rule_parser_is_closed_and_restricts_omissions() {
    assert_eq!(
        parse_effect_rule(
            "platform-call debug-diagnostic allowed-omission debug-diagnostic",
            7,
        )
        .unwrap(),
        (
            EffectSelector::PlatformCall {
                operation: PlatformOperation::DebugDiagnostic,
            },
            EffectDisposition::AllowedOmission(OmissionReason::DebugDiagnostic),
        )
    );
    assert!(parse_effect_rule("vendor-effect magic required", 8).is_err());
    assert!(
        parse_effect_rule(
            "mmio-write 32 0x20107030 allowed-omission debug-diagnostic",
            9,
        )
        .is_err()
    );
}

#[test]
fn semantic_boundary_dispositions_require_exact_typed_replacements() {
    let state_field = |field: &str| StateField {
        projection: "sta".to_owned(),
        field: field.to_owned(),
        width: 32,
    };
    let vendor = [
        read(),
        ContractEffect::PlatformCall {
            operation: PlatformOperation::RtosSchedulingAdapter,
            arguments: Vec::new(),
        },
        ContractEffect::StateWrite {
            field: state_field("event"),
            value: ContractValue::Concrete(1),
        },
        ContractEffect::StateWrite {
            field: state_field("initialized"),
            value: ContractValue::Concrete(1),
        },
    ];
    let policy = EffectPolicy::new(
        EffectComparison::ExactEffectsV1,
        [
            (
                vendor[0].selector(),
                EffectDisposition::PlatformProvidedInput {
                    input: "station-mac".to_owned(),
                },
            ),
            (
                vendor[1].selector(),
                EffectDisposition::PlatformProvidedService {
                    service: "embassy-wakeup".to_owned(),
                },
            ),
            (
                vendor[2].selector(),
                EffectDisposition::PublishedEvent {
                    event: "rx-ready".to_owned(),
                },
            ),
            (
                vendor[3].selector(),
                EffectDisposition::InitializationPrerequisite {
                    prerequisite: "mac-clock-enabled".to_owned(),
                },
            ),
        ],
    )
    .unwrap();
    let rust = [
        ContractEffect::PlatformProvidedInput {
            input: "station-mac".to_owned(),
        },
        ContractEffect::PlatformProvidedService {
            service: "embassy-wakeup".to_owned(),
        },
        ContractEffect::PublishedEvent {
            event: "rx-ready".to_owned(),
        },
        ContractEffect::InitializationPrerequisite {
            prerequisite: "mac-clock-enabled".to_owned(),
        },
    ];
    assert_eq!(
        compare_effects(&vendor, &rust, &policy).unwrap(),
        EquivalenceOutcome::matched(EquivalenceMode::Semantic)
    );
    let outcome = compare_effects(&vendor, &rust[..3], &policy).unwrap();
    assert_eq!(outcome.verdict, EquivalenceVerdict::Diff);
    assert!(
        outcome
            .reason
            .is_some_and(|reason| reason.contains("initialization-prerequisite"))
    );
}

#[test]
fn boundary_effect_selectors_are_valid_contract_rules() {
    for rule in [
        "platform-provided-input station-mac required",
        "platform-provided-service embassy-wakeup required",
        "published-event rx-success required",
        "initialization-prerequisite power-irqs-disabled required",
    ] {
        assert!(parse_effect_rule(rule, 17).is_ok(), "{rule}");
    }
}

#[test]
fn semantic_boundary_rule_syntax_is_closed_and_canonical() {
    for rule in [
        "mmio-read 32 0x20107030 platform-provided-input station-mac",
        "platform-call rtos-scheduling-adapter platform-provided-service embassy-wakeup",
        "state-write 32 sta.event published-event rx-ready",
        "mmio-write 32 0x20107030 initialization-prerequisite mac-clock-enabled",
    ] {
        let (selector, disposition) = parse_effect_rule(rule, 11).unwrap();
        assert_eq!(
            format!("{} {}", selector.canonical(), disposition.canonical()),
            rule
        );
    }
    assert!(
        parse_effect_rule(
            "mmio-write 32 0x20107030 platform-provided-input station-mac",
            12,
        )
        .is_err()
    );
    assert!(parse_effect_rule("platform-call random published-event Invalid/Event", 13,).is_err());
}
