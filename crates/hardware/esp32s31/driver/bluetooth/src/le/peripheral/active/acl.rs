//! Bidirectional ACL ownership retained beside one live connection.

use embassy_sync::blocking_mutex::raw::RawMutex;
use oer_bluetooth_hci::{
    HciChannelError, LeControllerAclPacket, LeControllerCommandEndpoint, LeHostAclFragment,
    LeHostAclPacket, LeHostAclPacketRejection, bt_hci::param::ConnHandle,
};

pub(crate) const LEGACY_LE_DATA_PAYLOAD_CAPACITY: usize = 27;
const CONTROLLER_ACL_QUEUE_DEPTH: usize = 2;
const CONNECTION_HANDLE: u16 = 1;

pub(crate) enum ControllerAclPublication {
    None,
    Published,
    Pending,
    FlowControlled,
    Fault(HciChannelError),
}

#[derive(Clone, Copy)]
struct ControllerAclQueueEntry {
    packet: LeControllerAclPacket,
    published: u16,
}

pub(crate) struct PeripheralConnectionAcl {
    host_tx: Option<LeHostAclPacket>,
    completed_host_packets: u16,
    controller_rx: [Option<ControllerAclQueueEntry>; CONTROLLER_ACL_QUEUE_DEPTH],
    controller_rx_len: usize,
    controller_packets_outstanding: u32,
    controller_event_reserved: bool,
}

impl PeripheralConnectionAcl {
    pub(crate) const fn new() -> Self {
        Self {
            host_tx: None,
            completed_host_packets: 0,
            controller_rx: [None, None],
            controller_rx_len: 0,
            controller_packets_outstanding: 0,
            // The active session is constructed only after the first event's
            // RUN has already consumed its empty-queue admission.
            controller_event_reserved: true,
        }
    }

    pub(crate) const fn can_accept_host_packet(&self) -> bool {
        self.host_tx.is_none() && self.completed_host_packets != u16::MAX
    }

    pub(crate) fn accept_host_packet(
        &mut self,
        packet: Result<LeHostAclPacket, LeHostAclPacketRejection>,
    ) {
        match packet {
            Ok(packet) if self.host_tx.is_none() => {
                self.host_tx = Some(packet);
            }
            Ok(_) | Err(_) => self.complete_one_host_packet(),
        }
    }

    pub(crate) fn next_fragment(&self, maximum_payload: usize) -> Option<LeHostAclFragment<'_>> {
        self.host_tx.as_ref()?.next_fragment(maximum_payload)
    }

    pub(crate) fn fragment_enqueued(&mut self, length: usize) {
        self.host_tx
            .as_mut()
            .expect("an enqueued fragment retains its HCI packet")
            .fragment_enqueued(length);
    }

    pub(crate) fn observe_transmission_completion(&mut self, acknowledged: bool) {
        let Some(packet) = self.host_tx.as_mut() else {
            return;
        };
        packet.observe_fragment_completion(acknowledged);
        if packet.is_complete() {
            self.host_tx = None;
            self.complete_one_host_packet();
        }
    }

    pub(crate) fn cancel_host_packet(&mut self) {
        if self.host_tx.take().is_some() {
            self.complete_one_host_packet();
        }
    }

    pub(crate) fn take_completed_host_packets(&mut self) -> u16 {
        core::mem::take(&mut self.completed_host_packets)
    }

    /// Reserve enough software ownership before consuming a completed RX batch.
    pub(crate) const fn can_accept_controller_batch(&self, batch_len: usize) -> bool {
        batch_len <= CONTROLLER_ACL_QUEUE_DEPTH - self.controller_rx_len
    }

    /// Whether the complete next hardware RX graph can be admitted losslessly.
    ///
    /// Reserving the worst-case two copied PDUs before RUN means a later
    /// completion never has to leave a control PDU behind an already-full Host
    /// queue after hardware may have acknowledged the peer packet.
    pub(crate) const fn can_reserve_controller_event(&self) -> bool {
        !self.controller_event_reserved
            && self.can_accept_controller_batch(
                oer_esp32s31_bluetooth_memory::BLUETOOTH_NON_SCANNING_RX_NODE_COUNT,
            )
    }

    /// Bind the worst-case queue capacity to one successor before its RUN.
    pub(crate) fn reserve_controller_event(&mut self) -> bool {
        if !self.can_reserve_controller_event() {
            return false;
        }
        self.controller_event_reserved = true;
        true
    }

    /// Whether this completed event owns the reservation made before its RUN.
    pub(crate) const fn controller_event_is_reserved(&self) -> bool {
        self.controller_event_reserved
    }

    /// Consume the exact reservation after dispatching the completed RX batch.
    pub(crate) fn complete_controller_event(&mut self) {
        assert!(
            self.controller_event_reserved,
            "a hardware completion must consume its exact RX reservation once"
        );
        self.controller_event_reserved = false;
    }

    pub(crate) fn accept_controller_fragment(
        &mut self,
        fragment: oer_bluetooth_ll::control::LePeripheralDataFragment<'_>,
    ) {
        let Some(packet) = LeControllerAclPacket::copy_from_ll(
            ConnHandle::new(CONNECTION_HANDLE),
            fragment.is_continuing(),
            fragment.payload(),
        ) else {
            debug_assert!(fragment.payload().is_empty());
            return;
        };
        assert!(
            self.controller_rx_len < CONTROLLER_ACL_QUEUE_DEPTH,
            "completed RX admission reserves every possible data packet"
        );
        self.controller_rx[self.controller_rx_len] = Some(ControllerAclQueueEntry {
            packet,
            published: 0,
        });
        self.controller_rx_len += 1;
    }

    pub(crate) const fn has_controller_packet(&self) -> bool {
        self.controller_rx_len != 0
    }

    /// A connection handle cannot be reused while the Host still owns ACL
    /// buffers published for the retiring connection generation.
    pub(crate) const fn controller_credits_settled(&self) -> bool {
        self.controller_packets_outstanding == 0
    }

    pub(crate) fn credit_handle(&self, live_handle: Option<ConnHandle>) -> Option<ConnHandle> {
        live_handle.or_else(|| {
            (!self.controller_credits_settled()).then(|| ConnHandle::new(CONNECTION_HANDLE))
        })
    }

    pub(crate) const fn controller_packet_is_flow_controlled(
        &self,
        flow_controlled: bool,
        total_packets: Option<u16>,
    ) -> bool {
        flow_controlled
            && match total_packets {
                Some(total) => self.controller_packets_outstanding >= total as u32,
                None => true,
            }
    }

    pub(crate) fn accept_host_completed_packets(
        &mut self,
        command: oer_bluetooth_hci::LeHostCompletedPacketsCommand,
        live_handle: Option<ConnHandle>,
    ) -> Result<(), oer_bluetooth_hci::LeHostCompletedPacketsErrorEvent> {
        let Some(completed) = command.completed_for(live_handle) else {
            return Err(oer_bluetooth_hci::LeHostCompletedPacketsErrorEvent::invalid_parameters());
        };
        if completed > self.controller_packets_outstanding {
            return Err(oer_bluetooth_hci::LeHostCompletedPacketsErrorEvent::invalid_parameters());
        }
        self.controller_packets_outstanding -= completed;
        Ok(())
    }

    pub(crate) fn try_publish_controller_packet<
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        &mut self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
        maximum_payload: usize,
        flow_controlled: bool,
        total_packets: Option<u16>,
    ) -> ControllerAclPublication {
        if self.controller_rx[0].is_none() {
            return ControllerAclPublication::None;
        }
        if self.controller_packet_is_flow_controlled(flow_controlled, total_packets) {
            return ControllerAclPublication::FlowControlled;
        }
        let entry = self.controller_rx[0]
            .as_mut()
            .expect("the nonempty queue front remains retained");
        let packet = entry
            .packet
            .next_host_fragment(usize::from(entry.published), maximum_payload)
            .expect("a queued Controller ACL packet retains unpublished payload");
        match controller.try_publish_controller_acl(&packet) {
            Ok(()) => {
                if flow_controlled {
                    self.controller_packets_outstanding += 1;
                }
                entry.published += packet.payload_length() as u16;
                if usize::from(entry.published) == entry.packet.payload_length() {
                    self.controller_rx[0] = self.controller_rx[1].take();
                    self.controller_rx_len -= 1;
                }
                ControllerAclPublication::Published
            }
            Err(HciChannelError::Full) => ControllerAclPublication::Pending,
            Err(error) => ControllerAclPublication::Fault(error),
        }
    }

    fn complete_one_host_packet(&mut self) {
        self.completed_host_packets = self
            .completed_host_packets
            .checked_add(1)
            .expect("bounded HCI intake cannot overflow completion accounting");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_bluetooth_hci::bt_hci::{
        data::{AclBroadcastFlag, AclPacket, AclPacketBoundary},
        param::ConnHandle,
    };

    fn owned(length: usize) -> LeHostAclPacket {
        let data = [0x5a; 60];
        LeHostAclPacket::copy_from(
            AclPacket::new(
                ConnHandle::new(1),
                AclPacketBoundary::FirstNonFlushable,
                AclBroadcastFlag::PointToPoint,
                &data[..length],
            ),
            Some(ConnHandle::new(1)),
            251,
        )
        .unwrap()
    }

    #[test]
    fn one_credit_returns_only_after_every_legacy_fragment_is_acknowledged() {
        let mut acl = PeripheralConnectionAcl::new();
        acl.accept_host_packet(Ok(owned(60)));
        assert!(!acl.can_accept_host_packet());

        for length in [27, 27, 6] {
            let fragment = acl.next_fragment(LEGACY_LE_DATA_PAYLOAD_CAPACITY).unwrap();
            assert_eq!(fragment.payload().len(), length);
            acl.fragment_enqueued(length);
            acl.observe_transmission_completion(false);
            assert_eq!(acl.take_completed_host_packets(), 0);
            acl.observe_transmission_completion(true);
        }

        assert!(acl.can_accept_host_packet());
        assert_eq!(acl.take_completed_host_packets(), 1);
        assert_eq!(acl.take_completed_host_packets(), 0);
    }

    #[test]
    fn encrypted_legacy_fragments_leave_room_for_the_four_octet_mic() {
        let mut acl = PeripheralConnectionAcl::new();
        acl.accept_host_packet(Ok(owned(47)));

        for length in [23, 23, 1] {
            let fragment = acl.next_fragment(23).unwrap();
            assert_eq!(fragment.payload().len(), length);
            acl.fragment_enqueued(length);
            acl.observe_transmission_completion(true);
        }

        assert!(acl.can_accept_host_packet());
        assert_eq!(acl.take_completed_host_packets(), 1);
    }

    #[test]
    fn rejection_and_disconnect_each_return_the_consumed_credit_once() {
        let mut acl = PeripheralConnectionAcl::new();
        acl.accept_host_packet(Err(LeHostAclPacketRejection::NoLiveConnection));
        assert_eq!(acl.take_completed_host_packets(), 1);

        acl.accept_host_packet(Ok(owned(5)));
        acl.cancel_host_packet();
        acl.cancel_host_packet();
        assert_eq!(acl.take_completed_host_packets(), 1);

        acl.accept_host_packet(Ok(owned(5)));
        acl.accept_host_packet(Ok(owned(5)));
        assert_eq!(acl.take_completed_host_packets(), 1);
        acl.cancel_host_packet();
        assert_eq!(acl.take_completed_host_packets(), 1);

        acl.completed_host_packets = u16::MAX;
        assert!(!acl.can_accept_host_packet());
    }

    #[test]
    fn controller_queue_preserves_two_fragments_and_empty_ack_consumes_no_slot() {
        use embassy_futures::block_on;
        use embassy_sync::blocking_mutex::raw::NoopRawMutex;
        use oer_bluetooth_hci::bt_hci::{
            ControllerToHostPacket, data::AclPacketBoundary, transport::Transport,
        };
        use oer_bluetooth_hci::{
            BluetoothPublicDeviceAddress, LeControllerBootstrapConfig, LeControllerHciResources,
        };
        use oer_bluetooth_ll::control::{LePeripheralControl, LePeripheralReceive};

        let mut control = LePeripheralControl::new();
        let mut acl = PeripheralConnectionAcl::new();
        for pdu in [&[2, 3, 1, 2, 3][..], &[1, 2, 4, 5][..]] {
            let LePeripheralReceive::Data(fragment) = control.receive(pdu, None).unwrap() else {
                panic!("data PDU must cross the ACL boundary")
            };
            acl.accept_controller_fragment(fragment);
        }
        assert!(acl.has_controller_packet());
        assert!(!acl.can_accept_controller_batch(1));

        let LePeripheralReceive::Data(empty) = control.receive(&[1, 0], None).unwrap() else {
            panic!("empty data PDU remains an ACL acknowledgement")
        };
        acl.accept_controller_fragment(empty);
        assert_eq!(acl.controller_rx_len, 2);

        let mut resources = LeControllerHciResources::<NoopRawMutex, 1, 1, 80>::new(
            LeControllerBootstrapConfig::new(
                BluetoothPublicDeviceAddress::from_canonical_bytes([1, 2, 3, 4, 5, 6]),
                27,
                1,
            )
            .unwrap(),
        )
        .unwrap();
        let endpoints = resources.split();
        assert!(matches!(
            acl.try_publish_controller_packet(&endpoints.controller, 2, false, None),
            ControllerAclPublication::Published
        ));
        assert!(matches!(
            acl.try_publish_controller_packet(&endpoints.controller, 2, false, None),
            ControllerAclPublication::Pending
        ));

        let mut buffer = [0; 80];
        let ControllerToHostPacket::Acl(first) =
            block_on(endpoints.host.read(&mut buffer)).unwrap()
        else {
            panic!("published packet changed HCI kind")
        };
        assert_eq!(first.boundary_flag(), AclPacketBoundary::FirstFlushable);
        assert_eq!(first.data(), [1, 2]);

        assert!(matches!(
            acl.try_publish_controller_packet(&endpoints.controller, 2, false, None),
            ControllerAclPublication::Published
        ));
        let ControllerToHostPacket::Acl(middle) =
            block_on(endpoints.host.read(&mut buffer)).unwrap()
        else {
            panic!("published packet changed HCI kind")
        };
        assert_eq!(middle.boundary_flag(), AclPacketBoundary::Continuing);
        assert_eq!(middle.data(), [3]);

        assert!(matches!(
            acl.try_publish_controller_packet(&endpoints.controller, 2, false, None),
            ControllerAclPublication::Published
        ));
        let ControllerToHostPacket::Acl(second) =
            block_on(endpoints.host.read(&mut buffer)).unwrap()
        else {
            panic!("published packet changed HCI kind")
        };
        assert_eq!(second.boundary_flag(), AclPacketBoundary::Continuing);
        assert_eq!(second.data(), [4, 5]);
        assert!(!acl.has_controller_packet());

        let ControllerAclPublication::Fault(error) =
            ControllerAclPublication::Fault(HciChannelError::InvalidDirection)
        else {
            unreachable!()
        };
        assert_eq!(error, HciChannelError::InvalidDirection);
    }

    #[test]
    fn successor_run_requires_space_for_the_complete_hardware_receive_batch() {
        use oer_bluetooth_ll::control::{LePeripheralControl, LePeripheralReceive};

        let mut control = LePeripheralControl::new();
        let mut acl = PeripheralConnectionAcl::new();
        assert!(acl.controller_event_is_reserved());

        let LePeripheralReceive::Data(fragment) = control.receive(&[2, 1, 0x5a], None).unwrap()
        else {
            panic!("nonempty data PDU must cross the ACL boundary")
        };
        acl.accept_controller_fragment(fragment);
        acl.complete_controller_event();

        assert!(acl.can_accept_controller_batch(1));
        assert!(!acl.can_reserve_controller_event());
    }

    #[test]
    fn event_reservation_is_consumed_and_reissued_exactly_once() {
        let mut acl = PeripheralConnectionAcl::new();
        assert!(acl.controller_event_is_reserved());
        assert!(!acl.reserve_controller_event());

        acl.complete_controller_event();
        assert!(!acl.controller_event_is_reserved());
        assert!(acl.reserve_controller_event());
        assert!(acl.controller_event_is_reserved());
        assert!(!acl.reserve_controller_event());
    }

    #[test]
    fn host_completed_command_releases_exactly_one_controller_credit() {
        use embassy_futures::block_on;
        use embassy_sync::blocking_mutex::raw::NoopRawMutex;
        use oer_bluetooth_hci::bt_hci::{
            ControllerToHostPacket, param::ConnHandleCompletedPackets, transport::Transport,
        };
        use oer_bluetooth_hci::{
            BluetoothPublicDeviceAddress, LeControllerActivePeripheralIntake,
            LeControllerBootstrapConfig, LeControllerCommandReadyClaim, LeControllerHciResources,
        };
        use oer_bluetooth_ll::control::{LePeripheralControl, LePeripheralReceive};

        let mut control = LePeripheralControl::new();
        let mut acl = PeripheralConnectionAcl::new();
        let LePeripheralReceive::Data(first) = control.receive(&[2, 1, 7], None).unwrap() else {
            panic!("data PDU must cross the ACL boundary")
        };
        acl.accept_controller_fragment(first);

        let mut resources = LeControllerHciResources::<NoopRawMutex, 1, 1, 80>::new(
            LeControllerBootstrapConfig::new(
                BluetoothPublicDeviceAddress::from_canonical_bytes([1, 2, 3, 4, 5, 6]),
                27,
                1,
            )
            .unwrap(),
        )
        .unwrap();
        let mut endpoints = resources.split();
        assert!(matches!(
            acl.try_publish_controller_packet(&endpoints.controller, 27, true, Some(1)),
            ControllerAclPublication::Published
        ));
        let mut packet_buffer = [0; 80];
        let _: ControllerToHostPacket<'_> =
            block_on(endpoints.host.read(&mut packet_buffer)).unwrap();

        let LePeripheralReceive::Data(second) = control.receive(&[1, 1, 8], None).unwrap() else {
            panic!("continuing data PDU must cross the ACL boundary")
        };
        acl.accept_controller_fragment(second);
        assert!(matches!(
            acl.try_publish_controller_packet(&endpoints.controller, 27, true, Some(1)),
            ControllerAclPublication::FlowControlled
        ));

        let completed = [ConnHandleCompletedPackets::new(ConnHandle::new(1), 1)];
        let credits = endpoints.host.acl_credit_sender();
        block_on(credits.return_completed_packets(&completed)).unwrap();
        let LeControllerCommandReadyClaim::Ready(ready) =
            endpoints.controller.claim_initial_command_ready(())
        else {
            panic!("fresh endpoint supplies one command authority")
        };
        let mut command_buffer = [0; 80];
        let LeControllerActivePeripheralIntake::HostCompletedPackets {
            command: Ok(command),
            ..
        } = endpoints
            .controller
            .try_receive_active_peripheral_with_buffer(
                ready,
                Some(ConnHandle::new(1)),
                &mut command_buffer,
                |owner, _| owner,
            )
        else {
            panic!("special credit command must bypass ordinary classification")
        };
        acl.accept_host_completed_packets(command, Some(ConnHandle::new(1)))
            .unwrap();
        assert!(matches!(
            acl.try_publish_controller_packet(&endpoints.controller, 27, true, Some(1)),
            ControllerAclPublication::Published
        ));
        assert!(!acl.controller_credits_settled());
        assert_eq!(
            acl.credit_handle(None),
            Some(ConnHandle::new(CONNECTION_HANDLE))
        );
        acl.controller_packets_outstanding = 0;
        assert!(acl.controller_credits_settled());
        assert_eq!(acl.credit_handle(None), None);
    }
}
