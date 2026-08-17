//! Canonical symbolic arithmetic and bitwise normalization.

use super::*;

// Symbolic values are evidence trees, not an unrestricted expression CAS.
// Repeated register updates can otherwise clone an expression into an
// exponential tree while analyzing a large compiler-generated function. A
// value beyond this boundary becomes Unknown, which preserves fail-closed
// analysis while bounding clone, formatting and drop costs.
const MAX_SYMBOLIC_VALUE_NODES: usize = 256;

fn symbolic_nodes(value: &SymbolicValue) -> usize {
    let mut count = 0_usize;
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        count += 1;
        if count > MAX_SYMBOLIC_VALUE_NODES {
            return count;
        }
        match value {
            SymbolicValue::Expression { left, right, .. } => {
                pending.push(left);
                pending.push(right);
            }
            SymbolicValue::FloatingPoint { operands, .. } => {
                pending.extend(operands.iter());
            }
            SymbolicValue::WideSignedDivide {
                dividend_low,
                dividend_high,
                divisor_low,
                divisor_high,
                ..
            } => {
                pending.push(dividend_low);
                pending.push(dividend_high);
                pending.push(divisor_low);
                pending.push(divisor_high);
            }
            _ => {}
        }
    }
    count
}

fn requires_expression_bits(value: &SymbolicValue) -> bool {
    matches!(
        value,
        SymbolicValue::Expression { .. } | SymbolicValue::FloatingPoint { .. }
    )
}

impl SymbolicValue {
    pub fn and(self, constant: u32) -> Self {
        if requires_expression_bits(&self) {
            return Self::expression(ExpressionOperation::BitAnd, self, Self::Constant(constant));
        }
        Self::from_bits(core::array::from_fn(|bit| {
            if constant & (1 << bit) == 0 {
                BitSource::Constant(false)
            } else {
                self.bits()[bit]
            }
        }))
    }

    pub fn or(self, constant: u32) -> Self {
        if requires_expression_bits(&self) {
            return Self::expression(ExpressionOperation::BitOr, self, Self::Constant(constant));
        }
        Self::from_bits(core::array::from_fn(|bit| {
            if constant & (1 << bit) != 0 {
                BitSource::Constant(true)
            } else {
                self.bits()[bit]
            }
        }))
    }

    pub fn symbolic_bitand(self, other: Self) -> Self {
        if let Some(constant) = self.as_constant() {
            return other.and(constant);
        }
        if let Some(constant) = other.as_constant() {
            return self.and(constant);
        }
        if requires_expression_bits(&self) || requires_expression_bits(&other) {
            return Self::expression(ExpressionOperation::BitAnd, self, other);
        }
        let left = self.bits();
        let right = other.bits();
        let simplified =
            Self::from_bits(core::array::from_fn(|bit| match (left[bit], right[bit]) {
                (BitSource::Constant(false), _) | (_, BitSource::Constant(false)) => {
                    BitSource::Constant(false)
                }
                (BitSource::Constant(true), source) | (source, BitSource::Constant(true)) => source,
                (left, right) if left == right => left,
                _ => BitSource::Unknown,
            }));
        if simplified.is_resolved() {
            simplified
        } else {
            Self::expression(ExpressionOperation::BitAnd, self, other)
        }
    }

    pub fn symbolic_bitor(self, other: Self) -> Self {
        if let Some(constant) = self.as_constant() {
            return other.or(constant);
        }
        if let Some(constant) = other.as_constant() {
            return self.or(constant);
        }
        if requires_expression_bits(&self) || requires_expression_bits(&other) {
            return Self::expression(ExpressionOperation::BitOr, self, other);
        }
        let left = self.bits();
        let right = other.bits();
        let simplified =
            Self::from_bits(core::array::from_fn(|bit| match (left[bit], right[bit]) {
                (BitSource::Constant(true), _) | (_, BitSource::Constant(true)) => {
                    BitSource::Constant(true)
                }
                (BitSource::Constant(false), source) | (source, BitSource::Constant(false)) => {
                    source
                }
                (left, right) if left == right => left,
                _ => BitSource::Unknown,
            }));
        if simplified.is_resolved() {
            simplified
        } else {
            Self::expression(ExpressionOperation::BitOr, self, other)
        }
    }

    pub fn shift_left(self, amount: u32) -> Self {
        if requires_expression_bits(&self) {
            return Self::expression(ExpressionOperation::ShiftLeft, self, Self::Constant(amount));
        }
        let source = self.bits();
        Self::from_bits(core::array::from_fn(|bit| {
            bit.checked_sub(amount as usize)
                .map_or(BitSource::Constant(false), |source_bit| source[source_bit])
        }))
    }

    pub fn shift_right(self, amount: u32) -> Self {
        if requires_expression_bits(&self) {
            return Self::expression(
                ExpressionOperation::ShiftRight,
                self,
                Self::Constant(amount),
            );
        }
        let source = self.bits();
        Self::from_bits(core::array::from_fn(|bit| {
            source
                .get(bit + amount as usize)
                .copied()
                .unwrap_or(BitSource::Constant(false))
        }))
    }

    pub fn add_constant(self, constant: u32) -> Self {
        if constant == 0 {
            return self;
        }
        let field_sum = |and_mask: u32, or_mask: u32| {
            or_mask
                .checked_add(constant)
                .filter(|sum| (sum ^ or_mask) & and_mask == 0)
        };
        match &self {
            Self::RegisterImage {
                read_token,
                address,
                and_mask,
                or_mask,
            } if field_sum(*and_mask, *or_mask).is_some() => {
                return Self::RegisterImage {
                    read_token: *read_token,
                    address: *address,
                    and_mask: *and_mask,
                    or_mask: field_sum(*and_mask, *or_mask).unwrap(),
                };
            }
            Self::IndexedRegisterImage {
                read_token,
                and_mask,
                or_mask,
            } if field_sum(*and_mask, *or_mask).is_some() => {
                return Self::IndexedRegisterImage {
                    read_token: *read_token,
                    and_mask: *and_mask,
                    or_mask: field_sum(*and_mask, *or_mask).unwrap(),
                };
            }
            Self::MemoryImage {
                read_token,
                and_mask,
                or_mask,
            } if field_sum(*and_mask, *or_mask).is_some() => {
                return Self::MemoryImage {
                    read_token: *read_token,
                    and_mask: *and_mask,
                    or_mask: field_sum(*and_mask, *or_mask).unwrap(),
                };
            }
            _ => {}
        }
        // Compilers freely select ADDI instead of ORI after clearing the
        // destination field. If every set bit in the addend is proven zero in
        // the symbolic value, addition cannot carry and is exactly the same
        // field insertion as bitwise OR. Canonicalize both instruction
        // selections to the existing mask/or representation.
        let bits = self.bits();
        if bits.iter().enumerate().all(|(bit, source)| {
            constant & (1_u32 << bit) == 0 || *source == BitSource::Constant(false)
        }) {
            return self.or(constant);
        }
        if let Self::Constant(value) = self {
            return Self::Constant(value.wrapping_add(constant));
        }
        if let Self::StackAddress(offset) = self {
            return Self::StackAddress(offset.wrapping_add(constant as i32));
        }
        if let Self::SymbolAddress {
            member,
            symbol,
            hi_addend,
            lo_addend,
            post_offset,
        } = self
        {
            return Self::SymbolAddress {
                member,
                symbol,
                hi_addend,
                lo_addend,
                post_offset: post_offset.wrapping_add(i64::from(constant as i32)),
            };
        }
        Self::expression(ExpressionOperation::Add, self, Self::Constant(constant))
    }

    pub fn symbolic_not(self) -> Self {
        Self::from_bits(self.bits().map(|source| match source {
            BitSource::Constant(value) => BitSource::Constant(!value),
            BitSource::Input {
                index,
                bit,
                inverted,
            } => BitSource::Input {
                index,
                bit,
                inverted: !inverted,
            },
            BitSource::Register {
                read_token,
                address,
                bit,
                inverted,
            } => BitSource::Register {
                read_token,
                address,
                bit,
                inverted: !inverted,
            },
            BitSource::IndexedRegister {
                read_token,
                bit,
                inverted,
            } => BitSource::IndexedRegister {
                read_token,
                bit,
                inverted: !inverted,
            },
            BitSource::Memory {
                read_token,
                bit,
                inverted,
            } => BitSource::Memory {
                read_token,
                bit,
                inverted: !inverted,
            },
            BitSource::PrivateStack {
                read_token,
                bit,
                inverted,
            } => BitSource::PrivateStack {
                read_token,
                bit,
                inverted: !inverted,
            },
            BitSource::CallResult {
                call_token,
                bit,
                inverted,
            } => BitSource::CallResult {
                call_token,
                bit,
                inverted: !inverted,
            },
            BitSource::ExternalResult {
                call_token,
                bit,
                inverted,
            } => BitSource::ExternalResult {
                call_token,
                bit,
                inverted: !inverted,
            },
            BitSource::ExternalResultHigh {
                call_token,
                bit,
                inverted,
            } => BitSource::ExternalResultHigh {
                call_token,
                bit,
                inverted: !inverted,
            },
            BitSource::ExternalOutput {
                call_token,
                output_index,
                bit,
                inverted,
            } => BitSource::ExternalOutput {
                call_token,
                output_index,
                bit,
                inverted: !inverted,
            },
            BitSource::Unknown => BitSource::Unknown,
        }))
    }

    pub fn xor(self, constant: u32) -> Self {
        if requires_expression_bits(&self) {
            return Self::expression(ExpressionOperation::BitXor, self, Self::Constant(constant));
        }
        let bits = self.bits();
        Self::from_bits(core::array::from_fn(|bit| {
            if constant & (1 << bit) == 0 {
                bits[bit]
            } else {
                match bits[bit] {
                    BitSource::Constant(value) => BitSource::Constant(!value),
                    BitSource::Input {
                        index,
                        bit,
                        inverted,
                    } => BitSource::Input {
                        index,
                        bit,
                        inverted: !inverted,
                    },
                    BitSource::Register {
                        read_token,
                        address,
                        bit,
                        inverted,
                    } => BitSource::Register {
                        read_token,
                        address,
                        bit,
                        inverted: !inverted,
                    },
                    BitSource::IndexedRegister {
                        read_token,
                        bit,
                        inverted,
                    } => BitSource::IndexedRegister {
                        read_token,
                        bit,
                        inverted: !inverted,
                    },
                    BitSource::Memory {
                        read_token,
                        bit,
                        inverted,
                    } => BitSource::Memory {
                        read_token,
                        bit,
                        inverted: !inverted,
                    },
                    BitSource::PrivateStack {
                        read_token,
                        bit,
                        inverted,
                    } => BitSource::PrivateStack {
                        read_token,
                        bit,
                        inverted: !inverted,
                    },
                    BitSource::CallResult {
                        call_token,
                        bit,
                        inverted,
                    } => BitSource::CallResult {
                        call_token,
                        bit,
                        inverted: !inverted,
                    },
                    BitSource::ExternalResult {
                        call_token,
                        bit,
                        inverted,
                    } => BitSource::ExternalResult {
                        call_token,
                        bit,
                        inverted: !inverted,
                    },
                    BitSource::ExternalResultHigh {
                        call_token,
                        bit,
                        inverted,
                    } => BitSource::ExternalResultHigh {
                        call_token,
                        bit,
                        inverted: !inverted,
                    },
                    BitSource::ExternalOutput {
                        call_token,
                        output_index,
                        bit,
                        inverted,
                    } => BitSource::ExternalOutput {
                        call_token,
                        output_index,
                        bit,
                        inverted: !inverted,
                    },
                    BitSource::Unknown => BitSource::Unknown,
                }
            }
        }))
    }

    pub fn symbolic_bitxor(self, other: Self) -> Self {
        if self == other {
            return Self::Constant(0);
        }
        match (self, other) {
            (Self::Constant(constant), value) | (value, Self::Constant(constant)) => {
                value.xor(constant)
            }
            (left, right) => Self::expression(ExpressionOperation::BitXor, left, right),
        }
    }

    pub fn expression(operation: ExpressionOperation, left: Self, right: Self) -> Self {
        if !left.is_resolved() || !right.is_resolved() {
            return Self::Unknown;
        }
        if 1_usize
            .saturating_add(symbolic_nodes(&left))
            .saturating_add(symbolic_nodes(&right))
            > MAX_SYMBOLIC_VALUE_NODES
        {
            return Self::Unknown;
        }
        Self::expression_unbounded(operation, left, right)
    }

    /// Rebuild an already-bounded expression after token or argument
    /// substitution. Rewriting must preserve a previously accepted evidence
    /// tree instead of applying the construction limit a second time.
    pub(super) fn expression_unbounded(
        operation: ExpressionOperation,
        left: Self,
        right: Self,
    ) -> Self {
        let caller_memory_location = match operation {
            ExpressionOperation::Add => {
                match (left.caller_memory_location(), right.as_constant()) {
                    (Some((index, offset)), Some(constant)) => {
                        Some((index, offset.wrapping_add(constant as i32)))
                    }
                    _ => match (right.caller_memory_location(), left.as_constant()) {
                        (Some((index, offset)), Some(constant)) => {
                            Some((index, offset.wrapping_add(constant as i32)))
                        }
                        _ => None,
                    },
                }
            }
            ExpressionOperation::Subtract => {
                match (left.caller_memory_location(), right.as_constant()) {
                    (Some((index, offset)), Some(constant)) => {
                        Some((index, offset.wrapping_sub(constant as i32)))
                    }
                    _ => None,
                }
            }
            _ => None,
        };
        Self::Expression {
            operation,
            left: Arc::new(left),
            right: Arc::new(right),
            caller_memory_location,
        }
    }

    // This becomes live when the exact, digest-gated __divdi3 call summary is
    // connected; keeping construction centralized prevents low/high drift.
    #[allow(dead_code)]
    pub fn wide_signed_divide_words(
        dividend_low: Self,
        dividend_high: Self,
        divisor_low: Self,
        divisor_high: Self,
    ) -> (Self, Self) {
        if !dividend_low.is_resolved()
            || !dividend_high.is_resolved()
            || !divisor_low.is_resolved()
            || !divisor_high.is_resolved()
        {
            return (Self::Unknown, Self::Unknown);
        }
        if 1_usize
            .saturating_add(symbolic_nodes(&dividend_low))
            .saturating_add(symbolic_nodes(&dividend_high))
            .saturating_add(symbolic_nodes(&divisor_low))
            .saturating_add(symbolic_nodes(&divisor_high))
            > MAX_SYMBOLIC_VALUE_NODES
        {
            return (Self::Unknown, Self::Unknown);
        }
        let word = |high_word| Self::WideSignedDivide {
            dividend_low: Arc::new(dividend_low.clone()),
            dividend_high: Arc::new(dividend_high.clone()),
            divisor_low: Arc::new(divisor_low.clone()),
            divisor_high: Arc::new(divisor_high.clone()),
            high_word,
        };
        (word(false), word(true))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expression_growth_becomes_unknown_before_it_can_exhaust_host_memory() {
        let mut value = SymbolicValue::input(0);
        for _ in 0..MAX_SYMBOLIC_VALUE_NODES {
            value = SymbolicValue::expression(
                ExpressionOperation::Add,
                value,
                SymbolicValue::Constant(1),
            );
        }
        assert_eq!(value, SymbolicValue::Unknown);
    }
}
