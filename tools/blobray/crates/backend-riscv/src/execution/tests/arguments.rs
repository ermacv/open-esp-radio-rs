use super::*;

#[test]
fn scalar_stack_arguments_survive_callee_frame_allocation() {
    let code = [
        0xff01_0113_u32, // addi sp, sp, -16
        0x0101_2283,     // lw t0, 16(sp): ninth argument
        0x0141_2303,     // lw t1, 20(sp): tenth argument
        0x0181_2383,     // lw t2, 24(sp): eleventh argument
        0x0115_0533,     // add a0, a0, a7
        0x0055_4533,     // xor a0, a0, t0
        0x0065_0533,     // add a0, a0, t1
        0x0075_4533,     // xor a0, a0, t2
        0x0101_0113,     // addi sp, sp, 16
        0x0000_8067,     // ret
    ];
    let image = tiny_image(code.into_iter().flat_map(u32::to_le_bytes).collect(), 40);
    for fill in [None, Some(0x5a), Some(0xa5)] {
        let result = execute(
            &image,
            &empty_svd(),
            "test",
            Scenario {
                arguments: vec![10, 11, 12, 13, 14, 15, 16, 20, 0x8000_0000, 3, 0xffff_ffff],
                private_stack_fill: fill,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            result.return_value,
            ((30_u32 ^ 0x8000_0000) + 3) ^ 0xffff_ffff
        );
        assert!(
            result.persistent_memory.is_empty(),
            "entry stack must not leak between sessions"
        );
        assert!(result.events.is_empty());
    }
}

#[test]
fn entry_stack_remains_aligned_for_every_supported_argument_count() {
    let image = tiny_image(vec![0x13, 0x75, 0xf1, 0x00, 0x67, 0x80, 0, 0], 8); // andi a0,sp,15; ret
    for count in 0..=72 {
        let result = execute(
            &image,
            &empty_svd(),
            "test",
            Scenario {
                arguments: vec![u32::MAX; count],
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(result.return_value, 0, "argument count {count}");
    }
}

#[test]
fn last_supported_stack_word_is_passed_without_truncation() {
    let image = tiny_image(vec![0x03, 0x25, 0xc1, 0x0f, 0x67, 0x80, 0, 0], 8); // lw a0,252(sp); ret
    let mut arguments = vec![0; 72];
    arguments[71] = 0x89ab_cdef;
    let result = execute(
        &image,
        &empty_svd(),
        "test",
        Scenario {
            arguments,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(result.return_value, 0x89ab_cdef);
}

#[test]
fn conflicting_stack_seed_and_missing_argument_fail_closed() {
    let image = tiny_image(vec![0x03, 0x25, 0x81, 0x00, 0x67, 0x80, 0, 0], 8); // lw a0,8(sp); ret
    let mut scenario = Scenario {
        arguments: vec![0; 9],
        ..Default::default()
    };
    scenario.memory_initial.insert(STACK_POINTER - 16, 1);
    assert!(
        execute(&image, &empty_svd(), "test", scenario)
            .unwrap_err()
            .to_string()
            .contains("stack argument conflicts")
    );
    let scenario = Scenario {
        arguments: vec![0; 9],
        ..Default::default()
    };
    assert!(
        execute(&image, &empty_svd(), "test", scenario)
            .unwrap_err()
            .to_string()
            .contains("poison/unmapped")
    );
}
