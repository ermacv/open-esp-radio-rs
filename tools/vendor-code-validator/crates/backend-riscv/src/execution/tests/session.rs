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
                width: 32,
                address,
                value: 0x1122_3344,
            },
            ExecutionTimelineEvent::RamRead {
                width: 32,
                address,
                value: 0x1122_3344,
            },
            ExecutionTimelineEvent::RamWrite {
                width: 32,
                address,
                value: 0x5566_7788,
            },
            ExecutionTimelineEvent::RamRead {
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
    assert_eq!(
        session
            .execute(&image, &svd, "test", first)
            .unwrap()
            .return_value,
        1
    );
    let second = Scenario {
        arguments: vec![0, external],
        ..Scenario::default()
    };
    assert_eq!(
        session
            .execute(&image, &svd, "test", second)
            .unwrap()
            .return_value,
        2
    );

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
