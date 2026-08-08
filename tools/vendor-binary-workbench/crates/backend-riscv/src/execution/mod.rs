//! Concrete RV32 execution with interceptable MMIO.

mod device;
mod image;
mod machine;
mod model;

pub use device::{
    DeviceModel, DeviceModelCoverage, DeviceModelDescriptor, DeviceModelInstance,
    DeviceModelOutcome, DeviceModelRegistry, DeviceModelSpec,
};
pub use image::{CoverageInventory, ExecutableImage};
pub use machine::execute;
pub use model::{
    ExecutionEvent, ExecutionResult, ExecutionSession, ExecutionTimelineEvent, IndirectCall,
    MemoryAlias, MemoryChange, MemoryOwner, MemoryOwnership, MemoryRange, OrderedCall, ResetPolicy,
    Scenario, TableInstance, TableInstanceSlot, TableLifecycleEvent, TableSlotTarget,
};

use image::{RETURN_SENTINEL, STACK_POINTER, execution_stack_contains};
#[cfg(test)]
use image::{RelocatedCall, Segment};
#[cfg(test)]
use machine::{Machine, atomic_word_result};
use model::MmioValue;

#[cfg(test)]
mod tests;
