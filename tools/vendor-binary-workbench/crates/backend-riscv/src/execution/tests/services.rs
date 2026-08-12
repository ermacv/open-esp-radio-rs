//! Stateful external FIFO service regressions.

use super::*;

fn fifo(id: &str, items: Vec<u32>) -> FifoServiceInstance {
    FifoServiceInstance {
        id: id.to_owned(),
        handle: 0x2000,
        item_width: 32,
        capacity: 2,
        items,
    }
}

fn binding(symbol: &str, operation: FifoServiceOperation) -> FifoServiceBinding {
    FifoServiceBinding {
        symbol: symbol.to_owned(),
        service_id: "events".to_owned(),
        handle_argument: 0,
        operation,
    }
}

#[test]
fn fifo_enqueue_and_dequeue_preserve_item_identity() {
    let image = tiny_image(vec![0x67, 0x80, 0x00, 0x00], 4);
    let svd = empty_svd();
    let input = STACK_POINTER - 8;
    let output = STACK_POINTER - 12;
    let mut scenario = Scenario {
        fifo_services: vec![fifo("events", Vec::new())],
        fifo_bindings: vec![
            binding(
                "send",
                FifoServiceOperation::Enqueue {
                    item: ServiceValueSource::PrivateStackPointer {
                        pointer_argument: 1,
                        width: 32,
                    },
                    success_return: 1,
                    full_return: 0,
                    wake_output: Some(ServiceOutput::PrivateStackPointer {
                        pointer_argument: 2,
                        width: 32,
                    }),
                },
            ),
            binding(
                "receive",
                FifoServiceOperation::Dequeue {
                    output: ServiceOutput::PrivateStackPointer {
                        pointer_argument: 1,
                        width: 32,
                    },
                    success_return: 1,
                    empty_return: 0,
                },
            ),
        ],
        ..Scenario::default()
    };
    for (offset, byte) in 0x19_u32.to_le_bytes().into_iter().enumerate() {
        scenario.memory_initial.insert(input + offset as u32, byte);
    }
    let mut machine = Machine::new(&image, &svd, 0x1000, scenario);

    machine.set_register(rv_asm::Reg::A0, 0x2000);
    machine.set_register(rv_asm::Reg::A1, input);
    machine.set_register(rv_asm::Reg::A2, output);
    assert!(machine.apply_fifo_service_call("send", 0x1000).unwrap());
    assert_eq!(machine.register(rv_asm::Reg::A0), 1);
    assert_eq!(machine.read(output, 32).unwrap(), 1);

    machine.set_register(rv_asm::Reg::A0, 0x2000);
    machine.set_register(rv_asm::Reg::A1, output);
    assert!(machine.apply_fifo_service_call("receive", 0x1004).unwrap());
    assert_eq!(machine.register(rv_asm::Reg::A0), 1);
    assert_eq!(machine.read(output, 32).unwrap(), 0x19);
    assert!(matches!(
        machine.fifo_lifecycle.as_slice(),
        [
            FifoLifecycleEvent::Enqueued {
                value: 0x19,
                woke_receiver: true,
                ..
            },
            FifoLifecycleEvent::Dequeued { value: 0x19, .. }
        ]
    ));
}

#[test]
fn fifo_full_and_empty_fail_without_inventing_items() {
    let image = tiny_image(vec![0x67, 0x80, 0x00, 0x00], 4);
    let svd = empty_svd();
    let mut scenario = Scenario {
        fifo_services: vec![FifoServiceInstance {
            capacity: 1,
            ..fifo("events", vec![7])
        }],
        fifo_bindings: vec![binding(
            "send",
            FifoServiceOperation::Enqueue {
                item: ServiceValueSource::Argument {
                    argument: 1,
                    width: 32,
                },
                success_return: 1,
                full_return: 0,
                wake_output: None,
            },
        )],
        ..Scenario::default()
    };
    let mut machine = Machine::new(&image, &svd, 0x1000, scenario.clone());
    machine.set_register(rv_asm::Reg::A0, 0x2000);
    machine.set_register(rv_asm::Reg::A1, 9);
    machine.apply_fifo_service_call("send", 0x1000).unwrap();
    assert_eq!(machine.register(rv_asm::Reg::A0), 0);
    assert_eq!(machine.fifo_services[0].items, vec![7]);
    assert!(matches!(
        machine.fifo_lifecycle.as_slice(),
        [FifoLifecycleEvent::Full { value: 9, .. }]
    ));

    scenario.fifo_services[0].items.clear();
    scenario.fifo_bindings = vec![binding(
        "receive",
        FifoServiceOperation::Dequeue {
            output: ServiceOutput::PrivateStackPointer {
                pointer_argument: 1,
                width: 32,
            },
            success_return: 1,
            empty_return: 0,
        },
    )];
    let mut machine = Machine::new(&image, &svd, 0x1000, scenario);
    machine.set_register(rv_asm::Reg::A0, 0x2000);
    machine.set_register(rv_asm::Reg::A1, STACK_POINTER - 4);
    machine.apply_fifo_service_call("receive", 0x1000).unwrap();
    assert_eq!(machine.register(rv_asm::Reg::A0), 0);
    assert!(matches!(
        machine.fifo_lifecycle.as_slice(),
        [FifoLifecycleEvent::Empty { .. }]
    ));
}

#[test]
fn fifo_configuration_fails_closed_before_execution() {
    let image = tiny_image(vec![0x67, 0x80, 0x00, 0x00], 4);
    let svd = empty_svd();
    let scenario = Scenario {
        fifo_services: vec![FifoServiceInstance {
            item_width: 8,
            ..fifo("events", vec![0x100])
        }],
        fifo_bindings: vec![binding(
            "send",
            FifoServiceOperation::Enqueue {
                item: ServiceValueSource::Argument {
                    argument: 1,
                    width: 32,
                },
                success_return: 1,
                full_return: 0,
                wake_output: None,
            },
        )],
        ..Scenario::default()
    };

    let error = execute(&image, &svd, "test", scenario).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("initial value 0x00000100 exceeds its 8-bit item width")
    );
}

#[test]
fn fifo_binding_cannot_compete_with_scripted_call_response() {
    let image = tiny_image(vec![0x67, 0x80, 0x00, 0x00], 4);
    let svd = empty_svd();
    let scenario = Scenario {
        fifo_services: vec![fifo("events", Vec::new())],
        fifo_bindings: vec![binding("length", FifoServiceOperation::Len)],
        call_responses: BTreeMap::from([("length".to_owned(), VecDeque::new())]),
        ..Scenario::default()
    };

    let error = execute(&image, &svd, "test", scenario).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("conflicts with a scripted call response")
    );
}
