//! The peripheral connection created by a connection indication.
//!
//! The connection opens the backend's connection, reports LE Connection
//! Complete and then listens at every connection event: the first across the
//! transmit window the indication set, later ones at the anchor the last
//! received packet fixed, widened for clock drift. The next event is planned
//! when the previous one ended; events that can no longer be admitted in time,
//! or that would find no room in the Host output for their receptions, are
//! skipped. While events are skipped for lack of room the supervision timeout
//! still runs against the current time, so a Host that withholds its credits
//! loses the connection with Disconnection Complete `0x08`. Peripheral latency is not used: the peripheral listens at every
//! event it plans.
//!
//! Between events at most one PDU waits for the central's acknowledgement.
//! The next one is chosen in this order: an encryption procedure response,
//! a Link Layer control response or request, then the next fragment of the
//! Host ACL packet in progress, as long as the effective transmit length of the
//! Data Length Update allows; an encrypted fragment leaves room for its MIC.
//! While encryption is being started, paused or restarted only its own
//! responses and a required termination may be sent. A new connection starts
//! the Data Length Update when the Host suggested a longer default.
//!
//! Received packets are authenticated first, then dispatched to the control
//! procedures or delivered to the Host as ACL data. A Channel Map Update or
//! Connection Update waits for the event to end and then enters the Link
//! Layer's pending instant.
//!
//! The connection ends when the central's `LL_TERMINATE_IND` arrives, when the
//! central acknowledged a local `LL_TERMINATE_IND`, when the supervision
//! timeout passes without a received packet, when six events pass without
//! establishment, at once with `0x3D` and without a terminate when a received
//! packet fails its MIC or the encryption procedure, or on Reset. The event in progress is cancelled, the
//! backend's connection is closed and Disconnection Complete follows, except
//! after Reset.

mod receive;
mod timing;

use bt_hci::{
    data::AclPacket,
    param::{AddrKind, BdAddr, ClockAccuracy, ConnHandle, Error as HciError, Status},
};
use oer_bluetooth_hci::{
    LeControllerAclPacket, LeHostAclPacket, LeRandomSource, LeReadRemoteFeaturesCommand,
    LeReadRemoteVersionInformationCommand,
};
use oer_bluetooth_ll::{
    LeDeviceAddressKind,
    connection::{
        LEGACY_CONNECT_IND_LE_1M_AIRTIME_MICROS, LeChannelSelectionAlgorithm,
        LeLegacyConnectionRequest, LePeripheralConnection, LePeripheralConnectionEventCompleted,
        LePeripheralConnectionEventDelta, LePeripheralConnectionEventInFlight,
        LePeripheralConnectionEventPeerActivity, LePeripheralConnectionEventPrepared,
        LePeripheralConnectionState,
    },
    control::{
        LeChannelMapUpdate, LeConnectionUpdate, LePeripheralControl, LePeripheralControlError,
        LePeripheralReceive, LeRemoteFeaturesAdmission, LeRemoteFeaturesResult,
        LeRemoteVersionAdmission, LeRemoteVersionResult, LeVersionInformation,
    },
    data_length::{LE_DATA_LENGTH_MAXIMUM_OCTETS, LeDataLength, LeDataLengths},
    security::{
        LE_ACL_MIC_BYTES, LeLongTermKey, LePeripheralEncryptionProcedure,
        LePeripheralEncryptionRandom, LePeripheralEncryptionTermination,
    },
};
use oer_bluetooth_radio::{
    AccessAddress, ConnectionConfiguration, ConnectionEvent, ConnectionId, CrcInit, DataChannel,
    DataPdu, DataPduKind, EventId, EventResult, LeInstant, LePhy, LeWindow, RadioDuration,
    RadioOutcome, RadioRequest, RadioTiming, TxPower,
};

use crate::planning::{
    PlanningCalculation as C, PlanningCause, PlanningError, PlanningOperation as O,
    PlanningRole as R,
};
use oer_bluetooth_radio::TimingError;

use crate::{RadioWork, advertising::ConnectionIndication, coexistence};

/// The connection's identifier at the backend.
pub(crate) const CONNECTION: ConnectionId = ConnectionId::new(0);
/// The connection handle reported to the Host.
pub(crate) fn handle() -> ConnHandle {
    ConnHandle::new(0)
}
/// Host events one connection event can produce: its receptions plus the
/// procedure events that follow them.
pub(crate) const EVENT_OUTPUT: usize = 8;
/// Largest data channel payload the Data Length Extension allows.
const LL_PAYLOAD: usize = LE_DATA_LENGTH_MAXIMUM_OCTETS as usize;
/// Scheduling priority of the first event and of later events (vendor
/// first-event transform and recurring baseline).
const FIRST_PRIORITY: u8 = 13;
const PRIORITY: u8 = 8;
/// Link Layer procedure response timeout (Core Vol 6 Part B 5.2).
const PROCEDURE_TIMEOUT: RadioDuration = RadioDuration::from_micros(40_000_000);
/// Events tried per plan before the connection waits for the next call.
const PLAN_ATTEMPTS: usize = 64;
/// Connection Terminated due to MIC Failure.
const MIC_FAILURE: u8 = 0x3d;

/// What the connection reports to the Host.
#[derive(Clone, Copy, Debug)]
pub(crate) enum PeripheralEvent {
    Connected {
        peer_kind: AddrKind,
        peer: BdAddr,
        interval_units: u16,
        latency: u16,
        timeout_units: u16,
        accuracy: ClockAccuracy,
    },
    ConnectionFailed(Status),
    Disconnected(Status),
    ConnectionUpdated {
        interval_units: u16,
        latency: u16,
        timeout_units: u16,
    },
    RemoteFeatures(Status, [u8; 8]),
    RemoteVersion(Status, LeVersionInformation),
    LongTermKeyRequest {
        random: [u8; 8],
        diversifier: u16,
    },
    EncryptionChanged(Status, bool),
    KeyRefreshed(Status),
    DataLengthChanged(LeDataLengths),
    CompletedPackets(u16),
}

/// The PDU waiting for the central's acknowledgement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Pending {
    Security,
    Control,
    Acl,
}

/// The Link Layer connection between and during events.
enum Link {
    /// Before the first event.
    Created(LePeripheralConnection),
    InFlight(LePeripheralConnectionEventInFlight),
    Completed(LePeripheralConnectionEventCompleted),
    /// Transiently moved out.
    Moved,
}

#[derive(Clone, Copy, Debug)]
struct Event {
    id: EventId,
    reservation: LeWindow,
    anchor: LeInstant,
    transmit_window: RadioDuration,
}

#[derive(Clone, Copy, Debug)]
struct Closing {
    /// `None` after Reset: no Host event follows.
    reason: Option<Status>,
    cancel_sent: bool,
}

struct Connection {
    request: LeLegacyConnectionRequest,
    created_at: LeInstant,
    link: Link,
    phase: timing::Phase,
    /// The last anchor a received packet established.
    last_activity: Option<LeInstant>,
    event: Option<Event>,
    /// The event whose request is at the backend.
    submitting: Option<(LePeripheralConnectionEventPrepared, Event)>,
    opened: bool,
    open_sent: bool,
    control: LePeripheralControl,
    security: LePeripheralEncryptionProcedure,
    /// Since when a procedure awaits the central.
    procedure_since: Option<LeInstant>,
    pending: Option<Pending>,
    /// The transmission whose request is at the backend.
    transmitting: Option<(Pending, usize)>,
    host_acl: Option<LeHostAclPacket>,
    /// The next received encrypted data PDU has its MIC corrupted.
    #[cfg(feature = "diagnostic-mic-fault")]
    corrupt_next_mic: bool,
    channel_map: Option<LeChannelMapUpdate>,
    connection_update: Option<LeConnectionUpdate>,
    peer_termination: Option<u8>,
    closing: Option<Closing>,
    tx: [u8; LL_PAYLOAD + LE_ACL_MIC_BYTES],
    rx: [u8; receive::PDU_CAPACITY],
}

/// Queue of Host events.
struct Events {
    items: [Option<PeripheralEvent>; 16],
    head: usize,
    len: usize,
}

impl Events {
    const fn new() -> Self {
        Self {
            items: [None; 16],
            head: 0,
            len: 0,
        }
    }

    fn push(&mut self, event: PeripheralEvent) {
        assert!(
            self.len < self.items.len(),
            "the Host event queue is drained per call"
        );
        self.items[(self.head + self.len) % self.items.len()] = Some(event);
        self.len += 1;
    }

    fn pop(&mut self) -> Option<PeripheralEvent> {
        if self.len == 0 {
            return None;
        }
        let event = self.items[self.head].take();
        self.head = (self.head + 1) % self.items.len();
        self.len -= 1;
        event
    }
}

// CAPABILITY: bluetooth-peripheral, bluetooth-peripheral-latency, bluetooth-establishment-supervision-timeout, bluetooth-missed-event-recovery, bluetooth-termination, bluetooth-ll-data-pdu-tx, bluetooth-sn-nesn-retransmission-duplicate-suppression-and-empty-pdu-acknowledgments
pub(crate) struct Peripheral {
    /// Accepted indication and its exact LL owner, before first-window validation.
    opening: Option<(ConnectionIndication, LePeripheralConnection)>,
    connection: Option<Connection>,
    events: Events,
    local_version: Option<LeVersionInformation>,
    /// The Host's suggested transmit length for new connections.
    suggested_data_length: LeDataLength,
    /// The request at the backend.
    requested: Option<RequestKind>,
    /// Data received for the Host, taken after every outcome.
    received: Option<LeControllerAclPacket>,
}

impl Peripheral {
    pub(crate) const fn new(local_version: Option<LeVersionInformation>) -> Self {
        Self {
            opening: None,
            connection: None,
            events: Events::new(),
            local_version,
            suggested_data_length: LeDataLength::MINIMUM,
            requested: None,
            received: None,
        }
    }

    /// Whether a connection exists or is still closing.
    pub(crate) const fn is_active(&self) -> bool {
        self.opening.is_some() || self.connection.is_some()
    }

    /// The handle of the Host-visible connection.
    pub(crate) fn handle(&self) -> Option<ConnHandle> {
        self.connection
            .as_ref()
            .filter(|connection| connection.opened && connection.closing.is_none())
            .map(|_| handle())
    }

    /// The Host's suggested transmit length for new connections.
    pub(crate) const fn suggested_data_length(&self) -> LeDataLength {
        self.suggested_data_length
    }

    /// Suggest the transmit length of later connections; Reset restores
    /// the minimum.
    pub(crate) fn suggest_data_length(&mut self, transmit: LeDataLength) {
        self.suggested_data_length = transmit;
    }

    /// Ask the live connection to send up to `transmit`.
    pub(crate) fn set_data_length(&mut self, transmit: LeDataLength) -> Admission {
        let Some(connection) = self
            .connection
            .as_mut()
            .filter(|c| c.opened && c.closing.is_none())
        else {
            return Admission::UnknownConnection;
        };
        connection.control.set_data_length(transmit);
        Admission::Accepted
    }

    pub(crate) fn take_event(&mut self) -> Option<PeripheralEvent> {
        self.events.pop()
    }

    /// Data received for the Host by the last outcome.
    pub(crate) fn take_received(&mut self) -> Option<LeControllerAclPacket> {
        self.received.take()
    }

    /// Create the connection from an accepted indication.
    pub(crate) fn open(&mut self, indication: ConnectionIndication) {
        let connection = LePeripheralConnection::from_request(
            indication.request,
            LeChannelSelectionAlgorithm::AlgorithmTwo,
        );
        self.opening = Some((indication, connection));
    }

    fn finish_open(&mut self, anchor: LeInstant, transmit_window: RadioDuration) {
        let (indication, connection) = self.opening.take().expect("validated pending indication");
        let request = indication.request;
        self.connection = Some(Connection {
            request,
            created_at: indication.at,
            link: Link::Created(connection),
            phase: timing::Phase {
                anchor,
                reference: anchor,
                transmit_window,
            },
            last_activity: None,
            event: None,
            submitting: None,
            opened: false,
            open_sent: false,
            control: LePeripheralControl::new()
                .with_local_version(self.local_version)
                .with_data_length(self.suggested_data_length),
            security: LePeripheralEncryptionProcedure::new(),
            procedure_since: None,
            pending: None,
            transmitting: None,
            host_acl: None,
            #[cfg(feature = "diagnostic-mic-fault")]
            corrupt_next_mic: false,
            channel_map: None,
            connection_update: None,
            peer_termination: None,
            closing: None,
            tx: [0; LL_PAYLOAD + LE_ACL_MIC_BYTES],
            rx: [0; receive::PDU_CAPACITY],
        });
    }

    /// End the connection without a Host event, as Reset does.
    pub(crate) fn abort(&mut self) {
        self.opening = None;
        if let Some(connection) = &mut self.connection {
            connection.closing = Some(Closing {
                reason: None,
                cancel_sent: false,
            });
        }
    }

    /// Whether the connection has work for the radio. `room` tells whether
    /// the Host output can take one event's receptions.
    pub(crate) fn wants_radio(&self, room: bool) -> bool {
        if self.opening.is_some() {
            return true;
        }
        let Some(connection) = &self.connection else {
            return false;
        };
        if connection.submitting.is_some() || connection.transmitting.is_some() {
            return false;
        }
        if !connection.open_sent {
            return true;
        }
        if let Some(closing) = connection.closing {
            return connection.event.is_none() || !closing.cancel_sent;
        }
        // Without room an established connection still wants the radio: the
        // request pass checks its supervision timeout against the current
        // time instead of planning an event.
        connection.event.is_none()
            && (room
                || connection.last_activity.is_some()
                || connection.pending.is_none() && connection.has_transmission())
    }

    /// The reservation of the event in progress.
    pub(crate) fn busy(&self) -> Option<LeWindow> {
        self.connection
            .as_ref()?
            .event
            .map(|event| event.reservation)
    }

    /// The reservation of the next event at its nominal anchor, which other
    /// roles leave free.
    pub(crate) fn planned(&self, timing: RadioTiming) -> Result<Option<LeWindow>, PlanningError> {
        let Some(connection) = self.connection.as_ref() else {
            return Ok(None);
        };
        if connection.closing.is_some() {
            return Ok(None);
        }
        let interval =
            RadioDuration::from_micros(u64::from(connection.request.timing().interval_micros()));
        let previous = connection
            .event
            .map_or(connection.phase.anchor, |event| event.anchor);
        let anchor = previous.checked_add(interval).ok_or(error(
            O::FutureReservation,
            C::Recurrence,
            TimingError::BeyondEpoch,
        ))?;
        let plan = timing::recurring(
            anchor,
            connection.phase.reference,
            connection.phase.transmit_window,
            connection.request.sleep_clock_accuracy().worst_case_ppm(),
            timing.connection,
        )?;
        timing
            .reservation(plan.window)
            .map(Some)
            .map_err(|cause| error(O::FutureReservation, C::Reservation, cause))
    }

    /// The next radio request. `earliest` is the earliest anchor the backend
    /// admits now.
    pub(crate) fn next_request(
        &mut self,
        ids: &mut u32,
        now: LeInstant,
        timing: RadioTiming,
        room: bool,
    ) -> Result<Option<RadioWork<'_>>, PlanningError> {
        if let Some((indication, _)) = &self.opening {
            let wire = indication.request.timing();
            let packet_end = indication
                .at
                .checked_add(RadioDuration::from_micros(u64::from(
                    LEGACY_CONNECT_IND_LE_1M_AIRTIME_MICROS,
                )))
                .ok_or(error(
                    O::ConnectionIndication,
                    C::FirstAnchor,
                    TimingError::BeyondEpoch,
                ))?;
            let anchor = packet_end
                .checked_add(RadioDuration::from_micros(u64::from(
                    wire.first_window_start_micros(),
                )))
                .ok_or(error(
                    O::ConnectionIndication,
                    C::FirstAnchor,
                    TimingError::BeyondEpoch,
                ))?;
            let transmit_window =
                RadioDuration::from_micros(u64::from(wire.first_window_end_micros()))
                    .checked_sub(RadioDuration::from_micros(u64::from(
                        wire.first_window_start_micros(),
                    )))
                    .ok_or(error(
                        O::ConnectionIndication,
                        C::TransmitWindow,
                        TimingError::DurationOverflow,
                    ))?;
            let plan = timing::first(anchor, transmit_window, timing.connection).map_err(
                |mut cause| {
                    cause.operation = O::ConnectionIndication;
                    cause
                },
            )?;
            timing
                .reservation(plan.window)
                .map_err(|cause| error(O::ConnectionIndication, C::Reservation, cause))?;
            self.finish_open(anchor, transmit_window);
        }
        if self
            .connection
            .as_ref()
            .is_some_and(|connection| connection.closing.is_some() && !connection.open_sent)
        {
            // Nothing reached the backend yet.
            self.connection = None;
            return Ok(None);
        }
        let Some(connection) = self.connection.as_mut() else {
            return Ok(None);
        };
        if !connection.open_sent {
            connection.open_sent = true;
            self.requested = Some(RequestKind::Open);
            let access_address = connection.request.access_address().value().to_le_bytes();
            return Ok(Some(RadioWork::Submit(RadioRequest::OpenConnection(
                ConnectionConfiguration {
                    connection: CONNECTION,
                    access_address: AccessAddress(access_address),
                    crc_init: CrcInit(connection.request.crc_initialization().wire_bytes()),
                    created_at: connection.created_at,
                    tx_power: TxPower::from_dbm(0),
                    phy: LePhy::Le1M,
                },
            ))));
        }
        if let Some(closing) = &mut connection.closing {
            return match connection.event {
                Some(event) if !closing.cancel_sent => {
                    closing.cancel_sent = true;
                    self.requested = Some(RequestKind::Cancel);
                    Ok(Some(RadioWork::Cancel(event.id)))
                }
                Some(_) => Ok(None),
                None => {
                    self.requested = Some(RequestKind::Close);
                    Ok(Some(RadioWork::Submit(RadioRequest::CloseConnection(
                        CONNECTION,
                    ))))
                }
            };
        }
        if connection.event.is_some() {
            return Ok(None);
        }
        connection.check_procedures(now, &mut self.events)?;
        if connection.closing.is_some() {
            // The first closing step: no event is in progress.
            self.requested = Some(RequestKind::Close);
            return Ok(Some(RadioWork::Submit(RadioRequest::CloseConnection(
                CONNECTION,
            ))));
        }
        if connection.pending.is_none()
            && let Some((pending, length)) = connection.prepare_transmission()
        {
            connection.transmitting = Some((pending, length));
            let kind = match pending {
                Pending::Security | Pending::Control => DataPduKind::Control,
                Pending::Acl => connection.acl_kind(),
            };
            let payload_length = connection.tx_length(pending, length);
            self.requested = Some(RequestKind::Transmit);
            return Ok(Some(RadioWork::Submit(RadioRequest::Transmit {
                connection: CONNECTION,
                pdu: DataPdu::new(kind, &connection.tx[..payload_length])
                    .expect("a legacy data PDU fits"),
            })));
        }
        if !room {
            if connection.supervision_lost(now)? {
                connection.closing = Some(Closing {
                    reason: Some(HciError::CONN_TIMEOUT.to_status()),
                    cancel_sent: false,
                });
                self.requested = Some(RequestKind::Close);
                return Ok(Some(RadioWork::Submit(RadioRequest::CloseConnection(
                    CONNECTION,
                ))));
            }
            return Ok(None);
        }
        let earliest = crate::planning::earliest(now, timing, R::Peripheral)?;
        let Some((prepared, plan, reservation)) = connection.plan(earliest, timing)? else {
            if connection.closing.is_some() {
                self.requested = Some(RequestKind::Close);
                return Ok(Some(RadioWork::Submit(RadioRequest::CloseConnection(
                    CONNECTION,
                ))));
            }
            return Ok(None);
        };
        let first = prepared.first_transmit_window_micros().is_some();
        let coexistence = coexistence::peripheral_connection_level(
            prepared.event_counter(),
            prepared.state().supervision_anchor_event_counter(),
            prepared.timing().interval_micros(),
            connection.local_procedure_pending(),
        );
        let channel = DataChannel::new(prepared.channel().get()).expect("a data channel index");
        let interval = RadioDuration::from_micros(u64::from(prepared.timing().interval_micros()));
        let id = crate::controller::allocate_event(ids);
        let event = Event {
            id,
            reservation,
            anchor: plan.anchor,
            transmit_window: plan.transmit_window,
        };
        connection.submitting = Some((prepared, event));
        self.requested = Some(RequestKind::Event);
        Ok(Some(RadioWork::Submit(RadioRequest::ConnectionEvent(
            ConnectionEvent {
                id,
                connection: CONNECTION,
                channel,
                window: plan.window,
                interval,
                timing: plan.timing,
                priority: if first { FIRST_PRIORITY } else { PRIORITY },
                coexistence,
            },
        ))))
    }

    /// The backend's answer to the last request of the connection.
    pub(crate) fn request_done(&mut self, accepted: bool) {
        let Some(request) = self.requested.take() else {
            return;
        };
        let Some(connection) = &mut self.connection else {
            return;
        };
        match request {
            RequestKind::Open => {
                if accepted {
                    connection.opened = true;
                    let request = connection.request;
                    let timing = request.timing();
                    let initiator = request.initiator();
                    self.events.push(PeripheralEvent::Connected {
                        peer_kind: match initiator.kind() {
                            LeDeviceAddressKind::Public => AddrKind::PUBLIC,
                            LeDeviceAddressKind::Random => AddrKind::RANDOM,
                        },
                        peer: BdAddr::new(initiator.wire_bytes()),
                        interval_units: timing.interval_units(),
                        latency: timing.peripheral_latency(),
                        timeout_units: timing.supervision_timeout_units(),
                        accuracy: clock_accuracy(request.sleep_clock_accuracy().encoded()),
                    });
                } else {
                    self.connection = None;
                    self.events.push(PeripheralEvent::ConnectionFailed(
                        HciError::CONN_REJECTED_LIMITED_RESOURCES.to_status(),
                    ));
                }
            }
            RequestKind::Cancel => {}
            RequestKind::Close => {
                let reason = connection.closing.and_then(|closing| closing.reason);
                connection.close_procedures(&mut self.events);
                self.connection = None;
                if let Some(reason) = reason {
                    self.events.push(PeripheralEvent::Disconnected(reason));
                }
            }
            RequestKind::Transmit => {
                let (pending, length) = connection
                    .transmitting
                    .take()
                    .expect("a transmission was requested");
                if accepted {
                    connection.enqueued(pending, length);
                    connection.pending = Some(pending);
                }
            }
            RequestKind::Event => {
                let (prepared, event) = connection
                    .submitting
                    .take()
                    .expect("an event was requested");
                let in_flight = prepared.into_submitted();
                connection.phase.anchor = event.anchor;
                connection.phase.transmit_window = event.transmit_window;
                if accepted {
                    connection.link = Link::InFlight(in_flight);
                    connection.event = Some(event);
                } else {
                    // A refused event is a missed one.
                    connection.link = Link::Completed(
                        in_flight.complete(LePeripheralConnectionEventPeerActivity::Missed),
                    );
                    connection.after_event(&mut self.events);
                }
            }
        }
    }

    /// Account one backend observation.
    pub(crate) fn outcome(
        &mut self,
        outcome: RadioOutcome<'_>,
        random: Option<&dyn LeRandomSource>,
    ) {
        let Some(connection) = &mut self.connection else {
            return;
        };
        match outcome {
            RadioOutcome::Received { id, pdu } if connection.owns(id) => {
                self.received =
                    connection.receive(pdu.pdu, random, self.local_version, &mut self.events);
            }
            RadioOutcome::TransmitAcknowledged(id) if id == CONNECTION => {
                connection.acknowledged(&mut self.events);
            }
            RadioOutcome::EventEnded { id, result } if connection.owns(id) => {
                connection.event = None;
                let Link::InFlight(in_flight) =
                    core::mem::replace(&mut connection.link, Link::Moved)
                else {
                    unreachable!("an event in progress is in flight")
                };
                let activity = match result {
                    EventResult::Executed {
                        anchor: Some(anchor),
                    }
                    | EventResult::TimingFailed {
                        anchor: Ok(Some(anchor)),
                        ..
                    } => {
                        connection.phase = timing::Phase {
                            anchor,
                            reference: anchor,
                            transmit_window: RadioDuration::from_micros(0),
                        };
                        connection.last_activity = Some(anchor);
                        LePeripheralConnectionEventPeerActivity::Observed
                    }
                    // Hardware captured a peer packet, but its timestamp is
                    // invalid. Account peer activity without inventing a time
                    // reference or renewing timestamp-based supervision.
                    EventResult::TimingFailed { anchor: Err(_), .. } => {
                        LePeripheralConnectionEventPeerActivity::Observed
                    }
                    EventResult::Executed { anchor: None }
                    | EventResult::NotExecuted
                    | EventResult::TimingFailed {
                        anchor: Ok(None), ..
                    } => LePeripheralConnectionEventPeerActivity::Missed,
                };
                connection.link = Link::Completed(in_flight.complete(activity));
                connection.after_event(&mut self.events);
            }
            _ => {}
        }
    }

    /// Host Disconnect.
    pub(crate) fn disconnect(&mut self, reason: u8) -> bool {
        match &mut self.connection {
            Some(connection) if connection.opened && connection.closing.is_none() => {
                connection.control.request_host_termination(reason);
                true
            }
            _ => false,
        }
    }

    /// Host LE Read Remote Features: whether the request was admitted.
    pub(crate) fn read_remote_features(&mut self, _: &LeReadRemoteFeaturesCommand) -> Admission {
        let Some(connection) = self.connection.as_mut().filter(|c| c.closing.is_none()) else {
            return Admission::UnknownConnection;
        };
        match connection.control.admit_remote_feature_request() {
            LeRemoteFeaturesAdmission::Cached(features) => {
                self.events
                    .push(PeripheralEvent::RemoteFeatures(Status::SUCCESS, features));
                Admission::Accepted
            }
            LeRemoteFeaturesAdmission::Admitted => {
                if let Some(features) = connection.control.activate_remote_feature_request() {
                    self.events
                        .push(PeripheralEvent::RemoteFeatures(Status::SUCCESS, features));
                }
                Admission::Accepted
            }
            LeRemoteFeaturesAdmission::Busy => Admission::Disallowed,
        }
    }

    /// Host Read Remote Version Information.
    pub(crate) fn read_remote_version(
        &mut self,
        _: &LeReadRemoteVersionInformationCommand,
    ) -> Admission {
        let Some(connection) = self.connection.as_mut().filter(|c| c.closing.is_none()) else {
            return Admission::UnknownConnection;
        };
        match connection.control.admit_remote_version_request() {
            LeRemoteVersionAdmission::Cached(version) => {
                self.events
                    .push(PeripheralEvent::RemoteVersion(Status::SUCCESS, version));
                Admission::Accepted
            }
            LeRemoteVersionAdmission::Admitted => {
                if let Some(version) = connection.control.activate_remote_version_request() {
                    self.events
                        .push(PeripheralEvent::RemoteVersion(Status::SUCCESS, version));
                }
                Admission::Accepted
            }
            LeRemoteVersionAdmission::Busy | LeRemoteVersionAdmission::LocalIdentityUnavailable => {
                Admission::Disallowed
            }
        }
    }

    /// Host LE Long Term Key Request Reply or Negative Reply.
    pub(crate) fn long_term_key(&mut self, key: Option<[u8; 16]>) -> Admission {
        let Some(connection) = self.connection.as_mut().filter(|c| c.closing.is_none()) else {
            return Admission::UnknownConnection;
        };
        let result = match key {
            Some(key) => connection
                .security
                .provide_long_term_key(LeLongTermKey::new(key))
                .map_err(|_| ()),
            None => connection.security.reject_long_term_key().map_err(|_| ()),
        };
        match result {
            Ok(()) => Admission::Accepted,
            Err(()) => Admission::Disallowed,
        }
    }

    /// Arm one received-MIC corruption on the connection `handle`, encrypted
    /// or not yet.
    #[cfg(feature = "diagnostic-mic-fault")]
    pub(crate) fn arm_mic_corruption(&mut self, handle: ConnHandle) -> Status {
        if self.handle() != Some(handle) {
            return HciError::UNKNOWN_CONN_IDENTIFIER.to_status();
        }
        match self.connection.as_mut() {
            Some(connection) if connection.closing.is_none() => {
                connection.corrupt_next_mic = true;
                Status::SUCCESS
            }
            _ => HciError::CMD_DISALLOWED.to_status(),
        }
    }

    /// Whether a Host ACL packet can be taken now.
    pub(crate) fn acl_ready(&self) -> bool {
        self.connection
            .as_ref()
            .is_some_and(|connection| connection.host_acl.is_none())
    }

    /// Take one Host ACL packet. A packet the connection cannot carry is
    /// discarded.
    pub(crate) fn acl(&mut self, packet: AclPacket<'_>, capacity: u16) {
        let handle = self.handle();
        let Some(connection) = &mut self.connection else {
            return;
        };
        if let Ok(packet) = LeHostAclPacket::copy_from(packet, handle, capacity) {
            connection.host_acl = Some(packet);
        }
    }
}

/// Which request of the connection is at the backend.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RequestKind {
    Open,
    Cancel,
    Close,
    Transmit,
    Event,
}

/// How a Host command about the connection was admitted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Admission {
    Accepted,
    Disallowed,
    UnknownConnection,
}

impl Connection {
    fn owns(&self, id: EventId) -> bool {
        self.event.is_some_and(|event| event.id == id)
    }

    fn has_transmission(&self) -> bool {
        self.security.pending_response().is_some()
            || self.control.pending_response().is_some()
            || self.host_acl.is_some()
    }

    /// Choose the next PDU and write it to `tx`: its kind and plaintext
    /// length.
    fn prepare_transmission(&mut self) -> Option<(Pending, usize)> {
        if let Some(response) = self.security.pending_response() {
            let bytes = response.as_bytes();
            self.tx[..bytes.len()].copy_from_slice(bytes);
            return Some((Pending::Security, bytes.len()));
        }
        if let Some(response) = self.control.pending_response() {
            let bytes = response.as_bytes();
            if self.security.blocks_unrelated_transmission()
                && !self.security.permits_control_pdu(bytes)
            {
                return None;
            }
            let length = bytes.len();
            self.tx[..length].copy_from_slice(bytes);
            return Some((Pending::Control, length));
        }
        if self.security.blocks_unrelated_transmission() {
            return None;
        }
        let fragment = self.host_acl.as_ref()?.next_fragment(self.acl_payload())?;
        let length = fragment.payload().len();
        self.tx[..length].copy_from_slice(fragment.payload());
        Some((Pending::Acl, length))
    }

    /// The longest ACL fragment the effective transmit length allows; an
    /// encrypted fragment leaves room for its MIC in the air time.
    fn acl_payload(&self) -> usize {
        let mic = if self.security.is_active() {
            LE_ACL_MIC_BYTES as u16
        } else {
            0
        };
        usize::from(self.control.data_lengths().transmit.payload_octets(mic))
    }

    fn acl_kind(&self) -> DataPduKind {
        let continuing = self
            .host_acl
            .as_ref()
            .and_then(|packet| packet.next_fragment(self.acl_payload()))
            .is_some_and(|fragment| fragment.is_continuing());
        if continuing {
            DataPduKind::Continuation
        } else {
            DataPduKind::Start
        }
    }

    /// The payload length on air: encrypted packets carry a MIC. Security
    /// responses arrive already encrypted when they must be.
    fn tx_length(&mut self, pending: Pending, length: usize) -> usize {
        if pending == Pending::Security {
            return length;
        }
        let header = match pending {
            Pending::Acl => match self.acl_kind() {
                DataPduKind::Continuation => 0x01,
                _ => 0x02,
            },
            _ => 0x03,
        };
        match self.security.active_encryption() {
            Some(encryption) => encryption
                .encrypt_new_packet(header, &mut self.tx, length)
                .unwrap_or(length),
            None => length,
        }
    }

    fn enqueued(&mut self, pending: Pending, length: usize) {
        match pending {
            Pending::Security => {
                let _ = self.security.response_enqueued();
            }
            Pending::Control => self.control.response_enqueued(),
            Pending::Acl => {
                if let Some(packet) = &mut self.host_acl {
                    packet.fragment_enqueued(length);
                }
            }
        }
    }

    fn acknowledged(&mut self, events: &mut Events) {
        match self.pending.take() {
            Some(Pending::Security) => self.security.observe_transmission_completion(true),
            Some(Pending::Control) => self.control.observe_transmission_completion(true),
            Some(Pending::Acl) => {
                if let Some(packet) = &mut self.host_acl {
                    packet.observe_fragment_completion(true);
                    if packet.is_complete() {
                        self.host_acl = None;
                        events.push(PeripheralEvent::CompletedPackets(1));
                    }
                }
            }
            None => {}
        }
        self.poll_procedures(events);
    }

    fn receive(
        &mut self,
        pdu: &[u8],
        random: Option<&dyn LeRandomSource>,
        local_version: Option<LeVersionInformation>,
        events: &mut Events,
    ) -> Option<LeControllerAclPacket> {
        let mut received = None;
        #[cfg(feature = "diagnostic-mic-fault")]
        let mut corrupted = [0; receive::PDU_CAPACITY];
        #[cfg(feature = "diagnostic-mic-fault")]
        let pdu = match self
            .corrupt_next_mic
            .then(|| crate::diagnostic::corrupt_data_mic(pdu, &mut corrupted))
            .flatten()
        {
            Some(corrupted) if self.security.is_active() => {
                self.corrupt_next_mic = false;
                corrupted
            }
            _ => pdu,
        };
        let decoded = receive::decode(&mut self.security, pdu, &mut self.rx, || {
            let source = random?;
            let diversifier = source.random_bytes().ok()?;
            let vector = source.random_bytes().ok()?;
            Some(LePeripheralEncryptionRandom::new(
                diversifier,
                [vector[0], vector[1], vector[2], vector[3]],
            ))
        });
        if let Some(decoded) = decoded {
            match self.control.receive(decoded, local_version) {
                Ok(LePeripheralReceive::Data(fragment)) => {
                    received = LeControllerAclPacket::copy_from_ll(
                        handle(),
                        fragment.is_continuing(),
                        fragment.payload(),
                    );
                }
                Ok(LePeripheralReceive::Control) => {}
                Ok(LePeripheralReceive::ChannelMapUpdate(update)) => {
                    self.channel_map = Some(update);
                }
                Ok(LePeripheralReceive::ConnectionUpdate(update)) => {
                    self.connection_update = Some(update);
                }
                Err(LePeripheralControlError::PeerTermination { reason }) => {
                    self.peer_termination = Some(reason);
                }
                Err(error) => {
                    if let Some(reason) = error.termination_reason() {
                        self.control.request_local_termination(reason);
                    }
                }
            }
        }
        self.poll_procedures(events);
        received
    }

    /// Report procedure results and required terminations.
    fn poll_procedures(&mut self, events: &mut Events) {
        if let Some(request) = self.security.take_long_term_key_request() {
            events.push(PeripheralEvent::LongTermKeyRequest {
                random: request.random_number(),
                diversifier: request.encrypted_diversifier(),
            });
        }
        if self.security.take_encryption_enabled() {
            events.push(PeripheralEvent::EncryptionChanged(Status::SUCCESS, true));
        }
        if self.security.take_encryption_refreshed() {
            events.push(PeripheralEvent::KeyRefreshed(Status::SUCCESS));
        }
        match self.security.termination() {
            Some(LePeripheralEncryptionTermination::Terminate(reason)) => {
                self.control.request_local_termination(reason);
            }
            Some(LePeripheralEncryptionTermination::Exit) if self.closing.is_none() => {
                self.closing = Some(Closing {
                    reason: Some(Status::new(MIC_FAILURE)),
                    cancel_sent: false,
                });
            }
            Some(LePeripheralEncryptionTermination::Exit) | None => {}
        }
        if let Some(result) = self.control.take_remote_features_result() {
            events.push(features_event(result));
        }
        if let Some(result) = self.control.take_remote_version_result() {
            events.push(version_event(result));
        }
        if let Some(lengths) = self.control.take_data_length_change() {
            events.push(PeripheralEvent::DataLengthChanged(lengths));
        }
    }

    /// Procedure timeouts, checked before planning.
    fn check_procedures(
        &mut self,
        now: LeInstant,
        events: &mut Events,
    ) -> Result<(), PlanningError> {
        if !self.local_procedure_pending() {
            self.procedure_since = None;
            return Ok(());
        }
        let since = self.procedure_since.unwrap_or(now);
        let elapsed = now.checked_duration_since(since).ok_or(error(
            O::ProcedureTimeout,
            C::Elapsed,
            TimingError::ReversedTime,
        ))?;
        if elapsed < PROCEDURE_TIMEOUT {
            self.procedure_since = Some(since);
            return Ok(());
        }
        self.procedure_since = None;
        if !self.security.is_idle() && !self.security.is_active() {
            self.control.request_local_termination(
                HciError::LMP_LL_RESPONSE_TIMEOUT.to_status().into_inner(),
            );
        } else {
            self.control.expire_local_procedure();
            self.poll_procedures(events);
        }
        Ok(())
    }

    fn supervision_lost(&self, now: LeInstant) -> Result<bool, PlanningError> {
        let Some(last) = self.last_activity else {
            return Ok(false);
        };
        let elapsed = now.checked_duration_since(last).ok_or(error(
            O::Supervision,
            C::Elapsed,
            TimingError::ReversedTime,
        ))?;
        Ok(elapsed
            > RadioDuration::from_micros(u64::from(
                self.request.timing().supervision_timeout_micros(),
            )))
    }

    /// Whether a local procedure awaits the central.
    fn local_procedure_pending(&self) -> bool {
        !self.security.is_idle() && !self.security.is_active()
            || self.control.local_feature_request_transmitted()
            || self.control.local_version_request_transmitted()
            || self.control.local_data_length_request_transmitted()
    }

    fn close_procedures(&mut self, events: &mut Events) {
        self.control.close_remote_feature_request();
        self.control.close_remote_version_request();
        self.poll_procedures(events);
        if self.host_acl.take().is_some() {
            // The Host's credit returns with the connection.
        }
    }

    /// Apply what the ended event decided.
    fn after_event(&mut self, events: &mut Events) {
        let Link::Completed(completed) = &mut self.link else {
            return;
        };
        if let Some(transition) = completed.connection_timing_transition()
            && transition.host_parameters_changed()
        {
            let timing = transition.updated();
            events.push(PeripheralEvent::ConnectionUpdated {
                interval_units: timing.interval_units(),
                latency: timing.peripheral_latency(),
                timeout_units: timing.supervision_timeout_units(),
            });
        }
        let mut failure = None;
        if let Some(update) = self.channel_map.take()
            && let Err(error) =
                completed.schedule_channel_map_update(update.channel_map(), update.instant())
        {
            failure = LePeripheralControlError::ChannelMapUpdate(error).termination_reason();
        }
        if let Some(update) = self.connection_update.take()
            && let Err(error) =
                completed.schedule_connection_update(update.timing(), update.instant())
        {
            failure = LePeripheralControlError::ConnectionUpdate(error).termination_reason();
        }
        if let Some(reason) = failure {
            self.control.request_local_termination(reason);
        }
        let established = completed.connection_state() != LePeripheralConnectionState::Created;
        let reason = if let Some(reason) = self.peer_termination {
            Some(Status::new(reason))
        } else if self.control.local_termination_acknowledged() {
            self.control
                .local_termination_completion_reason()
                .map(Status::new)
        } else if !established && completed.establishment_failed() {
            Some(HciError::CONN_FAILED_SYNCHRONIZATION_TIMEOUT.to_status())
        } else {
            None
        };
        if let Some(reason) = reason
            && self.closing.is_none()
        {
            self.closing = Some(Closing {
                reason: Some(reason),
                cancel_sent: false,
            });
        }
        self.poll_procedures(events);
    }

    /// Plan the next event no earlier than `earliest`.
    fn plan(
        &mut self,
        earliest: LeInstant,
        timing: RadioTiming,
    ) -> Result<Option<(LePeripheralConnectionEventPrepared, timing::Plan, LeWindow)>, PlanningError>
    {
        match core::mem::replace(&mut self.link, Link::Moved) {
            Link::Created(connection) => {
                let prepared = connection.prepare_event();
                let geometry = timing::first(
                    self.phase.anchor,
                    self.phase.transmit_window,
                    timing.connection,
                )
                .and_then(|plan| {
                    timing
                        .reservation(plan.window)
                        .map(|reservation| (plan, reservation))
                        .map_err(|cause| error(O::Event, C::Reservation, cause))
                });
                let (plan, reservation) = match geometry {
                    Ok(geometry) => geometry,
                    Err(cause) => {
                        self.link = Link::Created(prepared.cancel());
                        return Err(cause);
                    }
                };
                if plan.window.start() >= earliest {
                    return Ok(Some((prepared, plan, reservation)));
                }
                // A legitimate missed first window advances the LL policy.
                self.link = Link::Completed(
                    prepared
                        .into_submitted()
                        .complete(LePeripheralConnectionEventPeerActivity::Missed),
                );
                self.plan(earliest, timing)
            }
            Link::Completed(completed) => self.plan_recurring(completed, earliest, timing),
            link => {
                self.link = link;
                Ok(None)
            }
        }
    }

    fn plan_recurring(
        &mut self,
        mut completed: LePeripheralConnectionEventCompleted,
        earliest: LeInstant,
        timing: RadioTiming,
    ) -> Result<Option<(LePeripheralConnectionEventPrepared, timing::Plan, LeWindow)>, PlanningError>
    {
        let peer_ppm = self.request.sleep_clock_accuracy().worst_case_ppm();
        let mut delta = 1_u16;
        for _ in 0..PLAN_ATTEMPTS {
            let step = LePeripheralConnectionEventDelta::new(delta)
                .expect("a validated nonzero event distance");
            let provisional = completed.prepare_recurring_event(step);
            let geometry = (|| {
                let (anchor, transmit_window) = self.anchor_of(&provisional, delta)?;
                let lost = if let Some(last) = self.last_activity {
                    let elapsed = anchor.checked_duration_since(last).ok_or(error(
                        O::Supervision,
                        C::Elapsed,
                        TimingError::ReversedTime,
                    ))?;
                    elapsed
                        > RadioDuration::from_micros(u64::from(
                            provisional.timing().supervision_timeout_micros(),
                        ))
                } else {
                    false
                };
                if lost {
                    return Ok(RecurringCandidate::Lost(HciError::CONN_TIMEOUT.to_status()));
                }
                if provisional.connection_state() == LePeripheralConnectionState::Created
                    && provisional.event_counter() >= 6
                {
                    return Ok(RecurringCandidate::Lost(
                        HciError::CONN_FAILED_SYNCHRONIZATION_TIMEOUT.to_status(),
                    ));
                }
                let plan = timing::recurring(
                    anchor,
                    self.phase.reference,
                    transmit_window,
                    peer_ppm,
                    timing.connection,
                )?;
                let reservation = timing
                    .reservation(plan.window)
                    .map_err(|cause| error(O::Event, C::Reservation, cause))?;
                if plan.window.start() >= earliest {
                    return Ok(RecurringCandidate::Ready(plan, reservation));
                }
                let interval =
                    RadioDuration::from_micros(u64::from(provisional.timing().interval_micros()));
                // The decoded wire interval is nonzero. A late window may
                // have its nominal anchor ahead of earliest because of widening.
                let behind = if earliest > anchor {
                    earliest.checked_duration_since(anchor).ok_or(error(
                        O::Event,
                        C::SlotAlignment,
                        TimingError::ReversedTime,
                    ))?
                } else {
                    RadioDuration::from_micros(0)
                };
                let skip = (behind.as_micros() / interval.as_micros()).max(1);
                let next = u64::from(delta)
                    .checked_add(skip)
                    .and_then(|next| u16::try_from(next).ok())
                    .ok_or(PlanningError {
                        role: R::Peripheral,
                        operation: O::Event,
                        calculation: C::SlotAlignment,
                        cause: PlanningCause::EventDeltaOutsideRange,
                    })?;
                Ok(RecurringCandidate::Late(next))
            })();
            match geometry {
                Ok(RecurringCandidate::Ready(plan, reservation)) => {
                    return Ok(Some((provisional.commit(), plan, reservation)));
                }
                Ok(RecurringCandidate::Lost(reason)) => {
                    self.link = Link::Completed(provisional.cancel());
                    self.closing = Some(Closing {
                        reason: Some(reason),
                        cancel_sent: false,
                    });
                    return Ok(None);
                }
                Ok(RecurringCandidate::Late(next)) => {
                    completed = provisional.cancel();
                    delta = next;
                }
                Err(cause) => {
                    self.link = Link::Completed(provisional.cancel());
                    return Err(cause);
                }
            }
        }
        self.link = Link::Completed(completed);
        Ok(None)
    }

    fn anchor_of(
        &self,
        provisional: &oer_bluetooth_ll::connection::LePeripheralConnectionRecurringEventProvisional,
        delta: u16,
    ) -> Result<(LeInstant, RadioDuration), PlanningError> {
        let last = provisional.event_counter().wrapping_sub(delta);
        let span = match provisional.connection_timing_transition() {
            Some(transition) => {
                // Wire event counters are modular. The physical intervals
                // up to and after the instant are checked portable durations.
                let before = transition.instant().wrapping_sub(last);
                let after = delta.checked_sub(before).ok_or(error(
                    O::Event,
                    C::IntervalTransition,
                    TimingError::DurationOverflow,
                ))?;
                RadioDuration::from_micros(u64::from(transition.previous().interval_micros()))
                    .checked_mul(u64::from(before))
                    .and_then(|span| {
                        span.checked_add(RadioDuration::from_micros(
                            u64::from(transition.updated().window_offset_units()) * 1_250,
                        ))
                    })
                    .and_then(|span| {
                        RadioDuration::from_micros(u64::from(
                            transition.updated().interval_micros(),
                        ))
                        .checked_mul(u64::from(after))
                        .and_then(|later| span.checked_add(later))
                    })
            }
            None => RadioDuration::from_micros(u64::from(provisional.timing().interval_micros()))
                .checked_mul(u64::from(delta)),
        }
        .ok_or(error(
            O::Event,
            C::IntervalTransition,
            TimingError::DurationOverflow,
        ))?;
        let transmit_window = match provisional.connection_timing_transition() {
            Some(transition) => RadioDuration::from_micros(
                u64::from(transition.updated().window_size_units()) * 1_250,
            ),
            None => self.phase.transmit_window,
        };
        let anchor = self.phase.anchor.checked_add(span).ok_or(error(
            O::Event,
            C::Recurrence,
            TimingError::BeyondEpoch,
        ))?;
        Ok((anchor, transmit_window))
    }
}

fn features_event(result: LeRemoteFeaturesResult) -> PeripheralEvent {
    match result {
        LeRemoteFeaturesResult::Success(features) => {
            PeripheralEvent::RemoteFeatures(Status::SUCCESS, features)
        }
        LeRemoteFeaturesResult::Unsupported => PeripheralEvent::RemoteFeatures(
            HciError::UNSUPPORTED_REMOTE_FEATURE.to_status(),
            [0; 8],
        ),
        LeRemoteFeaturesResult::Rejected { reason } => {
            PeripheralEvent::RemoteFeatures(Status::new(reason), [0; 8])
        }
        LeRemoteFeaturesResult::ResponseTimeout => {
            PeripheralEvent::RemoteFeatures(HciError::LMP_LL_RESPONSE_TIMEOUT.to_status(), [0; 8])
        }
        LeRemoteFeaturesResult::ConnectionClosed => {
            PeripheralEvent::RemoteFeatures(HciError::UNKNOWN_CONN_IDENTIFIER.to_status(), [0; 8])
        }
    }
}

fn version_event(result: LeRemoteVersionResult) -> PeripheralEvent {
    let none = LeVersionInformation::new(0, 0, 0);
    match result {
        LeRemoteVersionResult::Success(version) => {
            PeripheralEvent::RemoteVersion(Status::SUCCESS, version)
        }
        LeRemoteVersionResult::Unsupported => {
            PeripheralEvent::RemoteVersion(HciError::UNSUPPORTED_REMOTE_FEATURE.to_status(), none)
        }
        LeRemoteVersionResult::Rejected { reason } => {
            PeripheralEvent::RemoteVersion(Status::new(reason), none)
        }
        LeRemoteVersionResult::ResponseTimeout => {
            PeripheralEvent::RemoteVersion(HciError::LMP_LL_RESPONSE_TIMEOUT.to_status(), none)
        }
        LeRemoteVersionResult::ConnectionClosed => {
            PeripheralEvent::RemoteVersion(HciError::UNKNOWN_CONN_IDENTIFIER.to_status(), none)
        }
    }
}

const fn clock_accuracy(encoded: u8) -> ClockAccuracy {
    match encoded {
        0 => ClockAccuracy::Ppm500,
        1 => ClockAccuracy::Ppm250,
        2 => ClockAccuracy::Ppm150,
        3 => ClockAccuracy::Ppm100,
        4 => ClockAccuracy::Ppm75,
        5 => ClockAccuracy::Ppm50,
        6 => ClockAccuracy::Ppm30,
        _ => ClockAccuracy::Ppm20,
    }
}

enum RecurringCandidate {
    Ready(timing::Plan, LeWindow),
    Late(u16),
    Lost(Status),
}

fn error(operation: O, calculation: C, cause: TimingError) -> PlanningError {
    PlanningError::timing(R::Peripheral, operation, calculation, cause)
}
