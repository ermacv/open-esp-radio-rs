//! Fail-closed Rust generation for exact supported symbolic traces.
//!
//! The output is an executable reference model, not a guessed production
//! driver. It deliberately exposes ordered MMIO through a trait and reports an
//! unresolved return value as `None` instead of inventing a C prototype.

mod events;
mod flow;
mod value;

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
};

use crate::{
    BranchCondition, BranchOperation, ExternalFunctionRef, ExternalReturnModel, ExternalTableRef,
    IndexedMmioGuard, IndexedMmioRegister, MemoryAccess, ObservableEvent,
    RV32_MODELED_ARGUMENT_COUNT, RV32_REGISTER_ARGUMENT_COUNT, RV32_STACK_ARGUMENT_COUNT,
    ResolvedReferenceBody, ResolvedReferenceEvent, ResolvedReferenceFlow, ResolvedReferenceProgram,
    ResolvedReferenceTerminator, SECONDARY_CALL_RESULT_TOKEN_FLAG, SymbolicValue,
};
use events::render_events;
use flow::{FlowReturn, collect_external_tables, render_flow, render_outcome};
#[cfg(test)]
use value::render_value;
use value::{CallResultAvailability, MmioReadAddress, render_value_scoped};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GeneratedReference {
    pub source: String,
    pub exit_a0_modeled: bool,
}

pub fn reference_function_name(symbol: &str) -> String {
    let mut output = String::from("open_phy_reference_");
    for character in symbol.chars() {
        if character.is_ascii_alphanumeric() || character == '_' {
            output.push(character);
        } else {
            output.push('_');
        }
    }
    output
}

pub(in crate::codegen) fn comment_text(value: &str) -> String {
    value.replace(['\r', '\n'], " ")
}

#[derive(Clone, Debug)]
pub(in crate::codegen) struct RenderState {
    pub(in crate::codegen) reads: Vec<MmioReadAddress>,
    pub(in crate::codegen) mmio_access_count: usize,
    pub(in crate::codegen) memory_read_count: usize,
    pub(in crate::codegen) memory_access_count: usize,
    pub(in crate::codegen) bounded_poll_count: usize,
    pub(in crate::codegen) call_results: Vec<CallResultAvailability>,
    pub(in crate::codegen) external_results: Vec<ExternalFunctionRef>,
    pub(in crate::codegen) validated_external_tables: BTreeSet<ExternalTableRef>,
    pub(in crate::codegen) arguments: [String; RV32_MODELED_ARGUMENT_COUNT],
}

impl Default for RenderState {
    fn default() -> Self {
        Self {
            reads: Vec::new(),
            mmio_access_count: 0,
            memory_read_count: 0,
            memory_access_count: 0,
            bounded_poll_count: 0,
            call_results: Vec::new(),
            external_results: Vec::new(),
            validated_external_tables: BTreeSet::new(),
            arguments: core::array::from_fn(|index| format!("args[{index}]")),
        }
    }
}

pub(in crate::codegen) fn render_state_value(
    value: &SymbolicValue,
    state: &RenderState,
) -> Result<String, String> {
    render_value_scoped(
        value,
        &state.reads,
        state.memory_read_count,
        &state.call_results,
        state.external_results.len(),
        &state.arguments,
    )
}

pub(in crate::codegen) fn render_condition(
    condition: &BranchCondition,
    state: &RenderState,
) -> Result<String, String> {
    let left = render_state_value(&condition.left, state)?;
    let right = render_state_value(&condition.right, state)?;
    Ok(match condition.operation {
        BranchOperation::Equal => format!("({left}) == ({right})"),
        BranchOperation::NotEqual => format!("({left}) != ({right})"),
        BranchOperation::LessSigned => format!("(({left}) as i32) < (({right}) as i32)"),
        BranchOperation::GreaterEqualSigned => {
            format!("(({left}) as i32) >= (({right}) as i32)")
        }
        BranchOperation::LessUnsigned => format!("({left}) < ({right})"),
        BranchOperation::GreaterEqualUnsigned => format!("({left}) >= ({right})"),
    })
}

pub(in crate::codegen) fn render_indexed_mmio_address(
    output: &mut String,
    state: &mut RenderState,
    indent: &str,
    address: &SymbolicValue,
    registers: &[IndexedMmioRegister],
    guard: Option<&IndexedMmioGuard>,
) -> Result<usize, String> {
    let access_token = state.mmio_access_count;
    state.mmio_access_count += 1;
    if registers.is_empty() {
        return Err("indexed MMIO event has no SVD register domain".to_owned());
    }
    if let Some(guard) = guard {
        let selector = render_state_value(&guard.selector, state)?;
        writeln!(
            output,
            "{indent}let mmio_selector{access_token} = {selector};"
        )
        .unwrap();
        writeln!(
            output,
            "{indent}assert!(mmio_selector{access_token} <= {:#010x}_u32, \"indexed MMIO selector is outside the recovered SVD register bank\");",
            guard.maximum
        )
        .unwrap();
    }
    let address = render_state_value(address, state)?;
    let domain = registers
        .iter()
        .map(|register| format!("{:#010x}_u32", register.address))
        .collect::<Vec<_>>()
        .join(" | ");
    let names = registers
        .iter()
        .map(|register| register.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    writeln!(
        output,
        "{indent}// Indexed MMIO SVD bank: {}.",
        comment_text(&names)
    )
    .unwrap();
    writeln!(
        output,
        "{indent}let mmio_address{access_token} = {address};"
    )
    .unwrap();
    writeln!(
        output,
        "{indent}assert!(matches!(mmio_address{access_token}, {domain}), \"indexed MMIO address is outside the recovered SVD register bank\");"
    )
    .unwrap();
    Ok(access_token)
}

pub fn generate(
    trace: &ResolvedReferenceProgram,
    artifact: &str,
    artifact_sha256: &str,
    member: Option<&str>,
    companions: &[(String, String)],
) -> Result<GeneratedReference, String> {
    let function_name = reference_function_name(&trace.symbol);
    let exit_a0_modeled = trace.exit_return_modeled;
    let mut output = String::new();
    writeln!(
        output,
        "// @generated by vendor-code-validator; do not edit."
    )
    .unwrap();
    writeln!(
        output,
        "// Generator version: {}",
        env!("CARGO_PKG_VERSION")
    )
    .unwrap();
    writeln!(output, "// Source artifact: {}", comment_text(artifact)).unwrap();
    writeln!(output, "// Source SHA-256: {artifact_sha256}").unwrap();
    if let Some(member) = member {
        writeln!(output, "// Archive member: {}", comment_text(member)).unwrap();
    }
    for (path, sha256) in companions {
        writeln!(output, "// Companion artifact: {}", comment_text(path)).unwrap();
        writeln!(output, "// Companion SHA-256: {sha256}").unwrap();
    }
    writeln!(output, "// Source symbol: {}", comment_text(&trace.symbol)).unwrap();
    for dependency in &trace.dependencies {
        writeln!(
            output,
            "// Composed direct-call dependency: {}",
            comment_text(dependency)
        )
        .unwrap();
    }
    let mut external_tables = BTreeSet::new();
    match &trace.body {
        ResolvedReferenceBody::Linear { events, .. } => {
            for event in events {
                match event {
                    ResolvedReferenceEvent::ExternalCall { table, .. } => {
                        external_tables.insert(*table);
                    }
                    ResolvedReferenceEvent::ComposedCall { flow, .. } => {
                        collect_external_tables(flow, &mut external_tables);
                    }
                    _ => {}
                }
            }
        }
        ResolvedReferenceBody::Flow(flow) => {
            collect_external_tables(flow, &mut external_tables);
        }
    }
    for table in external_tables {
        let spec = table.spec();
        writeln!(output, "// External ABI: {}", spec.id).unwrap();
        writeln!(output, "// External ABI pointer: {}", spec.pointer_symbol).unwrap();
        writeln!(output, "// External ABI backing: {}", spec.backing_symbol).unwrap();
        writeln!(output, "// External ABI version: {:#010x}", spec.version).unwrap();
        writeln!(output, "// External ABI magic: {:#010x}", spec.magic).unwrap();
        writeln!(output, "// External ABI size: {:#x}", spec.size).unwrap();
        writeln!(
            output,
            "// External ABI magic offset: {:#x}",
            spec.magic_offset
        )
        .unwrap();
    }
    writeln!(
        output,
        "// Exit a0: {}",
        if exit_a0_modeled {
            "modeled"
        } else {
            "unresolved"
        }
    )
    .unwrap();
    writeln!(output).unwrap();
    writeln!(
        output,
        "/// Ordered MMIO/delay/fence boundary used by the generated reference model."
    )
    .unwrap();
    writeln!(output, "#[allow(dead_code)]").unwrap();
    writeln!(output, "pub trait ReferenceIo {{").unwrap();
    writeln!(
        output,
        "    /// Returns the zero-extended value observed by a read of `width` bits."
    )
    .unwrap();
    writeln!(
        output,
        "    fn read(&mut self, width: u8, address: u32) -> u32;"
    )
    .unwrap();
    writeln!(
        output,
        "    /// Records a write; only the low `width` bits are observable."
    )
    .unwrap();
    writeln!(
        output,
        "    fn write(&mut self, width: u8, address: u32, value: u32);"
    )
    .unwrap();
    writeln!(output, "    fn delay_micros(&mut self, micros: u32);").unwrap();
    writeln!(
        output,
        "    fn fence(&mut self, fm: u8, predecessor: u8, successor: u8);"
    )
    .unwrap();
    writeln!(output, "}}").unwrap();
    writeln!(output).unwrap();
    writeln!(
        output,
        "/// CPU-visible ELF/RAM state used by the generated reference model."
    )
    .unwrap();
    writeln!(
        output,
        "/// Implementations must reject ABI-derived addresses outside declared CPU-owned ranges."
    )
    .unwrap();
    writeln!(
        output,
        "/// MMIO and undeclared, interrupt-owned, DMA-owned or shared memory are not valid here."
    )
    .unwrap();
    writeln!(output, "#[allow(dead_code)]").unwrap();
    writeln!(output, "pub trait ReferenceMemory {{").unwrap();
    writeln!(
        output,
        "    /// Resolves an archive/ELF symbol in the exact linked image used by the scenario."
    )
    .unwrap();
    writeln!(
        output,
        "    fn symbol_address(&mut self, member: Option<&str>, symbol: &str) -> u32;"
    )
    .unwrap();
    writeln!(
        output,
        "    /// Returns the zero-extended value currently stored in `width` bits."
    )
    .unwrap();
    writeln!(
        output,
        "    fn read(&mut self, width: u8, address: u32) -> u32;"
    )
    .unwrap();
    writeln!(
        output,
        "    /// Updates only the low `width` bits at the addressed location."
    )
    .unwrap();
    writeln!(
        output,
        "    fn write(&mut self, width: u8, address: u32, value: u32);"
    )
    .unwrap();
    writeln!(output, "}}").unwrap();
    writeln!(output).unwrap();
    writeln!(output, "#[allow(dead_code)]").unwrap();
    writeln!(
        output,
        "struct ReferenceScratchMemory<'a, M: ReferenceMemory> {{"
    )
    .unwrap();
    writeln!(output, "    inner: &'a mut M,").unwrap();
    writeln!(output, "    base: u32,").unwrap();
    writeln!(output, "    len: usize,").unwrap();
    writeln!(output, "    bytes: [u8; 256],").unwrap();
    writeln!(output, "    initialized: [bool; 256],").unwrap();
    writeln!(output, "}}").unwrap();
    writeln!(output, "#[allow(dead_code)]").unwrap();
    writeln!(
        output,
        "impl<'a, M: ReferenceMemory> ReferenceScratchMemory<'a, M> {{"
    )
    .unwrap();
    writeln!(
        output,
        "    fn new(inner: &'a mut M, base: u32, len: u16) -> Self {{"
    )
    .unwrap();
    writeln!(
        output,
        "        assert!(len != 0 && len <= 256, \"reference scratch size is outside 1..=256\");"
    )
    .unwrap();
    writeln!(output, "        Self {{ inner, base, len: usize::from(len), bytes: [0; 256], initialized: [false; 256] }}").unwrap();
    writeln!(output, "    }}").unwrap();
    writeln!(
        output,
        "    fn local_range(&self, width: u8, address: u32) -> Option<core::ops::Range<usize>> {{"
    )
    .unwrap();
    writeln!(output, "        let byte_count = match width {{ 8 => 1_u32, 16 => 2, 32 => 4, _ => panic!(\"unsupported reference scratch width {{width}}\") }};").unwrap();
    writeln!(output, "        let end = address.checked_add(byte_count).expect(\"reference scratch address overflow\");").unwrap();
    writeln!(output, "        let limit = self.base.checked_add(self.len as u32).expect(\"reference scratch limit overflow\");").unwrap();
    writeln!(
        output,
        "        let inside = address >= self.base && end <= limit;"
    )
    .unwrap();
    writeln!(
        output,
        "        let disjoint = end <= self.base || address >= limit;"
    )
    .unwrap();
    writeln!(output, "        assert!(inside || disjoint, \"reference memory access partially overlaps private scratch\");").unwrap();
    writeln!(
        output,
        "        inside.then(|| (address - self.base) as usize..(end - self.base) as usize)"
    )
    .unwrap();
    writeln!(output, "    }}").unwrap();
    writeln!(output, "}}").unwrap();
    writeln!(output, "#[allow(dead_code)]").unwrap();
    writeln!(
        output,
        "impl<M: ReferenceMemory> ReferenceMemory for ReferenceScratchMemory<'_, M> {{"
    )
    .unwrap();
    writeln!(output, "    fn symbol_address(&mut self, member: Option<&str>, symbol: &str) -> u32 {{ self.inner.symbol_address(member, symbol) }}").unwrap();
    writeln!(
        output,
        "    fn read(&mut self, width: u8, address: u32) -> u32 {{"
    )
    .unwrap();
    writeln!(output, "        let Some(range) = self.local_range(width, address) else {{ return self.inner.read(width, address); }};").unwrap();
    writeln!(output, "        assert!(range.clone().all(|index| self.initialized[index]), \"read from uninitialized reference scratch\");").unwrap();
    writeln!(output, "        range.enumerate().fold(0_u32, |value, (shift, index)| value | (u32::from(self.bytes[index]) << (shift * 8)))").unwrap();
    writeln!(output, "    }}").unwrap();
    writeln!(
        output,
        "    fn write(&mut self, width: u8, address: u32, value: u32) {{"
    )
    .unwrap();
    writeln!(output, "        let Some(range) = self.local_range(width, address) else {{ self.inner.write(width, address, value); return; }};").unwrap();
    writeln!(output, "        for (shift, index) in range.enumerate() {{ self.bytes[index] = (value >> (shift * 8)) as u8; self.initialized[index] = true; }}").unwrap();
    writeln!(output, "    }}").unwrap();
    writeln!(output, "}}").unwrap();
    writeln!(output).unwrap();
    writeln!(
        output,
        "/// Harness-reviewed external ABI and diagnostic boundaries."
    )
    .unwrap();
    writeln!(output, "#[allow(dead_code)]").unwrap();
    writeln!(output, "pub trait ReferencePlatform {{").unwrap();
    writeln!(
        output,
        "    fn external_table_version(&mut self, table: &str) -> u32;"
    )
    .unwrap();
    writeln!(
        output,
        "    fn external_table_magic(&mut self, table: &str) -> u32;"
    )
    .unwrap();
    writeln!(
        output,
        "    fn external_table_size(&mut self, table: &str) -> u32;"
    )
    .unwrap();
    writeln!(
        output,
        "    /// Returns the modeled result selected by the harness contract."
    )
    .unwrap();
    writeln!(
        output,
        "    fn external_call(&mut self, table: &str, function: &str, arguments: &[u32]) -> u32;"
    )
    .unwrap();
    writeln!(
        output,
        "    fn diagnostic_call(&mut self, function: &str, arguments: &[u32]);"
    )
    .unwrap();
    writeln!(output, "}}").unwrap();
    writeln!(output).unwrap();
    writeln!(output, "#[allow(dead_code)]").unwrap();
    writeln!(
        output,
        "fn riscv_hi20_lo12_address(symbol: u32, hi_addend: u32, lo_addend: u32) -> u32 {{"
    )
    .unwrap();
    writeln!(
        output,
        "    let high = symbol.wrapping_add(hi_addend).wrapping_add(0x00000800) & 0xfffff000;"
    )
    .unwrap();
    writeln!(
        output,
        "    let low = ((symbol.wrapping_add(lo_addend).wrapping_shl(20) as i32) >> 20) as u32;"
    )
    .unwrap();
    writeln!(output, "    high.wrapping_add(low)").unwrap();
    writeln!(output, "}}").unwrap();
    writeln!(output, "#[allow(dead_code)]").unwrap();
    writeln!(output, "fn riscv_div(left: u32, right: u32) -> u32 {{").unwrap();
    writeln!(output, "    if right == 0 {{ u32::MAX }} else if left == i32::MIN as u32 && right == u32::MAX {{ i32::MIN as u32 }} else {{ ((left as i32) / (right as i32)) as u32 }}").unwrap();
    writeln!(output, "}}").unwrap();
    writeln!(output, "#[allow(dead_code)]").unwrap();
    writeln!(output, "fn riscv_divu(left: u32, right: u32) -> u32 {{").unwrap();
    writeln!(output, "    left.checked_div(right).unwrap_or(u32::MAX)").unwrap();
    writeln!(output, "}}").unwrap();
    writeln!(output, "#[allow(dead_code)]").unwrap();
    writeln!(
        output,
        "fn riscv_div_i64_words(dividend_low: u32, dividend_high: u32, divisor_low: u32, divisor_high: u32) -> (u32, u32) {{"
    )
    .unwrap();
    writeln!(
        output,
        "    let dividend = (((dividend_high as u64) << 32) | dividend_low as u64) as i64;"
    )
    .unwrap();
    writeln!(
        output,
        "    let divisor = (((divisor_high as u64) << 32) | divisor_low as u64) as i64;"
    )
    .unwrap();
    writeln!(
        output,
        "    assert!(divisor != 0, \"modeled __divdi3 precondition violated: divisor is zero\");"
    )
    .unwrap();
    writeln!(
        output,
        "    let quotient = dividend.wrapping_div(divisor) as u64;"
    )
    .unwrap();
    writeln!(output, "    (quotient as u32, (quotient >> 32) as u32)").unwrap();
    writeln!(output, "}}").unwrap();
    writeln!(output, "#[allow(dead_code)]").unwrap();
    writeln!(output, "fn riscv_rem(left: u32, right: u32) -> u32 {{").unwrap();
    writeln!(output, "    if right == 0 {{ left }} else if left == i32::MIN as u32 && right == u32::MAX {{ 0 }} else {{ ((left as i32) % (right as i32)) as u32 }}").unwrap();
    writeln!(output, "}}").unwrap();
    writeln!(output, "#[allow(dead_code)]").unwrap();
    writeln!(output, "fn riscv_remu(left: u32, right: u32) -> u32 {{").unwrap();
    writeln!(
        output,
        "    if right == 0 {{ left }} else {{ left % right }}"
    )
    .unwrap();
    writeln!(output, "}}").unwrap();
    writeln!(output).unwrap();
    writeln!(output, "#[derive(Clone, Copy, Debug, Eq, PartialEq)]").unwrap();
    writeln!(output, "pub struct ReferenceOutcome {{").unwrap();
    writeln!(
        output,
        "    /// SymbolicValue of the ABI `a0` register at exit; this does not infer a C prototype."
    )
    .unwrap();
    writeln!(output, "    pub exit_a0: Option<u32>,").unwrap();
    writeln!(output, "}}").unwrap();
    writeln!(output).unwrap();
    writeln!(output, "#[derive(Clone, Copy, Debug, Eq, PartialEq)]").unwrap();
    writeln!(output, "pub struct Rv32ReferenceArguments {{").unwrap();
    writeln!(
        output,
        "    pub registers: [u32; {RV32_REGISTER_ARGUMENT_COUNT}],"
    )
    .unwrap();
    writeln!(output, "    pub stack: [u32; {RV32_STACK_ARGUMENT_COUNT}],").unwrap();
    writeln!(output, "}}").unwrap();
    writeln!(
        output,
        "impl core::ops::Index<usize> for Rv32ReferenceArguments {{"
    )
    .unwrap();
    writeln!(output, "    type Output = u32;").unwrap();
    writeln!(
        output,
        "    fn index(&self, index: usize) -> &Self::Output {{"
    )
    .unwrap();
    writeln!(output, "        if index < {RV32_REGISTER_ARGUMENT_COUNT} {{ &self.registers[index] }} else {{ &self.stack[index - {RV32_REGISTER_ARGUMENT_COUNT}] }}").unwrap();
    writeln!(output, "    }}").unwrap();
    writeln!(output, "}}").unwrap();
    writeln!(output).unwrap();
    writeln!(output, "#[allow(dead_code, non_snake_case)]").unwrap();
    writeln!(output, "#[inline(always)]").unwrap();
    writeln!(output, "pub fn {function_name}(").unwrap();
    writeln!(output, "    io: &mut impl ReferenceIo,").unwrap();
    writeln!(output, "    memory: &mut impl ReferenceMemory,").unwrap();
    writeln!(output, "    platform: &mut impl ReferencePlatform,").unwrap();
    writeln!(output, "    args: Rv32ReferenceArguments,").unwrap();
    writeln!(output, ") -> ReferenceOutcome {{").unwrap();
    writeln!(output, "    let _ = &mut *io;").unwrap();
    writeln!(output, "    let _ = &mut *memory;").unwrap();
    writeln!(output, "    let _ = &mut *platform;").unwrap();
    writeln!(output, "    let _ = &args;").unwrap();

    let mut state = RenderState::default();
    match &trace.body {
        ResolvedReferenceBody::Flow(flow) => {
            render_flow(&mut output, flow, state, "    ", FlowReturn::Outcome)?;
        }
        ResolvedReferenceBody::Linear {
            events,
            return_value,
        } => {
            render_events(&mut output, events, &mut state, "    ")?;
            render_outcome(&mut output, return_value, &state, "    ")?;
        }
    }
    writeln!(output, "}}").unwrap();

    Ok(GeneratedReference {
        source: output,
        exit_a0_modeled,
    })
}

#[cfg(test)]
mod tests;
