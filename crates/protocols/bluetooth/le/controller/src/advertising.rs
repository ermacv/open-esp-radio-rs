//! Legacy undirected advertising, non-connectable or connectable.
//!
//! Enable configures the backend's advertising set with the `ADV_NONCONN_IND`
//! PDU, or with `ADV_IND` and its `SCAN_RSP`, and then schedules one event per
//! advertising interval plus a pseudo-random advertising delay of 0 to 10 ms
//! (Core Vol 6 Part B 4.4.2.2.1). Each event transmits on the selected
//! primary channels in order. A non-connectable channel reserves the radio's
//! preparation lead plus the PDU's air time, following the vendor item
//! geometry; a connectable channel also covers the vendor's four-microsecond
//! response-capable tail and the longest exchange that follows: a scan
//! request answered by the scan response, or a connection indication. An
//! event the arbiter cannot place within the delay range is skipped. Disable
//! cancels the event in progress, waits for it to end and removes the set
//! before it completes.
//!
//! A valid connection indication addressed to the set ends advertising: the
//! remaining channels are cancelled, the set is removed and the indication
//! passes to the peripheral connection.

use bt_hci::param::{Error as HciError, Status};
use oer_bluetooth_hci::{LeLegacyAdvertisingAddress, LeLegacyAdvertisingEnableRequest};
use oer_bluetooth_ll::{
    LeDeviceAddress, LeDeviceAddressKind,
    advertising::{LegacyAdvertisingData, LegacyNonconnectableAdvertisement},
    connectable_advertising::{
        LeChannelSelectionAlgorithmTwoSupport, LegacyConnectableAdvertisement,
        LegacyScanResponseData,
    },
    connection::{LEGACY_CONNECT_IND_LE_1M_AIRTIME_MICROS, LeLegacyConnectionRequest},
};
use oer_bluetooth_radio::{
    AdvertisingChannels, AdvertisingConfiguration, AdvertisingEvent, AdvertisingPdu,
    AdvertisingSetId, EventId, RadioDuration, RadioInstant, RadioOutcome, RadioRequest,
    RadioTiming, RadioWindow, TxPower,
};

use crate::arbiter::{Proposal, reservation};

/// Longest legacy advertising PDU: header, AdvA and 31 data octets.
pub(crate) const ADVERTISING_PDU_CAPACITY: usize = 2 + 6 + 31;
/// Upper bound of the advertising delay.
pub(crate) const ADVERTISING_DELAY_MAX: RadioDuration = RadioDuration::from_micros(10_000);
const SET: AdvertisingSetId = AdvertisingSetId::new(0);
/// Inter-frame space.
const T_IFS_MICROS: u32 = 150;
/// LE 1M air time of `SCAN_REQ`.
const SCAN_REQ_AIR_MICROS: u32 = (1 + 4 + 2 + 12 + 3) * 8;
/// The vendor's response-capable scheduler tail after `ADV_IND`.
const RESPONSE_CAPABLE_TAIL_MICROS: u32 = 4;

/// A connection indication accepted by a connectable set.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ConnectionIndication {
    pub(crate) request: LeLegacyConnectionRequest,
    /// On-air start of the indication.
    pub(crate) at: RadioInstant,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Idle,
    /// Enable waits for the backend to take the set.
    Configuring {
        sent: bool,
    },
    Running,
    /// Disable cancels the event in progress.
    Cancelling {
        sent: bool,
    },
    /// Disable removes the set.
    Removing {
        sent: bool,
    },
}

#[derive(Clone, Copy, Debug)]
struct Outstanding {
    id: EventId,
    reservation: RadioWindow,
}

#[derive(Debug)]
pub(crate) struct Advertiser {
    phase: Phase,
    pdu: [u8; ADVERTISING_PDU_CAPACITY],
    pdu_len: usize,
    /// The scan response of a connectable set.
    scan_response: [u8; ADVERTISING_PDU_CAPACITY],
    scan_response_len: Option<usize>,
    advertiser: LeDeviceAddress,
    channels: AdvertisingChannels,
    interval: RadioDuration,
    next_anchor: Option<RadioInstant>,
    outstanding: Option<Outstanding>,
    /// The removal in progress is followed by a new configuration.
    restart: bool,
    completion: Option<Status>,
    connection: Option<ConnectionIndication>,
    /// Advertising stops because a connection was created.
    connected: bool,
}

impl Advertiser {
    pub(crate) const fn new() -> Self {
        Self {
            phase: Phase::Idle,
            pdu: [0; ADVERTISING_PDU_CAPACITY],
            pdu_len: 0,
            scan_response: [0; ADVERTISING_PDU_CAPACITY],
            scan_response_len: None,
            advertiser: LeDeviceAddress::from_wire_bytes([0; 6], LeDeviceAddressKind::Public),
            channels: AdvertisingChannels::ALL,
            interval: RadioDuration::from_micros(0),
            next_anchor: None,
            outstanding: None,
            restart: false,
            completion: None,
            connection: None,
            connected: false,
        }
    }

    /// Whether advertising is enabled or still stopping.
    pub(crate) const fn is_active(&self) -> bool {
        !matches!(self.phase, Phase::Idle)
    }

    /// Start a set; the completion follows once the backend took it.
    pub(crate) fn enable(
        &mut self,
        request: LeLegacyAdvertisingEnableRequest,
    ) -> Result<(), HciError> {
        self.load(request)?;
        self.next_anchor = None;
        self.outstanding = None;
        self.phase = Phase::Configuring { sent: false };
        Ok(())
    }

    /// Replace the running set's PDU. The set is removed and configured
    /// again; the completion follows once the backend took the new one.
    pub(crate) fn update(
        &mut self,
        request: LeLegacyAdvertisingEnableRequest,
    ) -> Result<(), HciError> {
        self.load(request)?;
        self.restart = true;
        self.disable();
        Ok(())
    }

    fn load(&mut self, request: LeLegacyAdvertisingEnableRequest) -> Result<(), HciError> {
        let (parameters, advertiser, data) = match request {
            LeLegacyAdvertisingEnableRequest::Nonconnectable(request) => {
                (request.parameters(), request.advertiser(), request.data())
            }
            LeLegacyAdvertisingEnableRequest::Connectable(request) => {
                (request.parameters(), request.advertiser(), request.data())
            }
        };
        let kind = match advertiser {
            LeLegacyAdvertisingAddress::Public(_) => LeDeviceAddressKind::Public,
            LeLegacyAdvertisingAddress::Random(_) => LeDeviceAddressKind::Random,
        };
        let bytes: [u8; 6] = advertiser
            .wire_address()
            .raw()
            .try_into()
            .expect("a device address has six octets");
        let advertiser = LeDeviceAddress::from_wire_bytes(bytes, kind);
        let data = LegacyAdvertisingData::new(data.as_bytes())
            .map_err(|_| HciError::INVALID_HCI_PARAMETERS)?;
        let (pdu_len, scan_response_len) = match request {
            LeLegacyAdvertisingEnableRequest::Nonconnectable(_) => (
                LegacyNonconnectableAdvertisement::new(advertiser, data).encode(&mut self.pdu),
                None,
            ),
            LeLegacyAdvertisingEnableRequest::Connectable(request) => {
                let response = request.scan_response_data();
                let response = LegacyScanResponseData::new(response.as_bytes())
                    .map_err(|_| HciError::INVALID_HCI_PARAMETERS)?;
                let scan_response = response
                    .encode(advertiser, &mut self.scan_response)
                    .map_err(|_| HciError::INVALID_HCI_PARAMETERS)?;
                (
                    LegacyConnectableAdvertisement::new(
                        advertiser,
                        data,
                        LeChannelSelectionAlgorithmTwoSupport::Supported,
                    )
                    .encode(&mut self.pdu),
                    Some(scan_response),
                )
            }
        };
        self.pdu_len = pdu_len.map_err(|_| HciError::INVALID_HCI_PARAMETERS)?;
        self.scan_response_len = scan_response_len;
        self.advertiser = advertiser;
        let channels = parameters.channels();
        self.channels = AdvertisingChannels::new(
            channels.channel_37(),
            channels.channel_38(),
            channels.channel_39(),
        )
        .ok_or(HciError::INVALID_HCI_PARAMETERS)?;
        self.interval = RadioDuration::from_micros(
            u32::from(parameters.interval().minimum_units_625_us()) * 625,
        );
        Ok(())
    }

    /// Stop the set; the completion follows once it is removed.
    pub(crate) fn disable(&mut self) {
        self.phase = match self.outstanding {
            Some(_) => Phase::Cancelling { sent: false },
            None => Phase::Removing { sent: false },
        };
    }

    /// Whether the role has a request for the radio.
    pub(crate) fn wants_radio(&self) -> bool {
        self.has_control_request() || (self.phase == Phase::Running && self.outstanding.is_none())
    }

    pub(crate) fn has_control_request(&self) -> bool {
        matches!(
            self.phase,
            Phase::Configuring { sent: false }
                | Phase::Cancelling { sent: false }
                | Phase::Removing { sent: false }
        )
    }

    /// A configuration request the role needs now.
    pub(crate) fn control_request(&mut self) -> Option<RadioRequest<'_>> {
        match &mut self.phase {
            Phase::Configuring { sent: sent @ false } => {
                *sent = true;
                Some(RadioRequest::ConfigureAdvertising(
                    AdvertisingConfiguration {
                        set: SET,
                        pdu: AdvertisingPdu::new(&self.pdu[..self.pdu_len])
                            .expect("the encoded PDU fits the legacy bounds"),
                        scan_response: self.scan_response_len.map(|length| {
                            AdvertisingPdu::new(&self.scan_response[..length])
                                .expect("the encoded response fits the legacy bounds")
                        }),
                        tx_power: TxPower::from_dbm(0),
                    },
                ))
            }
            Phase::Cancelling { sent: sent @ false } => {
                *sent = true;
                let id = self.outstanding.expect("an event is cancelled").id;
                Some(RadioRequest::Cancel(id))
            }
            Phase::Removing { sent: sent @ false } => {
                *sent = true;
                Some(RadioRequest::RemoveAdvertising(SET))
            }
            _ => None,
        }
    }

    /// The backend's answer to this role's control request.
    pub(crate) fn control_done(&mut self, accepted: bool) {
        match self.phase {
            Phase::Configuring { .. } if accepted => {
                self.phase = Phase::Running;
                self.completion = Some(Status::SUCCESS);
            }
            Phase::Configuring { .. } => {
                self.phase = Phase::Idle;
                self.completion = Some(HciError::HARDWARE_FAILURE.to_status());
            }
            // A cancelled event ends either way; its outcome follows.
            Phase::Cancelling { .. } => {}
            Phase::Removing { .. } if accepted && self.restart => {
                self.restart = false;
                self.phase = Phase::Configuring { sent: false };
            }
            // Advertising that ended in a connection completes no command.
            Phase::Removing { .. } if self.connected => {
                self.connected = false;
                self.phase = Phase::Idle;
            }
            Phase::Removing { .. } => {
                self.restart = false;
                self.phase = Phase::Idle;
                self.completion = Some(if accepted {
                    Status::SUCCESS
                } else {
                    HciError::HARDWARE_FAILURE.to_status()
                });
            }
            Phase::Idle | Phase::Running => {}
        }
    }

    /// Air time of one channel of the event and its responses, plus the
    /// lead.
    fn channel_spacing(&self, timing: RadioTiming) -> RadioDuration {
        let mut air = air_micros(self.pdu_len);
        if let Some(response) = self.scan_response_len {
            let scan = SCAN_REQ_AIR_MICROS + T_IFS_MICROS + air_micros(response);
            air += RESPONSE_CAPABLE_TAIL_MICROS
                + T_IFS_MICROS
                + scan.max(LEGACY_CONNECT_IND_LE_1M_AIRTIME_MICROS);
        }
        RadioDuration::from_micros(timing.preparation_lead.as_micros() + air)
    }

    fn event_duration(&self, timing: RadioTiming) -> RadioDuration {
        let spacing = self.channel_spacing(timing).as_micros();
        let channels = self.channels.iter().count() as u32;
        // The last channel needs no lead after it.
        RadioDuration::from_micros(spacing * channels - timing.preparation_lead.as_micros())
    }

    /// The next event, when the role needs one.
    pub(crate) fn proposal(&self, earliest: RadioInstant, timing: RadioTiming) -> Option<Proposal> {
        if self.phase != Phase::Running || self.outstanding.is_some() {
            return None;
        }
        let earliest = self
            .next_anchor
            .map_or(earliest, |anchor| anchor.max(earliest));
        Some(Proposal {
            earliest,
            latest: earliest.checked_add(ADVERTISING_DELAY_MAX)?,
            duration: self.event_duration(timing),
        })
    }

    /// The reservation of the next event at its nominal anchor, which other
    /// roles leave free. It is known once the previous event is placed.
    pub(crate) fn planned(&self, timing: RadioTiming) -> Option<RadioWindow> {
        if self.phase != Phase::Running {
            return None;
        }
        reservation(
            self.next_anchor?,
            self.event_duration(timing),
            timing.preparation_lead,
        )
    }

    /// The reservation of the event in progress.
    pub(crate) fn busy(&self) -> Option<RadioWindow> {
        self.outstanding.map(|event| event.reservation)
    }

    /// Build the event placed at `anchor`; `delay` is the advertising delay
    /// that follows it.
    pub(crate) fn build(
        &mut self,
        id: EventId,
        anchor: RadioInstant,
        timing: RadioTiming,
        delay: RadioDuration,
    ) -> RadioRequest<'static> {
        self.outstanding = Some(Outstanding {
            id,
            reservation: reservation(anchor, self.event_duration(timing), timing.preparation_lead)
                .expect("a placed event has a reservation"),
        });
        self.advance(anchor, delay);
        RadioRequest::Advertise(AdvertisingEvent {
            id,
            set: SET,
            anchor,
            channels: self.channels,
            channel_spacing: self.channel_spacing(timing),
        })
    }

    /// The backend's answer to the event request. A refused event counts as
    /// skipped; the next one keeps its planned anchor.
    pub(crate) fn event_done(&mut self, accepted: bool) {
        if !accepted {
            self.outstanding = None;
        }
    }

    /// Skip the event the arbiter could not place.
    pub(crate) fn skip(&mut self, earliest: RadioInstant, delay: RadioDuration) {
        let anchor = self.next_anchor.unwrap_or(earliest);
        self.advance(anchor, delay);
    }

    fn advance(&mut self, anchor: RadioInstant, delay: RadioDuration) {
        let step = self.interval.as_micros() + delay.as_micros();
        self.next_anchor = anchor.checked_add(RadioDuration::from_micros(step));
    }

    /// Account one outcome.
    pub(crate) fn outcome(&mut self, outcome: RadioOutcome<'_>) {
        match outcome {
            RadioOutcome::Received { id, pdu } if self.owns(id) => {
                let (Some(_), Some(at), Phase::Running) =
                    (self.scan_response_len, pdu.captured_at, self.phase)
                else {
                    return;
                };
                let Ok(request) = LeLegacyConnectionRequest::decode(pdu.pdu) else {
                    return;
                };
                if request.is_addressed_to(self.advertiser) {
                    self.connection = Some(ConnectionIndication { request, at });
                    self.connected = true;
                    self.phase = Phase::Cancelling { sent: false };
                }
            }
            RadioOutcome::EventEnded { id, .. } if self.owns(id) => {
                self.outstanding = None;
                if let Phase::Cancelling { .. } = self.phase {
                    self.phase = Phase::Removing { sent: false };
                }
            }
            _ => {}
        }
    }

    /// The connection indication that ended advertising, once.
    pub(crate) fn take_connection(&mut self) -> Option<ConnectionIndication> {
        self.connection.take()
    }

    pub(crate) fn owns(&self, id: EventId) -> bool {
        self.outstanding.is_some_and(|event| event.id == id)
    }

    pub(crate) fn take_completion(&mut self) -> Option<Status> {
        self.completion.take()
    }
}

/// LE 1M air time of a PDU of `length` octets, header included.
const fn air_micros(length: usize) -> u32 {
    (length as u32 - 2) * 8 + 80
}
