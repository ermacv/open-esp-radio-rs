//! Controller-SRAM storage, affine memory owners and completion values.
//! Definitions live in the dedicated Bluetooth memory crate; these exports
//! expose only the memory contracts used by this backend.

#[cfg(target_arch = "riscv32")]
pub use oer_esp32s31_bluetooth_memory::DtmSchedulerItemCompletionStatus;

pub use oer_esp32s31_bluetooth_memory::{
    BlePhyEngineBindError, BlePhyEngineBindFailure, BlePhyEngineCpuOwned, BlePhyEngineStorage,
    ControllerSramLinkAddress, ControllerSramLinkAddressError, DtmMemoryGraphCpuOwned,
    DtmMemoryGraphPrepareError, DtmMemoryGraphPrepareFailure, DtmMemoryGraphReclaimed,
    DtmPositionalEventWords, DtmRxResultProjection, DtmRxResultProjectionError, DtmRxRssi,
};

#[cfg(not(target_arch = "riscv32"))]
pub use oer_esp32s31_bluetooth_memory::LegacyAdvertisingMemoryGraphModelAddress;

pub use oer_esp32s31_bluetooth_memory::{
    LegacyAdvertisingMemoryGraphBindError, LegacyAdvertisingMemoryGraphBindFailure,
    LegacyAdvertisingMemoryGraphCpuOwned, LegacyAdvertisingMemoryGraphStorage, RxMemoryListClass,
};
