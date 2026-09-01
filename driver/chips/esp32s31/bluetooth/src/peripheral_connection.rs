//! Production ownership for the first ESP32-S31 BLE peripheral connection.
//!
//! The runtime currently retains only the recovered allocation graph. It has
//! no transition that can lower the portable LL connection event into this
//! graph or publish it; the S31 anchor/deadline and descriptor semantics must
//! be closed first.

#![forbid(unsafe_code)]

#[cfg(not(target_arch = "riscv32"))]
use open_esp_radio_esp32s31_bluetooth_memory::BluetoothPeripheralConnectionMemoryGraphModelAddress;
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothPeripheralConnectionMemoryGraphBindFailure,
    BluetoothPeripheralConnectionMemoryGraphCpuOwned,
    BluetoothPeripheralConnectionMemoryGraphStorage,
};

/// Composition-owned allocation graph for one future peripheral connection.
#[must_use = "the connection runtime retains the sole production graph"]
pub struct BluetoothPeripheralConnectionRuntimeResources {
    graph: BluetoothPeripheralConnectionMemoryGraphCpuOwned,
}

impl BluetoothPeripheralConnectionRuntimeResources {
    fn from_claimed_graph(graph: BluetoothPeripheralConnectionMemoryGraphCpuOwned) -> Self {
        Self { graph }
    }

    /// Bind one real statically placed peripheral-connection allocation.
    #[cfg(target_arch = "riscv32")]
    pub fn claim_static(
        storage: &'static mut BluetoothPeripheralConnectionMemoryGraphStorage,
    ) -> Result<Self, BluetoothPeripheralConnectionMemoryGraphBindFailure> {
        let graph = BluetoothPeripheralConnectionMemoryGraphStorage::pin_static(storage)?;
        Ok(Self::from_claimed_graph(graph))
    }

    /// Bind one deterministic native model allocation.
    #[cfg(not(target_arch = "riscv32"))]
    pub fn claim_static_model(
        storage: &'static mut BluetoothPeripheralConnectionMemoryGraphStorage,
        base: BluetoothPeripheralConnectionMemoryGraphModelAddress,
    ) -> Result<Self, BluetoothPeripheralConnectionMemoryGraphBindFailure> {
        let graph =
            BluetoothPeripheralConnectionMemoryGraphStorage::pin_static_model(storage, base)?;
        Ok(Self::from_claimed_graph(graph))
    }

    /// Whether the retained allocation still has its initial queue topology.
    pub fn allocation_is_idle(&self) -> bool {
        self.graph.has_recovered_scheduler_pool()
            && self.graph.has_empty_receive_queue()
            && self.graph.has_empty_transmit_queue()
    }
}

#[cfg(test)]
mod tests {
    use open_esp_radio_esp32s31_bluetooth_memory::{
        BluetoothPeripheralConnectionMemoryGraphModelAddress,
        BluetoothPeripheralConnectionMemoryGraphStorage,
    };

    use super::BluetoothPeripheralConnectionRuntimeResources;

    #[test]
    fn claimed_runtime_retains_the_idle_allocation() {
        let storage = std::boxed::Box::leak(std::boxed::Box::new(
            BluetoothPeripheralConnectionMemoryGraphStorage::new(),
        ));
        let base = BluetoothPeripheralConnectionMemoryGraphModelAddress::new(0x2f00_1000)
            .expect("the model base is a controller SRAM address");
        let runtime =
            BluetoothPeripheralConnectionRuntimeResources::claim_static_model(storage, base)
                .expect("the model graph fits controller SRAM");

        assert!(runtime.allocation_is_idle());
    }
}
