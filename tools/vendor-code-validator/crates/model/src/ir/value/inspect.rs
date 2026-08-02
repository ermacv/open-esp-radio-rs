//! Provenance queries, resolved-state checks and canonical rendering.

use super::*;

impl SymbolicValue {
    pub fn direct_input_index(&self) -> Option<u8> {
        if let Self::InputConstant { index, .. } = self {
            return Some(*index);
        }
        let Self::Bits(bits) = self else {
            return None;
        };
        let mut index = None;
        for (destination, source) in bits.iter().enumerate() {
            let BitSource::Input {
                index: source_index,
                bit,
                inverted: false,
            } = source
            else {
                return None;
            };
            if usize::from(*bit) != destination {
                return None;
            }
            match index {
                Some(index) if index != *source_index => return None,
                Some(_) => {}
                None => index = Some(*source_index),
            }
        }
        index
    }

    pub fn caller_memory_address(&self) -> bool {
        if self.direct_input_index().is_some() {
            return true;
        }
        match self {
            Self::Expression {
                operation: ExpressionOperation::Add,
                left,
                right,
            } => {
                (left.caller_memory_address() && matches!(right.as_ref(), Self::Constant(_)))
                    || (right.caller_memory_address() && matches!(left.as_ref(), Self::Constant(_)))
            }
            Self::Expression {
                operation: ExpressionOperation::Subtract,
                left,
                right,
            } => left.caller_memory_address() && matches!(right.as_ref(), Self::Constant(_)),
            _ => false,
        }
    }

    /// Returns the byte offset when this is an affine address into the current
    /// function's private stack frame.
    ///
    /// This remains deliberately narrower than general expression evaluation:
    /// only a stack base plus or minus a constant is accepted. It is used while
    /// composing calls so private scratch memory never becomes a generated host
    /// pointer.
    pub fn private_stack_offset(&self) -> Option<i32> {
        match self {
            Self::StackAddress(offset) => Some(*offset),
            Self::Expression {
                operation: ExpressionOperation::Add,
                left,
                right,
            } => match (left.private_stack_offset(), right.as_constant()) {
                (Some(offset), Some(constant)) => Some(offset.wrapping_add(constant as i32)),
                _ => match (right.private_stack_offset(), left.as_constant()) {
                    (Some(offset), Some(constant)) => Some(offset.wrapping_add(constant as i32)),
                    _ => None,
                },
            },
            Self::Expression {
                operation: ExpressionOperation::Subtract,
                left,
                right,
            } => match (left.private_stack_offset(), right.as_constant()) {
                (Some(offset), Some(constant)) => Some(offset.wrapping_sub(constant as i32)),
                _ => None,
            },
            _ => None,
        }
    }

    pub fn depends_on_private_stack_read(&self) -> bool {
        match self {
            Self::Expression { left, right, .. } => {
                left.depends_on_private_stack_read() || right.depends_on_private_stack_read()
            }
            Self::WideSignedDivide {
                dividend_low,
                dividend_high,
                divisor_low,
                divisor_high,
                ..
            } => {
                dividend_low.depends_on_private_stack_read()
                    || dividend_high.depends_on_private_stack_read()
                    || divisor_low.depends_on_private_stack_read()
                    || divisor_high.depends_on_private_stack_read()
            }
            Self::Bits(bits) => bits
                .iter()
                .any(|source| matches!(source, BitSource::PrivateStack { .. })),
            _ => false,
        }
    }

    pub fn as_constant(&self) -> Option<u32> {
        match self {
            Self::Constant(value) => Some(*value),
            Self::InputConstant { value, .. } => Some(*value),
            _ => None,
        }
    }

    pub fn seqz(self) -> Self {
        if let Self::Constant(value) = &self {
            return Self::Constant((*value == 0) as u32);
        }
        let mut nonzero = self
            .bits()
            .into_iter()
            .filter(|source| *source != BitSource::Constant(false));
        let source = nonzero.next();
        if nonzero.next().is_some() {
            return Self::expression(ExpressionOperation::Equal, self, Self::Constant(0));
        }
        let inverse = match source {
            None => BitSource::Constant(true),
            Some(BitSource::Constant(true)) => BitSource::Constant(false),
            Some(BitSource::Input {
                index,
                bit,
                inverted,
            }) => BitSource::Input {
                index,
                bit,
                inverted: !inverted,
            },
            Some(BitSource::Register {
                read_token,
                address,
                bit,
                inverted,
            }) => BitSource::Register {
                read_token,
                address,
                bit,
                inverted: !inverted,
            },
            Some(BitSource::IndexedRegister {
                read_token,
                bit,
                inverted,
            }) => BitSource::IndexedRegister {
                read_token,
                bit,
                inverted: !inverted,
            },
            Some(BitSource::Memory {
                read_token,
                bit,
                inverted,
            }) => BitSource::Memory {
                read_token,
                bit,
                inverted: !inverted,
            },
            Some(BitSource::PrivateStack {
                read_token,
                bit,
                inverted,
            }) => BitSource::PrivateStack {
                read_token,
                bit,
                inverted: !inverted,
            },
            Some(BitSource::CallResult {
                call_token,
                bit,
                inverted,
            }) => BitSource::CallResult {
                call_token,
                bit,
                inverted: !inverted,
            },
            Some(BitSource::ExternalResult {
                call_token,
                bit,
                inverted,
            }) => BitSource::ExternalResult {
                call_token,
                bit,
                inverted: !inverted,
            },
            Some(BitSource::Unknown) => BitSource::Unknown,
            Some(BitSource::Constant(false)) => unreachable!(),
        };
        Self::from_bits(core::array::from_fn(|bit| {
            if bit == 0 {
                inverse
            } else {
                BitSource::Constant(false)
            }
        }))
    }

    pub fn is_resolved(&self) -> bool {
        match self {
            Self::Expression { left, right, .. } => left.is_resolved() && right.is_resolved(),
            Self::WideSignedDivide {
                dividend_low,
                dividend_high,
                divisor_low,
                divisor_high,
                ..
            } => {
                dividend_low.is_resolved()
                    && dividend_high.is_resolved()
                    && divisor_low.is_resolved()
                    && divisor_high.is_resolved()
            }
            Self::SymbolAddress { lo_addend, .. } => lo_addend.is_some(),
            Self::ExternalResult(_) => true,
            Self::ExternalTable(_)
            | Self::ExternalFunction { .. }
            | Self::FunctionTable(_)
            | Self::FunctionPointer { .. }
            | Self::StackAddress(_) => false,
            _ => !matches!(self, Self::Unknown) && !self.bits().contains(&BitSource::Unknown),
        }
    }

    pub fn canonical(&self) -> String {
        match self {
            Self::Unknown => "unknown".to_owned(),
            Self::Constant(value) => format!("const:{value:#010x}"),
            Self::InputConstant { index, .. } => Self::input(*index).canonical(),
            Self::StackAddress(offset) => format!("private-stack:{offset:+#x}"),
            Self::SymbolAddress {
                member,
                symbol,
                hi_addend,
                lo_addend,
                post_offset,
            } => format!(
                "symbol:{}::{symbol}:hi{hi_addend:+#x}:lo{}:post{post_offset:+#x}",
                member.as_deref().unwrap_or("<linked>"),
                lo_addend.map_or_else(|| "?".to_owned(), |addend| format!("{addend:+#x}"))
            ),
            Self::CallResult(call_token) => format!("call-result:{call_token}"),
            Self::ExternalTable(table) => {
                format!("external-table:{}", table.spec().id)
            }
            Self::ExternalFunction { table, function } => {
                format!("external-function:{}::{function:?}", table.spec().id)
            }
            Self::FunctionTable(table) => format!("function-table:{}", table.id()),
            Self::FunctionPointer { table, target } => {
                format!("function-pointer:{}::{target:#010x}", table.id())
            }
            Self::ExternalResult(call_token) => format!("external-result:{call_token}"),
            Self::Expression {
                operation,
                left,
                right,
            } => format!(
                "expr:{operation:?}({},{})",
                left.canonical(),
                right.canonical()
            ),
            Self::WideSignedDivide {
                dividend_low,
                dividend_high,
                divisor_low,
                divisor_high,
                high_word,
            } => format!(
                "wide-sdiv64:{}({},{},{},{})",
                if *high_word { "high" } else { "low" },
                dividend_low.canonical(),
                dividend_high.canonical(),
                divisor_low.canonical(),
                divisor_high.canonical(),
            ),
            Self::RegisterImage {
                read_token,
                address,
                and_mask,
                or_mask,
            } => format!("rmw:read{read_token}[{address:#010x}]&{and_mask:#010x}|{or_mask:#010x}"),
            Self::IndexedRegisterImage {
                read_token,
                and_mask,
                or_mask,
            } => format!("indexed-rmw:read{read_token}&{and_mask:#010x}|{or_mask:#010x}"),
            Self::MemoryImage {
                read_token,
                and_mask,
                or_mask,
            } => format!("ram:read{read_token}&{and_mask:#010x}|{or_mask:#010x}"),
            Self::Bits(bits) => {
                let terms = bits
                    .iter()
                    .enumerate()
                    .filter_map(|(bit, source)| match source {
                        BitSource::Constant(false) => None,
                        BitSource::Constant(true) => Some(format!("{bit}=1")),
                        BitSource::Input {
                            index,
                            bit: source,
                            inverted,
                        } => {
                            let inverse = if *inverted { "!" } else { "" };
                            Some(format!("{bit}={inverse}arg{index}.{source}"))
                        }
                        BitSource::Register {
                            read_token,
                            address,
                            bit: source,
                            inverted,
                        } => {
                            let inverse = if *inverted { "!" } else { "" };
                            Some(format!(
                                "{bit}={inverse}read{read_token}[{address:#010x}].{source}"
                            ))
                        }
                        BitSource::IndexedRegister {
                            read_token,
                            bit: source,
                            inverted,
                        } => {
                            let inverse = if *inverted { "!" } else { "" };
                            Some(format!("{bit}={inverse}indexed-read{read_token}.{source}"))
                        }
                        BitSource::Memory {
                            read_token,
                            bit: source,
                            inverted,
                        } => {
                            let inverse = if *inverted { "!" } else { "" };
                            Some(format!("{bit}={inverse}ramread{read_token}.{source}"))
                        }
                        BitSource::PrivateStack {
                            read_token,
                            bit: source,
                            inverted,
                        } => {
                            let inverse = if *inverted { "!" } else { "" };
                            Some(format!(
                                "{bit}={inverse}private-stack-read{read_token}.{source}"
                            ))
                        }
                        BitSource::CallResult {
                            call_token,
                            bit: source,
                            inverted,
                        } => {
                            let inverse = if *inverted { "!" } else { "" };
                            Some(format!("{bit}={inverse}call{call_token}.return.{source}"))
                        }
                        BitSource::ExternalResult {
                            call_token,
                            bit: source,
                            inverted,
                        } => {
                            let inverse = if *inverted { "!" } else { "" };
                            Some(format!("{bit}={inverse}external{call_token}.{source}"))
                        }
                        BitSource::Unknown => Some(format!("{bit}=?")),
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                format!("bits:{terms}")
            }
        }
    }
}
