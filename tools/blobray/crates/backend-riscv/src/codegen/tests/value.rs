//! Symbolic-value, read-token and branch-expression rendering.

use super::*;

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
fn validates_external_result_availability() {
    let value = SymbolicValue::expression(
        crate::ExpressionOperation::RemainderUnsigned,
        SymbolicValue::ExternalResult(0),
        SymbolicValue::Constant(11),
    )
    .add_constant(0xfa)
    .shift_left(21);

    assert!(render_value(&value, &[], &[], 1).is_ok());
    assert!(render_value(&value, &[], &[], 0).is_err());
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
