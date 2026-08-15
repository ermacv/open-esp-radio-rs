//! Symbolic-value, read-token and branch-expression rendering.

use super::*;

#[test]
fn groups_shifted_argument_bits_into_a_readable_expression() {
    let value = SymbolicValue::input(0).and(1).shift_left(5);
    assert_eq!(
        render_value(&value, &[], &[], 0).unwrap(),
        "(args[0] << 5) & 0x00000020_u32"
    );
}

#[test]
fn validates_the_address_behind_a_read_token() {
    let value = SymbolicValue::RegisterImage {
        read_token: 0,
        address: 0x2010_7030,
        and_mask: u32::MAX,
        or_mask: 0,
    };
    assert!(render_value(&value, &[0x2010_7030], &[], 0).is_ok());
    assert!(render_value(&value, &[0x2010_7034], &[], 0).is_err());
}

#[test]
fn distinguishes_static_and_indexed_read_tokens() {
    let value = SymbolicValue::IndexedRegisterImage {
        read_token: 0,
        and_mask: u32::MAX,
        or_mask: 0,
    };
    let arguments = core::array::from_fn(|index| format!("args[{index}]"));

    assert!(
        render_value_scoped(&value, &[MmioReadAddress::Indexed], 0, &[], 0, &arguments,).is_ok()
    );
    assert!(
        render_value_scoped(
            &value,
            &[MmioReadAddress::Static(0x2010_7030)],
            0,
            &[],
            0,
            &arguments,
        )
        .is_err()
    );
}

#[test]
fn renders_external_results_through_exact_riscv_arithmetic() {
    let value = SymbolicValue::expression(
        crate::ExpressionOperation::RemainderUnsigned,
        SymbolicValue::ExternalResult(0),
        SymbolicValue::Constant(11),
    )
    .add_constant(0xfa)
    .shift_left(21);

    let rendered = render_value(&value, &[], &[], 1).unwrap();
    assert!(rendered.contains("riscv_remu(external_result0, 0x0000000b_u32)"));
    assert!(rendered.contains("wrapping_add(0x000000fa_u32)"));
    assert!(rendered.contains("wrapping_shl"));
    assert!(render_value(&value, &[], &[], 0).is_err());
}

#[test]
fn renders_dynamic_arithmetic_shift_with_rv32_masking() {
    let value = SymbolicValue::expression(
        crate::ExpressionOperation::ShiftRightArithmetic,
        SymbolicValue::Constant((-0x81_i32) as u32),
        SymbolicValue::input(0),
    );

    assert_eq!(
        render_value(&value, &[], &[], 0).unwrap(),
        "((0xffffff7f_u32) as i32).wrapping_shr((args[0] & 0xffffffff_u32) & 31) as u32"
    );
}

#[test]
fn signed_branch_casts_the_complete_rendered_expression() {
    let condition = BranchCondition {
        site: 0,
        operation: BranchOperation::LessSigned,
        left: SymbolicValue::input(1),
        right: SymbolicValue::Constant(0),
    };

    assert_eq!(
        render_condition(&condition, &RenderState::default()).unwrap(),
        "((args[1] & 0xffffffff_u32) as i32) < ((0x00000000_u32) as i32)"
    );
}

#[test]
fn rejects_structural_floating_value_without_architectural_execution_model() {
    let value = SymbolicValue::floating_point(
        crate::FloatingPointOperation::SignedWordToSingle,
        crate::FloatingRoundingMode::Dynamic,
        vec![SymbolicValue::input(0)],
    );

    let error = render_value(&value, &[], &[], 0).unwrap_err();
    assert!(error.contains("SignedWordToSingle"), "{error}");
    assert!(error.contains("Dynamic"), "{error}");
    assert!(error.contains("no executable reference model"), "{error}");
}
