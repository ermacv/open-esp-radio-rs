//! Guard normalization and MMIO/result provenance.

use super::*;

#[test]
fn cfg_guard_result_sources_link_masks_to_producer_targets() {
    let mut bits = [BitSource::Constant(false); 32];
    for (bit, source) in bits.iter_mut().enumerate().take(8).skip(4) {
        *source = BitSource::CallResult {
            call_token: 10,
            bit: bit as u8,
            inverted: false,
        };
    }
    let condition = BranchCondition {
        site: 0x20,
        operation: BranchOperation::NotEqual,
        left: SymbolicValue::Bits(Box::new(bits)),
        right: SymbolicValue::Constant(0),
    };
    let call_results = BTreeMap::from([(10, "hal::interrupt_status".to_owned())]);

    assert_eq!(
        guard_result_sources(&condition, &call_results),
        [LinkedCallGuardResultSource {
            kind: "call-result",
            token: 10,
            target: Some("hal::interrupt_status".to_owned()),
            operand: "left",
            value_bits: Some(0x0000_00f0),
            source_bits: 0x0000_00f0,
            inverted: false,
            comparison_value: Some(0),
            source_comparison_value: Some(0),
            producer_return_exact: None,
            mmio_sources: Vec::new(),
        }]
    );
}

#[test]
fn allocation_guard_sources_use_the_real_external_call_identity() {
    let token = open_radio_vendor_analysis_model::ALLOCATED_EXTERNAL_RESULT_TOKEN_FLAG | 3;
    let condition = BranchCondition {
        site: 0x24,
        operation: BranchOperation::NotEqual,
        left: SymbolicValue::ExternalResult(token),
        right: SymbolicValue::Constant(0),
    };
    let call_results = BTreeMap::from([(3, "wifi_zalloc".to_owned())]);

    let sources = guard_result_sources(&condition, &call_results);
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].token, 3);
    assert_eq!(sources[0].target.as_deref(), Some("wifi_zalloc"));

    let svd = MmioMap {
        registers: Vec::new(),
        regions: Vec::new(),
    };
    let provenance = return_provenance(&condition.left, &call_results, &svd);
    assert_eq!(provenance.sources[0].token, Some(3));
    assert_eq!(provenance.sources[0].target.as_deref(), Some("wifi_zalloc"));
}

#[test]
fn direct_mmio_predicate_maps_shifted_comparison_back_to_register_bits() {
    let address = 0x2010_4c48;
    let condition = BranchCondition {
        site: 0x24,
        operation: BranchOperation::Equal,
        left: SymbolicValue::register_read(7, address, 32, false)
            .and(0x0000_00f0)
            .shift_right(4),
        right: SymbolicValue::Constant(3),
    };
    let svd = MmioMap {
        registers: vec![crate::Register {
            address,
            name: "WIFI_MAC_INTERRUPT.STATUS".to_owned(),
        }],
        regions: Vec::new(),
    };

    assert_eq!(
        direct_mmio_predicate_sources(&condition, &svd),
        [LinkedDirectMmioPredicateSource {
            operand: "left",
            read_token: 7,
            address,
            register: "WIFI_MAC_INTERRUPT.STATUS".to_owned(),
            value_bits: 0x0000_000f,
            register_bits: 0x0000_00f0,
            inverted: false,
            comparison_value: Some(3),
            register_comparison_value: Some(0x30),
        }]
    );
}

#[test]
fn direct_mmio_branch_is_inventoried_without_a_guarded_call() {
    let address = 0x2010_4cb4;
    let event = DraftReferenceEvent::BranchDecision {
        condition: BranchCondition {
            site: 0x54,
            operation: BranchOperation::Equal,
            left: SymbolicValue::register_read(2, address, 32, false).and(0xf0),
            right: SymbolicValue::Constant(0),
        },
        taken: true,
    };
    let resolver = empty_resolver();
    let identities = IrIdentityCatalog::new(&resolver, None);
    let svd = MmioMap {
        registers: vec![crate::Register {
            address,
            name: "WIFI_MAC_TX_COMMON.QUEUE_STATE".to_owned(),
        }],
        regions: Vec::new(),
    };
    let mut evidence = DirectTraceEvidence::default();

    collect_guarded_direct_event(
        &event,
        &resolver,
        &identities,
        &svd,
        &BTreeMap::new(),
        &mut evidence,
    );

    assert!(evidence.calls.is_empty());
    assert_eq!(evidence.direct_mmio_predicates.len(), 1);
    let predicate = evidence
        .direct_mmio_predicates
        .first()
        .expect("direct predicate was retained");
    assert_eq!(predicate.operation, "equal");
    assert_eq!(predicate.sources[0].register_bits, 0xf0);
    assert_eq!(predicate.sources[0].register_comparison_value, Some(0));
    assert_eq!(
        current_guard_path(&evidence.guards).guards[0].direct_mmio_sources,
        predicate.sources
    );
}

#[test]
fn return_provenance_maps_result_ranges_back_to_mmio_bits() {
    let mut bits = [BitSource::Constant(false); 32];
    for (output_bit, source) in bits.iter_mut().enumerate().take(4) {
        *source = BitSource::Register {
            read_token: 3,
            address: 0x2010_4c48,
            bit: output_bit as u8 + 4,
            inverted: false,
        };
    }
    let svd = MmioMap {
        registers: vec![crate::Register {
            address: 0x2010_4c48,
            name: "WIFI_MAC_INTERRUPT.STATUS".to_owned(),
        }],
        regions: vec![crate::MmioRegion {
            name: "wifi".to_owned(),
            start: 0x2010_0000,
            end: 0x2011_0000,
            readable: true,
            writable: true,
        }],
    };

    let provenance =
        return_provenance(&SymbolicValue::Bits(Box::new(bits)), &BTreeMap::new(), &svd);

    assert!(provenance.exact);
    assert_eq!(provenance.known_zero_bits, 0xffff_fff0);
    assert_eq!(provenance.unknown_bits, 0);
    assert_eq!(provenance.sources.len(), 1);
    assert_eq!(provenance.sources[0].output_bits, 0x0000_000f);
    assert_eq!(provenance.sources[0].source_bits, 0x0000_00f0);
    let result_source = LinkedCallGuardResultSource {
        kind: "call-result",
        token: 10,
        target: Some("hal::interrupt_status".to_owned()),
        operand: "left",
        value_bits: Some(0x0000_0005),
        source_bits: 0x0000_0005,
        inverted: false,
        comparison_value: Some(1),
        source_comparison_value: Some(1),
        producer_return_exact: None,
        mmio_sources: Vec::new(),
    };
    let producers = BTreeMap::from([("hal::interrupt_status".to_owned(), provenance)]);
    assert_eq!(
        guard_mmio_sources(&result_source, "hal::interrupt_status", &producers),
        [LinkedCallGuardMmioSource {
            address: 0x2010_4c48,
            register: "WIFI_MAC_INTERRUPT.STATUS".to_owned(),
            producer_path: vec!["hal::interrupt_status".to_owned()],
            result_bits: 0x0000_0005,
            register_bits: 0x0000_0050,
            inverted: false,
            result_comparison_value: Some(1),
            register_comparison_value: Some(0x10),
        }]
    );
}

#[test]
fn guard_comparison_projects_through_shifted_inverted_producer_return() {
    let address = 0x2010_4c48;
    let mut condition_bits = [BitSource::Constant(false); 32];
    for (value_bit, source) in condition_bits.iter_mut().enumerate().take(4) {
        *source = BitSource::CallResult {
            call_token: 10,
            bit: value_bit as u8 + 4,
            inverted: false,
        };
    }
    let condition = BranchCondition {
        site: 0x20,
        operation: BranchOperation::Equal,
        left: SymbolicValue::Bits(Box::new(condition_bits)),
        right: SymbolicValue::Constant(3),
    };
    let result_source = guard_result_sources(
        &condition,
        &BTreeMap::from([(10, "hal::interrupt_status".to_owned())]),
    )
    .into_iter()
    .next()
    .expect("shifted call result has exact provenance");

    assert_eq!(result_source.value_bits, Some(0x0000_000f));
    assert_eq!(result_source.source_bits, 0x0000_00f0);
    assert_eq!(result_source.comparison_value, Some(3));
    assert_eq!(result_source.source_comparison_value, Some(0x30));

    let mut return_bits = [BitSource::Constant(false); 32];
    for (output_bit, source) in return_bits.iter_mut().enumerate().take(8).skip(4) {
        *source = BitSource::Register {
            read_token: 3,
            address,
            bit: output_bit as u8 + 4,
            inverted: true,
        };
    }
    let provenance = return_provenance(
        &SymbolicValue::Bits(Box::new(return_bits)),
        &BTreeMap::new(),
        &MmioMap {
            registers: vec![crate::Register {
                address,
                name: "WIFI_MAC_INTERRUPT.STATUS".to_owned(),
            }],
            regions: Vec::new(),
        },
    );
    let producers = BTreeMap::from([("hal::interrupt_status".to_owned(), provenance)]);

    assert_eq!(
        guard_mmio_sources(&result_source, "hal::interrupt_status", &producers),
        [LinkedCallGuardMmioSource {
            address,
            register: "WIFI_MAC_INTERRUPT.STATUS".to_owned(),
            producer_path: vec!["hal::interrupt_status".to_owned()],
            result_bits: 0x0000_00f0,
            register_bits: 0x0000_0f00,
            inverted: true,
            result_comparison_value: Some(0x30),
            register_comparison_value: Some(0xc00),
        }]
    );
}

#[test]
fn guard_mmio_sources_follow_exact_internal_return_wrappers() {
    let address = 0x2010_4c48;
    let svd = MmioMap {
        registers: vec![crate::Register {
            address,
            name: "WIFI_MAC_INTERRUPT.STATUS".to_owned(),
        }],
        regions: Vec::new(),
    };
    let mut leaf_bits = [BitSource::Constant(false); 32];
    for (output_bit, source) in leaf_bits.iter_mut().enumerate().take(8).skip(4) {
        *source = BitSource::Register {
            read_token: 3,
            address,
            bit: output_bit as u8 + 4,
            inverted: true,
        };
    }
    let leaf = return_provenance(
        &SymbolicValue::Bits(Box::new(leaf_bits)),
        &BTreeMap::new(),
        &svd,
    );
    let mut wrapper_bits = [BitSource::Constant(false); 32];
    for (output_bit, source) in wrapper_bits.iter_mut().enumerate().take(4) {
        *source = BitSource::CallResult {
            call_token: 7,
            bit: output_bit as u8 + 4,
            inverted: true,
        };
    }
    let wrapper = return_provenance(
        &SymbolicValue::Bits(Box::new(wrapper_bits)),
        &BTreeMap::from([(7, "hal::leaf_status".to_owned())]),
        &svd,
    );
    let result_source = LinkedCallGuardResultSource {
        kind: "call-result",
        token: 10,
        target: Some("hal::status_wrapper".to_owned()),
        operand: "left",
        value_bits: Some(0x5),
        source_bits: 0x5,
        inverted: false,
        comparison_value: Some(1),
        source_comparison_value: Some(1),
        producer_return_exact: None,
        mmio_sources: Vec::new(),
    };
    let producers = BTreeMap::from([
        ("hal::leaf_status".to_owned(), leaf),
        ("hal::status_wrapper".to_owned(), wrapper),
    ]);

    assert_eq!(
        guard_mmio_sources(&result_source, "hal::status_wrapper", &producers),
        [LinkedCallGuardMmioSource {
            address,
            register: "WIFI_MAC_INTERRUPT.STATUS".to_owned(),
            producer_path: vec![
                "hal::status_wrapper".to_owned(),
                "hal::leaf_status".to_owned(),
            ],
            result_bits: 0x5,
            register_bits: 0x500,
            inverted: false,
            result_comparison_value: Some(1),
            register_comparison_value: Some(0x100),
        }]
    );
}

#[test]
fn guard_mmio_sources_stop_at_recursive_return_cycles() {
    let call_return = |target: &str| LinkedReturnProvenance {
        exact: true,
        known_zero_bits: u32::MAX ^ 1,
        known_one_bits: 0,
        unknown_bits: 0,
        sources: vec![LinkedReturnBitSource {
            kind: "call-result",
            output_lsb: 0,
            source_lsb: 0,
            width: 1,
            output_bits: 1,
            source_bits: 1,
            inverted: false,
            argument: None,
            token: Some(0),
            target: Some(target.to_owned()),
            address: None,
            register: None,
        }],
    };
    let producers = BTreeMap::from([
        ("a".to_owned(), call_return("b")),
        ("b".to_owned(), call_return("a")),
    ]);
    let source = LinkedCallGuardResultSource {
        kind: "call-result",
        token: 0,
        target: Some("a".to_owned()),
        operand: "left",
        value_bits: Some(1),
        source_bits: 1,
        inverted: false,
        comparison_value: Some(0),
        source_comparison_value: Some(0),
        producer_return_exact: None,
        mmio_sources: Vec::new(),
    };

    assert!(guard_mmio_sources(&source, "a", &producers).is_empty());
}
