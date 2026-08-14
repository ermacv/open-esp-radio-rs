//! Concrete fail-closed RV32 interpreter state and execution facade.

mod events;
mod memory;
mod step;

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use rv_asm::Reg;

use super::{
    AllocationLifecycleEvent, DeviceModelDescriptor, DeviceModelInstance, DeviceModelOutcome,
    ExecutableImage, ExecutionEvent, ExecutionProducer, ExecutionResult, ExecutionTimelineEvent,
    IndirectCall, MemoryAlias, MemoryOwnership, MemoryRange, MmioValue, ModeledCallResponse,
    OrderedCall, RETURN_SENTINEL, STACK_POINTER, Scenario, TableLifecycleEvent,
};
use super::{FifoLifecycleEvent, FifoServiceBinding, FifoServiceInstance};
use crate::{MmioMap, Result};
use open_radio_vendor_execution_model::ExecutionGoal;

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
    pub(super) allocations: Vec<AllocationLifecycleEvent>,
    pub(super) table_layouts: Vec<TableRuntimeLayout>,
    pub(super) modeled_call_targets: BTreeMap<u32, String>,
    pub(super) table_lifecycle: Vec<TableLifecycleEvent>,
    pub(super) table_lifecycle_complete: bool,
    pub(super) call_responses: BTreeMap<String, VecDeque<ModeledCallResponse>>,
    pub(super) fifo_services: Vec<FifoServiceInstance>,
    pub(super) fifo_bindings: BTreeMap<String, FifoServiceBinding>,
    pub(super) fifo_lifecycle: Vec<FifoLifecycleEvent>,
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

struct RuntimeBindings {
    devices: Vec<DeviceRuntime>,
    table_layouts: Vec<TableRuntimeLayout>,
    modeled_call_targets: BTreeMap<u32, String>,
    table_lifecycle: Vec<TableLifecycleEvent>,
}

type MaterializedTables = (
    Vec<TableRuntimeLayout>,
    BTreeMap<u32, String>,
    Vec<TableLifecycleEvent>,
);

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
            RuntimeBindings {
                devices: Vec::new(),
                table_layouts: Vec::new(),
                modeled_call_targets: BTreeMap::new(),
                table_lifecycle: Vec::new(),
            },
        )
    }

    fn new_with_devices(
        image: &'a ExecutableImage,
        svd: &'a MmioMap,
        start: u32,
        scenario: Scenario,
        runtime: RuntimeBindings,
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
            devices: runtime.devices,
            events: Vec::new(),
            event_producers: Vec::new(),
            timeline: Vec::new(),
            branches: BTreeSet::new(),
            ordered_branches: Vec::new(),
            calls: BTreeSet::new(),
            ordered_calls: Vec::new(),
            indirect_calls: BTreeSet::new(),
            allocations: Vec::new(),
            table_layouts: runtime.table_layouts,
            modeled_call_targets: runtime.modeled_call_targets,
            table_lifecycle: runtime.table_lifecycle,
            table_lifecycle_complete: true,
            call_responses: scenario.call_responses,
            fifo_services: scenario.fifo_services,
            fifo_bindings: scenario
                .fifo_bindings
                .into_iter()
                .map(|binding| (binding.symbol.clone(), binding))
                .collect(),
            fifo_lifecycle: Vec::new(),
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

    pub(super) fn call_symbol_at(&self, address: u32) -> Option<&str> {
        self.image
            .call_symbol_at(address)
            .or_else(|| self.modeled_call_targets.get(&address).map(String::as_str))
    }

    pub(super) fn is_modeled_call_target(&self, address: u32) -> bool {
        self.modeled_call_targets.contains_key(&address)
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
    validate_fifo_services(&scenario)?;
    let start = image
        .symbol_address(symbol)
        .ok_or_else(|| format!("execution symbol {symbol} was not found"))?;
    let goal = resolve_execution_goal(image, &scenario.goal)?;
    let (table_layouts, modeled_call_targets, table_lifecycle) =
        materialize_table_instances(image, &mut scenario)?;
    for symbol in modeled_call_targets.values() {
        if !scenario
            .fifo_bindings
            .iter()
            .any(|binding| binding.symbol == *symbol)
            && !scenario.call_responses.contains_key(symbol)
        {
            return Err(format!(
                "modeled external table target {symbol} has no FIFO binding or call response model"
            )
            .into());
        }
    }
    let devices = instantiate_device_models(&scenario)?;
    let mut machine = Machine::new_with_devices(
        image,
        svd,
        start,
        scenario,
        RuntimeBindings {
            devices,
            table_layouts,
            modeled_call_targets,
            table_lifecycle,
        },
    );
    let completion = loop {
        if goal.is_satisfied(&machine) {
            break super::ExecutionCompletion::GoalReached(scenario_goal(&goal));
        }
        if !machine.step()? {
            if matches!(goal, ResolvedExecutionGoal::Return) {
                break super::ExecutionCompletion::Returned;
            }
            return Err(format!("execution returned before reaching goal {}", goal.label()).into());
        }
    };
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
        .call_responses
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
        completion,
        steps: machine.steps,
        branches: machine.branches,
        ordered_branches: machine.ordered_branches,
        calls: machine.calls,
        ordered_calls: machine.ordered_calls,
        indirect_calls: machine.indirect_calls,
        allocations: machine.allocations,
        table_lifecycle: machine.table_lifecycle,
        table_lifecycle_complete: machine.table_lifecycle_complete,
        fifo_lifecycle: machine.fifo_lifecycle,
        fifo_services: machine.fifo_services,
        device_model_coverage,
        memory_changes,
        initial_memory,
        persistent_memory,
    })
}

#[derive(Clone, Debug)]
enum ResolvedExecutionGoal {
    Return,
    ReachSymbol {
        symbol: String,
        address: u32,
    },
    ObserveCall {
        symbol: String,
    },
    ObserveFifoDequeue {
        service_id: String,
        value: Option<u32>,
    },
}

impl ResolvedExecutionGoal {
    fn is_satisfied(&self, machine: &Machine<'_>) -> bool {
        match self {
            Self::Return => false,
            Self::ReachSymbol { address, .. } => machine.pc == *address,
            Self::ObserveCall { symbol } => machine.calls.contains(symbol),
            Self::ObserveFifoDequeue { service_id, value } => machine.fifo_lifecycle.iter().any(
                |event| {
                    matches!(
                        event,
                        FifoLifecycleEvent::Dequeued {
                            service_id: observed,
                            value: observed_value,
                            ..
                        } if observed == service_id && value.is_none_or(|value| value == *observed_value)
                    )
                },
            ),
        }
    }

    fn label(&self) -> String {
        match self {
            Self::Return => "return".to_owned(),
            Self::ReachSymbol { symbol, .. } => format!("reach symbol {symbol}"),
            Self::ObserveCall { symbol } => format!("observe call {symbol}"),
            Self::ObserveFifoDequeue { service_id, value } => value.map_or_else(
                || format!("observe dequeue from FIFO {service_id}"),
                |value| format!("observe dequeue {value:#010x} from FIFO {service_id}"),
            ),
        }
    }
}

fn resolve_execution_goal(
    image: &ExecutableImage,
    goal: &ExecutionGoal,
) -> Result<ResolvedExecutionGoal> {
    Ok(match goal {
        ExecutionGoal::Return => ResolvedExecutionGoal::Return,
        ExecutionGoal::ReachSymbol { symbol } => ResolvedExecutionGoal::ReachSymbol {
            symbol: symbol.clone(),
            address: image
                .symbol_address(symbol)
                .ok_or_else(|| format!("execution goal refers to missing symbol {symbol}"))?,
        },
        ExecutionGoal::ObserveCall { symbol } => {
            if symbol.trim().is_empty() {
                return Err("execution call goal has an empty symbol".into());
            }
            ResolvedExecutionGoal::ObserveCall {
                symbol: symbol.clone(),
            }
        }
        ExecutionGoal::ObserveFifoDequeue { service_id, value } => {
            if service_id.trim().is_empty() {
                return Err("execution FIFO goal has an empty service id".into());
            }
            ResolvedExecutionGoal::ObserveFifoDequeue {
                service_id: service_id.clone(),
                value: *value,
            }
        }
    })
}

fn scenario_goal(goal: &ResolvedExecutionGoal) -> ExecutionGoal {
    match goal {
        ResolvedExecutionGoal::Return => ExecutionGoal::Return,
        ResolvedExecutionGoal::ReachSymbol { symbol, .. } => ExecutionGoal::ReachSymbol {
            symbol: symbol.clone(),
        },
        ResolvedExecutionGoal::ObserveCall { symbol } => ExecutionGoal::ObserveCall {
            symbol: symbol.clone(),
        },
        ResolvedExecutionGoal::ObserveFifoDequeue { service_id, value } => {
            ExecutionGoal::ObserveFifoDequeue {
                service_id: service_id.clone(),
                value: *value,
            }
        }
    }
}

fn validate_fifo_services(scenario: &Scenario) -> Result<()> {
    let mut service_ids = BTreeSet::new();
    let mut service_handles = BTreeSet::new();
    for service in &scenario.fifo_services {
        if service.id.trim().is_empty() {
            return Err("FIFO service has an empty id".into());
        }
        if !service_ids.insert(service.id.as_str()) {
            return Err(format!("duplicate FIFO service id {}", service.id).into());
        }
        if !service_handles.insert(service.handle) {
            return Err(format!(
                "FIFO service {} repeats handle {:#010x}",
                service.id, service.handle
            )
            .into());
        }
        if !matches!(service.item_width, 8 | 16 | 32) {
            return Err(format!(
                "FIFO service {} has unsupported item width {}",
                service.id, service.item_width
            )
            .into());
        }
        if service.capacity == 0 || service.items.len() > service.capacity {
            return Err(format!(
                "FIFO service {} has {} initial items for capacity {}",
                service.id,
                service.items.len(),
                service.capacity
            )
            .into());
        }
        if let Some(value) = service
            .items
            .iter()
            .copied()
            .find(|value| *value & !MmioValue::mask(service.item_width) != 0)
        {
            return Err(format!(
                "FIFO service {} initial value {value:#010x} exceeds its {}-bit item width",
                service.id, service.item_width
            )
            .into());
        }
    }

    let mut binding_symbols = BTreeSet::new();
    for binding in &scenario.fifo_bindings {
        if binding.symbol.trim().is_empty() {
            return Err("FIFO service binding has an empty symbol".into());
        }
        if !binding_symbols.insert(binding.symbol.as_str()) {
            return Err(format!("duplicate FIFO service binding {}", binding.symbol).into());
        }
        if scenario.call_responses.contains_key(&binding.symbol) {
            return Err(format!(
                "FIFO binding {} conflicts with a scripted call response",
                binding.symbol
            )
            .into());
        }
        if binding.handle_argument >= 8 {
            return Err(format!(
                "FIFO binding {} handle argument a{} exceeds RV32 a0..a7",
                binding.symbol, binding.handle_argument
            )
            .into());
        }
        let service = scenario
            .fifo_services
            .iter()
            .find(|service| service.id == binding.service_id)
            .ok_or_else(|| {
                format!(
                    "FIFO binding {} refers to missing service {}",
                    binding.symbol, binding.service_id
                )
            })?;
        validate_fifo_operation(binding, service)?;
    }
    Ok(())
}

fn validate_fifo_operation(
    binding: &FifoServiceBinding,
    service: &FifoServiceInstance,
) -> Result<()> {
    let validate_input = |source: open_radio_vendor_execution_model::ServiceValueSource| {
        let (argument, width) = match source {
            open_radio_vendor_execution_model::ServiceValueSource::Argument { argument, width } => {
                (argument, width)
            }
            open_radio_vendor_execution_model::ServiceValueSource::PrivateStackPointer {
                pointer_argument,
                width,
            } => (pointer_argument, width),
        };
        validate_fifo_abi_value(
            &binding.symbol,
            argument,
            width,
            service.item_width,
            "input",
        )
    };
    let validate_output = |output: open_radio_vendor_execution_model::ServiceOutput,
                           expected_width: Option<u8>| {
        let open_radio_vendor_execution_model::ServiceOutput::PrivateStackPointer {
            pointer_argument,
            width,
        } = output;
        validate_fifo_abi_value(
            &binding.symbol,
            pointer_argument,
            width,
            expected_width.unwrap_or(width),
            "output",
        )
    };

    match binding.operation {
        open_radio_vendor_execution_model::FifoServiceOperation::Enqueue {
            item,
            wake_output,
            ..
        } => {
            validate_input(item)?;
            if let Some(output) = wake_output {
                validate_output(output, None)?;
            }
        }
        open_radio_vendor_execution_model::FifoServiceOperation::Dequeue { output, .. } => {
            validate_output(output, Some(service.item_width))?;
        }
        open_radio_vendor_execution_model::FifoServiceOperation::Len => {}
    }
    Ok(())
}

fn validate_fifo_abi_value(
    symbol: &str,
    argument: u8,
    width: u8,
    expected_width: u8,
    role: &str,
) -> Result<()> {
    if argument >= 8 {
        return Err(format!(
            "FIFO binding {symbol} {role} argument a{argument} exceeds RV32 a0..a7"
        )
        .into());
    }
    if !matches!(width, 8 | 16 | 32) {
        return Err(format!("FIFO binding {symbol} has unsupported {role} width {width}").into());
    }
    if width != expected_width {
        return Err(format!(
            "FIFO binding {symbol} {role} width {width} does not match service item width {expected_width}"
        )
        .into());
    }
    Ok(())
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
) -> Result<MaterializedTables> {
    const MODELED_CALL_TARGET_START: u32 = 0xffff_0000;
    let mut layout_ids = BTreeSet::new();
    let mut layouts = Vec::new();
    let mut modeled_targets_by_name = BTreeMap::<String, u32>::new();
    let mut modeled_call_targets = BTreeMap::new();
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
                super::TableSlotTarget::ModeledSymbol(symbol) => {
                    if symbol.trim().is_empty() {
                        return Err(format!(
                            "runtime table {} slot {:#x} has an empty modeled symbol",
                            instance.layout_id, slot.offset
                        )
                        .into());
                    }
                    if image.symbol_address(&symbol).is_some() {
                        return Err(format!(
                            "runtime table {} slot {:#x} modeled symbol {symbol} already exists in the linked image; use kind = \"symbol\"",
                            instance.layout_id, slot.offset
                        )
                        .into());
                    }
                    if let Some(target) = modeled_targets_by_name.get(&symbol) {
                        *target
                    } else {
                        let index = u32::try_from(modeled_targets_by_name.len())
                            .map_err(|_| "too many modeled external call targets")?;
                        let target = MODELED_CALL_TARGET_START
                            .checked_add(
                                index
                                    .checked_mul(4)
                                    .ok_or("modeled external call target range overflow")?,
                            )
                            .ok_or("modeled external call target range overflow")?;
                        let overlaps_segment = image.segments.iter().any(|segment| {
                            target
                                .checked_sub(segment.address)
                                .is_some_and(|offset| offset < segment.memory_size)
                        });
                        if target == RETURN_SENTINEL
                            || image.symbol_at(target).is_some()
                            || overlaps_segment
                        {
                            return Err(format!(
                                "modeled external call target {target:#010x} collides with executable control state"
                            )
                            .into());
                        }
                        modeled_targets_by_name.insert(symbol.clone(), target);
                        modeled_call_targets.insert(target, symbol);
                        target
                    }
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
        let symbolic_pointer_cells = instance
            .pointer_cell_symbols
            .into_iter()
            .map(|symbol| {
                image.symbol_address(&symbol).ok_or_else(|| {
                    format!(
                        "runtime table {} pointer cell refers to missing linked symbol {symbol}",
                        instance.layout_id
                    )
                    .into()
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let mut pointer_cells = BTreeSet::new();
        for pointer_cell in instance
            .pointer_cells
            .into_iter()
            .chain(symbolic_pointer_cells)
        {
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
    Ok((layouts, modeled_call_targets, lifecycle))
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
