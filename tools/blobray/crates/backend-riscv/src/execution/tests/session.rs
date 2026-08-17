//! Execution-session lifecycle, persistence and ownership regressions.

use super::*;

#[test]
fn ordered_timeline_retains_intermediate_ram_values() {
    let image = tiny_image(vec![0x67, 0x80, 0x00, 0x00], 4);
    let address = 0x3fff_0000;
    let mut scenario = Scenario::default();
    scenario
        .memory_initial
        .extend((0..4).map(|offset| (address + offset, 0)));
    let svd = empty_svd();
    let mut machine = Machine::new(&image, &svd, 0x1000, scenario);

    machine.write(address, 32, 0x1122_3344).unwrap();
    assert_eq!(machine.read(address, 32).unwrap(), 0x1122_3344);
    machine.write(address, 32, 0x5566_7788).unwrap();
    assert_eq!(machine.read(address, 32).unwrap(), 0x5566_7788);

    assert_eq!(
        machine.timeline,
        vec![
            ExecutionTimelineEvent::RamWrite {
                site: 0x1000,
                width: 32,
                address,
                value: 0x1122_3344,
            },
            ExecutionTimelineEvent::RamRead {
                site: 0x1000,
                width: 32,
                address,
                value: 0x1122_3344,
            },
            ExecutionTimelineEvent::RamWrite {
                site: 0x1000,
                width: 32,
                address,
                value: 0x5566_7788,
            },
            ExecutionTimelineEvent::RamRead {
                site: 0x1000,
                width: 32,
                address,
                value: 0x5566_7788,
            },
        ]
    );
}

#[test]
fn execution_session_retains_elf_and_declared_ram_but_not_stack() {
    let mut image = tiny_image(
        vec![
            0x03, 0xa5, 0x05, 0x00, // lw a0, 0(a1)
            0x13, 0x05, 0x15, 0x00, // addi a0, a0, 1
            0x23, 0xa0, 0xa5, 0x00, // sw a0, 0(a1)
            0x67, 0x80, 0x00, 0x00, // ret
            0, 0, 0, 0, // ELF-backed mutable word
            0x13, 0x01, 0xc1, 0xff, // stack_writer: addi sp, sp, -4
            0x23, 0x20, 0xa1, 0x00, // stack_writer: sw a0, 0(sp)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        32,
    );
    image
        .symbols_by_name
        .insert("stack_writer".to_owned(), 0x1014);
    image
        .symbols_by_address
        .insert(0x1014, "stack_writer".to_owned());
    let svd = empty_svd();
    let mut session = ExecutionSession::default();

    for expected in [1, 2] {
        let scenario = Scenario {
            arguments: vec![0, 0x1010],
            ..Scenario::default()
        };
        let result = session.execute(&image, &svd, "test", scenario).unwrap();
        assert_eq!(result.return_value, expected);
    }
    assert_eq!(session.byte(&image, 0x1010), Some(2));

    let external = 0x2000;
    let first = Scenario {
        arguments: vec![0, external],
        memory_initial: (0..4).map(|offset| (external + offset, 0)).collect(),
        persistent_memory: vec![MemoryRange {
            start: external,
            length: 4,
        }],
        ..Scenario::default()
    };
    let first = session.execute(&image, &svd, "test", first).unwrap();
    assert_eq!(first.return_value, 1);
    assert_eq!(first.explicit_memory.len(), 4);
    assert!(!first.carried_memory.contains_key(&external));
    let second = Scenario {
        arguments: vec![0, external],
        ..Scenario::default()
    };
    let second = session.execute(&image, &svd, "test", second).unwrap();
    assert_eq!(second.return_value, 2);
    assert!(second.explicit_memory.is_empty());
    assert_eq!(second.carried_memory.get(&external), Some(&1));

    let stack = session
        .execute(
            &image,
            &svd,
            "stack_writer",
            Scenario {
                arguments: vec![0xdead_beef],
                ..Scenario::default()
            },
        )
        .unwrap();
    assert!(
        stack
            .persistent_memory
            .keys()
            .all(|address| !execution_stack_contains(*address))
    );
}

#[test]
fn stateful_vendor_rust_pair_exposes_a_second_event_divergence() {
    let vendor = tiny_image(
        vec![
            0x03, 0xa5, 0x05, 0x00, // lw a0, 0(a1)
            0x13, 0x05, 0x15, 0x00, // addi a0, a0, 1
            0x23, 0xa0, 0xa5, 0x00, // sw a0, 0(a1)
            0x67, 0x80, 0x00, 0x00, // ret
            0, 0, 0, 0, // persistent ELF-backed counter
        ],
        20,
    );
    let rust = tiny_image(
        vec![
            0x13, 0x05, 0x10, 0x00, // addi a0, zero, 1
            0x23, 0xa0, 0xa5, 0x00, // sw a0, 0(a1)
            0x67, 0x80, 0x00, 0x00, // ret
            0x13, 0x00, 0x00, 0x00, // nop (same data address as vendor)
            0, 0, 0, 0, // persistent ELF-backed counter
        ],
        20,
    );
    let mut vendor_session = ExecutionSession::default();
    let mut rust_session = ExecutionSession::default();
    let svd = empty_svd();
    let mut outcomes = Vec::new();

    for _ in 0..2 {
        let scenario = || Scenario {
            arguments: vec![0, 0x1010],
            observed_memory: vec![MemoryRange {
                start: 0x1010,
                length: 4,
            }],
            ..Scenario::default()
        };
        let vendor_result = vendor_session
            .execute(&vendor, &svd, "test", scenario())
            .unwrap();
        let rust_result = rust_session
            .execute(&rust, &svd, "test", scenario())
            .unwrap();
        outcomes.push(
            vendor_result.return_value == rust_result.return_value
                && vendor_result.memory_changes == rust_result.memory_changes,
        );
    }

    assert_eq!(outcomes, [true, false]);
    assert_eq!(vendor_session.byte(&vendor, 0x1010), Some(2));
    assert_eq!(rust_session.byte(&rust, 0x1010), Some(1));
}

#[test]
fn execution_session_invalidates_externally_mutable_ram_between_calls() {
    let image = tiny_image(
        vec![
            0x03, 0xa5, 0x05, 0x00, // lw a0, 0(a1)
            0x13, 0x05, 0x15, 0x00, // addi a0, a0, 1
            0x23, 0xa0, 0xa5, 0x00, // sw a0, 0(a1)
            0x67, 0x80, 0x00, 0x00, // ret
            0, 0, 0, 0,
        ],
        20,
    );
    let range = MemoryRange {
        start: 0x1010,
        length: 4,
    };
    let ownership = MemoryOwnership {
        range,
        owner: MemoryOwner::SharedUnknown,
    };
    let mut session = ExecutionSession::default();
    let unseeded = Scenario {
        arguments: vec![0, range.start],
        memory_ownership: vec![ownership],
        ..Scenario::default()
    };
    let error = session
        .execute(&image, &empty_svd(), "test", unseeded)
        .unwrap_err();
    assert!(error.to_string().contains("externally mutable RAM"));

    let seeded = Scenario {
        arguments: vec![0, range.start],
        memory_initial: (0..4)
            .map(|offset| (range.start + offset, u8::from(offset == 0) * 9))
            .collect(),
        ..Scenario::default()
    };
    assert_eq!(
        session
            .execute(&image, &empty_svd(), "test", seeded)
            .unwrap()
            .return_value,
        10
    );
    assert_eq!(session.byte(&image, range.start), None);
}

#[test]
fn execution_session_distinguishes_cold_and_warm_reset() {
    let image = tiny_image(
        vec![
            0x03, 0xa5, 0x05, 0x00, // lw a0, 0(a1)
            0x13, 0x05, 0x15, 0x00, // addi a0, a0, 1
            0x23, 0xa0, 0xa5, 0x00, // sw a0, 0(a1)
            0x67, 0x80, 0x00, 0x00, // ret
            0, 0, 0, 0,
        ],
        20,
    );
    let mut session = ExecutionSession::default();
    for (reset_policy, expected) in [
        (ResetPolicy::Continue, 1),
        (ResetPolicy::Continue, 2),
        (ResetPolicy::ColdBoot, 1),
    ] {
        let result = session
            .execute(
                &image,
                &empty_svd(),
                "test",
                Scenario {
                    arguments: vec![0, 0x1010],
                    reset_policy,
                    ..Scenario::default()
                },
            )
            .unwrap();
        assert_eq!(result.return_value, expected);
    }

    let external = 0x2000;
    let first = Scenario {
        arguments: vec![0, external],
        memory_initial: (0..4).map(|offset| (external + offset, 0)).collect(),
        persistent_memory: vec![MemoryRange {
            start: external,
            length: 4,
        }],
        ..Scenario::default()
    };
    assert_eq!(
        session
            .execute(&image, &empty_svd(), "test", first)
            .unwrap()
            .return_value,
        1
    );
    let warm = Scenario {
        arguments: vec![0, external],
        reset_policy: ResetPolicy::WarmReset,
        ..Scenario::default()
    };
    assert_eq!(
        session
            .execute(&image, &empty_svd(), "test", warm)
            .unwrap()
            .return_value,
        2
    );
    assert_eq!(session.byte(&image, 0x1010), Some(0));
}

#[test]
fn ownership_conflicts_and_immutable_writes_fail_closed() {
    let image = tiny_image(
        vec![
            0x23, 0xa0, 0xa5, 0x00, // sw a0, 0(a1)
            0x67, 0x80, 0x00, 0x00, // ret
            0, 0, 0, 0,
        ],
        12,
    );
    let range = MemoryRange {
        start: 0x1008,
        length: 4,
    };
    let immutable = Scenario {
        arguments: vec![1, range.start],
        memory_ownership: vec![MemoryOwnership {
            range,
            owner: MemoryOwner::Immutable,
        }],
        ..Scenario::default()
    };
    let error = execute(&image, &empty_svd(), "test", immutable).unwrap_err();
    assert!(error.to_string().contains("immutable RAM"));

    let mut session = ExecutionSession::default();
    session
        .execute(
            &image,
            &empty_svd(),
            "test",
            Scenario {
                arguments: vec![1, range.start],
                memory_ownership: vec![MemoryOwnership {
                    range,
                    owner: MemoryOwner::Cpu,
                }],
                ..Scenario::default()
            },
        )
        .unwrap();
    let conflict = session
        .execute(
            &image,
            &empty_svd(),
            "test",
            Scenario {
                arguments: vec![1, range.start],
                memory_ownership: vec![MemoryOwnership {
                    range,
                    owner: MemoryOwner::Dma,
                }],
                ..Scenario::default()
            },
        )
        .unwrap_err();
    assert!(conflict.to_string().contains("conflicting RAM ownership"));
}

#[test]
fn multi_phase_replay_delivers_one_fifo_item_across_calls() {
    let mut image = tiny_image(
        vec![
            0x13, 0x84, 0x00, 0x00, // addi s0, ra, 0
            0xef, 0x00, 0x00, 0x01, // jal ra, 16
            0x93, 0x00, 0x04, 0x00, // addi ra, s0, 0
            0x67, 0x80, 0x00, 0x00, // ret
            0, 0, 0, 0, 0x73, 0x00, 0x10, 0x00, // service: ebreak (intercepted)
        ],
        24,
    );
    image.symbols_by_name.insert("service".to_owned(), 0x1014);
    image
        .symbols_by_address
        .insert(0x1014, "service".to_owned());
    let fifo = FifoServiceInstance {
        id: "events".to_owned(),
        handle: 0x2000,
        item_width: 32,
        capacity: 4,
        items: Vec::new(),
    };
    let enqueue = FifoServiceBinding {
        symbol: "service".to_owned(),
        service_id: "events".to_owned(),
        handle_argument: 0,
        operation: FifoServiceOperation::Enqueue {
            item: ServiceValueSource::Argument {
                argument: 1,
                width: 32,
            },
            success_return: 1,
            full_return: 0,
            wake_output: None,
        },
    };
    let dequeue = FifoServiceBinding {
        symbol: "service".to_owned(),
        service_id: "events".to_owned(),
        handle_argument: 0,
        operation: FifoServiceOperation::Dequeue {
            output: ServiceOutput::PrivateStackPointer {
                pointer_argument: 1,
                width: 32,
            },
            success_return: 1,
            empty_return: 0,
        },
    };
    let mut session = ExecutionSession::default();
    let results = session
        .execute_phases(
            &image,
            &empty_svd(),
            vec![
                ExecutionPhase {
                    name: "post".to_owned(),
                    symbol: "test".to_owned(),
                    scenario: Scenario {
                        arguments: vec![0x2000, 0x19],
                        fifo_services: vec![fifo],
                        fifo_bindings: vec![enqueue],
                        ..Scenario::default()
                    },
                },
                ExecutionPhase {
                    name: "receive".to_owned(),
                    symbol: "test".to_owned(),
                    scenario: Scenario {
                        arguments: vec![0x2000, STACK_POINTER - 4],
                        fifo_bindings: vec![dequeue],
                        goal: ExecutionGoal::ObserveFifoDequeue {
                            service_id: "events".to_owned(),
                            value: Some(0x19),
                        },
                        ..Scenario::default()
                    },
                },
            ],
        )
        .unwrap();

    assert_eq!(results.len(), 2);
    assert!(matches!(
        results[0].result.fifo_lifecycle.as_slice(),
        [FifoLifecycleEvent::Enqueued { value: 0x19, .. }]
    ));
    assert!(matches!(
        results[1].result.fifo_lifecycle.as_slice(),
        [FifoLifecycleEvent::Dequeued { value: 0x19, .. }]
    ));
    assert!(results[1].result.fifo_services[0].items.is_empty());
}

#[test]
fn local_libpp_replay_reaches_selector_25_handler_through_modeled_osi_queue() {
    let Some(artifact) = std::env::var_os("OPEN_RADIO_VENDOR_LIBPP_REPLAY_ELF") else {
        return;
    };
    let artifact = std::path::Path::new(&artifact);
    if !artifact.exists() {
        return;
    }
    let image = ExecutableImage::load(artifact).unwrap();
    let osi_pointer = image.symbol_address("g_osi_funcs_p").unwrap();
    let queue_cell = image.symbol_address("xphyQueue").unwrap();
    let create_sem_cell = image.symbol_address("s_pp_task_create_sem").unwrap();
    let critical_lock_cell = image.symbol_address("g_intr_lock_mux").unwrap();
    let signal_counts = image.symbol_address("pp_sig_cnt").unwrap();
    let table_base = 0x3fff_0000;
    let queue_handle = 0x3fff_1000;

    let mut memory_initial = BTreeMap::new();
    seed_test_word(&mut memory_initial, queue_cell, queue_handle);
    seed_test_word(&mut memory_initial, create_sem_cell, 0x3fff_2000);
    seed_test_word(&mut memory_initial, critical_lock_cell, 0x3fff_3000);
    memory_initial.insert(signal_counts + 0x19, 1);
    let modeled_response = |value| VecDeque::from([ModeledCallResponse::scalar(value)]);
    let scenario = Scenario {
        memory_initial,
        table_instances: vec![TableInstance {
            layout_id: "reviewed-platform-services-v9".to_owned(),
            base_address: table_base,
            layout_size: 0x200,
            pointer_cells: vec![osi_pointer],
            pointer_cell_symbols: Vec::new(),
            slots: vec![
                TableInstanceSlot {
                    offset: 0x028,
                    target: TableSlotTarget::ModeledSymbol("wifi_int_disable".to_owned()),
                },
                TableInstanceSlot {
                    offset: 0x02c,
                    target: TableSlotTarget::ModeledSymbol("wifi_int_restore".to_owned()),
                },
                TableInstanceSlot {
                    offset: 0x040,
                    target: TableSlotTarget::ModeledSymbol("semphr_give".to_owned()),
                },
                TableInstanceSlot {
                    offset: 0x074,
                    target: TableSlotTarget::ModeledSymbol("queue_recv".to_owned()),
                },
            ],
        }],
        fifo_services: vec![FifoServiceInstance {
            id: "pp-events".to_owned(),
            handle: queue_handle,
            item_width: 32,
            capacity: 8,
            items: vec![0x19],
        }],
        fifo_bindings: vec![FifoServiceBinding {
            symbol: "queue_recv".to_owned(),
            service_id: "pp-events".to_owned(),
            handle_argument: 0,
            operation: FifoServiceOperation::Dequeue {
                output: ServiceOutput::PrivateStackPointer {
                    pointer_argument: 1,
                    width: 32,
                },
                success_return: 1,
                empty_return: 0,
            },
        }],
        call_responses: BTreeMap::from([
            ("wifi_int_disable".to_owned(), modeled_response(0)),
            ("wifi_int_restore".to_owned(), modeled_response(0)),
            ("semphr_give".to_owned(), modeled_response(1)),
        ]),
        goal: ExecutionGoal::ReachSymbol {
            symbol: "wdevProcessRxSucDataAll".to_owned(),
        },
        max_steps: 256,
        ..Scenario::default()
    };

    let result = execute(&image, &empty_svd(), "ppTask", scenario).unwrap();
    assert!(matches!(
        result.completion,
        ExecutionCompletion::GoalReached(ExecutionGoal::ReachSymbol { ref symbol })
            if symbol == "wdevProcessRxSucDataAll"
    ));
    assert!(matches!(
        result.fifo_lifecycle.as_slice(),
        [FifoLifecycleEvent::Dequeued {
            service_id,
            value: 0x19,
            ..
        }] if service_id == "pp-events"
    ));
    assert!(
        result
            .ordered_calls
            .iter()
            .any(|call| { call.symbol == "queue_recv" && call.arguments[0] == queue_handle })
    );
    assert_eq!(
        result
            .persistent_memory
            .get(&(signal_counts + 0x19))
            .copied(),
        Some(0),
        "the dispatch consumes the latched signal count before entering the handler"
    );
}

#[test]
fn local_libpp_replay_carries_signal_25_from_pp_post_to_pp_task_handler() {
    let Some(artifact) = std::env::var_os("OPEN_RADIO_VENDOR_LIBPP_REPLAY_ELF") else {
        return;
    };
    let artifact = std::path::Path::new(&artifact);
    if !artifact.exists() {
        return;
    }
    let image = ExecutableImage::load(artifact).unwrap();
    let osi_pointer = image.symbol_address("g_osi_funcs_p").unwrap();
    let queue_cell = image.symbol_address("xphyQueue").unwrap();
    let create_sem_cell = image.symbol_address("s_pp_task_create_sem").unwrap();
    let critical_lock_cell = image.symbol_address("g_intr_lock_mux").unwrap();
    let signal_counts = image.symbol_address("pp_sig_cnt").unwrap();
    let table_base = 0x3fff_0000;
    let queue_handle = 0x3fff_1000;

    let mut producer_memory = BTreeMap::new();
    seed_test_word(&mut producer_memory, queue_cell, queue_handle);
    seed_test_word(&mut producer_memory, create_sem_cell, 0x3fff_2000);
    seed_test_word(&mut producer_memory, critical_lock_cell, 0x3fff_3000);
    let response = |value| VecDeque::from([ModeledCallResponse::scalar(value)]);
    let mut session = ExecutionSession::default();
    let phases = session
        .execute_phases(
            &image,
            &empty_svd(),
            vec![
                ExecutionPhase {
                    name: "post-signal-25".to_owned(),
                    symbol: "pp_post".to_owned(),
                    scenario: Scenario {
                        arguments: vec![0x19, 0],
                        memory_initial: producer_memory,
                        table_instances: vec![libpp_osi_table(
                            osi_pointer,
                            table_base,
                            &[
                                (0x028, "wifi_int_disable"),
                                (0x02c, "wifi_int_restore"),
                                (0x030, "task_yield_from_isr"),
                                (0x068, "queue_send_from_isr"),
                            ],
                        )],
                        fifo_services: vec![FifoServiceInstance {
                            id: "pp-events".to_owned(),
                            handle: queue_handle,
                            item_width: 32,
                            capacity: 8,
                            items: Vec::new(),
                        }],
                        fifo_bindings: vec![FifoServiceBinding {
                            symbol: "queue_send_from_isr".to_owned(),
                            service_id: "pp-events".to_owned(),
                            handle_argument: 0,
                            operation: FifoServiceOperation::Enqueue {
                                item: ServiceValueSource::PrivateStackPointer {
                                    pointer_argument: 1,
                                    width: 32,
                                },
                                success_return: 1,
                                full_return: 0,
                                wake_output: Some(ServiceOutput::PrivateStackPointer {
                                    pointer_argument: 2,
                                    width: 8,
                                }),
                            },
                        }],
                        call_responses: BTreeMap::from([
                            ("wifi_int_disable".to_owned(), response(0)),
                            ("wifi_int_restore".to_owned(), response(0)),
                            ("task_yield_from_isr".to_owned(), response(0)),
                        ]),
                        max_steps: 256,
                        ..Scenario::default()
                    },
                },
                ExecutionPhase {
                    name: "dispatch-signal-25".to_owned(),
                    symbol: "ppTask".to_owned(),
                    scenario: Scenario {
                        table_instances: vec![libpp_osi_table(
                            osi_pointer,
                            table_base,
                            &[
                                (0x028, "wifi_int_disable"),
                                (0x02c, "wifi_int_restore"),
                                (0x040, "semphr_give"),
                                (0x074, "queue_recv"),
                            ],
                        )],
                        fifo_bindings: vec![FifoServiceBinding {
                            symbol: "queue_recv".to_owned(),
                            service_id: "pp-events".to_owned(),
                            handle_argument: 0,
                            operation: FifoServiceOperation::Dequeue {
                                output: ServiceOutput::PrivateStackPointer {
                                    pointer_argument: 1,
                                    width: 32,
                                },
                                success_return: 1,
                                empty_return: 0,
                            },
                        }],
                        call_responses: BTreeMap::from([
                            ("wifi_int_disable".to_owned(), response(0)),
                            ("wifi_int_restore".to_owned(), response(0)),
                            ("semphr_give".to_owned(), response(1)),
                        ]),
                        goal: ExecutionGoal::ReachSymbol {
                            symbol: "wdevProcessRxSucDataAll".to_owned(),
                        },
                        max_steps: 256,
                        ..Scenario::default()
                    },
                },
            ],
        )
        .unwrap();

    assert!(matches!(
        phases[0].result.fifo_lifecycle.as_slice(),
        [FifoLifecycleEvent::Enqueued {
            value: 0x19,
            depth_before: 0,
            depth_after: 1,
            woke_receiver: true,
            ..
        }]
    ));
    assert_eq!(
        session.byte(&image, signal_counts + 0x19),
        Some(0),
        "pp_post increments and ppTask consumes the same signal latch"
    );
    assert!(matches!(
        phases[1].result.completion,
        ExecutionCompletion::GoalReached(ExecutionGoal::ReachSymbol { ref symbol })
            if symbol == "wdevProcessRxSucDataAll"
    ));
    assert!(matches!(
        phases[1].result.fifo_lifecycle.as_slice(),
        [FifoLifecycleEvent::Dequeued { value: 0x19, .. }]
    ));
}

fn libpp_osi_table(pointer_cell: u32, base_address: u32, slots: &[(u32, &str)]) -> TableInstance {
    TableInstance {
        layout_id: "reviewed-platform-services-v9".to_owned(),
        base_address,
        layout_size: 0x200,
        pointer_cells: vec![pointer_cell],
        pointer_cell_symbols: Vec::new(),
        slots: slots
            .iter()
            .map(|(offset, symbol)| TableInstanceSlot {
                offset: *offset,
                target: TableSlotTarget::ModeledSymbol((*symbol).to_owned()),
            })
            .collect(),
    }
}

fn seed_test_word(memory: &mut BTreeMap<u32, u8>, address: u32, value: u32) {
    memory.extend(
        value
            .to_le_bytes()
            .into_iter()
            .enumerate()
            .map(|(offset, byte)| (address + offset as u32, byte)),
    );
}
