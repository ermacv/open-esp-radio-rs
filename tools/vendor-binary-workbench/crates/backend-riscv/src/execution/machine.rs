//! Concrete fail-closed RV32 interpreter state and execution facade.

mod events;
mod memory;
mod step;

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use rv_asm::Reg;

use super::{
    DeviceModelDescriptor, DeviceModelInstance, DeviceModelOutcome, ExecutableImage,
    ExecutionEvent, ExecutionProducer, ExecutionResult, ExecutionTimelineEvent, IndirectCall,
    MemoryAlias, MemoryOwnership, MemoryRange, OrderedCall, RETURN_SENTINEL, STACK_POINTER,
    Scenario, TableLifecycleEvent,
};
use crate::{MmioMap, Result};

#[cfg(test)]
pub(super) use step::atomic_word_result;

pub(super) struct Machine<'a> {
    pub(super) image: &'a ExecutableImage,
    pub(super) svd: &'a MmioMap,
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
    pub(super) devices: Vec<DeviceRuntime>,
    pub(super) events: Vec<ExecutionEvent>,
    pub(super) event_producers: Vec<ExecutionProducer>,
    pub(super) timeline: Vec<ExecutionTimelineEvent>,
    pub(super) branches: BTreeSet<(u32, bool)>,
    pub(super) ordered_branches: Vec<(u32, bool)>,
    pub(super) calls: BTreeSet<String>,
    pub(super) ordered_calls: Vec<OrderedCall>,
    pub(super) indirect_calls: BTreeSet<IndirectCall>,
    pub(super) table_layouts: Vec<TableRuntimeLayout>,
    pub(super) table_lifecycle: Vec<TableLifecycleEvent>,
    pub(super) table_lifecycle_complete: bool,
    pub(super) call_returns: BTreeMap<String, VecDeque<u32>>,
    /// Address reserved by the most recent `lr.w` on this hart. The concrete
    /// executor is intentionally single-hart, so an overlapping local RAM
    /// write is the only modeled cause of reservation loss.
    pub(super) word_reservation: Option<u32>,
    pub(super) steps: u64,
    pub(super) max_steps: u64,
}

pub(super) struct DeviceRuntime {
    descriptor: DeviceModelDescriptor,
    instance: Box<dyn DeviceModelInstance>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct TableRuntimeLayout {
    layout_id: String,
    base_address: u32,
    layout_size: u32,
}

impl DeviceRuntime {
    pub(super) fn contains_access(&self, address: u32, width: u8) -> bool {
        self.descriptor.range.contains_access(address, width)
    }
}

impl<'a> Machine<'a> {
    #[cfg(test)]
    pub(super) fn new(
        image: &'a ExecutableImage,
        svd: &'a MmioMap,
        start: u32,
        scenario: Scenario,
    ) -> Self {
        assert!(
            scenario.device_models.is_empty(),
            "direct Machine construction does not instantiate device models; use execute"
        );
        Self::new_with_devices(
            image,
            svd,
            start,
            scenario,
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
    }

    fn new_with_devices(
        image: &'a ExecutableImage,
        svd: &'a MmioMap,
        start: u32,
        scenario: Scenario,
        devices: Vec<DeviceRuntime>,
        table_layouts: Vec<TableRuntimeLayout>,
        table_lifecycle: Vec<TableLifecycleEvent>,
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
            devices,
            events: Vec::new(),
            event_producers: Vec::new(),
            timeline: Vec::new(),
            branches: BTreeSet::new(),
            ordered_branches: Vec::new(),
            calls: BTreeSet::new(),
            ordered_calls: Vec::new(),
            indirect_calls: BTreeSet::new(),
            table_layouts,
            table_lifecycle,
            table_lifecycle_complete: true,
            call_returns: scenario.call_returns,
            word_reservation: None,
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
    svd: &MmioMap,
    symbol: &str,
    mut scenario: Scenario,
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
    let (table_layouts, table_lifecycle) = materialize_table_instances(image, &mut scenario)?;
    let devices = instantiate_device_models(&scenario)?;
    let mut machine = Machine::new_with_devices(
        image,
        svd,
        start,
        scenario,
        devices,
        table_layouts,
        table_lifecycle,
    );
    while machine.step()? {}
    let device_model_coverage = machine
        .devices
        .iter_mut()
        .map(|device| {
            let coverage = device.instance.finish().map_err(|error| {
                format!(
                    "device model {} ({}) did not finish cleanly: {error}",
                    device.descriptor.id, device.descriptor.kind
                )
            })?;
            Ok(DeviceModelOutcome {
                descriptor: device.descriptor.clone(),
                coverage,
            })
        })
        .collect::<Result<Vec<_>>>()?;
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
        event_producers: machine.event_producers,
        timeline: machine.timeline,
        return_value,
        steps: machine.steps,
        branches: machine.branches,
        ordered_branches: machine.ordered_branches,
        calls: machine.calls,
        ordered_calls: machine.ordered_calls,
        indirect_calls: machine.indirect_calls,
        table_lifecycle: machine.table_lifecycle,
        table_lifecycle_complete: machine.table_lifecycle_complete,
        device_model_coverage,
        memory_changes,
        initial_memory,
        persistent_memory,
    })
}

fn instantiate_device_models(scenario: &Scenario) -> Result<Vec<DeviceRuntime>> {
    let mut descriptors = Vec::with_capacity(scenario.device_models.len());
    for model in &scenario.device_models {
        let descriptor = model.descriptor();
        if descriptor.id.trim().is_empty() {
            return Err("device model has an empty id".into());
        }
        if descriptor.kind.trim().is_empty() {
            return Err(format!("device model {} has an empty kind", descriptor.id).into());
        }
        if descriptor.range.length == 0
            || descriptor
                .range
                .start
                .checked_add(descriptor.range.length)
                .is_none()
        {
            return Err(format!(
                "device model {} has an empty or overflowing range at {:#010x}+{:#x}",
                descriptor.id, descriptor.range.start, descriptor.range.length
            )
            .into());
        }
        if descriptors.iter().any(|previous: &DeviceModelDescriptor| {
            previous.id == descriptor.id || previous.range.overlaps(descriptor.range)
        }) {
            return Err(format!(
                "device model {} duplicates an id or overlaps another device range",
                descriptor.id
            )
            .into());
        }
        let conflicts_with_seed = scenario
            .mmio_initial
            .keys()
            .chain(scenario.mmio_reads.keys())
            .any(|address| descriptor.range.contains(*address));
        if conflicts_with_seed {
            return Err(format!(
                "device model {} overlaps an explicit MMIO seed or response",
                descriptor.id
            )
            .into());
        }
        descriptors.push(descriptor);
    }

    scenario
        .device_models
        .iter()
        .zip(descriptors)
        .map(|(model, descriptor)| {
            Ok(DeviceRuntime {
                descriptor,
                instance: model.instantiate()?,
            })
        })
        .collect()
}

fn materialize_table_instances(
    image: &ExecutableImage,
    scenario: &mut Scenario,
) -> Result<(Vec<TableRuntimeLayout>, Vec<TableLifecycleEvent>)> {
    let mut layout_ids = BTreeSet::new();
    let mut layouts = Vec::new();
    let mut lifecycle = Vec::new();
    let instances = std::mem::take(&mut scenario.table_instances);
    for instance in instances {
        if instance.layout_id.trim().is_empty() {
            return Err("runtime table instance has an empty layout id".into());
        }
        if !layout_ids.insert(instance.layout_id.clone()) {
            return Err(format!(
                "duplicate runtime table instance for layout {}",
                instance.layout_id
            )
            .into());
        }
        if instance.layout_size == 0 || instance.layout_size % 4 != 0 {
            return Err(format!(
                "runtime table {} has invalid 32-bit layout size {:#x}",
                instance.layout_id, instance.layout_size
            )
            .into());
        }
        if instance.base_address % 4 != 0 {
            return Err(format!(
                "runtime table {} base {:#010x} is not 32-bit aligned",
                instance.layout_id, instance.base_address
            )
            .into());
        }
        instance
            .base_address
            .checked_add(instance.layout_size)
            .ok_or_else(|| {
                format!(
                    "runtime table {} layout overflows the RV32 address space",
                    instance.layout_id
                )
            })?;
        layouts.push(TableRuntimeLayout {
            layout_id: instance.layout_id.clone(),
            base_address: instance.base_address,
            layout_size: instance.layout_size,
        });
        let mut offsets = BTreeSet::new();
        for slot in instance.slots {
            if slot.offset % 4 != 0
                || slot
                    .offset
                    .checked_add(4)
                    .is_none_or(|end| end > instance.layout_size)
            {
                return Err(format!(
                    "runtime table {} slot {:#x} is outside its {:#x}-byte 32-bit layout",
                    instance.layout_id, slot.offset, instance.layout_size
                )
                .into());
            }
            if !offsets.insert(slot.offset) {
                return Err(format!(
                    "runtime table {} has duplicate slot {:#x}",
                    instance.layout_id, slot.offset
                )
                .into());
            }
            let target = match slot.target {
                super::TableSlotTarget::Null => 0,
                super::TableSlotTarget::Address(address) => {
                    if image.symbol_at(address).is_none() {
                        return Err(format!(
                            "runtime table {} slot {:#x} target {address:#010x} is not a named function",
                            instance.layout_id, slot.offset
                        )
                        .into());
                    }
                    address
                }
                super::TableSlotTarget::Symbol(symbol) => {
                    image.symbol_address(&symbol).ok_or_else(|| {
                        format!(
                            "runtime table {} slot {:#x} refers to missing symbol {symbol}",
                            instance.layout_id, slot.offset
                        )
                    })?
                }
            };
            seed_table_word(
                image,
                &mut scenario.memory_initial,
                instance
                    .base_address
                    .checked_add(slot.offset)
                    .ok_or_else(|| {
                        format!(
                            "runtime table {} slot {:#x} address overflows",
                            instance.layout_id, slot.offset
                        )
                    })?,
                target,
                &instance.layout_id,
            )?;
            lifecycle.push(TableLifecycleEvent::SlotInitialized {
                layout_id: instance.layout_id.clone(),
                offset: slot.offset,
                target,
            });
        }
        let mut pointer_cells = BTreeSet::new();
        for pointer_cell in instance.pointer_cells {
            if pointer_cell % 4 != 0 {
                return Err(format!(
                    "runtime table {} pointer cell {pointer_cell:#010x} is not 32-bit aligned",
                    instance.layout_id
                )
                .into());
            }
            if !pointer_cells.insert(pointer_cell) {
                return Err(format!(
                    "runtime table {} repeats pointer cell {pointer_cell:#010x}",
                    instance.layout_id
                )
                .into());
            }
            seed_table_word(
                image,
                &mut scenario.memory_initial,
                pointer_cell,
                instance.base_address,
                &instance.layout_id,
            )?;
            lifecycle.push(TableLifecycleEvent::PointerInstalled {
                layout_id: instance.layout_id.clone(),
                address: pointer_cell,
                base_address: instance.base_address,
            });
        }
    }
    Ok((layouts, lifecycle))
}

fn seed_table_word(
    image: &ExecutableImage,
    memory: &mut BTreeMap<u32, u8>,
    address: u32,
    value: u32,
    layout_id: &str,
) -> Result<()> {
    for (offset, byte) in value.to_le_bytes().into_iter().enumerate() {
        let byte_address = address.checked_add(offset as u32).ok_or_else(|| {
            format!("runtime table {layout_id} word at {address:#010x} overflows")
        })?;
        if image.contains_memory(byte_address) && !image.contains_writable_memory(byte_address) {
            return Err(format!(
                "runtime table {layout_id} cannot seed read-only ELF memory at {byte_address:#010x}"
            )
            .into());
        }
        if let Some(existing) = memory.insert(byte_address, byte)
            && existing != byte
        {
            return Err(format!(
                "runtime table {layout_id} conflicts with scenario RAM at {byte_address:#010x}"
            )
            .into());
        }
    }
    Ok(())
}
