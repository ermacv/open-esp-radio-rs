//! Effect Contract validation and comparison tests.

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
        EffectComparison::ExactEffectsV2,
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
        EffectComparison::ExactEffectsV2,
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
fn required_when_observed_accepts_an_inactive_vendor_branch() {
    let selector = read().selector();
    let policy = EffectPolicy::new(
        EffectComparison::ExactEffectsV2,
        [(selector, EffectDisposition::RequiredWhenObserved)],
    )
    .unwrap();

    assert_eq!(
        compare_effects(&[], &[], &policy).unwrap(),
        EquivalenceOutcome::matched(EquivalenceMode::Semantic)
    );
    assert_eq!(
        compare_effects(&[read()], &[read()], &policy).unwrap(),
        EquivalenceOutcome::matched(EquivalenceMode::Semantic)
    );
    assert_eq!(
        compare_effects(&[], &[read()], &policy).unwrap().verdict,
        EquivalenceVerdict::Incomplete
    );
}

#[test]
fn v2_requires_an_exactly_declared_rust_device_ordering_fence() {
    let selector = read().selector();
    let fence = ContractEffect::Fence {
        fm: 0,
        predecessor: 15,
        successor: 15,
    };
    let policy = EffectPolicy::new(
        EffectComparison::ExactEffectsV2,
        [
            (selector, EffectDisposition::Required),
            (
                fence.selector(),
                EffectDisposition::RustAddition(RustAdditionReason::DeviceOrdering),
            ),
        ],
    )
    .unwrap();
    assert_eq!(
        compare_effects(&[read()], &[read(), fence.clone()], &policy).unwrap(),
        EquivalenceOutcome::matched(EquivalenceMode::Semantic)
    );
    assert_eq!(
        compare_effects(&[read()], &[fence.clone(), read()], &policy).unwrap(),
        EquivalenceOutcome::matched(EquivalenceMode::Semantic)
    );
    assert_eq!(
        compare_effects(&[read(), read()], &[read(), fence.clone(), read()], &policy,).unwrap(),
        EquivalenceOutcome::matched(EquivalenceMode::Semantic)
    );
    let wrong_fence = ContractEffect::Fence {
        fm: 0,
        predecessor: 3,
        successor: 3,
    };
    assert_eq!(
        compare_effects(&[read()], &[read(), wrong_fence], &policy)
            .unwrap()
            .verdict,
        EquivalenceVerdict::Incomplete
    );
    assert_eq!(
        compare_effects(&[read()], &[read()], &policy)
            .unwrap()
            .verdict,
        EquivalenceVerdict::Incomplete
    );
}

#[test]
fn v2_preserves_observed_fence_parameters() {
    assert_eq!(
        effects_from_observable(&[ObservableEvent::Fence {
            fm: 1,
            predecessor: 3,
            successor: 12,
        }])
        .unwrap(),
        vec![ContractEffect::Fence {
            fm: 1,
            predecessor: 3,
            successor: 12,
        }]
    );
}

#[test]
fn blocking_effect_requires_an_explicit_await_ready_replacement() {
    let policy = EffectPolicy::new(
        EffectComparison::ExactEffectsV2,
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
    #[derive(serde::Deserialize)]
    struct Vocabulary {
        omission: OmissionReason,
        operation: PlatformOperation,
    }

    let vocabulary: Vocabulary = toml_edit::de::from_str(
        "omission = \"debug-diagnostic\"\noperation = \"nvs-calibration-cache\"\n",
    )
    .unwrap();
    assert_eq!(vocabulary.omission, OmissionReason::DebugDiagnostic);
    assert_eq!(vocabulary.operation, PlatformOperation::NvsCalibrationCache);
    assert!(
        toml_edit::de::from_str::<Vocabulary>(
            "omission = \"whatever\"\noperation = \"vendor-magic\"\n"
        )
        .is_err()
    );
}

#[test]
fn typed_effect_rules_are_closed_and_restrict_omissions() {
    assert!(
        EffectPolicy::new(
            EffectComparison::ExactEffectsV2,
            [(
                EffectSelector::PlatformCall {
                    operation: PlatformOperation::DebugDiagnostic,
                },
                EffectDisposition::AllowedOmission(OmissionReason::DebugDiagnostic),
            )],
        )
        .is_ok()
    );
    assert!(
        EffectPolicy::new(
            EffectComparison::ExactEffectsV2,
            [(
                EffectSelector::MmioWrite {
                    width: 32,
                    address: 0x2010_7030,
                },
                EffectDisposition::AllowedOmission(OmissionReason::DebugDiagnostic),
            )],
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
        EffectComparison::ExactEffectsV2,
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
    for selector in [
        EffectSelector::PlatformProvidedInput {
            input: "station-mac".to_owned(),
        },
        EffectSelector::PlatformProvidedService {
            service: "embassy-wakeup".to_owned(),
        },
        EffectSelector::PublishedEvent {
            event: "rx-success".to_owned(),
        },
        EffectSelector::InitializationPrerequisite {
            prerequisite: "power-irqs-disabled".to_owned(),
        },
    ] {
        assert!(
            EffectPolicy::new(
                EffectComparison::ExactEffectsV2,
                [(selector, EffectDisposition::Required)],
            )
            .is_ok()
        );
    }
}

#[test]
fn semantic_boundary_rule_syntax_is_closed_and_canonical() {
    for (selector, disposition, canonical) in [
        (
            EffectSelector::MmioRead {
                width: 32,
                address: 0x2010_7030,
            },
            EffectDisposition::PlatformProvidedInput {
                input: "station-mac".to_owned(),
            },
            "mmio-read 32 0x20107030 platform-provided-input station-mac",
        ),
        (
            EffectSelector::PlatformCall {
                operation: PlatformOperation::RtosSchedulingAdapter,
            },
            EffectDisposition::PlatformProvidedService {
                service: "embassy-wakeup".to_owned(),
            },
            "platform-call rtos-scheduling-adapter platform-provided-service embassy-wakeup",
        ),
        (
            EffectSelector::StateWrite {
                width: 32,
                field: "sta.event".to_owned(),
            },
            EffectDisposition::PublishedEvent {
                event: "rx-ready".to_owned(),
            },
            "state-write 32 sta.event published-event rx-ready",
        ),
        (
            EffectSelector::MmioWrite {
                width: 32,
                address: 0x2010_7030,
            },
            EffectDisposition::InitializationPrerequisite {
                prerequisite: "mac-clock-enabled".to_owned(),
            },
            "mmio-write 32 0x20107030 initialization-prerequisite mac-clock-enabled",
        ),
    ] {
        EffectPolicy::new(
            EffectComparison::ExactEffectsV2,
            [(selector.clone(), disposition.clone())],
        )
        .unwrap();
        assert_eq!(
            format!("{} {}", selector.canonical(), disposition.canonical()),
            canonical
        );
    }
    assert!(
        EffectPolicy::new(
            EffectComparison::ExactEffectsV2,
            [(
                EffectSelector::MmioWrite {
                    width: 32,
                    address: 0x2010_7030,
                },
                EffectDisposition::PlatformProvidedInput {
                    input: "station-mac".to_owned(),
                },
            )],
        )
        .is_err()
    );
    assert!(
        EffectPolicy::new(
            EffectComparison::ExactEffectsV2,
            [(
                EffectSelector::PlatformCall {
                    operation: PlatformOperation::Random,
                },
                EffectDisposition::PublishedEvent {
                    event: "Invalid/Event".to_owned(),
                },
            )],
        )
        .is_err()
    );
}
