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
        self.caller_memory_location().is_some()
    }

    /// Return a stable affine object root and byte offset when the address is
    /// based on an ABI argument or a completed data-symbol relocation.
    pub fn memory_object_location(&self) -> Option<MemoryObjectLocation> {
        if let Some((index, offset)) = self.caller_memory_location() {
            return Some(MemoryObjectLocation {
                root: MemoryObjectRoot::Argument { index },
                offset: i64::from(offset),
            });
        }
        match self {
            Self::Constant(address) => Some(MemoryObjectLocation {
                root: MemoryObjectRoot::Absolute { address: *address },
                offset: 0,
            }),
            Self::SymbolAddress {
                member,
                symbol,
                lo_addend: Some(addend),
                post_offset,
                ..
            } => Some(MemoryObjectLocation {
                root: MemoryObjectRoot::RelocatedSymbol {
                    member: member.clone(),
                    symbol: symbol.clone(),
                },
                offset: addend.wrapping_add(*post_offset),
            }),
            _ => None,
        }
    }

    /// Resolve an affine address through exact 32-bit pointer loads whose
    /// source memory object is known.
    ///
    /// The mapping is explicit so a memory-read token is never guessed from
    /// event ordering by an individual consumer.
    pub fn memory_object_location_with_reads(
        &self,
        read_sources: &BTreeMap<u32, MemoryObjectLocation>,
    ) -> Option<MemoryObjectLocation> {
        if let Some(location) = self.memory_object_location() {
            return Some(location);
        }
        match self {
            Self::MemoryImage {
                read_token,
                and_mask: u32::MAX,
                or_mask: 0,
            } => {
                let source = read_sources.get(read_token)?;
                let MemoryObjectRoot::RelocatedSymbol { member, symbol } = &source.root else {
                    return None;
                };
                Some(MemoryObjectLocation {
                    root: MemoryObjectRoot::DereferencedGlobal {
                        member: member.clone(),
                        symbol: symbol.clone(),
                        pointer_offset: source.offset,
                    },
                    offset: 0,
                })
            }
            Self::Expression {
                operation: ExpressionOperation::Add,
                left,
                right,
            } => match (
                left.memory_object_location_with_reads(read_sources),
                right.as_constant(),
            ) {
                (Some(mut location), Some(offset)) => {
                    location.offset = location.offset.wrapping_add(i64::from(offset as i32));
                    Some(location)
                }
                _ => match (
                    right.memory_object_location_with_reads(read_sources),
                    left.as_constant(),
                ) {
                    (Some(mut location), Some(offset)) => {
                        location.offset = location.offset.wrapping_add(i64::from(offset as i32));
                        Some(location)
                    }
                    _ => None,
                },
            },
            _ => None,
        }
    }

    /// Return `(argument index, byte offset)` for an affine address rooted in
    /// one ABI argument.
    ///
    /// The intentionally narrow form is enough for compiler-generated struct
    /// field accesses and remains safe to substitute while composing calls.
    pub fn caller_memory_location(&self) -> Option<(u8, i32)> {
        if let Some(index) = self.direct_input_index() {
            return Some((index, 0));
        }
        match self {
            Self::Expression {
                operation: ExpressionOperation::Add,
                left,
                right,
            } => match (left.caller_memory_location(), right.as_constant()) {
                (Some((index, offset)), Some(constant)) => {
                    Some((index, offset.wrapping_add(constant as i32)))
                }
                _ => match (right.caller_memory_location(), left.as_constant()) {
                    (Some((index, offset)), Some(constant)) => {
                        Some((index, offset.wrapping_add(constant as i32)))
                    }
                    _ => None,
                },
            },
            Self::Expression {
                operation: ExpressionOperation::Subtract,
                left,
                right,
            } => match (left.caller_memory_location(), right.as_constant()) {
                (Some((index, offset)), Some(constant)) => {
                    Some((index, offset.wrapping_sub(constant as i32)))
                }
                _ => None,
            },
            _ => None,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caller_memory_location_recovers_nested_struct_offsets() {
        let address = SymbolicValue::input(2)
            .add_constant(0x20)
            .add_constant((-4_i32) as u32);

        assert_eq!(address.caller_memory_location(), Some((2, 0x1c)));
    }

    #[test]
    fn caller_memory_location_rejects_dynamic_indexing() {
        let address = SymbolicValue::expression(
            ExpressionOperation::Add,
            SymbolicValue::input(0),
            SymbolicValue::input(1),
        );

        assert_eq!(address.caller_memory_location(), None);
    }

    #[test]
    fn memory_object_location_recovers_completed_global_relocations() {
        let address = SymbolicValue::SymbolAddress {
            member: Some("state.o".to_owned()),
            symbol: "phy_state".to_owned(),
            hi_addend: 4,
            lo_addend: Some(4),
            post_offset: 8,
        };
        assert_eq!(
            address.memory_object_location(),
            Some(MemoryObjectLocation {
                root: MemoryObjectRoot::RelocatedSymbol {
                    member: Some("state.o".to_owned()),
                    symbol: "phy_state".to_owned(),
                },
                offset: 12,
            })
        );
    }

    #[test]
    fn memory_object_location_recovers_exact_dereferenced_global_pointer() {
        let mut reads = BTreeMap::new();
        reads.insert(
            3,
            MemoryObjectLocation {
                root: MemoryObjectRoot::RelocatedSymbol {
                    member: Some("globals.o".to_owned()),
                    symbol: "g_state".to_owned(),
                },
                offset: 4,
            },
        );
        let address = SymbolicValue::memory_read(3, 32, false).add_constant(0x1c);

        assert_eq!(
            address.memory_object_location_with_reads(&reads),
            Some(MemoryObjectLocation {
                root: MemoryObjectRoot::DereferencedGlobal {
                    member: Some("globals.o".to_owned()),
                    symbol: "g_state".to_owned(),
                    pointer_offset: 4,
                },
                offset: 0x1c,
            })
        );
    }

    #[test]
    fn memory_object_location_keeps_absolute_storage_identity() {
        assert_eq!(
            SymbolicValue::Constant(0x3fc8_1000).memory_object_location(),
            Some(MemoryObjectLocation {
                root: MemoryObjectRoot::Absolute {
                    address: 0x3fc8_1000,
                },
                offset: 0,
            })
        );
    }
}
