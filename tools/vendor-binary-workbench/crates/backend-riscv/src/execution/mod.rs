//! Concrete RV32 execution with interceptable MMIO.

mod image;
mod machine;
mod model;

pub use image::{CoverageInventory, ExecutableImage};
pub use machine::execute;
pub use model::{
    AllocationLifecycleEvent, AtomicOperation, AtomicOrdering, ExecutionEvent, ExecutionProducer,
    ExecutionResult, ExecutionSession, ExecutionTimelineEvent, IndirectCall, MemoryAlias,
    MemoryChange, MemoryOwner, MemoryOwnership, ModeledAllocation, ModeledCallOutput,
    ModeledCallResponse, OrderedCall, ResetPolicy, Scenario,
};

use open_radio_vendor_execution_model::{
    DeviceModelDescriptor, DeviceModelInstance, DeviceModelOutcome, MemoryRange,
    TableLifecycleEvent, TableSlotTarget,
};
#[cfg(test)]
use open_radio_vendor_execution_model::{
    DeviceModelRegistry, DeviceModelSpec, TableInstance, TableInstanceSlot,
};

use image::{RETURN_SENTINEL, STACK_POINTER, execution_stack_contains};
#[cfg(test)]
use image::{RelocatedCall, Segment};
#[cfg(test)]
use machine::{Machine, atomic_word_result};
use model::MmioValue;

#[cfg(test)]
mod tests;
