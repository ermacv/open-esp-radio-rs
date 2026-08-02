use super::*;
use crate::{EffectComparison, ResolvedReferenceBody, ResolvedReferenceEvent, Timeout};

const BINDINGS: &str = "\
pac-binding-index 2
crate fixture_radio_pac
register 0x20100034 32 write-only PHY_ORACLE.I2C_WORD1 PHY_ORACLE PhyOracle phy_oracle - i2c_word 1 -
field 0x20100034 PHY_ORACLE.I2C_WORD1 VALUE value - 0 32 write-only
register 0x20104c48 32 read-only WIFI_MAC_INTERRUPT.STATUS WIFI_MAC_INTERRUPT WifiMacInterrupt wifi_mac_interrupt - status - -
field 0x20104c48 WIFI_MAC_INTERRUPT.STATUS EVENTS events - 0 32 read-only
register 0x20104c4c 32 write-only WIFI_MAC_INTERRUPT.CLEAR WIFI_MAC_INTERRUPT WifiMacInterrupt wifi_mac_interrupt - clear - -
field 0x20104c4c WIFI_MAC_INTERRUPT.CLEAR EVENTS events - 0 32 write-only
register 0x20104d40 32 read-write WIFI_MAC_TX_QUEUE_CONTROL.CONTROL0 WIFI_MAC_TX_QUEUE_CONTROL WifiMacTxQueueControl wifi_mac_tx_queue_control - control 0 -
register 0x20104d50 32 read-write WIFI_MAC_TX_QUEUE_CONTROL.CONTROL1 WIFI_MAC_TX_QUEUE_CONTROL WifiMacTxQueueControl wifi_mac_tx_queue_control - control 1 -
register 0x20104d60 32 read-write WIFI_MAC_TX_QUEUE_CONTROL.CONTROL2 WIFI_MAC_TX_QUEUE_CONTROL WifiMacTxQueueControl wifi_mac_tx_queue_control - control 2 -
register 0x20104d70 32 read-write WIFI_MAC_TX_QUEUE_CONTROL.CONTROL3 WIFI_MAC_TX_QUEUE_CONTROL WifiMacTxQueueControl wifi_mac_tx_queue_control - control 3 -
";

fn get_event_program() -> ResolvedReferenceProgram {
    ResolvedReferenceProgram {
        symbol: "hal_mac_interrupt_get_event".to_owned(),
        dependencies: Vec::new(),
        body: ResolvedReferenceBody::Linear {
            events: vec![ResolvedReferenceEvent::Observable(
                ObservableEvent::Memory {
                    access: MemoryAccess::Read,
                    width: 32,
                    address: 0x2010_4c48,
                    register: "WIFI_MAC_INTERRUPT.STATUS".to_owned(),
                    value: None,
                },
            )],
            return_value: SymbolicValue::RegisterImage {
                read_token: 0,
                address: 0x2010_4c48,
                and_mask: u32::MAX,
                or_mask: 0,
            },
        },
        exit_return_modeled: true,
    }
}

fn get_event_policy(disposition: EffectDisposition) -> EffectPolicy {
    EffectPolicy::new(
        EffectComparison::ExactEffectsV1,
        [(
            EffectSelector::MmioRead {
                width: 32,
                address: 0x2010_4c48,
            },
            disposition,
        )],
    )
    .unwrap()
}

fn indexed_queue_address() -> SymbolicValue {
    SymbolicValue::Expression {
        operation: crate::ExpressionOperation::ShiftLeft,
        left: Box::new(SymbolicValue::Expression {
            operation: crate::ExpressionOperation::Subtract,
            left: Box::new(SymbolicValue::Constant(0x0201_04d7)),
            right: Box::new(SymbolicValue::input(0)),
        }),
        right: Box::new(SymbolicValue::Constant(4)),
    }
}

fn indexed_queue_registers() -> Vec<crate::IndexedMmioRegister> {
    (0..4)
        .map(|index| crate::IndexedMmioRegister {
            address: 0x2010_4d40 + index * 0x10,
            name: format!("WIFI_MAC_TX_QUEUE_CONTROL.CONTROL{index}"),
        })
        .collect()
}

fn indexed_queue_event(
    access: MemoryAccess,
    value: Option<SymbolicValue>,
) -> ResolvedReferenceEvent {
    ResolvedReferenceEvent::IndexedMmio {
        access,
        width: 32,
        address: indexed_queue_address(),
        registers: indexed_queue_registers(),
        guard: Some(crate::IndexedMmioGuard {
            selector: SymbolicValue::input(0),
            maximum: 3,
        }),
        value,
    }
}

fn indexed_queue_policy() -> EffectPolicy {
    EffectPolicy::new(
        EffectComparison::ExactEffectsV1,
        (0..4).flat_map(|index| {
            let address = 0x2010_4d40 + index * 0x10;
            [
                (
                    EffectSelector::MmioRead { width: 32, address },
                    EffectDisposition::Required,
                ),
                (
                    EffectSelector::MmioWrite { width: 32, address },
                    EffectDisposition::Required,
                ),
            ]
        }),
    )
    .unwrap()
}

#[test]
fn parses_exact_pac_paths_including_array_indices() {
    let bindings = PacBindingIndex::parse(BINDINGS).unwrap();
    assert_eq!(bindings.crate_name, "fixture_radio_pac");
    assert_eq!(
        bindings
            .register(0x2010_0034, 32, "PHY_ORACLE.I2C_WORD1")
            .unwrap()
            .method_path("registers"),
        "registers.i2c_word(1)"
    );
}

#[test]
fn lowers_an_exact_vendor_leaf_to_generated_pac_access() {
    let bindings = PacBindingIndex::parse(BINDINGS).unwrap();
    let plan = DriverPlan::from_resolved(
        &get_event_program(),
        &get_event_policy(EffectDisposition::Required),
        &bindings,
    )
    .unwrap();
    let generated = lower_pac_leaf(&plan, &bindings.crate_name).unwrap();
    assert!(
        generated
            .source
            .contains("wifi_mac_interrupt_registers.status().read().bits() as u32")
    );
    assert!(generated.source.contains("(read0 & 0xffffffff_u32)"));
    assert!(!generated.source.contains("0x20104c48"));
}

#[test]
fn shifted_bit_sources_keep_parentheses_in_generated_write_expressions() {
    let bindings = PacBindingIndex::parse(BINDINGS).unwrap();
    let mut bits = [crate::BitSource::Constant(false); 32];
    bits[1] = crate::BitSource::Register {
        read_token: 0,
        address: 0x2010_4c48,
        bit: 1,
        inverted: false,
    };
    let program = ResolvedReferenceProgram {
        symbol: "shifted_status_bit".to_owned(),
        dependencies: Vec::new(),
        body: ResolvedReferenceBody::Linear {
            events: vec![
                ResolvedReferenceEvent::Observable(ObservableEvent::Memory {
                    access: MemoryAccess::Read,
                    width: 32,
                    address: 0x2010_4c48,
                    register: "WIFI_MAC_INTERRUPT.STATUS".to_owned(),
                    value: None,
                }),
                ResolvedReferenceEvent::Observable(ObservableEvent::Memory {
                    access: MemoryAccess::Write,
                    width: 32,
                    address: 0x2010_4c4c,
                    register: "WIFI_MAC_INTERRUPT.CLEAR".to_owned(),
                    value: Some(SymbolicValue::Bits(Box::new(bits))),
                }),
            ],
            return_value: SymbolicValue::Constant(0),
        },
        exit_return_modeled: true,
    };
    let policy = EffectPolicy::new(
        EffectComparison::ExactEffectsV1,
        [
            (
                EffectSelector::MmioRead {
                    width: 32,
                    address: 0x2010_4c48,
                },
                EffectDisposition::Required,
            ),
            (
                EffectSelector::MmioWrite {
                    width: 32,
                    address: 0x2010_4c4c,
                },
                EffectDisposition::Required,
            ),
        ],
    )
    .unwrap();
    let plan = DriverPlan::from_resolved(&program, &policy, &bindings).unwrap();
    let generated = lower_pac_leaf(&plan, &bindings.crate_name).unwrap();

    assert!(generated.source.contains("(((read0 >> 1) & 1_u32) << 1)"));
}

#[test]
fn lowers_a_guarded_indexed_queue_rmw_to_a_finite_pac_match() {
    let bindings = PacBindingIndex::parse(BINDINGS).unwrap();
    let program = ResolvedReferenceProgram {
        symbol: "hal_mac_set_txq_invalid".to_owned(),
        dependencies: Vec::new(),
        body: ResolvedReferenceBody::Linear {
            events: vec![
                indexed_queue_event(MemoryAccess::Read, None),
                indexed_queue_event(
                    MemoryAccess::Write,
                    Some(SymbolicValue::IndexedRegisterImage {
                        read_token: 0,
                        and_mask: 0x3fff_ffff,
                        or_mask: 0,
                    }),
                ),
            ],
            return_value: indexed_queue_address(),
        },
        exit_return_modeled: false,
    };
    let plan = DriverPlan::from_resolved(&program, &indexed_queue_policy(), &bindings).unwrap();
    let generated = lower_pac_leaf(&plan, &bindings.crate_name).unwrap();

    assert!(generated.source.contains("let read0 = match arg0"));
    assert!(
        generated
            .source
            .contains("0 => wifi_mac_tx_queue_control_registers.control(3).read().bits() as u32")
    );
    assert!(
        generated
            .source
            .contains("3 => wifi_mac_tx_queue_control_registers.control(0).read().bits() as u32")
    );
    assert!(
        generated
            .source
            .contains("writer.bits(((read0 & 0x3fffffff_u32) | 0x00000000_u32) as u32)")
    );
    assert!(!generated.source.contains("0x20104d"));
}

#[test]
fn indexed_queue_plan_rejects_one_missing_address_policy() {
    let bindings = PacBindingIndex::parse(BINDINGS).unwrap();
    let program = ResolvedReferenceProgram {
        symbol: "hal_mac_is_txq_enabled".to_owned(),
        dependencies: Vec::new(),
        body: ResolvedReferenceBody::Linear {
            events: vec![indexed_queue_event(MemoryAccess::Read, None)],
            return_value: SymbolicValue::IndexedRegisterImage {
                read_token: 0,
                and_mask: 0x8000_0000,
                or_mask: 0,
            },
        },
        exit_return_modeled: true,
    };
    let incomplete = EffectPolicy::new(
        EffectComparison::ExactEffectsV1,
        (0..3).map(|index| {
            (
                EffectSelector::MmioRead {
                    width: 32,
                    address: 0x2010_4d40 + index * 0x10,
                },
                EffectDisposition::Required,
            )
        }),
    )
    .unwrap();

    assert!(DriverPlan::from_resolved(&program, &incomplete, &bindings).is_err());
}

#[test]
fn transition_skeleton_exposes_async_replacement_without_an_executor() {
    let bindings = PacBindingIndex::parse(BINDINGS).unwrap();
    let disposition = EffectDisposition::ReplacedByAsync {
        condition: "mac-event-ready".to_owned(),
        timeout: Timeout::Attempts(8),
    };
    let plan = DriverPlan::from_resolved(
        &get_event_program(),
        &get_event_policy(disposition),
        &bindings,
    )
    .unwrap();
    let generated = lower_transition_skeleton(&plan).unwrap();
    assert!(generated.source.contains(
        "TransitionAction::AwaitReady { condition: \"mac-event-ready\", timeout: TransitionTimeout::Attempts(8) }"
    ));
    assert!(
        generated
            .source
            .contains("TransitionCompletion::Value(value)")
    );
    assert!(!generated.source.contains("embassy"));
}

#[test]
fn driver_plan_rejects_an_unclassified_effect() {
    let bindings = PacBindingIndex::parse(BINDINGS).unwrap();
    let policy = EffectPolicy::new(
        EffectComparison::ExactEffectsV1,
        [(
            EffectSelector::MmioWrite {
                width: 32,
                address: 0x2010_4c4c,
            },
            EffectDisposition::Required,
        )],
    )
    .unwrap();
    assert!(DriverPlan::from_resolved(&get_event_program(), &policy, &bindings).is_err());
}
