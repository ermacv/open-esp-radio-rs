//! Provenance queries, resolved-state checks and canonical rendering.

use super::*;

impl SymbolicValue {
    pub fn direct_input_index(&self) -> Option<u8> {
        if let Self::Input { index } | Self::InputConstant { index, .. } = self {
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
            Self::ExternalResult(token) if token & ALLOCATED_EXTERNAL_RESULT_TOKEN_FLAG != 0 => {
                Some(MemoryObjectLocation {
                    root: MemoryObjectRoot::ZeroedAllocation {
                        call_token: external_result_call_token(*token),
                    },
                    offset: 0,
                })
            }
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
            Self::MemoryImage {
                read_token,
                and_mask: u32::MAX,
                or_mask: 0,
            } => {
                let source = read_sources.get(read_token)?;
                Some(MemoryObjectLocation {
                    root: MemoryObjectRoot::Dereferenced {
                        pointer: std::sync::Arc::new(source.root.clone()),
                        pointer_offset: source.offset,
                    },
                    offset: 0,
                })
            }
            Self::Expression {
                operation: ExpressionOperation::Add,
                left,
                right,
            } => {
                if let Some((argument, stride)) = right.scaled_input() {
                    return left
                        .memory_object_location_with_reads(read_sources)
                        .map(|location| location.with_index(argument, stride));
                }
                if let Some((argument, stride)) = left.scaled_input() {
                    return right
                        .memory_object_location_with_reads(read_sources)
                        .map(|location| location.with_index(argument, stride));
                }
                if let Some((argument, offset)) = right.caller_memory_location() {
                    return left.memory_object_location_with_reads(read_sources).map(
                        |mut location| {
                            location.offset = location.offset.wrapping_add(i64::from(offset));
                            location.with_index(argument, 1)
                        },
                    );
                }
                if let Some((argument, offset)) = left.caller_memory_location() {
                    return right.memory_object_location_with_reads(read_sources).map(
                        |mut location| {
                            location.offset = location.offset.wrapping_add(i64::from(offset));
                            location.with_index(argument, 1)
                        },
                    );
                }
                if let Some(offset) = right.as_constant() {
                    let mut location = left.memory_object_location_with_reads(read_sources)?;
                    location.offset = location.offset.wrapping_add(i64::from(offset as i32));
                    return Some(location);
                }
                if let Some(offset) = left.as_constant() {
                    let mut location = right.memory_object_location_with_reads(read_sources)?;
                    location.offset = location.offset.wrapping_add(i64::from(offset as i32));
                    return Some(location);
                }
                None
            }
            _ => None,
        }
    }

    /// Whether this exact symbolic value retains at least one pointer/value
    /// dependency on a read from a known memory object. This is intentionally
    /// weaker than `memory_object_location_with_reads`: it permits a dynamic
    /// address effect while making no static field or object-offset claim.
    pub fn has_memory_address_provenance(
        &self,
        read_sources: &BTreeMap<u32, MemoryObjectLocation>,
    ) -> bool {
        match self {
            Self::MemoryImage { read_token, .. } => read_sources.contains_key(read_token),
            Self::Bits(bits) => bits.iter().any(|source| {
                matches!(
                    source,
                    BitSource::Memory { read_token, .. }
                        if read_sources.contains_key(read_token)
                )
            }),
            Self::Expression { left, right, .. } => {
                left.has_memory_address_provenance(read_sources)
                    || right.has_memory_address_provenance(read_sources)
            }
            Self::WideSignedDivide {
                dividend_low,
                dividend_high,
                divisor_low,
                divisor_high,
                ..
            } => {
                dividend_low.has_memory_address_provenance(read_sources)
                    || dividend_high.has_memory_address_provenance(read_sources)
                    || divisor_low.has_memory_address_provenance(read_sources)
                    || divisor_high.has_memory_address_provenance(read_sources)
            }
            _ => false,
        }
    }

    fn scaled_input(&self) -> Option<(u8, i64)> {
        if let Some(argument) = self.direct_input_index() {
            return Some((argument, 1));
        }
        match self {
            Self::Expression {
                operation: ExpressionOperation::Multiply,
                left,
                right,
            } => match (left.direct_input_index(), right.as_constant()) {
                (Some(argument), Some(stride)) => Some((argument, i64::from(stride))),
                _ => match (right.direct_input_index(), left.as_constant()) {
                    (Some(argument), Some(stride)) => Some((argument, i64::from(stride))),
                    _ => None,
                },
            },
            Self::Expression {
                operation: ExpressionOperation::ShiftLeft,
                left,
                right,
            } => {
                let argument = left.direct_input_index()?;
                let shift = right.as_constant()?;
                (shift < 32).then_some((argument, 1_i64 << shift))
            }
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
            Some(BitSource::ExternalResultHigh {
                call_token,
                bit,
                inverted,
            }) => BitSource::ExternalResultHigh {
                call_token,
                bit,
                inverted: !inverted,
            },
            Some(BitSource::ExternalOutput {
                call_token,
                output_index,
                bit,
                inverted,
            }) => BitSource::ExternalOutput {
                call_token,
                output_index,
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
            Self::ExternalResult(_) | Self::ExternalResultHigh(_) | Self::ExternalOutput { .. } => {
                true
            }
            Self::ReviewedExternalTable(_)
            | Self::ReviewedExternalFunction { .. }
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
            Self::Input { index } => format!("arg{index}"),
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
            Self::ReviewedExternalTable(contract) => {
                format!("reviewed-external-table:{contract}")
            }
            Self::ReviewedExternalFunction { contract, offset } => {
                format!("reviewed-external-function:{contract}+{offset:#x}")
            }
            Self::FunctionTable(table) => format!("function-table:{}", table.id()),
            Self::FunctionPointer { table, target } => {
                format!("function-pointer:{}::{target:#010x}", table.id())
            }
            Self::ExternalResult(call_token)
                if call_token & ALLOCATED_EXTERNAL_RESULT_TOKEN_FLAG != 0 =>
            {
                format!(
                    "zeroed-allocation:{}",
                    external_result_call_token(*call_token)
                )
            }
            Self::ExternalResult(call_token) => format!("external-result:{call_token}"),
            Self::ExternalResultHigh(call_token) => {
                format!("external-result-high:{call_token}")
            }
            Self::ExternalOutput {
                call_token,
                output_index,
            } => format!("external-output:{call_token}:{output_index}"),
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
                            let call_token = external_result_call_token(*call_token);
                            Some(format!("{bit}={inverse}external{call_token}.{source}"))
                        }
                        BitSource::ExternalResultHigh {
                            call_token,
                            bit: source,
                            inverted,
                        } => {
                            let inverse = if *inverted { "!" } else { "" };
                            Some(format!("{bit}={inverse}external{call_token}.high.{source}"))
                        }
                        BitSource::ExternalOutput {
                            call_token,
                            output_index,
                            bit: source,
                            inverted,
                        } => {
                            let inverse = if *inverted { "!" } else { "" };
                            Some(format!(
                                "{bit}={inverse}external{call_token}.output{output_index}.{source}"
                            ))
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

impl MemoryObjectLocation {
    fn with_index(self, argument: u8, stride: i64) -> Self {
        Self {
            root: MemoryObjectRoot::Indexed {
                root: std::sync::Arc::new(self.root),
                argument,
                stride,
            },
            offset: self.offset,
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
    fn memory_object_location_preserves_index_argument_and_stride() {
        let index = SymbolicValue::expression(
            ExpressionOperation::Multiply,
            SymbolicValue::input(0),
            SymbolicValue::Constant(0x2c),
        );
        let address = SymbolicValue::expression(
            ExpressionOperation::Add,
            index,
            SymbolicValue::Constant(0x1002_f560),
        )
        .add_constant(0x14);

        assert_eq!(
            address.memory_object_location_with_reads(&BTreeMap::new()),
            Some(MemoryObjectLocation {
                root: MemoryObjectRoot::Indexed {
                    root: std::sync::Arc::new(MemoryObjectRoot::Absolute {
                        address: 0x1002_f560,
                    }),
                    argument: 0,
                    stride: 0x2c,
                },
                offset: 0x14,
            })
        );
        assert_eq!(address.caller_memory_location(), None);
    }

    #[test]
    fn memory_object_location_preserves_byte_index_into_relocated_global() {
        let base = SymbolicValue::SymbolAddress {
            member: Some("table.o".to_owned()),
            symbol: ".LANCHOR1".to_owned(),
            hi_addend: 0,
            lo_addend: Some(0),
            post_offset: 0,
        };
        let address =
            SymbolicValue::expression(ExpressionOperation::Add, base, SymbolicValue::input(0));

        assert_eq!(
            address.memory_object_location_with_reads(&BTreeMap::new()),
            Some(MemoryObjectLocation {
                root: MemoryObjectRoot::Indexed {
                    root: std::sync::Arc::new(MemoryObjectRoot::RelocatedSymbol {
                        member: Some("table.o".to_owned()),
                        symbol: ".LANCHOR1".to_owned(),
                    }),
                    argument: 0,
                    stride: 1,
                },
                offset: 0,
            })
        );
    }

    #[test]
    fn memory_object_location_preserves_biased_byte_index_into_relocated_global() {
        let base = SymbolicValue::SymbolAddress {
            member: Some("table.o".to_owned()),
            symbol: ".LANCHOR2".to_owned(),
            hi_addend: 0,
            lo_addend: Some(0),
            post_offset: 0,
        };
        let biased_index = SymbolicValue::input(0).add_constant(u32::MAX);
        let address = SymbolicValue::expression(ExpressionOperation::Add, base, biased_index);

        assert_eq!(
            address.memory_object_location_with_reads(&BTreeMap::new()),
            Some(MemoryObjectLocation {
                root: MemoryObjectRoot::Indexed {
                    root: std::sync::Arc::new(MemoryObjectRoot::RelocatedSymbol {
                        member: Some("table.o".to_owned()),
                        symbol: ".LANCHOR2".to_owned(),
                    }),
                    argument: 0,
                    stride: 1,
                },
                offset: -1,
            })
        );
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
    fn memory_object_location_recovers_exact_dereferenced_pointer() {
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
                root: MemoryObjectRoot::Dereferenced {
                    pointer: std::sync::Arc::new(MemoryObjectRoot::RelocatedSymbol {
                        member: Some("globals.o".to_owned()),
                        symbol: "g_state".to_owned(),
                    }),
                    pointer_offset: 4,
                },
                offset: 0x1c,
            })
        );
    }

    #[test]
    fn memory_object_location_recovers_pointer_loaded_from_absolute_ram() {
        let reads = BTreeMap::from([(
            7,
            MemoryObjectLocation {
                root: MemoryObjectRoot::Absolute {
                    address: 0x2010_4000,
                },
                offset: 0,
            },
        )]);
        let address = SymbolicValue::memory_read(7, 32, false).add_constant(0x28);

        assert_eq!(
            address.memory_object_location_with_reads(&reads),
            Some(MemoryObjectLocation {
                root: MemoryObjectRoot::Dereferenced {
                    pointer: std::sync::Arc::new(MemoryObjectRoot::Absolute {
                        address: 0x2010_4000,
                    }),
                    pointer_offset: 0,
                },
                offset: 0x28,
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

    #[test]
    fn cloned_memory_roots_share_recursive_provenance() {
        let root = MemoryObjectRoot::Dereferenced {
            pointer: std::sync::Arc::new(MemoryObjectRoot::Indexed {
                root: std::sync::Arc::new(MemoryObjectRoot::Argument { index: 2 }),
                argument: 1,
                stride: 0x20,
            }),
            pointer_offset: 8,
        };
        let cloned = root.clone();
        let (
            MemoryObjectRoot::Dereferenced { pointer, .. },
            MemoryObjectRoot::Dereferenced {
                pointer: cloned_pointer,
                ..
            },
        ) = (&root, &cloned)
        else {
            unreachable!("fixture uses dereferenced roots")
        };
        assert!(std::sync::Arc::ptr_eq(pointer, cloned_pointer));
    }

    #[test]
    fn zeroed_allocation_keeps_affine_identity_without_exposing_token_flag() {
        let value = SymbolicValue::ExternalResult(ALLOCATED_EXTERNAL_RESULT_TOKEN_FLAG | 3)
            .add_constant(0x14);

        assert_eq!(
            value.memory_object_location_with_reads(&BTreeMap::new()),
            Some(MemoryObjectLocation {
                root: MemoryObjectRoot::ZeroedAllocation { call_token: 3 },
                offset: 0x14,
            })
        );
        assert!(!value.canonical().contains("107374"));

        let bits = SymbolicValue::from_bits(
            SymbolicValue::ExternalResult(ALLOCATED_EXTERNAL_RESULT_TOKEN_FLAG | 3).bits(),
        );
        let rendered = bits.canonical();
        assert!(rendered.contains("external3."), "{rendered}");
        assert!(!rendered.contains("107374"), "{rendered}");
    }
}
