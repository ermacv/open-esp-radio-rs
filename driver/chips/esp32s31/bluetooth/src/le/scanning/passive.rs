#![forbid(unsafe_code)]

//! Production ownership for the restricted passive LE scanner graph.
//!
//! The controller owns one statically placed graph. Checking it out removes
//! it from the runtime until an exact cancellation or completed recycle
//! returns the same physical allocation. Scan policy and PDU parsing stay in
//! the portable Link Layer; this module owns only S31 graph placement policy.

#[cfg(target_arch = "riscv32")]
pub(crate) mod active;
#[cfg(target_arch = "riscv32")]
pub(crate) mod hci;
#[cfg(target_arch = "riscv32")]
pub(crate) mod runner;
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) mod timing;

#[cfg(not(target_arch = "riscv32"))]
use open_esp_radio_esp32s31_bluetooth_memory::BluetoothPassiveScanMemoryGraphModelAddress;
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothPassiveScanDefaultTxPowerDbm, BluetoothPassiveScanMemoryGraphBindFailure,
    BluetoothPassiveScanMemoryGraphCpuOwned, BluetoothPassiveScanMemoryGraphStorage,
    BluetoothPassiveScanResetConfig, BluetoothPassiveScanSchedulerAllocationConfig,
};
use open_esp_radio_esp32s31_hal::BluetoothControllerLatchedTime;

#[cfg(target_arch = "riscv32")]
pub(crate) const fn lower_primary_channel(
    channel: open_esp_radio_bluetooth_ll::scanning::PrimaryScanChannel,
) -> open_esp_radio_esp32s31_bluetooth_memory::BluetoothPassiveScanPrimaryChannel {
    match channel {
        open_esp_radio_bluetooth_ll::scanning::PrimaryScanChannel::Channel37 => {
            open_esp_radio_esp32s31_bluetooth_memory::BluetoothPassiveScanPrimaryChannel::Channel37
        }
        open_esp_radio_bluetooth_ll::scanning::PrimaryScanChannel::Channel38 => {
            open_esp_radio_esp32s31_bluetooth_memory::BluetoothPassiveScanPrimaryChannel::Channel38
        }
        open_esp_radio_bluetooth_ll::scanning::PrimaryScanChannel::Channel39 => {
            open_esp_radio_esp32s31_bluetooth_memory::BluetoothPassiveScanPrimaryChannel::Channel39
        }
    }
}

/// Immutable placement inputs for the sole scanner graph.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothPassiveScanRuntimeConfig {
    scheduler_allocation: BluetoothPassiveScanSchedulerAllocationConfig,
    default_tx_power_dbm: BluetoothPassiveScanDefaultTxPowerDbm,
}

impl BluetoothPassiveScanRuntimeConfig {
    pub const fn new(
        scheduler_allocation: BluetoothPassiveScanSchedulerAllocationConfig,
        default_tx_power_dbm: BluetoothPassiveScanDefaultTxPowerDbm,
    ) -> Self {
        Self {
            scheduler_allocation,
            default_tx_power_dbm,
        }
    }

    pub const fn scheduler_allocation_config(
        self,
    ) -> BluetoothPassiveScanSchedulerAllocationConfig {
        self.scheduler_allocation
    }

    pub const fn default_tx_power_dbm(self) -> BluetoothPassiveScanDefaultTxPowerDbm {
        self.default_tx_power_dbm
    }

    const fn reset_config(self) -> BluetoothPassiveScanResetConfig {
        BluetoothPassiveScanResetConfig::le_1m_public_accept_all(
            self.default_tx_power_dbm,
            BluetoothControllerLatchedTime::from_bits(0),
        )
    }
}

/// Why the sole scanner graph cannot begin another event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothPassiveScanRuntimeBeginError {
    EventActive,
}

/// Reclaimed scanner graph which did not belong to the production runtime.
#[cfg(target_arch = "riscv32")]
#[must_use = "retain the foreign graph and its copied receive result"]
#[allow(
    dead_code,
    reason = "the fail-stop owner intentionally keeps the foreign graph and copied result opaque"
)]
pub(crate) struct BluetoothPassiveScanRuntimeRestoreFailure {
    pub(crate) graph:
        open_esp_radio_esp32s31_bluetooth_memory::BluetoothPassiveScanMemoryGraphCpuOwned,
    pub(crate) received: open_esp_radio_esp32s31_bluetooth_memory::BluetoothLeReceivedBatch,
    pub(crate) status:
        open_esp_radio_esp32s31_bluetooth_memory::BluetoothPassiveScanSchedulerItemCompletionStatus,
}

/// Composition-owned immutable configuration and reusable scanner graph.
#[must_use = "the scanner runtime retains the sole production graph"]
pub struct BluetoothPassiveScanRuntimeResources {
    config: BluetoothPassiveScanRuntimeConfig,
    #[cfg(any(target_arch = "riscv32", test))]
    graph_range: (u32, u32),
    idle: Option<BluetoothPassiveScanMemoryGraphCpuOwned>,
}

impl BluetoothPassiveScanRuntimeResources {
    fn from_claimed_graph(
        config: BluetoothPassiveScanRuntimeConfig,
        graph: BluetoothPassiveScanMemoryGraphCpuOwned,
    ) -> Self {
        #[cfg(any(target_arch = "riscv32", test))]
        let graph_range = graph.range();
        Self {
            config,
            #[cfg(any(target_arch = "riscv32", test))]
            graph_range,
            idle: Some(graph),
        }
    }

    /// Bind one real statically placed scanner graph.
    #[cfg(target_arch = "riscv32")]
    pub fn claim_static(
        storage: &'static mut BluetoothPassiveScanMemoryGraphStorage,
        config: BluetoothPassiveScanRuntimeConfig,
    ) -> Result<Self, BluetoothPassiveScanMemoryGraphBindFailure> {
        let graph = BluetoothPassiveScanMemoryGraphStorage::pin_static(
            storage,
            config.reset_config(),
            config.scheduler_allocation_config(),
        )?;
        Ok(Self::from_claimed_graph(config, graph))
    }

    /// Bind one deterministic native model graph.
    #[cfg(not(target_arch = "riscv32"))]
    pub fn claim_static_model(
        storage: &'static mut BluetoothPassiveScanMemoryGraphStorage,
        base: BluetoothPassiveScanMemoryGraphModelAddress,
        config: BluetoothPassiveScanRuntimeConfig,
    ) -> Result<Self, BluetoothPassiveScanMemoryGraphBindFailure> {
        let graph = BluetoothPassiveScanMemoryGraphStorage::pin_static_model(
            storage,
            base,
            config.reset_config(),
            config.scheduler_allocation_config(),
        )?;
        Ok(Self::from_claimed_graph(config, graph))
    }

    pub const fn config(&self) -> BluetoothPassiveScanRuntimeConfig {
        self.config
    }

    pub const fn event_is_idle(&self) -> bool {
        self.idle.is_some()
    }

    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn begin_event(
        &mut self,
    ) -> Result<BluetoothPassiveScanMemoryGraphCpuOwned, BluetoothPassiveScanRuntimeBeginError>
    {
        self.idle
            .take()
            .ok_or(BluetoothPassiveScanRuntimeBeginError::EventActive)
    }

    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn restore_idle(
        &mut self,
        graph: BluetoothPassiveScanMemoryGraphCpuOwned,
    ) -> Result<(), BluetoothPassiveScanMemoryGraphCpuOwned> {
        if self.idle.is_some() || graph.range() != self.graph_range {
            return Err(graph);
        }
        self.idle = Some(graph);
        Ok(())
    }

    #[cfg(target_arch = "riscv32")]
    #[expect(
        clippy::result_large_err,
        reason = "the recoverable failure retains the exact affine radio state and continuation owners without allocation"
    )]
    pub(crate) fn restore_recycled(
        &mut self,
        recycled: crate::scheduler::core::BluetoothPassiveScanSchedulerRecycled,
    ) -> Result<
        (
            open_esp_radio_esp32s31_bluetooth_memory::BluetoothLeReceivedBatch,
            open_esp_radio_esp32s31_bluetooth_memory::BluetoothPassiveScanSchedulerItemCompletionStatus,
        ),
        BluetoothPassiveScanRuntimeRestoreFailure,
    >{
        let (graph, received, status) = recycled.into_parts();
        match self.restore_idle(graph) {
            Ok(()) => Ok((received, status)),
            Err(graph) => Err(BluetoothPassiveScanRuntimeRestoreFailure {
                graph,
                received,
                status,
            }),
        }
    }
}

#[cfg(test)]
mod tests;
