//! Fixed-depth expression observations, never physical field inference.
use crate::navigation::Facts;
use blobray_domain::*;
use oer_riscv_model::*;

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

/// One bit of a stored value: fixed, an entry register's bit, a loaded
/// value's bit, or unknown.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Bit<'a> {
    Zero,
    One,
    Entry(u8, u8),
    Load(&'a AbstractValue, u8, u8),
    Unknown,
}

/// The deepest expression chain followed; deeper values are unknown.
const DEPTH: u8 = 24;

fn constant_bits<'a>(value: u32) -> [Bit<'a>; 32] {
    std::array::from_fn(|i| {
        if value >> i & 1 == 1 {
            Bit::One
        } else {
            Bit::Zero
        }
    })
}

fn and<'a>(a: Bit<'a>, b: Bit<'a>) -> Bit<'a> {
    match (a, b) {
        (Bit::Zero, _) | (_, Bit::Zero) => Bit::Zero,
        (Bit::One, x) | (x, Bit::One) => x,
        (x, y) if x == y && x != Bit::Unknown => x,
        _ => Bit::Unknown,
    }
}

fn or<'a>(a: Bit<'a>, b: Bit<'a>) -> Bit<'a> {
    match (a, b) {
        (Bit::One, _) | (_, Bit::One) => Bit::One,
        (Bit::Zero, x) | (x, Bit::Zero) => x,
        (x, y) if x == y && x != Bit::Unknown => x,
        _ => Bit::Unknown,
    }
}

fn not(a: Bit<'_>) -> Bit<'_> {
    match a {
        Bit::Zero => Bit::One,
        Bit::One => Bit::Zero,
        _ => Bit::Unknown,
    }
}

fn xor<'a>(a: Bit<'a>, b: Bit<'a>) -> Bit<'a> {
    match (a, b) {
        (Bit::Zero, x) | (x, Bit::Zero) => x,
        (Bit::One, x) | (x, Bit::One) => not(x),
        _ => Bit::Unknown,
    }
}

/// `left + right` while no bit position can carry; from the first position
/// where both may be set, every higher bit is unknown.
fn add<'a>(left: [Bit<'a>; 32], right: [Bit<'a>; 32]) -> [Bit<'a>; 32] {
    let mut carry = false;
    std::array::from_fn(|i| {
        if carry {
            return Bit::Unknown;
        }
        match (left[i], right[i]) {
            (Bit::Zero, x) | (x, Bit::Zero) => x,
            _ => {
                carry = true;
                Bit::Unknown
            }
        }
    })
}

fn shift_left(bits: [Bit<'_>; 32], by: u32) -> [Bit<'_>; 32] {
    std::array::from_fn(|i| {
        if (i as u32) < by {
            Bit::Zero
        } else {
            bits[i - by as usize]
        }
    })
}

/// Bit-level sources of stored values over one function's expressions.
///
/// Expressions form a DAG whose shared nodes the analysis consolidates, so each
/// node is evaluated once per function and memoized by its id; every newly
/// evaluated node is charged one work unit, so the walk stays inside the
/// caller's budget and deadline.
pub struct StoredBitsEvaluator<'f, 'a, 'm> {
    facts: &'f Facts<'a, 'm>,
    /// Each expression's bits with the depth budget they were computed under.
    memo: std::collections::BTreeMap<u32, (u8, [Bit<'a>; 32])>,
}

impl<'f, 'a, 'm> StoredBitsEvaluator<'f, 'a, 'm> {
    pub fn new(facts: &'f Facts<'a, 'm>) -> Self {
        Self {
            facts,
            memo: std::collections::BTreeMap::new(),
        }
    }

    fn bits(
        &mut self,
        value: &'a AbstractValue,
        depth: u8,
        c: &mut dyn RunControl,
    ) -> Result<[Bit<'a>; 32]> {
        if depth == 0 {
            return Ok([Bit::Unknown; 32]);
        }
        let (id, expression) = match value {
            AbstractValue::Constant { value } | AbstractValue::ImageAddress { address: value } => {
                return Ok(constant_bits(*value));
            }
            AbstractValue::Expression { id } => match self.facts.value_expression(value) {
                Some(expression) => (*id, expression),
                None => return Ok([Bit::Unknown; 32]),
            },
            _ => return Ok([Bit::Unknown; 32]),
        };
        // A result computed with at least this much depth is reused: it is
        // never less exact than a fresh, shallower evaluation.
        if let Some((computed, bits)) = self.memo.get(&id)
            && *computed >= depth
        {
            return Ok(*bits);
        }
        c.checkpoint(1)?;
        let result = match expression {
            Expression::EntryRegister { register } => {
                std::array::from_fn(|i| Bit::Entry(*register, i as u8))
            }
            Expression::Load {
                address,
                width,
                signed,
            } => {
                let loaded = u32::from(*width) * 8;
                std::array::from_fn(|i| {
                    let i = i as u32;
                    if i < loaded {
                        Bit::Load(address, *width, i as u8)
                    } else if *signed && loaded != 0 {
                        Bit::Load(address, *width, (loaded - 1) as u8)
                    } else {
                        Bit::Zero
                    }
                })
            }
            Expression::Integer { op, left, right } => {
                let l = self.bits(left, depth - 1, c)?;
                let r = self.bits(right, depth - 1, c)?;
                let by = constant(right).map(|k| k & 31);
                match op {
                    IntegerOp::And => std::array::from_fn(|i| and(l[i], r[i])),
                    IntegerOp::AndNot => std::array::from_fn(|i| and(l[i], not(r[i]))),
                    IntegerOp::Or => std::array::from_fn(|i| or(l[i], r[i])),
                    IntegerOp::Xor => std::array::from_fn(|i| xor(l[i], r[i])),
                    IntegerOp::Shl => match by {
                        Some(by) => shift_left(l, by),
                        None => [Bit::Unknown; 32],
                    },
                    IntegerOp::Shr | IntegerOp::Sar => match by {
                        Some(by) => std::array::from_fn(|i| {
                            let from = i as u32 + by;
                            if from < 32 {
                                l[from as usize]
                            } else if *op == IntegerOp::Sar {
                                l[31]
                            } else {
                                Bit::Zero
                            }
                        }),
                        None => [Bit::Unknown; 32],
                    },
                    IntegerOp::Add => add(l, r),
                    IntegerOp::Sub if r.iter().all(|bit| *bit == Bit::Zero) => l,
                    IntegerOp::ShiftAdd1 => add(shift_left(l, 1), r),
                    IntegerOp::ShiftAdd2 => add(shift_left(l, 2), r),
                    IntegerOp::ShiftAdd3 => add(shift_left(l, 3), r),
                    _ => [Bit::Unknown; 32],
                }
            }
            Expression::CallResult { .. } => [Bit::Unknown; 32],
        };
        self.memo.insert(id, (depth, result));
        Ok(result)
    }
}

fn exact_address(value: &AbstractValue) -> Option<u32> {
    match value {
        AbstractValue::Constant { value } | AbstractValue::ImageAddress { address: value } => {
            Some(*value)
        }
        _ => None,
    }
}

/// Where each run of the `width`-byte value stored at `address` comes from,
/// low run first: fixed bits, a function entry register's bits (an argument
/// for `a0`..`a7`), a loaded value's bits (with `same_word` for the stored word
/// itself) or unknown. Each bit follows bitwise logic, constant shifts and
/// carry-free additions of the analysis' expressions; anything else is
/// unknown, never guessed.
pub fn stored_bits<'a>(
    evaluator: &mut StoredBitsEvaluator<'_, 'a, '_>,
    address: &AbstractValue,
    width: u8,
    value: &'a AbstractValue,
    c: &mut dyn RunControl,
) -> Result<Vec<StoredBits>> {
    let Some(mask) = width_mask(width) else {
        return Ok(Vec::new());
    };
    let value = evaluator.bits(value, DEPTH, c)?;
    let count = mask.count_ones() as usize;
    let mut runs: Vec<StoredBits> = Vec::new();
    let mut i = 0;
    while i < count {
        let start = i;
        let first = value[i];
        i += 1;
        let next = |previous: Bit<'a>, bit: Bit<'a>| match (previous, bit) {
            (Bit::Zero | Bit::One, Bit::Zero | Bit::One) => true,
            (Bit::Entry(r, b), Bit::Entry(s, c)) => r == s && c == b + 1,
            (Bit::Load(a, w, b), Bit::Load(x, v, c)) => a == x && w == v && c == b + 1,
            (Bit::Unknown, Bit::Unknown) => true,
            _ => false,
        };
        while i < count && next(value[i - 1], value[i]) {
            i += 1;
        }
        let source = match first {
            Bit::Zero | Bit::One => StoredBitsSource::Constant {
                value: (start..i).fold(0, |acc, bit| {
                    acc | u32::from(value[bit] == Bit::One) << (bit - start)
                }),
            },
            Bit::Entry(register, low) => StoredBitsSource::EntryRegister { register, low },
            Bit::Load(loaded, loaded_width, low) => StoredBitsSource::Load {
                address: exact_address(loaded),
                width: loaded_width,
                low,
                same_word: loaded == address && loaded_width == width,
            },
            Bit::Unknown => StoredBitsSource::Unknown,
        };
        runs.push(StoredBits {
            low: start as u8,
            width: (i - start) as u8,
            source,
        });
    }
    Ok(runs)
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
    #[test]
    fn stored_bits_name_fields_kept_bits_constants_and_unknowns() {
        let memory = WorkingMemory::new(65536).unwrap();
        let word = number(0x2010_713c);
        let records = records(vec![
            Expression::Load {
                address: word.clone(),
                width: 4,
                signed: false,
            },
            Expression::EntryRegister { register: 10 },
            Expression::Integer {
                op: IntegerOp::Shl,
                left: value(1),
                right: number(18),
            },
            Expression::Integer {
                op: IntegerOp::And,
                left: value(2),
                right: number(0x01fc_0000),
            },
            Expression::Integer {
                op: IntegerOp::And,
                left: value(0),
                right: number(0xfe03_ffff),
            },
            Expression::Integer {
                op: IntegerOp::Or,
                left: value(3),
                right: value(4),
            },
            Expression::Integer {
                op: IntegerOp::Or,
                left: value(4),
                right: number(0x1b << 18),
            },
            Expression::Integer {
                op: IntegerOp::Mul,
                left: value(1),
                right: number(3),
            },
            Expression::EntryRegister { register: 11 },
            Expression::Integer {
                op: IntegerOp::Add,
                left: value(1),
                right: value(8),
            },
        ]);
        let (field, fixed, product, sum, small) =
            (value(5), value(6), value(7), value(9), number(0x1ff));
        let facts = Facts::new(&records, &memory, &mut || Ok(())).unwrap();
        let mut evaluator = StoredBitsEvaluator::new(&facts);
        macro_rules! stored_bits {
            ($address:expr, $width:expr, $value:expr) => {
                super::stored_bits(&mut evaluator, $address, $width, $value, &mut || Ok(()))
                    .unwrap()
            };
        }
        let kept = |low, width| StoredBits {
            low,
            width,
            source: StoredBitsSource::Load {
                address: Some(0x2010_713c),
                width: 4,
                low,
                same_word: true,
            },
        };
        assert_eq!(
            stored_bits!(&word, 4, &field),
            [
                kept(0, 18),
                StoredBits {
                    low: 18,
                    width: 7,
                    source: StoredBitsSource::EntryRegister {
                        register: 10,
                        low: 0
                    },
                },
                kept(25, 7),
            ],
            "the argument's low seven bits replace bits 24:18; the rest are kept"
        );
        assert_eq!(
            stored_bits!(&word, 4, &fixed)[1],
            StoredBits {
                low: 18,
                width: 7,
                source: StoredBitsSource::Constant { value: 0x1b },
            }
        );
        let elsewhere = number(0x2010_7094);
        assert!(matches!(
            stored_bits!(&elsewhere, 4, &field)[0].source,
            StoredBitsSource::Load {
                same_word: false,
                address: Some(0x2010_713c),
                ..
            }
        ));
        assert_eq!(
            stored_bits!(&word, 4, &product),
            [StoredBits {
                low: 0,
                width: 32,
                source: StoredBitsSource::Unknown,
            }],
            "a product is never decomposed"
        );
        assert_eq!(
            stored_bits!(&word, 4, &sum),
            [StoredBits {
                low: 0,
                width: 32,
                source: StoredBitsSource::Unknown,
            }],
            "two register values can carry from bit zero"
        );
        assert_eq!(
            stored_bits!(&word, 1, &small),
            [StoredBits {
                low: 0,
                width: 8,
                source: StoredBitsSource::Constant { value: 0xff },
            }],
            "only the access width is described"
        );
    }
    #[test]
    fn a_shared_expression_dag_is_evaluated_once_per_node_inside_the_budget() {
        let memory = WorkingMemory::new(65536).unwrap();
        // x0 = a0; x(i+1) = x(i) + x(i): one node per step, 2^steps paths.
        let chain = |steps: u32| {
            let mut expressions = vec![Expression::EntryRegister { register: 10 }];
            for id in 0..steps {
                expressions.push(Expression::Integer {
                    op: IntegerOp::Add,
                    left: value(id),
                    right: value(id),
                });
            }
            records(expressions)
        };
        let visits = |records: &[FunctionRecord], top: &AbstractValue| {
            let facts = Facts::new(records, &memory, &mut || Ok(())).unwrap();
            let mut evaluator = StoredBitsEvaluator::new(&facts);
            let mut visits = 0_u32;
            let runs = stored_bits(&mut evaluator, &number(0x2010_702c), 4, top, &mut || {
                visits += 1;
                Ok(())
            })
            .unwrap();
            assert_eq!(
                runs,
                [StoredBits {
                    low: 0,
                    width: 32,
                    source: StoredBitsSource::Unknown,
                }],
                "a register added to itself can carry from bit zero"
            );
            visits
        };
        let (short, short_top) = (chain(20), value(20));
        assert_eq!(
            visits(&short, &short_top),
            21,
            "each of the 21 nodes is charged once"
        );
        let (long, long_top) = (chain(40), value(40));
        assert_eq!(
            visits(&long, &long_top),
            u32::from(DEPTH),
            "a chain deeper than the bound stops there"
        );
        let facts = Facts::new(&long, &memory, &mut || Ok(())).unwrap();
        let mut evaluator = StoredBitsEvaluator::new(&facts);
        let mut exhausted = || Err(Error::new(ErrorCode::ResourceLimited, "work budget"));
        assert!(
            stored_bits(
                &mut evaluator,
                &number(0x2010_702c),
                4,
                &long_top,
                &mut exhausted
            )
            .is_err(),
            "the walk stops when the budget does"
        );
    }
}
