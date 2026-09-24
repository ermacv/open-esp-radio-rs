//! Fixed-depth expression observations, never physical field inference.
use crate::navigation::Facts;
use blobray_domain::*;

fn constant(v: &AbstractValue) -> Option<u32> {
    if let AbstractValue::Constant { value } = v {
        Some(*value)
    } else {
        None
    }
}
fn masked(expression: &Expression) -> Option<(&AbstractValue, u32)> {
    if let Expression::Integer {
        op: IntegerOp::And,
        left,
        right,
    } = expression
    {
        constant(right)
            .map(|mask| (left, mask))
            .or_else(|| constant(left).map(|mask| (right, mask)))
    } else {
        None
    }
}
fn width_mask(width: u8) -> Option<u32> {
    match width {
        1 => Some(0xff),
        2 => Some(0xffff),
        4 => Some(u32::MAX),
        _ => None,
    }
}
/// Recognize `(load >> shift) & mask` or `load & mask` and retain source bit positions.
/// Signed extension bits beyond the load width are excluded.
pub fn read_mask<'a>(
    facts: &Facts<'a, '_>,
    expression: &'a Expression,
) -> Option<(&'a AbstractValue, u8, RegisterMask)> {
    let (mut value, bits) = masked(expression)?;
    let mut shift = 0;
    if let Some(Expression::Integer {
        op: IntegerOp::Shr | IntegerOp::Sar,
        left,
        right,
    }) = facts.value_expression(value)
    {
        shift = constant(right)? & 31;
        value = left;
    }
    if let Some(Expression::Load { address, width, .. }) = facts.value_expression(value) {
        let bits = bits.wrapping_shl(shift) & width_mask(*width)?;
        (bits != 0).then_some((
            address,
            *width,
            RegisterMask {
                kind: RegisterMaskKind::ReadSelection,
                bits,
            },
        ))
    } else {
        None
    }
}

/// Recognize a same-address/width load-preserve-OR store. The result includes
/// every bit the replacement may set; unknown replacement conservatively covers all bits.
pub fn write_mask(
    facts: &Facts<'_, '_>,
    address: &AbstractValue,
    width: u8,
    value: &AbstractValue,
) -> Option<RegisterMask> {
    let mut expression = facts.value_expression(value)?;
    if let Some((value, mask)) = masked(expression)
        && mask == width_mask(width)?
    {
        expression = facts.value_expression(value)?;
    }
    let Expression::Integer {
        op: IntegerOp::Or,
        left,
        right,
    } = expression
    else {
        return None;
    };
    for (preserved, replacement) in [(left, right), (right, left)] {
        let Some(e) = facts.value_expression(preserved) else {
            continue;
        };
        let Some((load, preserve)) = masked(e) else {
            continue;
        };
        if let Some(Expression::Load {
            address: original,
            width: original_width,
            ..
        }) = facts.value_expression(load)
            && address == original
            && width == *original_width
        {
            return Some(RegisterMask {
                kind: RegisterMaskKind::WriteReplacement,
                bits: (!preserve | constant(replacement).unwrap_or(u32::MAX)) & width_mask(width)?,
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    fn value(id: u32) -> AbstractValue {
        AbstractValue::Expression { id }
    }
    fn number(value: u32) -> AbstractValue {
        AbstractValue::Constant { value }
    }
    fn records(expressions: Vec<Expression>) -> Vec<FunctionRecord> {
        expressions
            .into_iter()
            .enumerate()
            .map(|(id, expression)| FunctionRecord::Expression {
                id: id as u32,
                offset: id as u64 * 4,
                origin: None,
                expression,
            })
            .collect()
    }
    #[test]
    fn masks_preserve_source_bit_positions_and_never_shrink_unknown_replacement() {
        let memory = WorkingMemory::new(65536).unwrap();
        let records = records(vec![
            Expression::Load {
                address: number(0x20000),
                width: 2,
                signed: true,
            },
            Expression::Integer {
                op: IntegerOp::Shr,
                left: value(0),
                right: number(4),
            },
            Expression::Integer {
                op: IntegerOp::And,
                left: value(1),
                right: number(15),
            },
            Expression::Integer {
                op: IntegerOp::And,
                left: value(0),
                right: number(!15),
            },
            Expression::Integer {
                op: IntegerOp::Or,
                left: value(3),
                right: AbstractValue::Unknown,
            },
            Expression::Integer {
                op: IntegerOp::Or,
                left: value(3),
                right: number(0x100),
            },
        ]);
        let facts = Facts::new(&records, &memory, &mut || Ok(())).unwrap();
        let Expression::Integer { .. } = facts.expression(2).unwrap() else {
            panic!()
        };
        let (_, width, mask) = read_mask(&facts, facts.expression(2).unwrap()).unwrap();
        assert_eq!((width, mask.bits), (2, 0xf0));
        assert_eq!(
            write_mask(&facts, &number(0x20000), 2, &value(4))
                .unwrap()
                .bits,
            0xffff
        );
        assert_eq!(
            write_mask(&facts, &number(0x20000), 2, &value(5))
                .unwrap()
                .bits,
            0x10f
        );
        assert!(write_mask(&facts, &number(0x20001), 2, &value(5)).is_none());
        assert!(write_mask(&facts, &number(0x20000), 4, &value(5)).is_none());
    }
}
