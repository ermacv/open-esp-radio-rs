//! Ordered Host event state retained beside the live peripheral graph.

#[cfg(target_arch = "riscv32")]
use super::acl;
#[cfg(not(target_arch = "riscv32"))]
use super::active_acl as acl;
use crate::le::peripheral::hci_order::LegacyConnectablePeripheralFirstHciAxis as Axis;
use embassy_sync::blocking_mutex::raw::RawMutex;
use oer_bluetooth_hci::{
    HciChannelError, LeConnectionUpdateCompleteEvent, LeControllerCommandEndpoint,
    LeDisconnectionCompleteEvent, LeEncryptionChangeEvent, LeEncryptionKeyRefreshCompleteEvent,
    LeLongTermKeyRequestEvent, LeNumberOfCompletedPacketsEvent,
    LePeripheralConnectionCompleteEvent, LePeripheralConnectionEventPublication,
    LeReadRemoteFeaturesCompleteEvent, LeReadRemoteVersionInformationCompleteEvent,
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
    connection_updates: [Option<LeConnectionUpdateCompleteEvent>; 2],
    connection_update_len: u8,
    remote_features: Option<LeReadRemoteFeaturesCompleteEvent>,
    remote_version: Option<LeReadRemoteVersionInformationCompleteEvent>,
    long_term_key_request: Option<LeLongTermKeyRequestEvent>,
    long_term_key_request_masked: bool,
    encryption_change: Option<LeEncryptionChangeEvent>,
    encryption_key_refresh: Option<LeEncryptionKeyRefreshCompleteEvent>,
    acl_completed: u16,
    established: bool,
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
            connection_updates: [None; 2],
            connection_update_len: 0,
            remote_features: None,
            remote_version: None,
            long_term_key_request: None,
            long_term_key_request_masked: false,
            encryption_change: None,
            encryption_key_refresh: None,
            acl_completed: 0,
            established: false,
        }
    }

    pub(super) const fn has_pending(&self) -> bool {
        matches!(self.connection, EventState::Pending(_))
            || self.acl_completed != 0
            || self.connection_update_len != 0
            || self.remote_features.is_some()
            || self.remote_version.is_some()
            || self.long_term_key_request.is_some()
            || self.encryption_change.is_some()
            || self.encryption_key_refresh.is_some()
            || matches!(self.disconnection, EventState::Pending(_))
    }

    pub(super) const fn ready_to_restore_idle(&self) -> bool {
        matches!(self.connection, EventState::Complete)
            && self.acl_completed == 0
            && self.connection_update_len == 0
            && self.remote_features.is_none()
            && self.remote_version.is_none()
            && self.long_term_key_request.is_none()
            && self.encryption_change.is_none()
            && self.encryption_key_refresh.is_none()
            && matches!(self.disconnection, EventState::Complete)
    }

    /// Full software barrier after physical stop, shared by readiness and handoff.
    pub(super) fn idle_retirement_ready(
        &self,
        axis: Axis,
        acl: &acl::PeripheralConnectionAcl,
    ) -> bool {
        axis == Axis::CommandReady
            && crate::le::peripheral::progress::retirement_barrier_is_ready(
                self.ready_to_restore_idle(),
                !acl.has_controller_packet(),
                acl.controller_credits_settled(),
            )
    }

    /// ACL data cannot precede the Host-visible connection result.
    pub(super) const fn connection_result_published(&self) -> bool {
        matches!(self.connection, EventState::Complete)
    }

    /// Whether the next output is ACL data waiting for a Host buffer credit.
    /// HCI events use transport capacity but do not consume ACL credits.
    pub(super) fn output_is_flow_controlled(
        &self,
        acl: &acl::PeripheralConnectionAcl,
        flow_controlled: bool,
        total_packets: Option<u16>,
    ) -> bool {
        !self.has_pending()
            && self.connection_result_published()
            && acl.has_controller_packet()
            && acl.controller_packet_is_flow_controlled(flow_controlled, total_packets)
    }

    pub(super) fn live_handle(&self) -> Option<ConnHandle> {
        (self.established && matches!(self.disconnection, EventState::Awaiting))
            .then(|| ConnHandle::new(SINGLE_PERIPHERAL_CONNECTION_HANDLE))
    }

    /// Record Host-visible state from one completed Link Layer event.
    ///
    /// `false` means both bounded update slots were already occupied; callers
    /// must terminate rather than silently lose a mandatory Host notification.
    pub(super) fn observe_completion(
        &mut self,
        completed: &LePeripheralConnectionEventCompleted,
    ) -> bool {
        if matches!(self.connection, EventState::Awaiting) {
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
                self.established = true;
            }
        }
        let Some(transition) = completed.connection_timing_transition() else {
            return true;
        };
        if !transition.host_parameters_changed() {
            return true;
        }
        if usize::from(self.connection_update_len) == self.connection_updates.len() {
            return false;
        }
        let timing = transition.updated();
        self.connection_updates[usize::from(self.connection_update_len)] =
            Some(LeConnectionUpdateCompleteEvent::new(
                ConnHandle::new(SINGLE_PERIPHERAL_CONNECTION_HANDLE),
                Duration::from_u16(timing.interval_units()),
                timing.peripheral_latency(),
                Duration::from_u16(timing.supervision_timeout_units()),
            ));
        self.connection_update_len += 1;
        true
    }

    pub(super) fn observe_disconnection(&mut self, reason: u8) {
        if matches!(self.disconnection, EventState::Awaiting) {
            self.disconnection = EventState::Pending(LeDisconnectionCompleteEvent::new(
                ConnHandle::new(SINGLE_PERIPHERAL_CONNECTION_HANDLE),
                oer_bluetooth_hci::bt_hci::param::Status::new(reason),
            ));
        }
    }

    /// Close Host-visible state after a local radio abort. Before
    /// establishment this is one failed Connection Complete; afterwards it is
    /// one Disconnection Complete for the already published handle.
    pub(super) fn observe_radio_abort(&mut self) {
        if matches!(self.connection, EventState::Awaiting) {
            self.connection = EventState::Pending(
                LePeripheralConnectionCompleteEvent::failed(HciError::HARDWARE_FAILURE.to_status())
                    .expect("Hardware Failure is a non-success status"),
            );
            self.disconnection = EventState::Complete;
        } else if self.established {
            self.observe_disconnection(HciError::HARDWARE_FAILURE.to_status().into_inner());
        }
    }

    pub(super) fn observe_acl_completed(&mut self, completed: u16) {
        self.acl_completed = self
            .acl_completed
            .checked_add(completed)
            .expect("bounded Controller ACL credits cannot overflow");
    }

    pub(super) fn observe_remote_features(
        &mut self,
        result: oer_bluetooth_ll::control::LeRemoteFeaturesResult,
    ) {
        let (status, features) = match result {
            oer_bluetooth_ll::control::LeRemoteFeaturesResult::Success(features) => {
                (oer_bluetooth_hci::bt_hci::param::Status::SUCCESS, features)
            }
            oer_bluetooth_ll::control::LeRemoteFeaturesResult::Unsupported => {
                (HciError::UNSUPPORTED_REMOTE_FEATURE.to_status(), [0; 8])
            }
            oer_bluetooth_ll::control::LeRemoteFeaturesResult::Rejected { reason } => (
                oer_bluetooth_hci::bt_hci::param::Status::new(reason),
                [0; 8],
            ),
            oer_bluetooth_ll::control::LeRemoteFeaturesResult::ResponseTimeout => {
                (HciError::LMP_LL_RESPONSE_TIMEOUT.to_status(), [0; 8])
            }
            oer_bluetooth_ll::control::LeRemoteFeaturesResult::ConnectionClosed => {
                (HciError::UNKNOWN_CONN_IDENTIFIER.to_status(), [0; 8])
            }
        };
        debug_assert!(self.remote_features.is_none());
        self.remote_features = Some(LeReadRemoteFeaturesCompleteEvent::new(
            status,
            ConnHandle::new(SINGLE_PERIPHERAL_CONNECTION_HANDLE),
            features,
        ));
    }

    pub(super) fn observe_remote_version(
        &mut self,
        result: oer_bluetooth_ll::control::LeRemoteVersionResult,
    ) {
        let (status, version) = match result {
            oer_bluetooth_ll::control::LeRemoteVersionResult::Success(version) => (
                oer_bluetooth_hci::bt_hci::param::Status::SUCCESS,
                Some(version),
            ),
            oer_bluetooth_ll::control::LeRemoteVersionResult::Unsupported => {
                (HciError::UNSUPPORTED_REMOTE_FEATURE.to_status(), None)
            }
            oer_bluetooth_ll::control::LeRemoteVersionResult::Rejected { reason } => {
                (oer_bluetooth_hci::bt_hci::param::Status::new(reason), None)
            }
            oer_bluetooth_ll::control::LeRemoteVersionResult::ResponseTimeout => {
                (HciError::LMP_LL_RESPONSE_TIMEOUT.to_status(), None)
            }
            oer_bluetooth_ll::control::LeRemoteVersionResult::ConnectionClosed => {
                (HciError::UNKNOWN_CONN_IDENTIFIER.to_status(), None)
            }
        };
        let (version, company_identifier, subversion) = version.map_or((0, 0, 0), |version| {
            (
                version.version(),
                version.company_identifier(),
                version.subversion(),
            )
        });
        debug_assert!(self.remote_version.is_none());
        self.remote_version = Some(LeReadRemoteVersionInformationCompleteEvent::new(
            status,
            ConnHandle::new(SINGLE_PERIPHERAL_CONNECTION_HANDLE),
            version,
            company_identifier,
            subversion,
        ));
    }

    pub(super) fn observe_long_term_key_request(
        &mut self,
        request: oer_bluetooth_ll::security::LeLongTermKeyRequest,
    ) {
        debug_assert!(self.long_term_key_request.is_none());
        self.long_term_key_request = Some(LeLongTermKeyRequestEvent::new(
            ConnHandle::new(SINGLE_PERIPHERAL_CONNECTION_HANDLE),
            request.random_number(),
            request.encrypted_diversifier(),
        ));
    }

    pub(super) fn take_long_term_key_request_masked(&mut self) -> bool {
        core::mem::take(&mut self.long_term_key_request_masked)
    }

    pub(super) fn observe_encryption_enabled(&mut self) {
        debug_assert!(self.encryption_change.is_none());
        self.encryption_change = Some(LeEncryptionChangeEvent::new(
            oer_bluetooth_hci::bt_hci::param::Status::SUCCESS,
            ConnHandle::new(SINGLE_PERIPHERAL_CONNECTION_HANDLE),
            true,
        ));
    }

    pub(super) fn observe_encryption_refreshed(&mut self) {
        debug_assert!(self.encryption_key_refresh.is_none());
        self.encryption_key_refresh = Some(LeEncryptionKeyRefreshCompleteEvent::new(
            oer_bluetooth_hci::bt_hci::param::Status::SUCCESS,
            ConnHandle::new(SINGLE_PERIPHERAL_CONNECTION_HANDLE),
        ));
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
            EventState::Complete => {
                if let Some(event) = &self.long_term_key_request {
                    return match controller.try_publish_long_term_key_request(event) {
                        Ok(LePeripheralConnectionEventPublication::Published) => {
                            self.long_term_key_request = None;
                            PeripheralConnectionHostEventPublication::Published
                        }
                        Ok(LePeripheralConnectionEventPublication::Masked) => {
                            self.long_term_key_request = None;
                            self.long_term_key_request_masked = true;
                            PeripheralConnectionHostEventPublication::Masked
                        }
                        Err(HciChannelError::Full) => {
                            PeripheralConnectionHostEventPublication::Pending
                        }
                        Err(error) => PeripheralConnectionHostEventPublication::Fault(error),
                    };
                }
                if let Some(event) = &self.encryption_change {
                    return match controller.try_publish_encryption_change(event) {
                        Ok(LePeripheralConnectionEventPublication::Published) => {
                            self.encryption_change = None;
                            PeripheralConnectionHostEventPublication::Published
                        }
                        Ok(LePeripheralConnectionEventPublication::Masked) => {
                            self.encryption_change = None;
                            PeripheralConnectionHostEventPublication::Masked
                        }
                        Err(HciChannelError::Full) => {
                            PeripheralConnectionHostEventPublication::Pending
                        }
                        Err(error) => PeripheralConnectionHostEventPublication::Fault(error),
                    };
                }
                if let Some(event) = &self.encryption_key_refresh {
                    return match controller.try_publish_encryption_key_refresh_complete(event) {
                        Ok(LePeripheralConnectionEventPublication::Published) => {
                            self.encryption_key_refresh = None;
                            PeripheralConnectionHostEventPublication::Published
                        }
                        Ok(LePeripheralConnectionEventPublication::Masked) => {
                            self.encryption_key_refresh = None;
                            PeripheralConnectionHostEventPublication::Masked
                        }
                        Err(HciChannelError::Full) => {
                            PeripheralConnectionHostEventPublication::Pending
                        }
                        Err(error) => PeripheralConnectionHostEventPublication::Fault(error),
                    };
                }
                if self.acl_completed != 0 {
                    let event = LeNumberOfCompletedPacketsEvent::new(
                        ConnHandle::new(SINGLE_PERIPHERAL_CONNECTION_HANDLE),
                        self.acl_completed,
                    );
                    return match controller.try_publish_number_of_completed_packets(&event) {
                        Ok(()) => {
                            self.acl_completed = 0;
                            PeripheralConnectionHostEventPublication::Published
                        }
                        Err(HciChannelError::Full) => {
                            PeripheralConnectionHostEventPublication::Pending
                        }
                        Err(error) => PeripheralConnectionHostEventPublication::Fault(error),
                    };
                }
                if let Some(event) = &self.connection_updates[0] {
                    return match controller.try_publish_connection_update_complete(event) {
                        Ok(LePeripheralConnectionEventPublication::Published) => {
                            self.complete_connection_update();
                            PeripheralConnectionHostEventPublication::Published
                        }
                        Ok(LePeripheralConnectionEventPublication::Masked) => {
                            self.complete_connection_update();
                            PeripheralConnectionHostEventPublication::Masked
                        }
                        Err(HciChannelError::Full) => {
                            PeripheralConnectionHostEventPublication::Pending
                        }
                        Err(error) => PeripheralConnectionHostEventPublication::Fault(error),
                    };
                }
                if let Some(event) = &self.remote_features {
                    return match controller.try_publish_read_remote_features_complete(event) {
                        Ok(LePeripheralConnectionEventPublication::Published) => {
                            self.remote_features = None;
                            PeripheralConnectionHostEventPublication::Published
                        }
                        Ok(LePeripheralConnectionEventPublication::Masked) => {
                            self.remote_features = None;
                            PeripheralConnectionHostEventPublication::Masked
                        }
                        Err(HciChannelError::Full) => {
                            PeripheralConnectionHostEventPublication::Pending
                        }
                        Err(error) => PeripheralConnectionHostEventPublication::Fault(error),
                    };
                }
                if let Some(event) = &self.remote_version {
                    return match controller
                        .try_publish_read_remote_version_information_complete(event)
                    {
                        Ok(LePeripheralConnectionEventPublication::Published) => {
                            self.remote_version = None;
                            PeripheralConnectionHostEventPublication::Published
                        }
                        Ok(LePeripheralConnectionEventPublication::Masked) => {
                            self.remote_version = None;
                            PeripheralConnectionHostEventPublication::Masked
                        }
                        Err(HciChannelError::Full) => {
                            PeripheralConnectionHostEventPublication::Pending
                        }
                        Err(error) => PeripheralConnectionHostEventPublication::Fault(error),
                    };
                }
                match &self.disconnection {
                    EventState::Pending(event) => controller
                        .try_publish_disconnection_complete(event)
                        .map(|publication| (false, publication)),
                    EventState::Awaiting | EventState::Complete => {
                        return PeripheralConnectionHostEventPublication::None;
                    }
                }
            }
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

    fn complete_connection_update(&mut self) {
        self.connection_updates[0] = self.connection_updates[1].take();
        self.connection_update_len -= 1;
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
    use embassy_futures::block_on;
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    use oer_bluetooth_hci::bt_hci::{
        ControllerToHostPacket, FromHciBytes,
        event::{Event, le::LeEvent},
        transport::Transport,
    };
    use oer_bluetooth_hci::{
        BluetoothPublicDeviceAddress, LeControllerBootstrapConfig, LeControllerHciResources,
    };
    use oer_bluetooth_ll::connection::{
        LeChannelSelectionAlgorithm, LeConnectionTiming, LeDataChannelMap, LePeripheralConnection,
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
    fn acl_credit_exhaustion_does_not_block_ordered_hci_events() {
        use acl::{ControllerAclPublication, PeripheralConnectionAcl};
        use oer_bluetooth_ll::control::{LePeripheralControl, LePeripheralReceive};

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
        let mut events = PeripheralConnectionHostEvents::new();
        events.connection = EventState::Complete;
        events.established = true;
        let mut acl = PeripheralConnectionAcl::new();
        let mut control = LePeripheralControl::new();
        for data in [7, 9] {
            let bytes = [2, 1, data];
            let LePeripheralReceive::Data(fragment) = control.receive(&bytes, None).unwrap() else {
                panic!("nonempty LL data");
            };
            acl.accept_controller_fragment(fragment);
        }
        assert!(matches!(
            acl.try_publish_controller_packet(&endpoints.controller, 27, true, Some(1)),
            ControllerAclPublication::Published
        ));
        assert!(events.output_is_flow_controlled(&acl, true, Some(1)));
        events.observe_acl_completed(1);
        assert!(!events.output_is_flow_controlled(&acl, true, Some(1)));
        // The event retains its own transport-capacity obligation.
        assert!(matches!(
            events.try_publish(&endpoints.controller),
            PeripheralConnectionHostEventPublication::Pending
        ));
        let mut buffer = [0; 80];
        assert!(matches!(
            block_on(endpoints.host.read(&mut buffer)).unwrap(),
            ControllerToHostPacket::Acl(_)
        ));
        // Free transport space permits the event without returning an ACL credit.
        assert!(!events.output_is_flow_controlled(&acl, true, Some(1)));
        assert!(matches!(
            events.try_publish(&endpoints.controller),
            PeripheralConnectionHostEventPublication::Published
        ));
        let ControllerToHostPacket::Event(packet) =
            block_on(endpoints.host.read(&mut buffer)).unwrap()
        else {
            panic!("TX credit must be an HCI event");
        };
        assert!(matches!(
            Event::try_from(packet).unwrap(),
            Event::NumberOfCompletedPackets(_)
        ));
        assert!(events.output_is_flow_controlled(&acl, true, Some(1)));
        assert!(!acl.controller_credits_settled());
        assert!(matches!(
            acl.try_publish_controller_packet(&endpoints.controller, 27, true, Some(1)),
            ControllerAclPublication::FlowControlled
        ));

        events.observe_encryption_enabled();
        events.observe_encryption_refreshed();
        for _ in 0..2 {
            assert!(!events.output_is_flow_controlled(&acl, true, Some(1)));
            assert!(matches!(
                events.try_publish(&endpoints.controller),
                PeripheralConnectionHostEventPublication::Masked
            ));
        }
        assert!(events.output_is_flow_controlled(&acl, true, Some(1)));

        // Even a masked event must be processed to release its lifecycle duty.
        events.observe_disconnection(0x08);
        assert!(!events.output_is_flow_controlled(&acl, true, Some(1)));
        assert!(matches!(
            events.try_publish(&endpoints.controller),
            PeripheralConnectionHostEventPublication::Masked
        ));
        assert!(events.ready_to_restore_idle());
        assert!(!acl.controller_credits_settled());
        assert!(!events.idle_retirement_ready(Axis::CommandReady, &acl));
    }

    #[test]
    fn establishment_and_peer_termination_queue_two_ordered_events() {
        let acl = acl::PeripheralConnectionAcl::new();
        let completed = LePeripheralConnection::from_request(
            request(),
            LeChannelSelectionAlgorithm::AlgorithmOne,
        )
        .prepare_event()
        .into_submitted()
        .complete(LePeripheralConnectionEventPeerActivity::Observed);
        let mut events = PeripheralConnectionHostEvents::new();
        assert_eq!(events.live_handle(), None);

        events.observe_completion(&completed);
        events.observe_disconnection(0x13);
        assert_eq!(events.live_handle(), None);

        assert!(matches!(events.connection, EventState::Pending(_)));
        assert!(!events.connection_result_published());
        assert!(matches!(events.disconnection, EventState::Pending(_)));
        assert!(!events.ready_to_restore_idle());
        assert!(!events.idle_retirement_ready(Axis::CommandReady, &acl));
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

        let mut resources = LeControllerHciResources::<NoopRawMutex, 1, 1, 80>::new(
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
        assert!(events.connection_result_published());
        assert!(events.has_pending());
        assert!(matches!(
            events.try_publish(&endpoints.controller),
            PeripheralConnectionHostEventPublication::Masked
        ));
        assert!(!events.has_pending());
        assert!(events.ready_to_restore_idle());
        assert!(events.idle_retirement_ready(Axis::CommandReady, &acl));
        assert!(!events.idle_retirement_ready(Axis::ResponsePending, &acl));
    }

    #[test]
    fn encryption_events_follow_connection_order_and_masked_ltk_unblocks_the_procedure() {
        let mut encryption = oer_bluetooth_ll::security::LePeripheralEncryptionProcedure::new();
        encryption
            .begin(
                &[
                    0x03, 0x90, 0x78, 0x56, 0x34, 0x12, 0xef, 0xcd, 0xab, 0x74, 0x24, 0x13, 0x02,
                    0xf1, 0xe0, 0xdf, 0xce, 0xbd, 0xac, 0x24, 0xab, 0xdc, 0xba,
                ],
                oer_bluetooth_ll::security::LePeripheralEncryptionRandom::new([1; 8], [2; 4]),
            )
            .unwrap();
        encryption.response_enqueued().unwrap();
        encryption.observe_transmission_completion(true);
        let request = encryption.take_long_term_key_request().unwrap();

        let mut events = PeripheralConnectionHostEvents::new();
        events.connection = EventState::Complete;
        events.established = true;
        events.observe_long_term_key_request(request);
        events.observe_encryption_enabled();
        events.observe_encryption_refreshed();

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
            events.try_publish(&endpoints.controller),
            PeripheralConnectionHostEventPublication::Masked
        ));
        assert!(events.take_long_term_key_request_masked());
        assert!(!events.take_long_term_key_request_masked());
        assert!(events.encryption_change.is_some());
        assert!(events.encryption_key_refresh.is_some());

        assert!(matches!(
            events.try_publish(&endpoints.controller),
            PeripheralConnectionHostEventPublication::Masked
        ));
        assert!(events.encryption_change.is_none());
        assert!(events.encryption_key_refresh.is_some());
        assert!(matches!(
            events.try_publish(&endpoints.controller),
            PeripheralConnectionHostEventPublication::Masked
        ));
        assert!(!events.has_pending());
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
        assert_eq!(events.live_handle(), None);

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

    #[test]
    fn radio_abort_selects_failed_establishment_or_live_disconnection() {
        let mut before_establishment = PeripheralConnectionHostEvents::new();
        before_establishment.observe_radio_abort();
        assert_eq!(before_establishment.live_handle(), None);
        assert!(matches!(
            before_establishment.disconnection,
            EventState::Complete
        ));
        let EventState::Pending(failed) = &before_establishment.connection else {
            panic!("an aborted initial event reports failed establishment")
        };
        let Event::Le(LeEvent::LeConnectionComplete(failed)) =
            Event::from_hci_bytes_complete(failed.as_bytes()).unwrap()
        else {
            panic!("abort failure changed event kind")
        };
        assert_eq!(failed.status, HciError::HARDWARE_FAILURE.to_status());

        let completed = LePeripheralConnection::from_request(
            request(),
            LeChannelSelectionAlgorithm::AlgorithmOne,
        )
        .prepare_event()
        .into_submitted()
        .complete(LePeripheralConnectionEventPeerActivity::Observed);
        let mut established = PeripheralConnectionHostEvents::new();
        established.observe_completion(&completed);
        assert_eq!(
            established.live_handle(),
            Some(ConnHandle::new(SINGLE_PERIPHERAL_CONNECTION_HANDLE))
        );
        established.observe_radio_abort();
        assert_eq!(established.live_handle(), None);
        assert!(matches!(established.connection, EventState::Pending(_)));
        let EventState::Pending(disconnected) = &established.disconnection else {
            panic!("an aborted established event reports disconnection")
        };
        let Event::DisconnectionComplete(disconnected) =
            Event::from_hci_bytes_complete(disconnected.as_bytes()).unwrap()
        else {
            panic!("abort disconnection changed event kind")
        };
        assert_eq!(disconnected.reason, HciError::HARDWARE_FAILURE.to_status());
    }

    #[test]
    fn acl_credits_publish_after_establishment_and_before_disconnection() {
        let completed = LePeripheralConnection::from_request(
            request(),
            LeChannelSelectionAlgorithm::AlgorithmOne,
        )
        .prepare_event()
        .into_submitted()
        .complete(LePeripheralConnectionEventPeerActivity::Observed);
        let mut events = PeripheralConnectionHostEvents::new();
        events.observe_completion(&completed);
        events.observe_acl_completed(3);
        events.observe_disconnection(0x13);

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
            events.try_publish(&endpoints.controller),
            PeripheralConnectionHostEventPublication::Masked
        ));
        assert!(matches!(
            events.try_publish(&endpoints.controller),
            PeripheralConnectionHostEventPublication::Published
        ));

        let mut buffer = [0; 80];
        let ControllerToHostPacket::Event(event) =
            block_on(endpoints.host.read(&mut buffer)).unwrap()
        else {
            panic!("ACL completion changed HCI packet kind");
        };
        assert_eq!(event.kind.0, 0x13);
        assert_eq!(event.data, [1, 1, 0, 3, 0]);

        assert!(matches!(
            events.try_publish(&endpoints.controller),
            PeripheralConnectionHostEventPublication::Masked
        ));
        assert!(events.ready_to_restore_idle());
    }

    #[test]
    fn connection_updates_are_ordered_after_establishment_and_bounded_without_loss() {
        let mut connection = LePeripheralConnection::from_request(
            request(),
            LeChannelSelectionAlgorithm::AlgorithmOne,
        );
        let mut events = PeripheralConnectionHostEvents::new();
        for (counter, interval) in [(0, 40), (1, 48), (2, 56)] {
            let mut completed = connection
                .prepare_event()
                .into_submitted()
                .complete(LePeripheralConnectionEventPeerActivity::Observed);
            let timing = LeConnectionTiming::new(2, 1, interval, 0, 200).unwrap();
            completed
                .schedule_connection_update(timing, counter)
                .unwrap();
            assert_eq!(events.observe_completion(&completed), counter < 2);
            connection = completed.into_connection();
        }
        assert_eq!(events.connection_update_len, 2);

        let EventState::Pending(connection) = &events.connection else {
            panic!("establishment must remain ahead of parameter updates");
        };
        let Event::Le(LeEvent::LeConnectionComplete(_)) =
            Event::from_hci_bytes_complete(connection.as_bytes()).unwrap()
        else {
            panic!("first event changed kind");
        };
        let first_update = events.connection_updates[0].unwrap();
        let Event::Le(LeEvent::LeConnectionUpdateComplete(update)) =
            Event::from_hci_bytes_complete(first_update.as_bytes()).unwrap()
        else {
            panic!("update event changed kind");
        };
        assert_eq!(update.conn_interval, Duration::from_u16(40));

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
        for _ in 0..3 {
            assert!(matches!(
                events.try_publish(&endpoints.controller),
                PeripheralConnectionHostEventPublication::Masked
            ));
        }
        assert_eq!(events.connection_update_len, 0);
    }

    #[test]
    fn anchor_move_without_host_parameter_change_emits_no_update_event() {
        let request = request();
        let mut completed = LePeripheralConnection::from_request(
            request,
            LeChannelSelectionAlgorithm::AlgorithmOne,
        )
        .prepare_event()
        .into_submitted()
        .complete(LePeripheralConnectionEventPeerActivity::Observed);
        let timing = request.timing();
        let moved = LeConnectionTiming::new(
            timing.window_size_units(),
            timing.window_offset_units() + 1,
            timing.interval_units(),
            timing.peripheral_latency(),
            timing.supervision_timeout_units(),
        )
        .unwrap();
        completed.schedule_connection_update(moved, 0).unwrap();

        let mut events = PeripheralConnectionHostEvents::new();
        assert!(events.observe_completion(&completed));
        assert_eq!(events.connection_update_len, 0);
    }

    #[test]
    fn remote_feature_results_retain_success_and_failure_status() {
        let mut events = PeripheralConnectionHostEvents::new();
        let features = [8, 2, 3, 4, 5, 6, 7, 8];
        events.observe_remote_features(oer_bluetooth_ll::control::LeRemoteFeaturesResult::Success(
            features,
        ));
        let event = events.remote_features.take().unwrap();
        let Event::Le(LeEvent::LeReadRemoteFeaturesComplete(decoded)) =
            Event::from_hci_bytes_complete(event.as_bytes()).unwrap()
        else {
            panic!("remote-feature completion changed event kind");
        };
        assert_eq!(
            decoded.status,
            oer_bluetooth_hci::bt_hci::param::Status::SUCCESS
        );
        assert_eq!(decoded.handle, ConnHandle::new(1));
        assert!(
            decoded
                .le_features
                .supports_peripheral_initiated_features_exchange()
        );
        assert_eq!(&event.as_bytes()[6..], &features);

        events.observe_remote_features(
            oer_bluetooth_ll::control::LeRemoteFeaturesResult::Unsupported,
        );
        let event = events.remote_features.take().unwrap();
        let Event::Le(LeEvent::LeReadRemoteFeaturesComplete(decoded)) =
            Event::from_hci_bytes_complete(event.as_bytes()).unwrap()
        else {
            panic!("remote-feature failure changed event kind");
        };
        assert_eq!(
            decoded.status,
            HciError::UNSUPPORTED_REMOTE_FEATURE.to_status()
        );
        assert!(!decoded.le_features.supports_le_encryption());
        assert_eq!(&event.as_bytes()[6..], &[0; 8]);
    }

    #[test]
    fn remote_version_results_retain_identity_and_failure_status() {
        let mut events = PeripheralConnectionHostEvents::new();
        events.observe_remote_version(oer_bluetooth_ll::control::LeRemoteVersionResult::Success(
            oer_bluetooth_ll::control::LeVersionInformation::new(0x0d, 0x1234, 0x5678),
        ));
        let event = events.remote_version.take().unwrap();
        let Event::ReadRemoteVersionInformationComplete(decoded) =
            Event::from_hci_bytes_complete(event.as_bytes()).unwrap()
        else {
            panic!("remote-version completion changed event kind");
        };
        assert_eq!(
            decoded.status,
            oer_bluetooth_hci::bt_hci::param::Status::SUCCESS
        );
        assert_eq!(decoded.handle, ConnHandle::new(1));
        assert_eq!(decoded.company_id, 0x1234);
        assert_eq!(decoded.subversion, 0x5678);

        events.observe_remote_version(
            oer_bluetooth_ll::control::LeRemoteVersionResult::ResponseTimeout,
        );
        let event = events.remote_version.take().unwrap();
        let Event::ReadRemoteVersionInformationComplete(decoded) =
            Event::from_hci_bytes_complete(event.as_bytes()).unwrap()
        else {
            panic!("remote-version failure changed event kind");
        };
        assert_eq!(
            decoded.status,
            HciError::LMP_LL_RESPONSE_TIMEOUT.to_status()
        );
        assert_eq!(&event.as_bytes()[5..], &[0; 5]);
    }
}
