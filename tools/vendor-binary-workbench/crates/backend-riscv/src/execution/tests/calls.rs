//! Unresolved and scenario-modeled call regressions.

use super::*;

#[test]
fn unresolved_external_tail_call_fails_closed() {
    let image = tail_relocation_image(None);
    let inventory = image.coverage_inventory("wrapper").unwrap();
    assert_eq!(inventory.unresolved_edges.len(), 1);
    assert!(inventory.branch_sites.is_empty());

    let svd = MmioMap {
        registers: Vec::new(),
        regions: vec![crate::MmioRegion {
            name: "sentinel".to_owned(),
            start: 0,
            end: 1,
            readable: true,
            writable: true,
        }],
    };
    let error = execute(&image, &svd, "wrapper", Scenario::default()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("unresolved external call callee")
    );
}

#[test]
fn reviewed_call_model_intercepts_linked_code_and_is_fully_consumed() {
    let mut image = tiny_image(
        vec![
            0x13, 0x84, 0x00, 0x00, // addi s0, ra, 0
            0xef, 0x00, 0x00, 0x01, // jal ra, 16
            0x93, 0x00, 0x04, 0x00, // addi ra, s0, 0
            0x67, 0x80, 0x00, 0x00, // ret
            0, 0, 0, 0, // padding
            0x73, 0x00, 0x10, 0x00, // callee: ebreak (must not execute)
        ],
        24,
    );
    image
        .symbols_by_name
        .insert("platform_service".to_owned(), 0x1014);
    image
        .symbols_by_address
        .insert(0x1014, "platform_service".to_owned());
    let scenario = Scenario {
        call_responses: BTreeMap::from([(
            "platform_service".to_owned(),
            VecDeque::from([ModeledCallResponse::scalar(0x1234_5678)]),
        )]),
        ..Scenario::default()
    };

    let result = execute(&image, &empty_svd(), "test", scenario).unwrap();
    assert_eq!(result.return_value, 0x1234_5678);
    assert_eq!(result.ordered_calls.len(), 1);
    assert_eq!(result.ordered_calls[0].symbol, "platform_service");
}

#[test]
fn modeled_call_response_applies_two_word_return_and_private_stack_output() {
    let image = tiny_image(vec![0x67, 0x80, 0x00, 0x00], 4);
    let svd = empty_svd();
    let mut machine = Machine::new(&image, &svd, 0x1000, Scenario::default());
    let output_address = STACK_POINTER - 4;
    machine.set_register(rv_asm::Reg::A0, 0x1111_1111);
    machine.set_register(rv_asm::Reg::A1, 0x2222_2222);
    machine.set_register(rv_asm::Reg::A2, output_address);
    machine.record_call(0x1000, "platform_service".to_owned());

    machine
        .apply_modeled_call_response(
            "platform_service",
            0x1000,
            ModeledCallResponse {
                return_words: [Some(0x1234_5678), Some(0x9abc_def0)],
                outputs: vec![ModeledCallOutput::PrivateStackU8 {
                    pointer_argument: 2,
                    value: 0x5a,
                }],
            },
        )
        .unwrap();

    assert_eq!(machine.register(rv_asm::Reg::A0), 0x1234_5678);
    assert_eq!(machine.register(rv_asm::Reg::A1), 0x9abc_def0);
    assert_eq!(machine.read(output_address, 8).unwrap(), 0x5a);
    assert_eq!(machine.ordered_calls[0].arguments[2], output_address);
    assert!(matches!(
        machine.timeline.as_slice(),
        [
            ExecutionTimelineEvent::Call(_),
            ExecutionTimelineEvent::RamWrite { width: 8, address, value: 0x5a },
            ExecutionTimelineEvent::RamRead { width: 8, address: read_address, value: 0x5a },
        ] if *address == output_address && *read_address == output_address
    ));
}

#[test]
fn modeled_call_response_rejects_non_stack_output_pointer() {
    let image = tiny_image(vec![0x67, 0x80, 0x00, 0x00], 4);
    let svd = empty_svd();
    let mut machine = Machine::new(&image, &svd, 0x1000, Scenario::default());
    machine.set_register(rv_asm::Reg::A2, 0x1000);

    let error = machine
        .apply_modeled_call_response(
            "platform_service",
            0x1000,
            ModeledCallResponse {
                return_words: [Some(1), None],
                outputs: vec![ModeledCallOutput::PrivateStackU8 {
                    pointer_argument: 2,
                    value: 1,
                }],
            },
        )
        .unwrap_err();

    assert!(error.to_string().contains("points outside private stack"));
}

#[test]
fn reviewed_call_model_rejects_missing_and_unused_responses() {
    let mut image = tiny_image(
        vec![
            0x13, 0x84, 0x00, 0x00, // addi s0, ra, 0
            0xef, 0x00, 0x00, 0x01, // jal ra, 16
            0x93, 0x00, 0x04, 0x00, // addi ra, s0, 0
            0x67, 0x80, 0x00, 0x00, // ret
            0, 0, 0, 0, // padding
            0x67, 0x80, 0x00, 0x00, // callee: ret
        ],
        24,
    );
    image
        .symbols_by_name
        .insert("platform_service".to_owned(), 0x1014);
    image
        .symbols_by_address
        .insert(0x1014, "platform_service".to_owned());

    let missing = Scenario {
        call_responses: BTreeMap::from([("platform_service".to_owned(), VecDeque::new())]),
        ..Scenario::default()
    };
    assert!(
        execute(&image, &empty_svd(), "test", missing)
            .unwrap_err()
            .to_string()
            .contains("without a remaining response")
    );

    let unused = Scenario {
        call_responses: BTreeMap::from([(
            "platform_service".to_owned(),
            VecDeque::from([
                ModeledCallResponse::scalar(1),
                ModeledCallResponse::scalar(2),
            ]),
        )]),
        ..Scenario::default()
    };
    assert!(
        execute(&image, &empty_svd(), "test", unused)
            .unwrap_err()
            .to_string()
            .contains("unconsumed modeled call responses")
    );
}

#[test]
fn runtime_table_instance_resolves_symbolic_slots_and_pointer_cells() {
    let mut image = tiny_image(
        vec![
            0x13, 0x84, 0x00, 0x00, // addi s0, ra, 0
            0xb7, 0x32, 0x00, 0x00, // lui t0, 0x3
            0x83, 0xa2, 0x02, 0x00, // lw t0, 0(t0)
            0x83, 0xa2, 0x42, 0x00, // lw t0, 4(t0)
            0xe7, 0x80, 0x02, 0x00, // jalr ra, 0(t0)
            0x93, 0x00, 0x04, 0x00, // addi ra, s0, 0
            0x67, 0x80, 0x00, 0x00, // ret
            0x67, 0x80, 0x00, 0x00, // callback: ret
        ],
        32,
    );
    image.symbols_by_name.insert("callback".to_owned(), 0x101c);
    image
        .symbols_by_address
        .insert(0x101c, "callback".to_owned());
    let scenario = Scenario {
        table_instances: vec![TableInstance {
            layout_id: "reviewed-services-v1".to_owned(),
            base_address: 0x4000,
            layout_size: 0x20,
            pointer_cells: vec![0x3000],
            slots: vec![TableInstanceSlot {
                offset: 4,
                target: TableSlotTarget::Symbol("callback".to_owned()),
            }],
        }],
        ..Scenario::default()
    };

    let result = execute(&image, &empty_svd(), "test", scenario).unwrap();
    assert_eq!(result.ordered_calls.len(), 1);
    assert_eq!(result.ordered_calls[0].symbol, "callback");
    assert!(
        result
            .indirect_calls
            .iter()
            .any(|call| { call.site == 0x1010 && call.symbol == "callback" })
    );
    assert!(result.table_lifecycle_complete);
    assert!(matches!(
        &result.table_lifecycle[0],
        TableLifecycleEvent::SlotInitialized {
            layout_id,
            offset: 4,
            target: 0x101c,
        } if layout_id == "reviewed-services-v1"
    ));
    assert!(matches!(
        result.table_lifecycle.last(),
        Some(TableLifecycleEvent::IndirectCall {
            layout_id: Some(layout_id),
            slot_offset: Some(4),
            site: 0x1010,
            target: 0x101c,
            symbol,
        }) if layout_id == "reviewed-services-v1" && symbol == "callback"
    ));
    assert_eq!(
        (0..4)
            .map(|offset| result.initial_memory[&(0x3000 + offset)])
            .collect::<Vec<_>>(),
        0x4000_u32.to_le_bytes()
    );
    assert_eq!(
        (0..4)
            .map(|offset| result.initial_memory[&(0x4004 + offset)])
            .collect::<Vec<_>>(),
        0x101c_u32.to_le_bytes()
    );
}

#[test]
fn runtime_table_lifecycle_records_slot_install_before_indirect_call() {
    let mut image = tiny_image(
        vec![
            0x13, 0x84, 0x00, 0x00, // addi s0, ra, 0
            0xb7, 0x42, 0x00, 0x00, // lui t0, 0x4
            0x03, 0xa3, 0x42, 0x00, // lw t1, 4(t0)
            0x23, 0xa2, 0x62, 0x00, // sw t1, 4(t0)
            0xe7, 0x00, 0x03, 0x00, // jalr ra, 0(t1)
            0x93, 0x00, 0x04, 0x00, // addi ra, s0, 0
            0x67, 0x80, 0x00, 0x00, // ret
            0x67, 0x80, 0x00, 0x00, // callback: ret
        ],
        32,
    );
    image.symbols_by_name.insert("callback".to_owned(), 0x101c);
    image
        .symbols_by_address
        .insert(0x101c, "callback".to_owned());
    let scenario = Scenario {
        table_instances: vec![TableInstance {
            layout_id: "services-v2".to_owned(),
            base_address: 0x4000,
            layout_size: 0x20,
            pointer_cells: Vec::new(),
            slots: vec![TableInstanceSlot {
                offset: 4,
                target: TableSlotTarget::Symbol("callback".to_owned()),
            }],
        }],
        ..Scenario::default()
    };

    let result = execute(&image, &empty_svd(), "test", scenario).unwrap();
    assert!(result.table_lifecycle_complete);
    assert!(
        matches!(
            &result.table_lifecycle[..],
            [
                TableLifecycleEvent::SlotInitialized {
                    layout_id: initialized,
                    offset: 4,
                    target: 0x101c,
                },
                TableLifecycleEvent::SlotWritten {
                    layout_id: written,
                    offset: 4,
                    width: 32,
                    value: 0x101c,
                    site: 0x100c,
                },
                TableLifecycleEvent::IndirectCall {
                    layout_id: Some(called),
                    slot_offset: Some(4),
                    site: 0x1010,
                    target: 0x101c,
                    symbol,
                },
            ] if initialized == "services-v2"
                && written == "services-v2"
                && called == "services-v2"
                && symbol == "callback"
        ),
        "{:#?}",
        result.table_lifecycle
    );
}

#[test]
fn runtime_table_instance_rejects_stale_layouts_and_ram_conflicts() {
    let image = tiny_image(vec![0x67, 0x80, 0x00, 0x00], 4);
    let outside_layout = Scenario {
        table_instances: vec![TableInstance {
            layout_id: "services".to_owned(),
            base_address: 0x4000,
            layout_size: 4,
            pointer_cells: Vec::new(),
            slots: vec![TableInstanceSlot {
                offset: 4,
                target: TableSlotTarget::Address(0x1000),
            }],
        }],
        ..Scenario::default()
    };
    assert!(
        execute(&image, &empty_svd(), "test", outside_layout)
            .unwrap_err()
            .to_string()
            .contains("outside its")
    );

    let conflict = Scenario {
        memory_initial: BTreeMap::from([(0x3000, 1)]),
        table_instances: vec![TableInstance {
            layout_id: "services".to_owned(),
            base_address: 0x4000,
            layout_size: 4,
            pointer_cells: vec![0x3000],
            slots: Vec::new(),
        }],
        ..Scenario::default()
    };
    assert!(
        execute(&image, &empty_svd(), "test", conflict)
            .unwrap_err()
            .to_string()
            .contains("conflicts with scenario RAM")
    );
}
