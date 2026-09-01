//! Production ownership for the first ESP32-S31 BLE peripheral connection.
//!
//! The runtime can join a portable LL event to the recovered allocation graph
//! and install only its reviewed Access Address and CRCInit fields. It cannot
//! publish that partial graph; the S31 anchor/deadline and remaining descriptor
//! semantics must be closed first.

#![forbid(unsafe_code)]

use open_esp_radio_bluetooth_ll::connection::{
    LeConnectionTiming, LeDataChannelIndex, LePeripheralConnection,
    LePeripheralConnectionEventPrepared,
};
#[cfg(not(target_arch = "riscv32"))]
use open_esp_radio_esp32s31_bluetooth_memory::BluetoothPeripheralConnectionMemoryGraphModelAddress;
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothPeripheralConnectionIdentity, BluetoothPeripheralConnectionMemoryGraphBindFailure,
    BluetoothPeripheralConnectionMemoryGraphCpuOwned,
    BluetoothPeripheralConnectionMemoryGraphIdentityPrepared,
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

    /// Join one portable event with the two reviewed S31 identity fields.
    ///
    /// This transition performs controller-SRAM writes only. It cannot publish
    /// a scheduler item or claim that an event has reached hardware.
    pub fn prepare_identity(
        self,
        event: LePeripheralConnectionEventPrepared,
    ) -> BluetoothPeripheralConnectionIdentityPrepared {
        let request = event.request();
        let identity = BluetoothPeripheralConnectionIdentity::new(
            request.access_address().value().to_le_bytes(),
            request.crc_initialization().wire_bytes(),
        );
        BluetoothPeripheralConnectionIdentityPrepared {
            graph: self.graph.prepare_identity(identity),
            event,
        }
    }
}

/// Exact portable event joined to a CPU-owned, identity-prepared S31 graph.
#[must_use = "the identity-prepared connection event must be retained or cancelled"]
pub struct BluetoothPeripheralConnectionIdentityPrepared {
    graph: BluetoothPeripheralConnectionMemoryGraphIdentityPrepared,
    event: LePeripheralConnectionEventPrepared,
}

impl BluetoothPeripheralConnectionIdentityPrepared {
    /// Link Layer event counter which has not advanced yet.
    pub const fn event_counter(&self) -> u16 {
        self.event.event_counter()
    }

    /// Selected Link Layer data channel for the unsubmitted event.
    pub const fn channel(&self) -> LeDataChannelIndex {
        self.event.channel()
    }

    /// Validated portable timing retained for the future anchor builder.
    pub const fn timing(&self) -> LeConnectionTiming {
        self.event.timing()
    }

    /// Cancel before publication and recover both unchanged protocol state and
    /// the pristine S31 runtime allocation.
    pub fn cancel(
        self,
    ) -> (
        BluetoothPeripheralConnectionRuntimeResources,
        LePeripheralConnection,
    ) {
        (
            BluetoothPeripheralConnectionRuntimeResources::from_claimed_graph(self.graph.cancel()),
            self.event.cancel(),
        )
    }
}

#[cfg(test)]
mod tests {
    use open_esp_radio_bluetooth_ll::connection::{
        LEGACY_CONNECT_IND_PAYLOAD_BYTES, LEGACY_CONNECT_IND_PDU_BYTES, LeLegacyConnectionRequest,
        LePeripheralConnection,
    };
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

    #[test]
    fn portable_event_can_prepare_identity_and_cancel_losslessly() {
        let storage = std::boxed::Box::leak(std::boxed::Box::new(
            BluetoothPeripheralConnectionMemoryGraphStorage::new(),
        ));
        let base = BluetoothPeripheralConnectionMemoryGraphModelAddress::new(0x2f00_2000)
            .expect("the model base is a controller SRAM address");
        let runtime =
            BluetoothPeripheralConnectionRuntimeResources::claim_static_model(storage, base)
                .expect("the model graph fits controller SRAM");
        let request = LeLegacyConnectionRequest::decode(&connection_request()).unwrap();
        let connection = LePeripheralConnection::from_request(request);

        let prepared = runtime.prepare_identity(connection.prepare_event());
        assert_eq!(prepared.event_counter(), 0);
        assert!(prepared.channel().get() < 37);
        assert_eq!(prepared.timing().interval_micros(), 30_000);

        let (runtime, connection) = prepared.cancel();
        assert!(runtime.allocation_is_idle());
        assert_eq!(connection.event_counter(), 0);
    }

    fn connection_request() -> [u8; LEGACY_CONNECT_IND_PDU_BYTES] {
        let mut pdu = [0; LEGACY_CONNECT_IND_PDU_BYTES];
        pdu[0] = 0x25;
        pdu[1] = LEGACY_CONNECT_IND_PAYLOAD_BYTES as u8;
        pdu[2..8].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
        pdu[8..14].copy_from_slice(&[7, 8, 9, 10, 11, 12]);
        pdu[14..18].copy_from_slice(&0xa1b2_c3d4u32.to_le_bytes());
        pdu[18..21].copy_from_slice(&[0x33, 0x22, 0x11]);
        pdu[21] = 2;
        pdu[22..24].copy_from_slice(&1u16.to_le_bytes());
        pdu[24..26].copy_from_slice(&24u16.to_le_bytes());
        pdu[28..30].copy_from_slice(&200u16.to_le_bytes());
        pdu[30..35].copy_from_slice(&[0xff, 0xff, 0xff, 0xff, 0x1f]);
        pdu[35] = 5;
        pdu
    }
}
