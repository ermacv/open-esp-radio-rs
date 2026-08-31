//! Token and caller/private-stack context substitution.

use super::*;

fn remap_external_result_token(
    call_token: u32,
    external_tokens: &[u32],
    scope: &str,
) -> std::result::Result<u32, String> {
    let source_token = external_result_call_token(call_token);
    let provenance_flags = call_token ^ source_token;
    external_tokens
        .get(source_token as usize)
        .map(|mapped_token| *mapped_token | provenance_flags)
        .ok_or_else(|| format!("{scope} external-call token {source_token} has no caller mapping"))
}

impl SymbolicValue {
    pub fn substitute(
        &self,
        arguments: &[SymbolicValue],
        read_tokens: &[u32],
        memory_read_tokens: &[u32],
        external_tokens: &[u32],
    ) -> std::result::Result<Self, String> {
        if let Some(index) = self.direct_input_index() {
            return arguments
                .get(usize::from(index))
                .cloned()
                .ok_or_else(|| format!("call argument {index} is outside the modeled call"));
        }
        if let Self::SymbolAddress { lo_addend, .. } = self {
            return lo_addend
                .is_some()
                .then(|| self.clone())
                .ok_or_else(|| "incomplete relocation escaped across a call boundary".to_owned());
        }
        if matches!(self, Self::FunctionTable(_) | Self::FunctionPointer { .. }) {
            return Ok(self.clone());
        }
        if let Self::ExternalResult(call_token) = self {
            return Ok(Self::ExternalResult(remap_external_result_token(
                *call_token,
                external_tokens,
                "callee",
            )?));
        }
        if let Self::Expression {
            operation,
            left,
            right,
            ..
        } = self
        {
            return Ok(Self::expression_unbounded(
                *operation,
                left.substitute(arguments, read_tokens, memory_read_tokens, external_tokens)?,
                right.substitute(arguments, read_tokens, memory_read_tokens, external_tokens)?,
            ));
        }
        if let Self::FloatingPoint {
            operation,
            rounding,
            operands,
        } = self
        {
            let operands = operands
                .iter()
                .map(|operand| {
                    operand.substitute(arguments, read_tokens, memory_read_tokens, external_tokens)
                })
                .collect::<std::result::Result<Vec<_>, _>>()?;
            return Ok(Self::floating_point(*operation, *rounding, operands));
        }
        if let Self::WideSignedDivide {
            dividend_low,
            dividend_high,
            divisor_low,
            divisor_high,
            high_word,
        } = self
        {
            return Ok(Self::WideSignedDivide {
                dividend_low: Arc::new(dividend_low.substitute(
                    arguments,
                    read_tokens,
                    memory_read_tokens,
                    external_tokens,
                )?),
                dividend_high: Arc::new(dividend_high.substitute(
                    arguments,
                    read_tokens,
                    memory_read_tokens,
                    external_tokens,
                )?),
                divisor_low: Arc::new(divisor_low.substitute(
                    arguments,
                    read_tokens,
                    memory_read_tokens,
                    external_tokens,
                )?),
                divisor_high: Arc::new(divisor_high.substitute(
                    arguments,
                    read_tokens,
                    memory_read_tokens,
                    external_tokens,
                )?),
                high_word: *high_word,
            });
        }
        if let Self::StackAddress(_) = self {
            return Ok(self.clone());
        }
        if matches!(
            self,
            Self::ReviewedExternalTable(_) | Self::ReviewedExternalFunction { .. }
        ) {
            return Err("non-scalar value escaped across a call boundary".to_owned());
        }
        let bits = self.bits();
        let mut substituted = [BitSource::Unknown; 32];
        for (destination, source) in bits.into_iter().enumerate() {
            substituted[destination] = match source {
                BitSource::Unknown => BitSource::Unknown,
                BitSource::Constant(value) => BitSource::Constant(value),
                BitSource::Input {
                    index,
                    bit,
                    inverted,
                } => {
                    let argument = arguments.get(usize::from(index)).ok_or_else(|| {
                        format!("call argument {index} is outside the modeled call")
                    })?;
                    let source = argument.bits()[usize::from(bit)];
                    if inverted { source.inverted() } else { source }
                }
                BitSource::Register {
                    read_token,
                    address,
                    bit,
                    inverted,
                } => BitSource::Register {
                    read_token: *read_tokens.get(read_token as usize).ok_or_else(|| {
                        format!("callee MMIO read token {read_token} has no caller mapping")
                    })?,
                    address,
                    bit,
                    inverted,
                },
                BitSource::IndexedRegister {
                    read_token,
                    bit,
                    inverted,
                } => BitSource::IndexedRegister {
                    read_token: *read_tokens.get(read_token as usize).ok_or_else(|| {
                        format!("callee MMIO read token {read_token} has no caller mapping")
                    })?,
                    bit,
                    inverted,
                },
                BitSource::Memory {
                    read_token,
                    bit,
                    inverted,
                } => {
                    let read_token =
                        *memory_read_tokens.get(read_token as usize).ok_or_else(|| {
                            format!("callee memory read token {read_token} has no caller mapping")
                        })?;
                    if read_token & PRIVATE_STACK_READ_TOKEN_FLAG != 0 {
                        BitSource::PrivateStack {
                            read_token: read_token & !PRIVATE_STACK_READ_TOKEN_FLAG,
                            bit,
                            inverted,
                        }
                    } else {
                        BitSource::Memory {
                            read_token,
                            bit,
                            inverted,
                        }
                    }
                }
                BitSource::PrivateStack { .. } => {
                    return Err(
                        "callee private-stack read escaped across a call boundary".to_owned()
                    );
                }
                BitSource::CallResult {
                    call_token,
                    bit,
                    inverted,
                } => BitSource::CallResult {
                    call_token,
                    bit,
                    inverted,
                },
                BitSource::ExternalResult {
                    call_token,
                    bit,
                    inverted,
                } => BitSource::ExternalResult {
                    call_token: remap_external_result_token(call_token, external_tokens, "callee")?,
                    bit,
                    inverted,
                },
                BitSource::ExternalResultHigh {
                    call_token,
                    bit,
                    inverted,
                } => BitSource::ExternalResultHigh {
                    call_token: *external_tokens.get(call_token as usize).ok_or_else(|| {
                        format!("callee external-call token {call_token} has no caller mapping")
                    })?,
                    bit,
                    inverted,
                },
                BitSource::ExternalOutput {
                    call_token,
                    output_index,
                    bit,
                    inverted,
                } => BitSource::ExternalOutput {
                    call_token: *external_tokens.get(call_token as usize).ok_or_else(|| {
                        format!("callee external-call token {call_token} has no caller mapping")
                    })?,
                    output_index,
                    bit,
                    inverted,
                },
            };
        }
        Ok(Self::from_bits(substituted))
    }

    pub fn rewrite_call_context(
        &self,
        read_tokens: &[u32],
        memory_read_tokens: &[u32],
        external_tokens: &[u32],
        call_results: &BTreeMap<u32, SymbolicValue>,
        private_stack_reads: &BTreeMap<u32, SymbolicValue>,
    ) -> std::result::Result<Self, String> {
        // Preserve the replacement as a symbolic tree when the caller uses a
        // whole call result. Converting it through `bits()` first destroys
        // expressions (their bit image is intentionally unknown). Values
        // which already select or transform individual result bits continue
        // through the bit-source rewrite below.
        if let Self::CallResult(call_token) = self {
            return call_results
                .get(call_token)
                .cloned()
                .ok_or_else(|| format!("call result {call_token} is not available"));
        }
        if let Self::SymbolAddress { lo_addend, .. } = self {
            return lo_addend
                .is_some()
                .then(|| self.clone())
                .ok_or_else(|| "incomplete relocation escaped across a call boundary".to_owned());
        }
        if matches!(self, Self::FunctionTable(_) | Self::FunctionPointer { .. }) {
            return Ok(self.clone());
        }
        if let Self::ExternalResult(call_token) = self {
            return Ok(Self::ExternalResult(remap_external_result_token(
                *call_token,
                external_tokens,
                "caller",
            )?));
        }
        if let Self::Expression {
            operation,
            left,
            right,
            ..
        } = self
        {
            return Ok(Self::expression_unbounded(
                *operation,
                left.rewrite_call_context(
                    read_tokens,
                    memory_read_tokens,
                    external_tokens,
                    call_results,
                    private_stack_reads,
                )?,
                right.rewrite_call_context(
                    read_tokens,
                    memory_read_tokens,
                    external_tokens,
                    call_results,
                    private_stack_reads,
                )?,
            ));
        }
        if let Self::FloatingPoint {
            operation,
            rounding,
            operands,
        } = self
        {
            let operands = operands
                .iter()
                .map(|operand| {
                    operand.rewrite_call_context(
                        read_tokens,
                        memory_read_tokens,
                        external_tokens,
                        call_results,
                        private_stack_reads,
                    )
                })
                .collect::<std::result::Result<Vec<_>, _>>()?;
            return Ok(Self::floating_point(*operation, *rounding, operands));
        }
        if let Self::WideSignedDivide {
            dividend_low,
            dividend_high,
            divisor_low,
            divisor_high,
            high_word,
        } = self
        {
            return Ok(Self::WideSignedDivide {
                dividend_low: Arc::new(dividend_low.rewrite_call_context(
                    read_tokens,
                    memory_read_tokens,
                    external_tokens,
                    call_results,
                    private_stack_reads,
                )?),
                dividend_high: Arc::new(dividend_high.rewrite_call_context(
                    read_tokens,
                    memory_read_tokens,
                    external_tokens,
                    call_results,
                    private_stack_reads,
                )?),
                divisor_low: Arc::new(divisor_low.rewrite_call_context(
                    read_tokens,
                    memory_read_tokens,
                    external_tokens,
                    call_results,
                    private_stack_reads,
                )?),
                divisor_high: Arc::new(divisor_high.rewrite_call_context(
                    read_tokens,
                    memory_read_tokens,
                    external_tokens,
                    call_results,
                    private_stack_reads,
                )?),
                high_word: *high_word,
            });
        }
        if let Self::StackAddress(_) = self {
            return Ok(self.clone());
        }
        if matches!(
            self,
            Self::ReviewedExternalTable(_) | Self::ReviewedExternalFunction { .. }
        ) {
            return Err("non-scalar value escaped across a call boundary".to_owned());
        }
        let bits = self.bits();
        let mut rewritten = [BitSource::Unknown; 32];
        for (destination, source) in bits.into_iter().enumerate() {
            rewritten[destination] = match source {
                BitSource::Unknown => BitSource::Unknown,
                BitSource::Constant(value) => BitSource::Constant(value),
                BitSource::Input {
                    index,
                    bit,
                    inverted,
                } => BitSource::Input {
                    index,
                    bit,
                    inverted,
                },
                BitSource::Register {
                    read_token,
                    address,
                    bit,
                    inverted,
                } => BitSource::Register {
                    read_token: *read_tokens.get(read_token as usize).ok_or_else(|| {
                        format!("caller MMIO read token {read_token} has no flattened mapping")
                    })?,
                    address,
                    bit,
                    inverted,
                },
                BitSource::IndexedRegister {
                    read_token,
                    bit,
                    inverted,
                } => BitSource::IndexedRegister {
                    read_token: *read_tokens.get(read_token as usize).ok_or_else(|| {
                        format!("caller MMIO read token {read_token} has no flattened mapping")
                    })?,
                    bit,
                    inverted,
                },
                BitSource::Memory {
                    read_token,
                    bit,
                    inverted,
                } => BitSource::Memory {
                    read_token: *memory_read_tokens.get(read_token as usize).ok_or_else(|| {
                        format!("caller memory read token {read_token} has no flattened mapping")
                    })?,
                    bit,
                    inverted,
                },
                BitSource::PrivateStack {
                    read_token,
                    bit,
                    inverted,
                } => {
                    let value = private_stack_reads.get(&read_token).ok_or_else(|| {
                        format!("private-stack read {read_token} is not available")
                    })?;
                    let source = value.bits()[usize::from(bit)];
                    if inverted { source.inverted() } else { source }
                }
                BitSource::CallResult {
                    call_token,
                    bit,
                    inverted,
                } => {
                    let result = call_results
                        .get(&call_token)
                        .ok_or_else(|| format!("call result {call_token} is not available"))?;
                    let source = result.bits()[usize::from(bit)];
                    if inverted { source.inverted() } else { source }
                }
                BitSource::ExternalResult {
                    call_token,
                    bit,
                    inverted,
                } => BitSource::ExternalResult {
                    call_token: remap_external_result_token(call_token, external_tokens, "caller")?,
                    bit,
                    inverted,
                },
                BitSource::ExternalResultHigh {
                    call_token,
                    bit,
                    inverted,
                } => BitSource::ExternalResultHigh {
                    call_token: *external_tokens.get(call_token as usize).ok_or_else(|| {
                        format!("caller external-call token {call_token} has no flattened mapping")
                    })?,
                    bit,
                    inverted,
                },
                BitSource::ExternalOutput {
                    call_token,
                    output_index,
                    bit,
                    inverted,
                } => BitSource::ExternalOutput {
                    call_token: *external_tokens.get(call_token as usize).ok_or_else(|| {
                        format!("caller external-call token {call_token} has no flattened mapping")
                    })?,
                    output_index,
                    bit,
                    inverted,
                },
            };
        }
        Ok(Self::from_bits(rewritten))
    }

    pub fn rewrite_private_stack_context(
        &self,
        private_stack_reads: &BTreeMap<u32, SymbolicValue>,
    ) -> std::result::Result<Self, String> {
        if let Self::Expression {
            operation,
            left,
            right,
            ..
        } = self
        {
            return Ok(Self::expression_unbounded(
                *operation,
                left.rewrite_private_stack_context(private_stack_reads)?,
                right.rewrite_private_stack_context(private_stack_reads)?,
            ));
        }
        if let Self::FloatingPoint {
            operation,
            rounding,
            operands,
        } = self
        {
            let operands = operands
                .iter()
                .map(|operand| operand.rewrite_private_stack_context(private_stack_reads))
                .collect::<std::result::Result<Vec<_>, _>>()?;
            return Ok(Self::floating_point(*operation, *rounding, operands));
        }
        if let Self::WideSignedDivide {
            dividend_low,
            dividend_high,
            divisor_low,
            divisor_high,
            high_word,
        } = self
        {
            return Ok(Self::WideSignedDivide {
                dividend_low: Arc::new(
                    dividend_low.rewrite_private_stack_context(private_stack_reads)?,
                ),
                dividend_high: Arc::new(
                    dividend_high.rewrite_private_stack_context(private_stack_reads)?,
                ),
                divisor_low: Arc::new(
                    divisor_low.rewrite_private_stack_context(private_stack_reads)?,
                ),
                divisor_high: Arc::new(
                    divisor_high.rewrite_private_stack_context(private_stack_reads)?,
                ),
                high_word: *high_word,
            });
        }
        if matches!(
            self,
            Self::SymbolAddress { .. }
                | Self::StackAddress(_)
                | Self::ReviewedExternalTable(_)
                | Self::ReviewedExternalFunction { .. }
                | Self::FunctionTable(_)
                | Self::FunctionPointer { .. }
        ) {
            return Ok(self.clone());
        }
        let mut rewritten = [BitSource::Unknown; 32];
        for (destination, source) in self.bits().into_iter().enumerate() {
            rewritten[destination] = match source {
                BitSource::PrivateStack {
                    read_token,
                    bit,
                    inverted,
                } => {
                    let value = private_stack_reads.get(&read_token).ok_or_else(|| {
                        format!("private-stack read {read_token} is not available")
                    })?;
                    let source = value.bits()[usize::from(bit)];
                    if inverted { source.inverted() } else { source }
                }
                other => other,
            };
        }
        Ok(Self::from_bits(rewritten))
    }
}
