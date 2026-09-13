//! Ordered Host event state retained beside the live peripheral graph.

use embassy_sync::blocking_mutex::raw::RawMutex;
use oer_bluetooth_hci::{
    HciChannelError, LeControllerCommandEndpoint, LeDisconnectionCompleteEvent,
    LePeripheralConnectionCompleteEvent, LePeripheralConnectionEventPublication,
    bt_hci::param::{AddrKind, BdAddr, ClockAccuracy, ConnHandle, Duration, Error as HciError},
};
use oer_bluetooth_ll::{
    LeDeviceAddressKind,
    connection::{LeLegacyConnectionRequest, LePeripheralConnectionEventCompleted},
};

const SINGLE_PERIPHERAL_CONNECTION_HANDLE: u16 = 1;

enum EventState<Event> {
    Awaiting,
    Pending(Event),
    Complete,
}

pub(super) struct PeripheralConnectionHostEvents {
    connection: EventState<LePeripheralConnectionCompleteEvent>,
    disconnection: EventState<LeDisconnectionCompleteEvent>,
}

#[cfg_attr(
    test,
    allow(
        dead_code,
        reason = "the host mirror cannot induce target-only transport corruption"
    )
)]
pub(super) enum PeripheralConnectionHostEventPublication {
    None,
    Published,
    Masked,
    Pending,
    Fault(HciChannelError),
}

impl PeripheralConnectionHostEvents {
    pub(super) const fn new() -> Self {
        Self {
            connection: EventState::Awaiting,
            disconnection: EventState::Awaiting,
        }
    }

    pub(super) const fn has_pending(&self) -> bool {
        matches!(self.connection, EventState::Pending(_))
            || matches!(self.disconnection, EventState::Pending(_))
    }

    pub(super) const fn ready_to_restore_idle(&self) -> bool {
        matches!(self.connection, EventState::Complete)
            && matches!(self.disconnection, EventState::Complete)
    }

    pub(super) fn observe_completion(&mut self, completed: &LePeripheralConnectionEventCompleted) {
        if !matches!(self.connection, EventState::Awaiting) {
            return;
        }
        if completed.establishment_failed() {
            self.connection = EventState::Pending(
                LePeripheralConnectionCompleteEvent::failed(
                    HciError::CONN_FAILED_SYNCHRONIZATION_TIMEOUT.to_status(),
                )
                .expect("the standard establishment-failure status is non-success"),
            );
            self.disconnection = EventState::Complete;
        } else if completed.connection_state().establishment_event_counter()
            == Some(completed.event_counter())
        {
            self.connection = EventState::Pending(success_event(completed.request()));
        }
    }

    pub(super) fn observe_disconnection(&mut self, reason: u8) {
        if matches!(self.disconnection, EventState::Awaiting) {
            self.disconnection = EventState::Pending(LeDisconnectionCompleteEvent::new(
                ConnHandle::new(SINGLE_PERIPHERAL_CONNECTION_HANDLE),
                oer_bluetooth_hci::bt_hci::param::Status::new(reason),
            ));
        }
    }

    pub(super) fn try_publish<
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        &mut self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
    ) -> PeripheralConnectionHostEventPublication {
        let result = match &self.connection {
            EventState::Pending(event) => controller
                .try_publish_peripheral_connection_complete(event)
                .map(|publication| (true, publication)),
            EventState::Awaiting => return PeripheralConnectionHostEventPublication::None,
            EventState::Complete => match &self.disconnection {
                EventState::Pending(event) => controller
                    .try_publish_disconnection_complete(event)
                    .map(|publication| (false, publication)),
                EventState::Awaiting | EventState::Complete => {
                    return PeripheralConnectionHostEventPublication::None;
                }
            },
        };
        match result {
            Ok((connection, LePeripheralConnectionEventPublication::Published)) => {
                self.complete(connection);
                PeripheralConnectionHostEventPublication::Published
            }
            Ok((connection, LePeripheralConnectionEventPublication::Masked)) => {
                self.complete(connection);
                PeripheralConnectionHostEventPublication::Masked
            }
            Err(HciChannelError::Full) => PeripheralConnectionHostEventPublication::Pending,
            Err(error) => PeripheralConnectionHostEventPublication::Fault(error),
        }
    }

    fn complete(&mut self, connection: bool) {
        if connection {
            self.connection = EventState::Complete;
        } else {
            self.disconnection = EventState::Complete;
        }
    }
}

fn success_event(request: LeLegacyConnectionRequest) -> LePeripheralConnectionCompleteEvent {
    let initiator = request.initiator();
    let address_kind = match initiator.kind() {
        LeDeviceAddressKind::Public => AddrKind::PUBLIC,
        LeDeviceAddressKind::Random => AddrKind::RANDOM,
    };
    let timing = request.timing();
    let central_clock_accuracy = match request.sleep_clock_accuracy().encoded() {
        0 => ClockAccuracy::Ppm500,
        1 => ClockAccuracy::Ppm250,
        2 => ClockAccuracy::Ppm150,
        3 => ClockAccuracy::Ppm100,
        4 => ClockAccuracy::Ppm75,
        5 => ClockAccuracy::Ppm50,
        6 => ClockAccuracy::Ppm30,
        7 => ClockAccuracy::Ppm20,
        _ => unreachable!("validated CONNECT_IND SCA occupies three bits"),
    };
    LePeripheralConnectionCompleteEvent::new(
        ConnHandle::new(SINGLE_PERIPHERAL_CONNECTION_HANDLE),
        address_kind,
        BdAddr::new(initiator.wire_bytes()),
        Duration::from_u16(timing.interval_units()),
        timing.peripheral_latency(),
        Duration::from_u16(timing.supervision_timeout_units()),
        central_clock_accuracy,
    )
    .expect("a validated CONNECT_IND has a legacy peer address kind")
}

#[cfg(test)]
mod tests {
    use super::*;
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    use oer_bluetooth_hci::bt_hci::{
        FromHciBytes,
        event::{Event, le::LeEvent},
    };
    use oer_bluetooth_hci::{
        BluetoothPublicDeviceAddress, LeControllerBootstrapConfig, LeControllerHciResources,
    };
    use oer_bluetooth_ll::connection::{
        LeChannelSelectionAlgorithm, LeDataChannelMap, LePeripheralConnection,
        LePeripheralConnectionEventPeerActivity,
    };

    fn request() -> LeLegacyConnectionRequest {
        let mut pdu = [0; 36];
        pdu[0] = 0x05;
        pdu[1] = 34;
        pdu[2..8].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
        pdu[8..14].copy_from_slice(&[7, 8, 9, 10, 11, 12]);
        pdu[14..18].copy_from_slice(&0xa1b2_c3d4_u32.to_le_bytes());
        pdu[18..21].copy_from_slice(&[0x33, 0x22, 0x11]);
        pdu[21] = 2;
        pdu[22..24].copy_from_slice(&1_u16.to_le_bytes());
        pdu[24..26].copy_from_slice(&24_u16.to_le_bytes());
        pdu[28..30].copy_from_slice(&200_u16.to_le_bytes());
        pdu[30..35].copy_from_slice(&LeDataChannelMap::all().wire_bytes());
        pdu[35] = 5 | (4 << 5);
        LeLegacyConnectionRequest::decode(&pdu).expect("fixture is a valid CONNECT_IND")
    }

    #[test]
    fn establishment_and_peer_termination_queue_two_ordered_events() {
        let completed = LePeripheralConnection::from_request(
            request(),
            LeChannelSelectionAlgorithm::AlgorithmOne,
        )
        .prepare_event()
        .into_submitted()
        .complete(LePeripheralConnectionEventPeerActivity::Observed);
        let mut events = PeripheralConnectionHostEvents::new();

        events.observe_completion(&completed);
        events.observe_disconnection(0x13);

        assert!(matches!(events.connection, EventState::Pending(_)));
        assert!(matches!(events.disconnection, EventState::Pending(_)));
        assert!(!events.ready_to_restore_idle());
        let EventState::Pending(connection) = &events.connection else {
            panic!("establishment must retain Connection Complete");
        };
        let Event::Le(LeEvent::LeConnectionComplete(connection)) =
            Event::from_hci_bytes_complete(connection.as_bytes())
                .expect("queued event remains standard HCI")
        else {
            panic!("queued event changed kind");
        };
        assert_eq!(
            connection.status,
            oer_bluetooth_hci::bt_hci::param::Status::SUCCESS
        );
        assert_eq!(
            connection.handle,
            ConnHandle::new(SINGLE_PERIPHERAL_CONNECTION_HANDLE)
        );

        let mut resources = LeControllerHciResources::<NoopRawMutex, 1, 1, 45>::new(
            LeControllerBootstrapConfig::new(
                BluetoothPublicDeviceAddress::from_canonical_bytes([1, 2, 3, 4, 5, 6]),
                27,
                1,
            )
            .expect("fixture HCI profile is nonzero"),
        )
        .expect("fixture events fit the queue");
        let endpoints = resources.split();
        assert!(events.has_pending());
        assert!(matches!(
            events.try_publish(&endpoints.controller),
            PeripheralConnectionHostEventPublication::Masked
        ));
        assert!(events.has_pending());
        assert!(matches!(
            events.try_publish(&endpoints.controller),
            PeripheralConnectionHostEventPublication::Masked
        ));
        assert!(!events.has_pending());
        assert!(events.ready_to_restore_idle());
    }

    #[test]
    fn six_misses_queue_failed_connection_without_disconnection() {
        let mut connection = LePeripheralConnection::from_request(
            request(),
            LeChannelSelectionAlgorithm::AlgorithmOne,
        );
        let mut events = PeripheralConnectionHostEvents::new();
        for _ in 0..6 {
            let completed = connection
                .prepare_event()
                .into_submitted()
                .complete(LePeripheralConnectionEventPeerActivity::Missed);
            events.observe_completion(&completed);
            connection = completed.into_connection();
        }

        let EventState::Pending(failed) = &events.connection else {
            panic!("failed establishment must retain Connection Complete");
        };
        assert!(matches!(events.disconnection, EventState::Complete));
        let Event::Le(LeEvent::LeConnectionComplete(failed)) =
            Event::from_hci_bytes_complete(failed.as_bytes())
                .expect("queued failure remains standard HCI")
        else {
            panic!("queued failure changed kind");
        };
        assert_eq!(
            failed.status,
            HciError::CONN_FAILED_SYNCHRONIZATION_TIMEOUT.to_status()
        );
        assert_eq!(failed.handle, ConnHandle::new(0));
    }
}
