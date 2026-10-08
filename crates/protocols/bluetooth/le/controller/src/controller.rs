//! The sans-IO LE Controller.

use bt_hci::{
    cmd::Opcode,
    data::AclPacket,
    param::{ControllerToHostFlowControl, Duration, Error as HciError, Status},
};
use oer_bluetooth_hci::{
    BootstrapCommandCompleteEvent, HciCommandPacket, LeAcceptListCommand,
    LeAcceptListCommandCompleteEvent, LeAcceptListEntry, LeConnectionUpdateCompleteEvent,
    LeControllerAclPacket, LeControllerBootstrap, LeControllerBootstrapConfig,
    LeControllerCommandClassification, LeDataLengthChangeEvent, LeDataLengthCommand,
    LeDataLengthCommandCompleteEvent, LeDataLengthParameters, LeDisconnectionCompleteEvent,
    LeDtmCommand, LeDtmCommandCompleteEvent, LeEncryptionChangeEvent,
    LeEncryptionKeyRefreshCompleteEvent, LeHostCompletedPacketsCommand,
    LeHostCompletedPacketsDecodeError, LeHostCompletedPacketsErrorEvent,
    LeLegacyAdvertisingCommandCompleteEvent, LeLegacyAdvertisingCommandKind,
    LeLegacyAdvertisingConfiguration, LeLegacyAdvertisingConfigurationCommand,
    LeLegacyAdvertisingEnableRequest, LeLegacyScanningCommandCompleteEvent,
    LeLegacyScanningCommandKind, LeLegacyScanningConfiguration, LeLongTermKeyCommandCompleteEvent,
    LeLongTermKeyRequestEvent, LeLongTermKeyRequestReplyCommand, LeNumberOfCompletedPacketsEvent,
    LePeripheralConnectionCompleteEvent, LeRandCommandCompleteEvent, LeRandomSource,
    LeReadRemoteFeaturesCompleteEvent, LeReadRemoteVersionInformationCompleteEvent,
    OwnedBootstrapCommand, classify_le_controller_command,
};
use oer_bluetooth_ll::{
    control::LeVersionInformation,
    data_length::{LeDataLength, LeDataLengths},
    dtm::DTM_MAX_PAYLOAD,
};
use oer_bluetooth_radio::{
    AcceptListChange, AcceptListDevice, EventId, LeInstant, RadioActivity, RadioDuration,
    RadioFault, RadioOutcome, RadioRequest, RadioTiming, RequestError,
};

use crate::planning::{
    PlanningCalculation as C, PlanningError, PlanningOperation as O, PlanningRole as R,
};

use crate::{
    HciPacket,
    accept_list::AcceptList,
    advertising::{ADVERTISING_DELAY_MAX, Advertiser},
    arbiter::place,
    dtm::{DtmRadioWork, DtmRole},
    output::Output,
    peripheral::{Admission, EVENT_OUTPUT, Peripheral, PeripheralEvent, handle},
    scanning::Scanner,
};

/// Controller-to-Host ACL packets held for Host credits: every reception of
/// one connection event.
const HELD_ACL: usize = EVENT_OUTPUT;

/// Static configuration of one Controller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeControllerConfig {
    /// Reset-scoped HCI profile.
    pub bootstrap: LeControllerBootstrapConfig,
    /// Identity for Link Layer version exchange. Without one, a central's
    /// version request terminates the connection and Read Remote Version
    /// Information is refused.
    pub version: Option<LeVersionInformation>,
}

/// Slack between planning an event and the earliest instant the backend
/// admits, covering the time until the request reaches it.
pub const PLANNING_SLACK: RadioDuration = RadioDuration::from_micros(500);

/// Placement attempts per [`LeController::next_request`]; each failed
/// attempt skips one event of a role.
const PLACEMENT_ATTEMPTS: usize = 8;

/// Event identities of the roles; Direct Test Mode uses the upper half.
const LAST_ROLE_EVENT_ID: u32 = 0x7fff_ffff;

/// The command was not taken: a command is in progress or the output has no
/// slot for its response.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControllerBusy;

/// The request in flight at the backend.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Owner {
    AcceptList,
    Dtm,
    AdvertiserControl,
    AdvertiserEvent,
    ScannerControl,
    ScannerWindow,
    Peripheral,
}

/// A command whose response waits for radio work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Pending {
    Reset,
    AcceptList(Opcode),
    Advertising(Opcode),
    Scanning(Opcode),
    TestEnd,
}

/// Sans-IO LE Controller: HCI command service, Link Layer roles and the
/// radio event arbiter.
///
/// The Host side feeds command packets through [`Self::command`] and drains
/// complete responses and events through [`Self::front`] and [`Self::pop`].
/// The radio side asks [`Self::next_request`] for one request at a time,
/// reports the backend's answer to [`Self::request_done`] before asking
/// again, and feeds every backend observation to [`Self::outcome`].
///
/// `OUTPUT` bounds the queued Controller-to-Host packets. One slot is kept
/// for the next command response; LE Advertising Reports that find the rest
/// full are dropped and counted. A connection event is planned only while the
/// output has room for everything it can produce, so connection data and
/// events are never dropped.
// CAPABILITY: bluetooth-hci-command-event-packets, bluetooth-hci-reset, bluetooth-event-masks, bluetooth-connection-handles-connection-complete-disconnection-complete, bluetooth-hci-acl-routing-and-bidirectional-flow-control
pub struct LeController<'r, const OUTPUT: usize> {
    bootstrap: LeControllerBootstrap,
    random: Option<&'r dyn LeRandomSource>,
    output: Output<OUTPUT>,
    pending: Option<Pending>,
    advertising: LeLegacyAdvertisingConfiguration,
    scanning: LeLegacyScanningConfiguration,
    dtm: DtmRole,
    advertiser: Advertiser,
    scanner: Scanner,
    peripheral: Peripheral,
    accept_list: AcceptList,
    /// Controller-to-Host ACL waiting for Host credits, with the offset
    /// already delivered of the first packet.
    held: [Option<LeControllerAclPacket>; HELD_ACL],
    held_offset: usize,
    /// Host ACL credits when Controller-to-Host flow control is on.
    host_credits: Option<u32>,
    in_flight: Option<Owner>,
    next_event: u32,
    prng: u64,
    fault: Option<RadioFault>,
    payload: [u8; DTM_MAX_PAYLOAD],
}

impl<'r, const OUTPUT: usize> LeController<'r, OUTPUT> {
    /// A Controller awaiting its first HCI Reset. `random` answers LE Rand
    /// and supplies the encryption procedure's random values; without it,
    /// connectable advertising is refused.
    pub fn new(config: LeControllerConfig, random: Option<&'r dyn LeRandomSource>) -> Self {
        const {
            assert!(
                OUTPUT > EVENT_OUTPUT,
                "the output holds one connection event and a command response"
            )
        };
        let version = config.version;
        let config = config.bootstrap;
        // The delay needs no entropy, only a sequence that differs between
        // devices; the public address gives one without spending the Host's
        // random source.
        let mut address = [0; 8];
        address[..6].copy_from_slice(&config.public_address().canonical_bytes());
        let seed = u64::from_le_bytes(address) ^ 0x9e37_79b9_7f4a_7c15;
        Self {
            bootstrap: LeControllerBootstrap::new(config),
            random,
            output: Output::new(),
            pending: None,
            advertising: LeLegacyAdvertisingConfiguration::new(),
            scanning: LeLegacyScanningConfiguration::new(),
            dtm: DtmRole::new(),
            advertiser: Advertiser::new(),
            scanner: Scanner::new(),
            peripheral: Peripheral::new(version),
            accept_list: AcceptList::new(),
            held: [None; HELD_ACL],
            held_offset: 0,
            host_credits: None,
            in_flight: None,
            next_event: 1,
            prng: seed | 1,
            fault: None,
            payload: [0; DTM_MAX_PAYLOAD],
        }
    }

    /// Reset-scoped HCI state.
    pub const fn bootstrap(&self) -> &LeControllerBootstrap {
        &self.bootstrap
    }

    /// Whether [`Self::command`] takes a command now.
    pub const fn is_command_ready(&self) -> bool {
        self.pending.is_none() && self.output.has_response_slot()
    }

    /// Whether a command waits for radio work before it completes.
    pub const fn is_command_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// LE Advertising Reports dropped for lack of output space.
    pub const fn dropped_events(&self) -> u32 {
        self.output.dropped()
    }

    /// The first fault the backend reported. The Controller keeps serving
    /// commands; recovery belongs to its owner.
    pub const fn fault(&self) -> Option<RadioFault> {
        self.fault
    }

    /// The oldest packet waiting for the Host.
    pub fn front(&self) -> Option<&HciPacket> {
        self.output.front()
    }

    /// Remove the oldest packet once the Host transport took it.
    pub fn pop(&mut self) {
        self.output.pop();
    }

    /// Take one command. Its response is queued now or once the radio work
    /// it started has finished.
    pub fn command(&mut self, packet: HciCommandPacket<'_>) -> Result<(), ControllerBusy> {
        if !self.is_command_ready() {
            return Err(ControllerBusy);
        }
        // Host Number Of Completed Packets has no response unless malformed.
        match LeHostCompletedPacketsCommand::decode(packet) {
            Ok(command) => {
                if let (Some(credits), Some(returned)) = (
                    &mut self.host_credits,
                    command.completed_for(self.peripheral.handle()),
                ) {
                    *credits = credits.saturating_add(returned);
                }
                self.deliver_held();
                return Ok(());
            }
            Err(LeHostCompletedPacketsDecodeError::Malformed) => {
                self.respond(LeHostCompletedPacketsErrorEvent::invalid_parameters().as_bytes());
                return Ok(());
            }
            Err(_) => {}
        }
        #[cfg(feature = "diagnostic-mic-fault")]
        if packet.opcode() == crate::diagnostic::ARM_MIC_CORRUPTION {
            let (status, handle) = match *packet.parameters() {
                [low, high] => (
                    self.peripheral
                        .arm_mic_corruption(bt_hci::param::ConnHandle::new(u16::from_le_bytes([
                            low, high,
                        ]))),
                    [low, high],
                ),
                _ => (HciError::INVALID_HCI_PARAMETERS.to_status(), [0, 0]),
            };
            self.respond(&crate::diagnostic::command_complete(status, handle));
            return Ok(());
        }
        match classify_le_controller_command(packet) {
            LeControllerCommandClassification::Bootstrap(command) => {
                self.bootstrap_command(command)
            }
            LeControllerCommandClassification::Random(_) => {
                let response = match self.random {
                    Some(source) => match source.random_bytes() {
                        Ok(bytes) => LeRandCommandCompleteEvent::success(bytes),
                        Err(_) => LeRandCommandCompleteEvent::error(HciError::HARDWARE_FAILURE),
                    },
                    None => LeRandCommandCompleteEvent::error(HciError::UNKNOWN_CMD),
                };
                self.respond(response.as_bytes());
            }
            // Radio roles need a Reset first.
            LeControllerCommandClassification::Dtm(command) if !self.is_configured() => {
                self.respond(
                    LeDtmCommandCompleteEvent::without_return_parameters(
                        command.kind().opcode(),
                        HciError::CMD_DISALLOWED.to_status(),
                    )
                    .as_bytes(),
                );
            }
            classification
            @ (LeControllerCommandClassification::LegacyAdvertisingConfiguration(
                _,
            )
            | LeControllerCommandClassification::LegacyAdvertisingEnable(_))
                if !self.is_configured() =>
            {
                self.respond_advertising(
                    classification.opcode(),
                    HciError::CMD_DISALLOWED.to_status(),
                );
            }
            classification @ (LeControllerCommandClassification::LegacyScanningConfiguration(_)
            | LeControllerCommandClassification::LegacyScanningEnable(_))
                if !self.is_configured() =>
            {
                self.respond(
                    LeLegacyScanningCommandCompleteEvent::new(
                        classification.opcode(),
                        HciError::CMD_DISALLOWED.to_status(),
                    )
                    .as_bytes(),
                );
            }
            LeControllerCommandClassification::AcceptList(command) => {
                self.accept_list_command(command);
            }
            LeControllerCommandClassification::MalformedAcceptList(response) => {
                self.respond(response.as_bytes());
            }
            LeControllerCommandClassification::DataLength(command) => {
                self.data_length_command(command);
            }
            LeControllerCommandClassification::MalformedDataLength(response) => {
                self.respond(response.as_bytes());
            }
            LeControllerCommandClassification::MalformedRandom(response) => {
                self.respond(response.as_bytes());
            }
            LeControllerCommandClassification::MalformedBootstrap(response) => {
                self.respond(response.as_bytes());
            }
            LeControllerCommandClassification::MalformedDtm(response) => {
                self.respond(response.as_bytes());
            }
            LeControllerCommandClassification::MalformedLegacyAdvertising(response) => {
                self.respond(response.as_bytes());
            }
            LeControllerCommandClassification::MalformedLegacyScanning(response) => {
                self.respond(response.as_bytes());
            }
            LeControllerCommandClassification::MalformedDisconnect(response) => {
                self.respond(response.as_bytes());
            }
            LeControllerCommandClassification::MalformedReadRemoteFeatures(response) => {
                self.respond(response.as_bytes());
            }
            LeControllerCommandClassification::MalformedReadRemoteVersionInformation(response) => {
                self.respond(response.as_bytes());
            }
            LeControllerCommandClassification::MalformedLongTermKeyReply(response) => {
                self.respond(response.as_bytes());
            }
            LeControllerCommandClassification::Unsupported(response) => {
                self.respond(response.as_bytes());
            }
            LeControllerCommandClassification::Disconnect(command) => {
                let live = self.peripheral.handle() == Some(command.handle());
                if live && self.peripheral.disconnect(command.reason()) {
                    self.respond(command.into_accepted_status().as_bytes());
                } else {
                    self.respond(command.into_unknown_connection_status().as_bytes());
                }
            }
            LeControllerCommandClassification::ReadRemoteFeatures(command) => {
                let admission = if self.peripheral.handle() == Some(command.handle()) {
                    self.peripheral.read_remote_features(&command)
                } else {
                    Admission::UnknownConnection
                };
                let response = match admission {
                    Admission::Accepted => command.into_accepted_status(),
                    Admission::Disallowed => command.into_command_disallowed_status(),
                    Admission::UnknownConnection => command.into_unknown_connection_status(),
                };
                self.respond(response.as_bytes());
            }
            LeControllerCommandClassification::ReadRemoteVersionInformation(command) => {
                let admission = if self.peripheral.handle() == Some(command.handle()) {
                    self.peripheral.read_remote_version(&command)
                } else {
                    Admission::UnknownConnection
                };
                let response = match admission {
                    Admission::Accepted => command.into_accepted_status(),
                    Admission::Disallowed => command.into_command_disallowed_status(),
                    Admission::UnknownConnection => command.into_unknown_connection_status(),
                };
                self.respond(response.as_bytes());
            }
            LeControllerCommandClassification::LongTermKeyReply(command) => {
                let handle = command.handle();
                let admission = if self.peripheral.handle() == Some(handle) {
                    self.peripheral
                        .long_term_key(Some(command.into_long_term_key()))
                } else {
                    Admission::UnknownConnection
                };
                let status = match admission {
                    Admission::Accepted => Status::SUCCESS,
                    Admission::Disallowed => HciError::CMD_DISALLOWED.to_status(),
                    Admission::UnknownConnection => HciError::UNKNOWN_CONN_IDENTIFIER.to_status(),
                };
                self.respond(
                    LeLongTermKeyCommandCompleteEvent::new(
                        LeLongTermKeyRequestReplyCommand::OPCODE,
                        status,
                        handle,
                    )
                    .as_bytes(),
                );
            }
            LeControllerCommandClassification::LongTermKeyNegativeReply(command) => {
                let admission = if self.peripheral.handle() == Some(command.handle()) {
                    self.peripheral.long_term_key(None)
                } else {
                    Admission::UnknownConnection
                };
                let response = match admission {
                    Admission::Accepted => command.into_accepted_complete(),
                    Admission::Disallowed => command.into_command_disallowed_complete(),
                    Admission::UnknownConnection => command.into_unknown_connection_complete(),
                };
                self.respond(response.as_bytes());
            }
            LeControllerCommandClassification::Dtm(command) => self.dtm_command(command),
            LeControllerCommandClassification::LegacyAdvertisingConfiguration(command) => {
                self.advertising_configuration(command);
            }
            LeControllerCommandClassification::LegacyAdvertisingEnable(command) => {
                self.advertising_enable(command.enable());
            }
            LeControllerCommandClassification::LegacyScanningConfiguration(command) => {
                let opcode = LeLegacyScanningCommandKind::SetParameters.opcode();
                let status = if self.scanner.is_active() {
                    HciError::CMD_DISALLOWED.to_status()
                } else {
                    self.scanning.configure(command);
                    Status::SUCCESS
                };
                self.respond(LeLegacyScanningCommandCompleteEvent::new(opcode, status).as_bytes());
            }
            LeControllerCommandClassification::LegacyScanningEnable(command) => {
                let opcode = LeLegacyScanningCommandKind::SetEnable.opcode();
                match (command.enable(), self.scanner.is_active()) {
                    // Enabling again keeps the running scanner; disabling an
                    // idle one has no effect.
                    (true, true) | (false, false) => self.respond(
                        LeLegacyScanningCommandCompleteEvent::new(opcode, Status::SUCCESS)
                            .as_bytes(),
                    ),
                    (true, false) if self.dtm.is_active() => self.respond(
                        LeLegacyScanningCommandCompleteEvent::new(
                            opcode,
                            HciError::CMD_DISALLOWED.to_status(),
                        )
                        .as_bytes(),
                    ),
                    (true, false) => match self.scanning.enable_request(command) {
                        Ok(request) => {
                            self.scanner.enable(request);
                            self.pending = Some(Pending::Scanning(opcode));
                        }
                        Err(_) => self.respond(
                            LeLegacyScanningCommandCompleteEvent::new(
                                opcode,
                                HciError::CMD_DISALLOWED.to_status(),
                            )
                            .as_bytes(),
                        ),
                    },
                    (false, true) => {
                        self.scanner.disable();
                        self.pending = Some(Pending::Scanning(opcode));
                    }
                }
            }
        }
        self.deliver_peripheral();
        self.settle();
        Ok(())
    }

    /// Queue the connection's Host events that the masks enable.
    fn deliver_peripheral(&mut self) {
        if let Some(packet) = self.peripheral.take_received() {
            let slot = self
                .held
                .iter_mut()
                .find(|slot| slot.is_none())
                .expect("a connection event is planned only with room for its data");
            *slot = Some(packet);
            self.deliver_held();
        }
        while let Some(event) = self.peripheral.take_event() {
            let mask = self.bootstrap.event_mask();
            let le = self.bootstrap.le_event_mask();
            let meta = mask.is_le_meta_enabled();
            match event {
                PeripheralEvent::Connected {
                    peer_kind,
                    peer,
                    interval_units,
                    latency,
                    timeout_units,
                    accuracy,
                } => {
                    self.host_credits = self.host_acl_credits();
                    if meta
                        && le.is_le_conn_complete_enabled()
                        && let Ok(event) = LePeripheralConnectionCompleteEvent::new(
                            handle(),
                            peer_kind,
                            peer,
                            Duration::from_u16(interval_units),
                            latency,
                            Duration::from_u16(timeout_units),
                            accuracy,
                        )
                    {
                        self.output.push_event(event.as_bytes());
                    }
                }
                PeripheralEvent::ConnectionFailed(status) => {
                    if meta
                        && le.is_le_conn_complete_enabled()
                        && let Ok(event) = LePeripheralConnectionCompleteEvent::failed(status)
                    {
                        self.output.push_event(event.as_bytes());
                    }
                }
                PeripheralEvent::Disconnected(reason) => {
                    // Undelivered data dies with the connection; the Host
                    // frees its buffers for the handle.
                    self.held = [None; HELD_ACL];
                    self.held_offset = 0;
                    self.host_credits = self.host_acl_credits();
                    if mask.is_disconnection_complete_enabled() {
                        self.output.push_event(
                            LeDisconnectionCompleteEvent::new(handle(), reason).as_bytes(),
                        );
                    }
                }
                PeripheralEvent::ConnectionUpdated {
                    interval_units,
                    latency,
                    timeout_units,
                } => {
                    if meta && le.is_le_conn_update_complete_enabled() {
                        self.output.push_event(
                            LeConnectionUpdateCompleteEvent::new(
                                handle(),
                                Duration::from_u16(interval_units),
                                latency,
                                Duration::from_u16(timeout_units),
                            )
                            .as_bytes(),
                        );
                    }
                }
                PeripheralEvent::RemoteFeatures(status, features) => {
                    if meta && le.is_le_read_remote_features_page_0_complete_enabled() {
                        self.output.push_event(
                            LeReadRemoteFeaturesCompleteEvent::new(status, handle(), features)
                                .as_bytes(),
                        );
                    }
                }
                PeripheralEvent::RemoteVersion(status, version) => {
                    if mask.is_read_remote_version_information_complete_enabled() {
                        self.output.push_event(
                            LeReadRemoteVersionInformationCompleteEvent::new(
                                status,
                                handle(),
                                version.version(),
                                version.company_identifier(),
                                version.subversion(),
                            )
                            .as_bytes(),
                        );
                    }
                }
                PeripheralEvent::LongTermKeyRequest {
                    random,
                    diversifier,
                } => {
                    if meta && le.is_le_long_term_key_request_enabled() {
                        self.output.push_event(
                            LeLongTermKeyRequestEvent::new(handle(), random, diversifier)
                                .as_bytes(),
                        );
                    } else {
                        // A Host that masks the request cannot answer it.
                        self.peripheral.long_term_key(None);
                    }
                }
                PeripheralEvent::EncryptionChanged(status, enabled) => {
                    if mask.is_encryption_change_v1_enabled() {
                        self.output.push_event(
                            LeEncryptionChangeEvent::new(status, handle(), enabled).as_bytes(),
                        );
                    }
                }
                PeripheralEvent::KeyRefreshed(status) => {
                    if mask.is_encryption_key_refresh_complete_enabled() {
                        self.output.push_event(
                            LeEncryptionKeyRefreshCompleteEvent::new(status, handle()).as_bytes(),
                        );
                    }
                }
                PeripheralEvent::DataLengthChanged(lengths) => {
                    if meta && le.is_le_data_length_change_enabled() {
                        let (transmit, receive) = data_length_parameters(lengths);
                        self.output.push_event(
                            LeDataLengthChangeEvent::new(handle(), transmit, receive).as_bytes(),
                        );
                    }
                }
                PeripheralEvent::CompletedPackets(count) => {
                    self.output.push_event(
                        LeNumberOfCompletedPacketsEvent::new(handle(), count).as_bytes(),
                    );
                }
            }
            self.deliver_held();
        }
    }

    /// Host credits of the current epoch, when flow control is on.
    fn host_acl_credits(&self) -> Option<u32> {
        match (
            self.bootstrap.controller_to_host_flow_control(),
            self.bootstrap.host_buffers(),
        ) {
            (ControllerToHostFlowControl::AclOnSyncOff, Some(buffers)) => {
                Some(u32::from(buffers.total_acl_data_packets))
            }
            _ => None,
        }
    }

    /// Move held Controller-to-Host ACL data to the output as Host credits,
    /// Host packet length and output room allow.
    fn deliver_held(&mut self) {
        let maximum = self.bootstrap.host_buffers().map_or(usize::MAX, |buffers| {
            usize::from(buffers.acl_data_packet_length)
        });
        while let Some(packet) = self.held[0] {
            let Some(fragment) = packet.next_host_fragment(self.held_offset, maximum) else {
                self.held.rotate_left(1);
                self.held[HELD_ACL - 1] = None;
                self.held_offset = 0;
                continue;
            };
            if self.output.free() <= 1 || self.host_credits == Some(0) {
                return;
            }
            self.output.push_acl(fragment.as_bytes());
            if let Some(credits) = &mut self.host_credits {
                *credits -= 1;
            }
            self.held_offset += fragment.payload_length();
        }
    }

    fn is_configured(&self) -> bool {
        !self.bootstrap.is_pristine()
    }

    fn data_length_command(&mut self, command: LeDataLengthCommand) {
        let length = |parameters: LeDataLengthParameters| {
            LeDataLength::new(parameters.octets, parameters.time_micros)
                .expect("HCI validated the specified ranges")
        };
        let response = match command {
            LeDataLengthCommand::Set { handle, transmit } => {
                let admission = if self.peripheral.handle() == Some(handle) {
                    self.peripheral.set_data_length(length(transmit))
                } else {
                    Admission::UnknownConnection
                };
                let status = match admission {
                    Admission::Accepted => Status::SUCCESS,
                    Admission::Disallowed => HciError::CMD_DISALLOWED.to_status(),
                    Admission::UnknownConnection => HciError::UNKNOWN_CONN_IDENTIFIER.to_status(),
                };
                LeDataLengthCommandCompleteEvent::set(status, handle)
            }
            LeDataLengthCommand::ReadSuggestedDefault => {
                let suggested = self.peripheral.suggested_data_length();
                LeDataLengthCommandCompleteEvent::suggested_default(LeDataLengthParameters {
                    octets: suggested.octets(),
                    time_micros: suggested.time_micros(),
                })
            }
            LeDataLengthCommand::WriteSuggestedDefault(transmit) => {
                self.peripheral.suggest_data_length(length(transmit));
                LeDataLengthCommandCompleteEvent::suggested_default_written(Status::SUCCESS)
            }
            LeDataLengthCommand::ReadMaximum => {
                let (transmit, receive) = data_length_parameters(LeDataLengths {
                    transmit: LeDataLength::SUPPORTED_MAXIMUM,
                    receive: LeDataLength::SUPPORTED_MAXIMUM,
                });
                LeDataLengthCommandCompleteEvent::maximum(transmit, receive)
            }
        };
        self.respond(response.as_bytes());
    }

    fn bootstrap_command(&mut self, command: OwnedBootstrapCommand) {
        match command {
            OwnedBootstrapCommand::Reset => {
                self.advertiser_stop();
                if self.scanner.is_active() {
                    self.scanner.disable();
                }
                self.dtm.abort();
                self.peripheral.abort();
                self.peripheral.suggest_data_length(LeDataLength::MINIMUM);
                self.accept_list.reset();
                self.pending = Some(Pending::Reset);
            }
            OwnedBootstrapCommand::LeSetRandomAddress(_)
                if self.advertiser.is_active() || self.scanner.is_active() =>
            {
                let response = BootstrapCommandCompleteEvent::error(
                    command.opcode(),
                    HciError::CMD_DISALLOWED,
                );
                self.respond(response.as_bytes());
            }
            command => {
                let response = self.bootstrap.dispatch(command, self.random.is_some());
                self.respond(response.as_bytes());
            }
        }
    }

    fn advertiser_stop(&mut self) {
        if self.advertiser.is_active() {
            self.advertiser.disable();
        }
    }

    fn dtm_command(&mut self, command: LeDtmCommand) {
        if self.advertiser.is_active() || self.scanner.is_active() || self.peripheral.is_active() {
            let response = LeDtmCommandCompleteEvent::without_return_parameters(
                command.kind().opcode(),
                HciError::CMD_DISALLOWED.to_status(),
            );
            self.respond(response.as_bytes());
            return;
        }
        match self.dtm.command(command) {
            Some(response) => self.respond(response.as_bytes()),
            None => self.pending = Some(Pending::TestEnd),
        }
    }

    fn advertising_configuration(&mut self, command: LeLegacyAdvertisingConfigurationCommand) {
        let opcode = command.kind().opcode();
        let running = self.advertiser.is_active();
        match command {
            // Parameters change only while advertising is disabled.
            LeLegacyAdvertisingConfigurationCommand::SetParameters(_) if running => {
                self.respond_advertising(opcode, HciError::CMD_DISALLOWED.to_status());
            }
            LeLegacyAdvertisingConfigurationCommand::SetData(_) if running => {
                let previous = self.advertising;
                self.advertising.configure(command);
                match self.advertising_request() {
                    Ok(request) => match self.advertiser.update(request) {
                        Ok(()) => self.pending = Some(Pending::Advertising(opcode)),
                        Err(error) => {
                            self.advertising = previous;
                            self.respond_advertising(opcode, error.to_status());
                        }
                    },
                    Err(error) => {
                        self.advertising = previous;
                        self.respond_advertising(opcode, error.to_status());
                    }
                }
            }
            command => {
                self.advertising.configure(command);
                self.respond_advertising(opcode, Status::SUCCESS);
            }
        }
    }

    fn advertising_enable(&mut self, enable: bool) {
        let opcode = LeLegacyAdvertisingCommandKind::SetEnable.opcode();
        match (enable, self.advertiser.is_active()) {
            // Enabling again keeps the running set; disabling an idle one
            // has no effect.
            (true, true) | (false, false) => self.respond_advertising(opcode, Status::SUCCESS),
            (true, false) if self.dtm.is_active() => {
                self.respond_advertising(opcode, HciError::CMD_DISALLOWED.to_status());
            }
            (true, false) => {
                if let Ok(
                    LeLegacyAdvertisingEnableRequest::Connectable(_)
                    | LeLegacyAdvertisingEnableRequest::Directed(_),
                ) = self.advertising.enable_request(
                    self.bootstrap.config().public_address(),
                    self.bootstrap.requested_random_address(),
                ) {
                    // One connection at a time, and encryption needs entropy.
                    if self.peripheral.is_active() {
                        self.respond_advertising(opcode, HciError::CMD_DISALLOWED.to_status());
                        return;
                    }
                    if self.random.is_none() {
                        self.respond_advertising(opcode, HciError::UNSUPPORTED.to_status());
                        return;
                    }
                }
                let result = self
                    .advertising_request()
                    .and_then(|request| self.advertiser.enable(request));
                match result {
                    Ok(()) => self.pending = Some(Pending::Advertising(opcode)),
                    Err(error) => self.respond_advertising(opcode, error.to_status()),
                }
            }
            (false, true) => {
                self.advertiser.disable();
                self.pending = Some(Pending::Advertising(opcode));
            }
        }
    }

    /// The configured set.
    fn advertising_request(&self) -> Result<LeLegacyAdvertisingEnableRequest, HciError> {
        self.advertising
            .enable_request(
                self.bootstrap.config().public_address(),
                self.bootstrap.requested_random_address(),
            )
            .map_err(|_| HciError::INVALID_HCI_PARAMETERS)
    }

    fn respond_advertising(&mut self, opcode: Opcode, status: Status) {
        self.respond(LeLegacyAdvertisingCommandCompleteEvent::new(opcode, status).as_bytes());
    }

    fn respond(&mut self, bytes: &[u8]) {
        self.output.push_response(bytes);
    }

    /// LE Clear, Add or Remove on the filter accept list. A scanner that
    /// filters by the list keeps it fixed while it runs.
    fn accept_list_command(&mut self, command: LeAcceptListCommand) {
        let opcode = command.opcode();
        if !self.is_configured() || self.scanner.uses_accept_list() {
            self.respond(
                LeAcceptListCommandCompleteEvent::new(opcode, HciError::CMD_DISALLOWED.to_status())
                    .as_bytes(),
            );
            return;
        }
        let device = |device: oer_bluetooth_hci::LeAcceptListDevice| AcceptListDevice {
            random: device.random,
            address: device.address,
        };
        let change = match command {
            LeAcceptListCommand::Clear => AcceptListChange::Clear,
            LeAcceptListCommand::Add(LeAcceptListEntry::Device(entry)) => {
                AcceptListChange::Add(device(entry))
            }
            LeAcceptListCommand::Remove(LeAcceptListEntry::Device(entry)) => {
                AcceptListChange::Remove(device(entry))
            }
            // Only extended advertising is anonymous, and no scanner here
            // receives it: the vendor Controller, too, accepts the entry
            // without a device-table entry.
            LeAcceptListCommand::Add(LeAcceptListEntry::Anonymous)
            | LeAcceptListCommand::Remove(LeAcceptListEntry::Anonymous) => {
                self.respond(
                    LeAcceptListCommandCompleteEvent::new(opcode, Status::SUCCESS).as_bytes(),
                );
                return;
            }
        };
        self.accept_list.change(change);
        self.pending = Some(Pending::AcceptList(opcode));
    }

    /// No role runs and no request is in flight.
    fn is_quiescent(&self) -> bool {
        !self.dtm.is_active()
            && !self.advertiser.is_active()
            && !self.scanner.is_active()
            && !self.peripheral.is_active()
            && !self.accept_list.is_active()
            && self.in_flight.is_none()
    }

    /// Complete the pending command once its radio work has finished.
    fn settle(&mut self) {
        let advertising = self.advertiser.take_completion();
        let scanning = self.scanner.take_completion();
        let test_end = self.dtm.take_completion();
        let accept_list = self.accept_list.take_completion();
        match self.pending {
            Some(Pending::AcceptList(opcode)) => {
                if let Some(status) = accept_list {
                    self.pending = None;
                    self.respond(LeAcceptListCommandCompleteEvent::new(opcode, status).as_bytes());
                }
            }
            Some(Pending::Advertising(opcode)) => {
                if let Some(status) = advertising {
                    self.pending = None;
                    self.respond_advertising(opcode, status);
                }
            }
            Some(Pending::Scanning(opcode)) => {
                if let Some(status) = scanning {
                    self.pending = None;
                    self.respond(
                        LeLegacyScanningCommandCompleteEvent::new(opcode, status).as_bytes(),
                    );
                }
            }
            Some(Pending::TestEnd) => {
                if let Some(response) = test_end {
                    self.pending = None;
                    self.respond(response.as_bytes());
                }
            }
            Some(Pending::Reset) if self.is_quiescent() => {
                self.pending = None;
                self.advertising = LeLegacyAdvertisingConfiguration::new();
                self.scanning = LeLegacyScanningConfiguration::new();
                self.held = [None; HELD_ACL];
                self.held_offset = 0;
                self.host_credits = None;
                let response = self
                    .bootstrap
                    .dispatch(OwnedBootstrapCommand::Reset, self.random.is_some());
                self.respond(response.as_bytes());
            }
            Some(Pending::Reset) | None => {}
        }
    }

    /// Whether a role has work for the radio. The caller then asks
    /// [`Self::next_request`] with a fresh backend time.
    pub fn wants_radio(&self) -> bool {
        self.in_flight.is_none()
            && (self.accept_list.wants_radio()
                || self.dtm.wants_radio()
                || self.peripheral.wants_radio(self.has_event_room())
                || self.advertiser.wants_radio()
                || self.scanner.wants_radio())
    }

    /// The roles active now: advertising and scanning while enabled or still
    /// stopping, a connection until it closes. Direct Test Mode is not a
    /// role here.
    pub const fn activity(&self) -> RadioActivity {
        RadioActivity {
            advertising: self.advertiser.is_active(),
            scanning: self.scanner.is_active(),
            connected: self.peripheral.is_active(),
        }
    }

    /// Whether the output can take everything one connection event produces.
    fn has_event_room(&self) -> bool {
        self.output.free() > EVENT_OUTPUT && self.held.iter().all(Option::is_none)
    }

    /// Whether [`Self::acl`] takes a Host ACL packet now. Without a
    /// connection every packet is taken and discarded.
    pub fn is_acl_ready(&self) -> bool {
        self.peripheral.acl_ready() || !self.peripheral.is_active()
    }

    /// Take one Host ACL packet for the connection.
    pub fn acl(&mut self, packet: AclPacket<'_>) {
        self.peripheral
            .acl(packet, self.bootstrap.config().le_acl_data_packet_length());
    }

    /// The next radio request. `now` is a fresh backend time and `timing` its
    /// admission rule. The caller submits it and reports the answer to
    /// [`Self::request_done`] before asking again.
    pub fn next_request(
        &mut self,
        now: LeInstant,
        timing: RadioTiming,
    ) -> Result<Option<RadioRequest<'_>>, PlanningError> {
        if self.in_flight.is_some() {
            return Ok(None);
        }
        // Control work must remain available without any future radio anchor.
        if let Some(request) = self.accept_list.request() {
            self.in_flight = Some(Owner::AcceptList);
            return Ok(Some(request));
        }
        if self.dtm.is_active() {
            if !self.dtm.wants_radio() {
                return Ok(None);
            }
            return match self.dtm.next_request(now, timing, &mut self.payload)? {
                DtmRadioWork::Request(request) => {
                    self.in_flight = Some(Owner::Dtm);
                    Ok(Some(request))
                }
                DtmRadioWork::None => Ok(None),
            };
        }
        let room = self.has_event_room();
        if self.peripheral.wants_radio(room) {
            let request = self
                .peripheral
                .next_request(&mut self.next_event, now, timing, room)?;
            if request.is_some() {
                self.in_flight = Some(Owner::Peripheral)
            }
            return Ok(request);
        }
        if self.advertiser.has_control_request() {
            let request = self.advertiser.control_request();
            if request.is_some() {
                self.in_flight = Some(Owner::AdvertiserControl)
            }
            return Ok(request);
        }
        if let Some(request) = self.scanner.control_request() {
            self.in_flight = Some(Owner::ScannerControl);
            return Ok(Some(request));
        }
        // Candidate progress and randomness stay local until the complete
        // plan is valid. Even several bounded conflicts cannot commit partial
        // continuation state before a later arithmetic error in this call.
        let mut advertising = self.advertiser.progress();
        let mut prng = self.prng;
        if self.advertiser.wants_event() {
            let earliest = crate::planning::earliest(now, timing, R::Advertising)?;
            for _ in 0..PLACEMENT_ATTEMPTS {
                let Some(proposal) = self.advertiser.proposal(earliest, timing, advertising)?
                else {
                    break;
                };
                let (next_prng, delay) = advertising_delay(prng);
                let busy = [
                    self.scanner.busy(),
                    self.peripheral.busy(),
                    self.peripheral.planned(timing)?,
                ];
                let anchor = place(proposal, timing, &busy).map_err(|cause| {
                    PlanningError::timing(R::Advertising, O::Event, C::Reservation, cause)
                })?;
                match anchor {
                    Some(anchor) => {
                        let id = EventId::new(self.next_event);
                        let request =
                            self.advertiser
                                .build(id, anchor, timing, delay, advertising)?;
                        self.allocate_event();
                        self.prng = next_prng;
                        self.in_flight = Some(Owner::AdvertiserEvent);
                        return Ok(Some(request));
                    }
                    None => advertising = self.advertiser.skip(earliest, delay, advertising)?,
                }
                prng = next_prng;
            }
        }
        let mut scan_anchor = self.scanner.next_anchor();
        if self.scanner.wants_window() {
            let earliest = crate::planning::earliest(now, timing, R::Scanning)?;
            for _ in 0..PLACEMENT_ATTEMPTS {
                let busy = [
                    self.advertiser.busy(),
                    self.advertiser.planned(timing, advertising)?,
                    self.peripheral.busy(),
                    self.peripheral.planned(timing)?,
                ];
                match self.scanner.place(scan_anchor, earliest, timing, &busy)? {
                    crate::scanning::Placement::Ready {
                        window,
                        reservation,
                        next_anchor,
                    } => {
                        let id = self.allocate_event();
                        let request = self.scanner.build(id, window, reservation, next_anchor);
                        self.advertiser.commit(advertising);
                        self.prng = prng;
                        self.in_flight = Some(Owner::ScannerWindow);
                        return Ok(Some(request));
                    }
                    crate::scanning::Placement::Skipped { next_anchor } => {
                        scan_anchor = Some(next_anchor)
                    }
                }
            }
        }
        self.advertiser.commit(advertising);
        self.scanner.commit_skip(scan_anchor);
        self.prng = prng;
        Ok(None)
    }

    /// The backend's answer to the last request.
    pub fn request_done(&mut self, result: Result<(), RequestError>) {
        let accepted = result.is_ok();
        match self.in_flight.take() {
            Some(Owner::AcceptList) => self.accept_list.request_done(result),
            Some(Owner::Dtm) => self.dtm.request_done(accepted),
            Some(Owner::AdvertiserControl) => {
                self.advertiser.control_done(accepted);
                if self.advertiser.take_timeout() {
                    let (mask, le) = (self.bootstrap.event_mask(), self.bootstrap.le_event_mask());
                    if mask.is_le_meta_enabled()
                        && le.is_le_conn_complete_enabled()
                        && let Ok(event) = LePeripheralConnectionCompleteEvent::failed(
                            HciError::ADV_TIMEOUT.to_status(),
                        )
                    {
                        self.output.push_event(event.as_bytes());
                    }
                }
            }
            Some(Owner::AdvertiserEvent) => self.advertiser.event_done(accepted),
            Some(Owner::ScannerControl) => self.scanner.control_done(accepted),
            Some(Owner::ScannerWindow) => self.scanner.window_done(accepted),
            Some(Owner::Peripheral) => self.peripheral.request_done(accepted),
            None => {}
        }
        self.deliver_peripheral();
        self.settle();
    }

    /// Account one backend observation.
    pub fn outcome(&mut self, outcome: RadioOutcome<'_>) {
        if let RadioOutcome::Fault(fault) = outcome {
            self.fault.get_or_insert(fault);
            return;
        }
        // The service loop ends on the port's terminal outcome; the roles
        // have nothing to account.
        if let RadioOutcome::Poisoned(_) = outcome {
            return;
        }
        self.dtm.outcome(outcome);
        self.peripheral.outcome(outcome, self.random);
        self.advertiser.outcome(outcome);
        if let Some(indication) = self.advertiser.take_connection() {
            self.peripheral.open(indication);
        }
        self.deliver_peripheral();
        if let Some(report) = self.scanner.outcome(outcome) {
            let masks = (self.bootstrap.event_mask(), self.bootstrap.le_event_mask());
            if masks.0.is_le_meta_enabled() && masks.1.is_le_adv_report_enabled() {
                self.output.push_event(report.as_bytes());
            }
        }
        self.settle();
    }

    fn allocate_event(&mut self) -> EventId {
        allocate_event(&mut self.next_event)
    }
}

/// The next role event identity from `next`.
/// The HCI form of both directions' lengths.
fn data_length_parameters(
    lengths: LeDataLengths,
) -> (LeDataLengthParameters, LeDataLengthParameters) {
    let parameters = |length: LeDataLength| LeDataLengthParameters {
        octets: length.octets(),
        time_micros: length.time_micros(),
    };
    (parameters(lengths.transmit), parameters(lengths.receive))
}

pub(crate) fn allocate_event(next: &mut u32) -> EventId {
    let id = EventId::new(*next);
    *next = if *next == LAST_ROLE_EVENT_ID {
        1
    } else {
        *next + 1
    };
    id
}

/// Preview xorshift64* without committing it before a plan is validated.
fn advertising_delay(mut prng: u64) -> (u64, RadioDuration) {
    prng ^= prng >> 12;
    prng ^= prng << 25;
    prng ^= prng >> 27;
    let value = prng.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 32;
    let span = ADVERTISING_DELAY_MAX.as_micros() + 1;
    (prng, RadioDuration::from_micros(value % span))
}
