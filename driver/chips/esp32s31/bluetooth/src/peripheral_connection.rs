//! Production ownership for the first ESP32-S31 BLE peripheral connection.
//!
//! The runtime can join a portable LL event to the recovered allocation graph
//! and install only its reviewed Access Address and CRCInit fields. It cannot
//! publish that partial graph; the S31 anchor/deadline and remaining descriptor
//! semantics must be closed first.

#![forbid(unsafe_code)]

use open_esp_radio_bluetooth_ll::connection::{
    LEGACY_CONNECT_IND_LE_1M_AIRTIME_MICROS, LeConnectionTiming, LeDataChannelIndex,
    LePeripheralConnection, LePeripheralConnectionEventPrepared,
};
#[cfg(not(target_arch = "riscv32"))]
use open_esp_radio_esp32s31_bluetooth_memory::BluetoothPeripheralConnectionMemoryGraphModelAddress;
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothPeripheralConnectionIdentity, BluetoothPeripheralConnectionMemoryGraphBindFailure,
    BluetoothPeripheralConnectionMemoryGraphCpuOwned,
    BluetoothPeripheralConnectionMemoryGraphIdentityPrepared,
    BluetoothPeripheralConnectionMemoryGraphStorage,
};

use crate::BluetoothSchedulerInstant;

/// PHY-calibrated on-air start of one received LE 1M packet.
///
/// Only the initialized S31 BLE PHY timing authority can create this value
/// from a hardware packet capture. It deliberately exposes no raw controller
/// ticks or scheduler image.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the packet-start timing must enter response admission or remain retained"]
pub struct BluetoothLe1MPacketStartTiming {
    packet_start: BluetoothSchedulerInstant,
}

impl BluetoothLe1MPacketStartTiming {
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn from_scheduler_micros(micros: u32) -> Self {
        Self {
            packet_start: BluetoothSchedulerInstant::from_image(micros),
        }
    }

    fn first_connection_window(
        self,
        timing: LeConnectionTiming,
    ) -> BluetoothPeripheralConnectionFirstWindow {
        let packet_end = self
            .packet_start
            .wrapping_add(LEGACY_CONNECT_IND_LE_1M_AIRTIME_MICROS);
        BluetoothPeripheralConnectionFirstWindow {
            anchor: packet_end.wrapping_add(timing.first_window_start_micros()),
            end: packet_end.wrapping_add(timing.first_window_end_micros()),
        }
    }
}

/// Absolute first transmit window derived from the accepted `CONNECT_IND`.
///
/// The positions stay private to the S31 scheduler boundary. Portable Link
/// Layer code owns only the relative WinOffset/WinSize semantics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BluetoothPeripheralConnectionFirstWindow {
    anchor: BluetoothSchedulerInstant,
    end: BluetoothSchedulerInstant,
}

impl BluetoothPeripheralConnectionFirstWindow {
    #[cfg(test)]
    pub(crate) const fn anchor(self) -> BluetoothSchedulerInstant {
        self.anchor
    }

    #[cfg(test)]
    pub(crate) const fn end(self) -> BluetoothSchedulerInstant {
        self.end
    }
}

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

    /// Join the first portable event to the exact accepted packet timestamp.
    ///
    /// The absolute window is derived before descriptor preparation so no
    /// later `now()` sample can replace the causal `CONNECT_IND` observation.
    pub fn prepare_first_event(
        self,
        connection: LePeripheralConnection,
        packet_start: BluetoothLe1MPacketStartTiming,
    ) -> BluetoothPeripheralConnectionFirstEventPrepared {
        let event = connection.prepare_event();
        let first_window = packet_start.first_connection_window(event.timing());
        BluetoothPeripheralConnectionFirstEventPrepared {
            prepared: self.prepare_identity(event),
            first_window,
        }
    }
}

/// First portable connection event joined to its causal S31 receive timing.
#[must_use = "the timed first connection event must be lowered or cancelled"]
pub struct BluetoothPeripheralConnectionFirstEventPrepared {
    prepared: BluetoothPeripheralConnectionIdentityPrepared,
    first_window: BluetoothPeripheralConnectionFirstWindow,
}

impl BluetoothPeripheralConnectionFirstEventPrepared {
    /// Link Layer event counter, still unadvanced before hardware admission.
    pub const fn event_counter(&self) -> u16 {
        self.prepared.event_counter()
    }

    /// Selected first data channel.
    pub const fn channel(&self) -> LeDataChannelIndex {
        self.prepared.channel()
    }

    #[cfg(test)]
    pub(crate) const fn first_window(&self) -> BluetoothPeripheralConnectionFirstWindow {
        self.first_window
    }

    /// Width of the first accepted transmit window.
    pub const fn first_window_width_micros(&self) -> u32 {
        self.first_window
            .end
            .image()
            .wrapping_sub(self.first_window.anchor.image())
    }

    /// Cancel before publication and recover both unchanged owners.
    pub fn cancel(
        self,
    ) -> (
        BluetoothPeripheralConnectionRuntimeResources,
        LePeripheralConnection,
    ) {
        self.prepared.cancel()
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
        LEGACY_CONNECT_IND_LE_1M_AIRTIME_MICROS, LEGACY_CONNECT_IND_PAYLOAD_BYTES,
        LEGACY_CONNECT_IND_PDU_BYTES, LeLegacyConnectionRequest, LePeripheralConnection,
    };
    use open_esp_radio_esp32s31_bluetooth_memory::{
        BluetoothPeripheralConnectionMemoryGraphModelAddress,
        BluetoothPeripheralConnectionMemoryGraphStorage,
    };

    use super::{BluetoothLe1MPacketStartTiming, BluetoothPeripheralConnectionRuntimeResources};

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

    #[test]
    fn first_event_uses_the_received_packet_start_for_its_absolute_window() {
        let storage = std::boxed::Box::leak(std::boxed::Box::new(
            BluetoothPeripheralConnectionMemoryGraphStorage::new(),
        ));
        let base = BluetoothPeripheralConnectionMemoryGraphModelAddress::new(0x2f00_3000)
            .expect("the model base is a controller SRAM address");
        let runtime =
            BluetoothPeripheralConnectionRuntimeResources::claim_static_model(storage, base)
                .expect("the model graph fits controller SRAM");
        let request = LeLegacyConnectionRequest::decode(&connection_request()).unwrap();
        let connection = LePeripheralConnection::from_request(request);

        let prepared = runtime.prepare_first_event(
            connection,
            BluetoothLe1MPacketStartTiming::from_scheduler_micros(u32::MAX - 100),
        );
        let window = prepared.first_window();
        assert_eq!(
            window.anchor().image(),
            (u32::MAX - 100)
                .wrapping_add(LEGACY_CONNECT_IND_LE_1M_AIRTIME_MICROS)
                .wrapping_add(request.timing().first_window_start_micros())
        );
        assert_eq!(
            window.end().image().wrapping_sub(window.anchor().image()),
            request
                .timing()
                .first_window_end_micros()
                .wrapping_sub(request.timing().first_window_start_micros())
        );

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
