//! Concrete fail-closed RV32 interpreter state and execution facade.

mod events;
mod memory;
mod step;

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use rv_asm::Reg;

use super::{
    ExecutableImage, ExecutionEvent, ExecutionResult, ExecutionTimelineEvent, IndirectCall,
    MemoryAlias, MemoryOwnership, MemoryRange, OrderedCall, RETURN_SENTINEL, STACK_POINTER,
    Scenario,
};
use crate::{MmioRegisterMap, Result};

#[cfg(test)]
pub(super) use step::atomic_word_result;

pub(super) struct Machine<'a> {
    pub(super) image: &'a ExecutableImage,
    pub(super) svd: &'a MmioRegisterMap,
    pub(super) registers: [u32; 32],
    pub(super) pc: u32,
    pub(super) overlay: BTreeMap<u32, u8>,
    pub(super) initial_overlay: BTreeMap<u32, u8>,
    pub(super) private_stack_fill: Option<u8>,
    pub(super) observed_memory: Vec<MemoryRange>,
    pub(super) memory_aliases: Vec<MemoryAlias>,
    pub(super) persistent_memory: Vec<MemoryRange>,
    pub(super) memory_ownership: Vec<MemoryOwnership>,
    /// Explicit stable read values supplied by the scenario. Bus writes do
    /// not update this map: storage/W1C/FIFO semantics belong to an explicit
    /// peripheral model, not to the generic transaction recorder.
    pub(super) mmio_read_seeds: BTreeMap<u32, u32>,
    pub(super) mmio_reads: BTreeMap<u32, VecDeque<u32>>,
    pub(super) events: Vec<ExecutionEvent>,
    pub(super) timeline: Vec<ExecutionTimelineEvent>,
    pub(super) branches: BTreeSet<(u32, bool)>,
    pub(super) ordered_branches: Vec<(u32, bool)>,
    pub(super) calls: BTreeSet<String>,
    pub(super) ordered_calls: Vec<OrderedCall>,
    pub(super) indirect_calls: BTreeSet<IndirectCall>,
    pub(super) call_returns: BTreeMap<String, VecDeque<u32>>,
    pub(super) steps: u64,
    pub(super) max_steps: u64,
}

impl<'a> Machine<'a> {
    pub(super) fn new(
        image: &'a ExecutableImage,
        svd: &'a MmioRegisterMap,
        start: u32,
        scenario: Scenario,
    ) -> Self {
        let mut registers = [0_u32; 32];
        registers[usize::from(Reg::RA.0)] = RETURN_SENTINEL;
        registers[usize::from(Reg::SP.0)] = STACK_POINTER;
        if let Some(global_pointer) = image.global_pointer {
            registers[usize::from(Reg::GP.0)] = global_pointer;
        }
        for (index, value) in scenario.arguments.into_iter().take(8).enumerate() {
            registers[10 + index] = value;
        }
        let initial_overlay = scenario.memory_initial;
        Self {
            image,
            svd,
            registers,
            pc: start,
            overlay: initial_overlay.clone(),
            initial_overlay,
            private_stack_fill: scenario.private_stack_fill,
            observed_memory: scenario.observed_memory,
            memory_aliases: scenario.memory_aliases,
            persistent_memory: scenario.persistent_memory,
            memory_ownership: scenario.memory_ownership,
            mmio_read_seeds: scenario.mmio_initial,
            mmio_reads: scenario.mmio_reads,
            events: Vec::new(),
            timeline: Vec::new(),
            branches: BTreeSet::new(),
            ordered_branches: Vec::new(),
            calls: BTreeSet::new(),
            ordered_calls: Vec::new(),
            indirect_calls: BTreeSet::new(),
            call_returns: scenario.call_returns,
            steps: 0,
            max_steps: if scenario.max_steps == 0 {
                100_000
            } else {
                scenario.max_steps
            },
        }
    }

    pub(super) fn register(&self, register: Reg) -> u32 {
        self.registers[usize::from(register.0)]
    }

    pub(super) fn set_register(&mut self, register: Reg, value: u32) {
        if register != Reg::ZERO {
            self.registers[usize::from(register.0)] = value;
        }
    }
}

pub fn execute(
    image: &ExecutableImage,
    svd: &MmioRegisterMap,
    symbol: &str,
    scenario: Scenario,
) -> Result<ExecutionResult> {
    if scenario.arguments.len() > 8 {
        return Err(format!(
            "{} arguments were provided, but stack arguments are not implemented; maximum is 8",
            scenario.arguments.len()
        )
        .into());
    }
    let start = image
        .symbol_address(symbol)
        .ok_or_else(|| format!("execution symbol {symbol} was not found"))?;
    let mut machine = Machine::new(image, svd, start, scenario);
    while machine.step()? {}
    let unconsumed: Vec<_> = machine
        .mmio_reads
        .iter()
        .filter_map(|(address, values)| (!values.is_empty()).then_some((*address, values.len())))
        .collect();
    if !unconsumed.is_empty() {
        return Err(format!("unconsumed MMIO read responses: {unconsumed:?}").into());
    }
    let unconsumed_calls: Vec<_> = machine
        .call_returns
        .iter()
        .filter_map(|(symbol, values)| (!values.is_empty()).then_some((symbol, values.len())))
        .collect();
    if !unconsumed_calls.is_empty() {
        return Err(format!("unconsumed modeled call responses: {unconsumed_calls:?}").into());
    }
    let return_value = machine.register(Reg::A0);
    let memory_changes = machine.memory_changes()?;
    let persistent_memory = machine.persistent_memory();
    let initial_memory = machine.initial_overlay.clone();
    Ok(ExecutionResult {
        events: machine.events,
        timeline: machine.timeline,
        return_value,
        steps: machine.steps,
        branches: machine.branches,
        ordered_branches: machine.ordered_branches,
        calls: machine.calls,
        ordered_calls: machine.ordered_calls,
        indirect_calls: machine.indirect_calls,
        memory_changes,
        initial_memory,
        persistent_memory,
    })
}
