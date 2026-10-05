//! The access point over the lower-MAC port.
//!
//! A [`PortAccessPoint`] owns its interface's
//! [`PortClient`], its
//! beacon and the [`AccessPointService`] of its BSS (peers, security and
//! the management sequence beacons and responses share).
//! [`PortAccessPoint::start`] tunes the port to the BSS's channel,
//! configures the access-point interface to receive its BSS and the Probe
//! Requests it answers, and restarts the interface's TSF.
//! [`PortAccessPoint::run_until`] then:
//!
//! - publishes a beacon at every TBTT of its own schedule (`ApBeacon`: an
//!   absolute cursor that a late publication does not move), its ERP and HT
//!   protection following the associated peers;
//! - answers each Probe Request for its SSID, or for any SSID, with the
//!   current advertisement;
//! - admits stations: Open System authentication and association, a
//!   repeated request answered again without resetting the peer;
//! - removes a peer that disassociates or deauthenticates, and closes one
//!   whose authentication or association went inactive with a
//!   Disassociation (when it was associated) and a Deauthentication;
//! - in a WPA2-Personal BSS, runs the four-way handshake as the
//!   authenticator: Message 1 after a successful association, Message 3 on
//!   a verified Message 2, their retransmissions, and on a verified
//!   Message 4 the peer's pairwise key installed in the port before the
//!   peer is authorized. [`PortAccessPoint::start`] installs the group key.
//!   A peer's pairwise key is removed whenever the peer is.
//!
//! - in a WPA3-Personal BSS, authenticates stations by SAE through the
//!   environment's [`PortApSae`] executor, which the composition places
//!   (another task, another core, or inline with [`InlineSae`]): the access
//!   point hands each SAE frame on and goes on serving its BSS until the
//!   output is ready, as the vendor runs its responder in a task of its own.
//!   The IGTK reaches stations in Message 3; the port holds only the group
//!   and pairwise keys.
//!
//! The composition enables the port and polls the router beside the access
//! point.

use core::{
    future::poll_fn,
    pin::pin,
    task::{Context, Poll},
};

use oer_ieee80211_ap::{
    AP_MAX_CLIENTS, AP_TX_BLOCK_ACK_TID, AccessPointService, ApAssociationCapabilities,
    ApMlmeAction, ApPeerClose, ApPeerCloseKind, ApPeerPhase, ApServiceError, ApWpa2Error,
    ApWpa2Progress, ApWpa2RetryProgress,
    beacon::ApBeacon,
    sae::{ApSaeFrame, ApSaeOutput, ApSaeRandom, ApSaeResponder, ApSaeResult},
};
use oer_ieee80211_datapath::{DestinationTxQueues, SoftwareTxFrame};
use oer_ieee80211_lower_mac::{
    Channel, Cipher, CoexPriority, Ieee80211LowerMacPort, KeyHandle, KeyInstall, KeyScope,
    KeySelector, LowerMacBeaconTiming, LowerMacSetting, PhyRate, ReceiveFilter, RxCryptoStatus,
    RxEvidence, RxMeta, SettingError, VifTsf,
};
use oer_ieee80211_mac::{
    ap::{
        ApAssociationResponseError, ApDataFrame, ApDataFrameError, ApManagementRequest,
        ApPeerDisconnectKind, ApProtectedDataFrame, ApUnprotectedDataFrame,
        parse_ap_management_request, probe, probe::ResponseError, profile::Advertisement,
        write_ap_peer_disconnect, write_ht_association_response_frame_for_security,
        write_open_authentication_response, write_sae_authentication,
    },
    beacon::{AP_BEACON_CAPACITY, ApBeaconBuildError, TimBitmapError},
    ccmp::{
        CCMP_HEADER_LEN, CcmpHeader, CcmpKeyId, CcmpPacketNumberStep, CcmpReplayLane,
        CcmpRxReplayState, CcmpTxPacketNumber,
    },
    channel::WifiChannel,
    data::{
        DataDecapError, DataInterfaceRole, IEEE80211_LEGACY_DATA_HEADER_LEN,
        IEEE80211_QOS_DATA_HEADER_LEN, RxDuplicateFilter, decapsulate_data_frames,
        plan_data_decapsulation,
    },
    ht::HtPeerCapabilities,
    protection::ApBssProtection,
    qos::WmmAccessCategory,
    security::{ApSecurityPolicy, LinkProtection},
    sequence::SequenceNumber,
    ssid::WifiSsid,
    station::association::PhyMode,
    tsf::TsfInstant,
};
use oer_ieee80211_rsn::{
    OwnedEapolFrame, Pmk, RsnInterface, frames::RsnTxFrame, runner::RSN_HANDSHAKE_EAPOL_CAPACITY,
};
use oer_ieee80211_upper_mac::{
    TxBody, TxReceiver, TxReport, TxRequest,
    aggregate::{AmpduLimits, AmpduRun},
    rate_control::{RateControl, RatePeer, link_metric},
};
use oer_ieee80211_upper_mac_service::{
    EventRouter, TxMpdu,
    aggregate::{AmpduSubframes, PORT_AMPDU_SUBFRAMES, PortAggregation},
    client::{
        PortClient, PortClientEnv, PortClientError, PortError, PortFrame, PortInput, PortMsdu,
        PortRxBuffer,
    },
    frame::{NetworkBody, PORT_MPDU_HEADER_CAPACITY, split_ethernet},
};
use oer_time::{Clock, Duration, Instant, Timer};

use oer_ieee80211_ap::{
    ApBufferedUnicastRelease, ApDownlinkDisposition, ApPeerPowerState, ApPowerSaveAction,
};
use oer_ieee80211_mac::ap::{ApPowerSaveObservation, observe_ap_power_save_for_access_point};
use oer_ieee80211_mac::beacon::dtim;

use oer_ieee80211_lower_mac::{RxBlockAckAgreement, VifId};
use oer_ieee80211_mac::ap::ApActionFrame;
use oer_ieee80211_mac::block_ack::{
    ADDBA_ACTION_BODY_LEN, BlockAckAction, TxBlockAckResponse, TxBlockAckRetry,
    write_declined_addba_response, write_successful_addba_response,
};
use oer_ieee80211_upper_mac_service::reorder::{
    CURRENT_SLOT, Offer, PORT_REORDER_WINDOW, ReorderRelease, RxReorder,
};

/// Exchanges of the access point that wait for a completion at once.
pub const PORT_AP_EXCHANGES: usize = 2;
/// Received frames the router keeps for the access point.
pub const PORT_AP_BACKLOG: usize = 4;

/// Probe Responses go out at most once per this interval, whoever asks: a
/// sender that changes its address gains no more air.
const PROBE_RESPONSE_INTERVAL: Duration = Duration::from_millis(10);

/// The reason of a Disassociation for inactivity (IEEE Std 802.11-2020
/// Table 9-90, `DISASSOC_DUE_TO_INACTIVITY`).
const REASON_INACTIVITY: u16 = 4;
/// The reason of every other access-point teardown: the previous
/// authentication is no longer valid.
const REASON_AUTHENTICATION_INVALID: u16 = 2;
/// The EtherType of EAPOL.
const EAPOL_ETHER_TYPE: u16 = 0x888e;
/// An EAPOL-Key frame of the four-way handshake.
type EapolFrame = RsnTxFrame<RSN_HANDSHAKE_EAPOL_CAPACITY>;

/// The event router of an access point's port.
pub type PortApRouter<'p, X> =
    EventRouter<'p, <X as PortClientEnv>::Port, PORT_AP_EXCHANGES, PORT_AP_BACKLOG>;

/// The access point's client of the port.
pub type PortApClient<'p, X> = PortClient<'p, X, PORT_AP_EXCHANGES, PORT_AP_BACKLOG>;

/// The types an access point over the port is built from: its port and
/// transmit policy ([`PortClientEnv`]), the image's monotonic time and the
/// source of each handshake's authenticator material.
pub trait PortApEnv: PortClientEnv {
    type Timer: Timer;
    type Authenticator: PortApAuthenticator;
    /// Where a WPA3 BSS's SAE responder runs; [`NoSae`] for another BSS.
    type Sae: PortApSae;
    /// How the access point picks the data rates of each associated
    /// station: one controller per peer (`FixedRateControl`, or the
    /// Espressif `EspressifRateControl` of `oer-espressif-ieee80211-policy`).
    type RateControl: RateControl;
    /// Where the frames the access point sends wait: the network's own
    /// owners, queued by Ethernet destination, which the access point takes
    /// when it sends them or holds for a dozing peer.
    type Frames: DestinationTxQueues<Frame = Self::NetworkFrame>;
}

/// A frame the access point sends: an owner of its network's source.
pub type PortApFrame<X> = <X as PortClientEnv>::NetworkFrame;

/// The executor of an access point's SAE responder: the access point hands
/// it one SAE frame at a time and takes the output when it is ready, so the
/// elliptic-curve work of a Commit runs where the composition places it.
pub trait PortApSae {
    /// Take one received SAE frame; `false` while the output of the last
    /// one was not taken, and the frame is dropped (the station repeats
    /// it).
    fn submit(&mut self, frame: ApSaeFrame<'_>, now: Instant) -> bool;
    /// The station and output of the frame taken, once ready.
    fn poll_output(&mut self, context: &mut Context<'_>) -> Poll<([u8; 6], ApSaeOutput)>;
    /// Drop the session of a station the access point removed.
    fn forget(&mut self, peer: [u8; 6]);
}

/// No SAE: the executor of an Open or WPA2 BSS.
pub struct NoSae;

impl PortApSae for NoSae {
    fn submit(&mut self, _frame: ApSaeFrame<'_>, _now: Instant) -> bool {
        false
    }

    fn poll_output(&mut self, _context: &mut Context<'_>) -> Poll<([u8; 6], ApSaeOutput)> {
        Poll::Pending
    }

    fn forget(&mut self, _peer: [u8; 6]) {}
}

/// The responder run inline: a submitted frame's work is done before
/// `submit` returns, and its output is ready at the next poll.
pub struct InlineSae<R> {
    responder: ApSaeResponder,
    random: R,
    ready: Option<([u8; 6], ApSaeOutput)>,
}

impl<R: ApSaeRandom> InlineSae<R> {
    pub const fn new(responder: ApSaeResponder, random: R) -> Self {
        Self {
            responder,
            random,
            ready: None,
        }
    }
}

impl<R: ApSaeRandom> PortApSae for InlineSae<R> {
    fn submit(&mut self, frame: ApSaeFrame<'_>, now: Instant) -> bool {
        if self.ready.is_some() {
            return false;
        }
        // Nothing waits in a queue: the frame is served at once.
        let output = self.responder.receive(frame, now, 0, &mut self.random);
        self.ready = Some((frame.peer, output));
        true
    }

    fn poll_output(&mut self, _context: &mut Context<'_>) -> Poll<([u8; 6], ApSaeOutput)> {
        self.ready.take().map_or(Poll::Pending, Poll::Ready)
    }

    fn forget(&mut self, peer: [u8; 6]) {
        self.responder.forget(peer);
    }
}

/// What the access point waits for between its deadlines.
#[expect(
    clippy::large_enum_variant,
    reason = "no_std without an allocator: one SAE output moves by value, once"
)]
enum Wake<B> {
    Input(Option<PortInput<B>>),
    Sae([u8; 6], ApSaeOutput),
    /// The network queued a frame.
    Frames,
}

/// The fresh material of each WPA2 four-way handshake the access point
/// starts.
pub trait PortApAuthenticator {
    /// The authenticator's nonce, unpredictable, and the initial EAPOL-Key
    /// replay counter.
    fn handshake_material(&mut self) -> ([u8; 32], u64);
}

/// The BSS an access point runs; its security is its service's.
#[derive(Clone, Copy, Debug)]
pub struct PortApProfile<'a> {
    pub ssid: &'a WifiSsid,
    pub channel: WifiChannel,
    pub beacon_interval_tu: u16,
    pub dtim_period: u8,
    /// The rates, HT capabilities and WMM parameters the access point
    /// claims.
    pub advertisement: &'a Advertisement,
    /// The rate of beacons, management frames and group data; each peer's
    /// data goes at its rate control's.
    pub management_rate: PhyRate,
    pub coex: CoexPriority,
    /// The step of the CCMP packet numbers the access point sends under.
    pub ccmp_step: CcmpPacketNumberStep,
    /// How long a receive reorder window keeps an MPDU behind a missing
    /// one before it releases its run past the gap.
    pub rx_reorder_gap: Duration,
    /// How many TX Block Ack offers a peer gets, and how long a failed one
    /// waits before the next goes with the peer's data.
    pub tx_block_ack_retry: TxBlockAckRetry,
}

/// The memory of one access point that the composition places: its beacon
/// template, the subframes of its A-MPDU, the `HELD` frames it holds for power save
/// (for every dozing peer and the next DTIM together; the composition sizes
/// it for its peers' traffic and its memory) and its peers' receive
/// reordering.
pub struct PortApStorage<const HELD: usize, F> {
    beacon: [u8; AP_BEACON_CAPACITY],
    subframes: AmpduSubframes<NetworkBody<F>>,
    held: [Option<HeldFrame<F>>; HELD],
    /// The peers' receive Block Ack agreements, one per peer at most on
    /// average, and their kept MPDUs.
    reorder: RxReorder<AP_MAX_CLIENTS>,
}

impl<const HELD: usize, F: SoftwareTxFrame> PortApStorage<HELD, F> {
    pub const fn new() -> Self {
        Self {
            beacon: [0; AP_BEACON_CAPACITY],
            subframes: AmpduSubframes::new(),
            held: [const { None }; HELD],
            reorder: RxReorder::new(),
        }
    }
}

/// One frame held for power save, the network's own owner: for a dozing
/// peer, or for the group until the next DTIM.
pub struct HeldFrame<F> {
    order: u32,
    frame: F,
}

impl<F: SoftwareTxFrame> HeldFrame<F> {
    fn destination(&self) -> [u8; 6] {
        destination(self.frame.ethernet()).unwrap_or_default()
    }

    fn is_group(&self) -> bool {
        self.frame
            .ethernet()
            .first()
            .is_some_and(|octet| octet & 1 != 0)
    }
}

/// The Ethernet destination of `ethernet`.
fn destination(ethernet: &[u8]) -> Option<[u8; 6]> {
    ethernet.get(..6)?.try_into().ok()
}

/// The frames held for power save, shared by every dozing peer and the
/// group, each released oldest first for its destination.
struct PowerSaveBuffer<'p, F> {
    frames: &'p mut [Option<HeldFrame<F>>],
    next_order: u32,
}

impl<'p, F: SoftwareTxFrame> PowerSaveBuffer<'p, F> {
    fn new(frames: &'p mut [Option<HeldFrame<F>>]) -> Self {
        frames.iter_mut().for_each(|slot| *slot = None);
        Self {
            frames,
            next_order: 0,
        }
    }

    /// Hold `frame`; the frame back when every slot is taken.
    fn hold(&mut self, frame: F) -> Result<(), F> {
        let Some(slot) = self.frames.iter_mut().find(|slot| slot.is_none()) else {
            return Err(frame);
        };
        *slot = Some(HeldFrame {
            order: self.next_order,
            frame,
        });
        self.next_order = self.next_order.wrapping_add(1);
        Ok(())
    }

    /// Take the oldest frame `matches` selects.
    fn take_oldest(&mut self, matches: impl Fn(&HeldFrame<F>) -> bool) -> Option<HeldFrame<F>> {
        let index = self
            .frames
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| {
                slot.as_ref()
                    .filter(|held| matches(held))
                    .map(|held| (index, held.order))
            })
            .min_by_key(|(_, order)| self.next_order.wrapping_sub(*order).wrapping_neg())
            .map(|(index, _)| index)?;
        self.frames[index].take()
    }

    /// Drop every frame held for `peer`.
    fn drop_for(&mut self, peer: [u8; 6]) {
        for slot in self.frames.iter_mut() {
            if slot.as_ref().is_some_and(|held| held.destination() == peer) {
                *slot = None;
            }
        }
    }
}

impl<const HELD: usize, F: SoftwareTxFrame> Default for PortApStorage<HELD, F> {
    fn default() -> Self {
        Self::new()
    }
}

/// An associated peer's link: its rate control, its pairwise key once
/// authorized in a protected BSS, the CCMP packet numbers sent to it and
/// received from it, and its duplicate filter.
struct PeerLink<R> {
    peer: [u8; 6],
    rate: R,
    key: Option<KeyHandle>,
    transmit: CcmpTxPacketNumber,
    replay: CcmpRxReplayState,
    duplicates: RxDuplicateFilter,
}

/// What the access point sent, admitted and ignored.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PortApCounters {
    pub beacons: u32,
    pub probe_responses: u32,
    /// Probe Requests answered by none: another SSID, or inside the
    /// response interval.
    pub probes_ignored: u32,
    /// Authentication Responses sent, successful or not.
    pub authentications: u32,
    /// Association Responses sent, successful or not.
    pub associations: u32,
    /// Peers that disassociated or deauthenticated.
    pub peers_left: u32,
    /// Peers the access point closed.
    pub peers_closed: u32,
    /// Management requests the access point does not serve: SAE outside a
    /// WPA3 BSS, Block Ack actions of a peer not authorized.
    pub unserved: u32,
    /// EAPOL-Key frames sent, retransmissions included.
    pub eapol_sent: u32,
    /// SAE frames handed to the executor.
    pub sae_frames: u32,
    /// SAE frames dropped while the executor was busy.
    pub sae_dropped: u32,
    /// Stations SAE authenticated.
    pub sae_accepted: u32,
    /// Data MPDUs sent to a peer or the group, alone or in an A-MPDU.
    pub data_sent: u32,
    /// TX Block Ack agreements the access point offered its peers.
    pub tx_agreements_offered: u32,
    /// TX Block Ack agreements its peers accepted.
    pub tx_agreements: u32,
    /// Offers that did not leave, or that a peer declined or left
    /// unanswered.
    pub tx_agreements_failed: u32,
    /// A-MPDUs sent.
    pub aggregates: u32,
    /// Subframes of those A-MPDUs the peers' BlockAcks acknowledged.
    pub aggregated_acknowledged: u32,
    /// Queued frames for no authorized destination, dropped.
    pub data_dropped: u32,
    /// MSDUs handed to the application.
    pub delivered: u32,
    /// Received data MPDUs of no authorized peer, or under the wrong
    /// protection.
    pub rx_rejected: u32,
    /// Retransmissions of an MPDU already received.
    pub duplicates: u32,
    /// Protected MPDUs whose packet number did not advance.
    pub replayed: u32,
    /// MPDUs that do not decapsulate, fragments included.
    pub malformed: u32,
    /// Receive Block Ack agreements accepted.
    pub rx_agreements: u32,
    /// MPDUs behind a Block Ack window.
    pub behind_window: u32,
    /// Out-of-order MPDUs longer than a storage slot.
    pub unbuffered: u32,
    /// Buffered runs a reorder window released past a gap.
    pub reorder_gap_timeouts: u32,
    /// Frames held for a dozing peer or the next DTIM.
    pub held: u32,
    /// Frames for a dozing peer or the group dropped with every slot
    /// taken.
    pub held_dropped: u32,
    /// Held frames sent on a wake-up, a PS-Poll or after a DTIM beacon.
    pub released: u32,
    /// Peers authorized by a completed four-way handshake.
    pub handshakes: u32,
}

/// Why an access point could not be built.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PortApBuildError {
    Beacon(ApBeaconBuildError),
    /// The service's address is not the client's interface address.
    AddressMismatch,
}

/// Why an access-point operation failed.
#[derive(Debug, Eq, PartialEq)]
pub enum PortApError<E> {
    Client(PortClientError<E>),
    /// The beacon template could not be stamped or protected.
    Beacon,
    Tim(TimBitmapError),
    Probe(ResponseError),
    Response(ApAssociationResponseError),
    Service(ApServiceError),
    Wpa2(ApWpa2Error),
    Data(ApDataFrameError),
    /// The port refused the interface's TSF restart.
    Tsf(SettingError),
    /// The port refused a key or its removal.
    Key(SettingError),
    /// The port holds the keys of no more peers.
    KeysFull,
    /// A key's transmit packet numbers are used up.
    PacketNumbers,
}

/// Whether the backend decrypted and verified a protected frame.
fn decrypted(meta: RxMeta) -> bool {
    matches!(
        meta.crypto,
        RxEvidence::HardwareObserved(RxCryptoStatus::DecryptedAndIntegrityVerified)
            | RxEvidence::ProtocolValidated(RxCryptoStatus::DecryptedAndIntegrityVerified)
    )
}

impl<E> From<ApWpa2Error> for PortApError<E> {
    fn from(error: ApWpa2Error) -> Self {
        Self::Wpa2(error)
    }
}

impl<E> From<ApDataFrameError> for PortApError<E> {
    fn from(error: ApDataFrameError) -> Self {
        Self::Data(error)
    }
}

impl<E> From<PortClientError<E>> for PortApError<E> {
    fn from(error: PortClientError<E>) -> Self {
        Self::Client(error)
    }
}

impl<E> From<ApAssociationResponseError> for PortApError<E> {
    fn from(error: ApAssociationResponseError) -> Self {
        Self::Response(error)
    }
}

impl<E> From<ApServiceError> for PortApError<E> {
    fn from(error: ApServiceError) -> Self {
        Self::Service(error)
    }
}

/// One access point over the port: see the [module](self).
pub struct PortAccessPoint<'p, X: PortApEnv> {
    client: PortApClient<'p, X>,
    timer: X::Timer,
    authenticator: X::Authenticator,
    sae: X::Sae,
    profile: PortApProfile<'p>,
    beacon: ApBeacon<'p>,
    service: AccessPointService<'p>,
    /// The group key the port holds, in a protected BSS.
    group_key: Option<KeyHandle>,
    /// The link of each associated peer.
    links: [Option<PeerLink<X::RateControl>>; AP_MAX_CLIENTS],
    /// What every peer's rate control is configured with.
    rate_control: <X::RateControl as RateControl>::Config,
    /// The packet numbers of group data.
    group_transmit: CcmpTxPacketNumber,
    /// The network's frames to send.
    frames: &'p X::Frames,
    /// The destination last served: the next turn goes to the one after it.
    cursor: Option<[u8; 6]>,
    /// The subframes of the A-MPDU being sent.
    subframes: &'p mut AmpduSubframes<NetworkBody<PortApFrame<X>>>,
    buffered: PowerSaveBuffer<'p, PortApFrame<X>>,
    reorder: &'p mut RxReorder<AP_MAX_CLIENTS>,
    /// The protection the beacon template carries.
    advertised: ApBssProtection,
    /// Before it no Probe Response goes out.
    next_probe_response: Instant,
    counters: PortApCounters,
}

/// The values of an access point's environment: its client of the port,
/// its timer, authenticator and SAE executor, and what each peer's rate
/// control is configured with.
pub struct PortApParts<'p, X: PortApEnv> {
    pub client: PortApClient<'p, X>,
    pub timer: X::Timer,
    pub authenticator: X::Authenticator,
    pub sae: X::Sae,
    pub rate_control: <X::RateControl as RateControl>::Config,
    /// The network's frames to send.
    pub frames: &'p X::Frames,
}

impl<'p, X: PortApEnv> PortAccessPoint<'p, X> {
    /// An access point of `profile` and `service` over `parts`' client, its
    /// beacon template in `storage`.
    pub fn new<const HELD: usize>(
        parts: PortApParts<'p, X>,
        profile: PortApProfile<'p>,
        service: AccessPointService<'p>,
        storage: &'p mut PortApStorage<HELD, PortApFrame<X>>,
    ) -> Result<Self, PortApBuildError> {
        let PortApParts {
            client,
            timer,
            authenticator,
            sae,
            rate_control,
            frames,
        } = parts;
        let PortApStorage {
            beacon,
            subframes,
            held,
            reorder,
        } = storage;
        let buffered = PowerSaveBuffer::new(held);
        if service.address() != client.config().address {
            return Err(PortApBuildError::AddressMismatch);
        }
        let beacon = ApBeacon::new(
            beacon,
            profile.advertisement,
            client.config().address,
            profile.ssid,
            profile.channel,
            profile.beacon_interval_tu,
            profile.dtim_period,
            SequenceNumber::ZERO,
            service.security_policy(),
        )
        .map_err(PortApBuildError::Beacon)?;
        Ok(Self {
            client,
            timer,
            authenticator,
            sae,
            profile,
            beacon,
            service,
            group_key: None,
            links: [const { None }; AP_MAX_CLIENTS],
            rate_control,
            group_transmit: CcmpTxPacketNumber::new(profile.ccmp_step),
            frames,
            cursor: None,
            subframes,
            buffered,
            reorder,
            advertised: ApBssProtection::default(),
            next_probe_response: Instant::EPOCH,
            counters: PortApCounters::default(),
        })
    }

    pub const fn counters(&self) -> PortApCounters {
        self.counters
    }

    pub const fn client(&self) -> &PortApClient<'p, X> {
        &self.client
    }

    /// The BSS's peers and security.
    pub const fn service(&self) -> &AccessPointService<'p> {
        &self.service
    }

    /// Tune to the BSS's channel, receive the BSS and the Probe Requests
    /// the access point answers, restart the interface's TSF and, in a
    /// protected BSS, install the group key.
    pub async fn start(&mut self) -> Result<(), PortApError<PortError<X>>> {
        self.client
            .retune(Channel::from_wifi_channel(self.profile.channel))
            .await?;
        self.client.configure(
            None,
            ReceiveFilter::BSS_MEMBER.union(ReceiveFilter::PROBE_REQUESTS),
        )?;
        let vif = self.client.config().vif;
        self.client
            .port()
            .set_tsf(VifTsf {
                vif,
                at: TsfInstant::from_micros(0),
            })
            .map_err(PortClientError::Port)?
            .map_err(PortApError::Tsf)?;
        if self.service.link_protection() == LinkProtection::Ccmp && self.group_key.is_none() {
            let gtk = self.service.gtk()?;
            let (key_id, key) = (gtk.key_id(), *gtk.key());
            self.group_key = Some(self.install_key(KeyScope::Group { key_id }, &key)?);
        }
        Ok(())
    }

    fn install_key(
        &self,
        scope: KeyScope,
        key: &[u8],
    ) -> Result<KeyHandle, PortApError<PortError<X>>> {
        self.client
            .port()
            .install_key(KeyInstall {
                vif: self.client.config().vif,
                cipher: Cipher::Ccmp128,
                scope,
                key,
            })
            .map_err(PortClientError::Port)?
            .map_err(PortApError::Key)
    }

    fn link_mut(&mut self, peer: [u8; 6]) -> Option<&mut PeerLink<X::RateControl>> {
        self.links
            .iter_mut()
            .flatten()
            .find(|link| link.peer == peer)
    }

    /// Open the link of `peer`, which just associated with `ht` receive
    /// capabilities, its rate control started from `link_metric`.
    fn open_link(
        &mut self,
        peer: [u8; 6],
        ht: Option<HtPeerCapabilities>,
        link_metric: Option<i8>,
    ) -> Result<(), PortApError<PortError<X>>> {
        // The peer's HT width is the BSS's where it supports 40 MHz.
        let phy = match ht {
            None => PhyMode::Legacy,
            Some(ht) if self.profile.channel.bandwidth_mhz() >= 40 && ht.supports_40_mhz() => {
                PhyMode::Ht40
            }
            Some(_) => PhyMode::Ht20,
        };
        let rate = X::RateControl::for_peer(
            self.rate_control,
            &RatePeer {
                phy,
                ht_capabilities: ht,
                he_capabilities: None,
                he_peer_state: None,
            },
            link_metric,
        );
        let Some(slot) = self.links.iter_mut().find(|slot| slot.is_none()) else {
            return Err(PortApError::KeysFull);
        };
        *slot = Some(PeerLink {
            peer,
            rate,
            key: None,
            transmit: CcmpTxPacketNumber::new(self.profile.ccmp_step),
            replay: CcmpRxReplayState::default(),
            duplicates: RxDuplicateFilter::new(),
        });
        Ok(())
    }

    /// Close `peer`'s link, removing its pairwise key from the port.
    fn close_link(&mut self, peer: [u8; 6]) -> Result<(), PortApError<PortError<X>>> {
        let Some(slot) = self
            .links
            .iter_mut()
            .find(|slot| slot.as_ref().is_some_and(|link| link.peer == peer))
        else {
            return Ok(());
        };
        let Some(PeerLink {
            key: Some(handle), ..
        }) = slot.take()
        else {
            return Ok(());
        };
        self.client
            .apply(LowerMacSetting::RemoveKey(handle))
            .map_err(|error| match error {
                PortClientError::Setting(error) => PortApError::Key(error),
                error => PortApError::Client(error),
            })
    }

    /// Forget a peer: its link and pairwise key, its SAE session, the frames
    /// held for it, then its state.
    fn remove_peer(&mut self, peer: [u8; 6]) -> Result<(), PortApError<PortError<X>>> {
        self.close_link(peer)?;
        self.buffered.drop_for(peer);
        self.stop_rx_agreements(peer)?;
        self.sae.forget(peer);
        self.service.remove_peer(peer)?;
        Ok(())
    }

    /// Serve the BSS until `deadline`: a beacon at every TBTT, a response
    /// to every management request it answers, a close of every peer that
    /// went inactive.
    pub async fn run_until(
        &mut self,
        deadline: Instant,
        deliver: &mut impl FnMut(PortMsdu<'_, PortRxBuffer<X>>),
    ) -> Result<(), PortApError<PortError<X>>> {
        loop {
            let now = self.timer.now();
            if self.beacon.publication_due(now) {
                self.publish_beacon(now).await?;
                continue;
            }
            if let Some(close) = self.service.begin_due_peer_close(now) {
                self.close_peer(close).await?;
                continue;
            }
            match self
                .service
                .take_due_wpa2_retry::<RSN_HANDSHAKE_EAPOL_CAPACITY>(now)?
            {
                ApWpa2RetryProgress::Transmit { peer, frame } => {
                    self.send_eapol(peer, &frame, true).await?;
                    continue;
                }
                ApWpa2RetryProgress::Close(close) => {
                    self.close_peer(close).await?;
                    continue;
                }
                ApWpa2RetryProgress::None => {}
            }
            self.expire_reorder_gaps(now, deliver);
            if self.expire_tx_block_ack(now) {
                continue;
            }
            if let Some(frame) = self.next_frame() {
                self.dispatch(frame).await?;
                continue;
            }
            if now >= deadline {
                return Ok(());
            }
            let wake = [
                self.beacon.next_publication(),
                self.service.next_peer_deadline(),
                self.service.next_wpa2_retry_deadline(),
                self.reorder.next_gap_deadline(),
                self.service.next_tx_block_ack_deadline(),
            ]
            .into_iter()
            .flatten()
            .fold(deadline, Instant::min);
            let woken = {
                let Self {
                    client,
                    timer,
                    sae,
                    frames,
                    ..
                } = self;
                let mut input = pin!(client.next_input(&*timer, wake));
                poll_fn(|context| {
                    if let Poll::Ready((peer, output)) = sae.poll_output(context) {
                        return Poll::Ready(Wake::Sae(peer, output));
                    }
                    if frames.poll_ready_any(context).is_ready() {
                        return Poll::Ready(Wake::Frames);
                    }
                    input.as_mut().poll(context).map(Wake::Input)
                })
                .await
            };
            match woken {
                Wake::Sae(peer, output) => self.sae_output(peer, output, now).await?,
                Wake::Frames => {}
                Wake::Input(Some(PortInput::Frame(frame))) => {
                    self.receive(frame, now, deliver).await?;
                }
                Wake::Input(Some(PortInput::Poisoned)) => {
                    return Err(PortApError::Client(PortClientError::Poisoned));
                }
                Wake::Input(Some(PortInput::Tbtt(_) | PortInput::EventsLost) | None) => {}
            }
        }
    }

    /// The protection the associated peers require of the BSS.
    fn required_protection(&self) -> ApBssProtection {
        self.service
            .bss_protection(self.profile.channel.bandwidth_mhz() >= 40)
    }

    /// Carry the protection the peers require in the beacon template, which
    /// probe responses repeat.
    fn advertise_current_protection(&mut self) -> Result<(), PortApError<PortError<X>>> {
        let required = self.required_protection();
        if required != self.advertised {
            self.beacon
                .set_bss_protection(required)
                .map_err(|_| PortApError::Beacon)?;
            self.advertised = required;
        }
        Ok(())
    }

    /// Send one management frame to a peer, acknowledged, on the voice
    /// queue at the management rate.
    async fn send_management(&mut self, frame: &[u8]) -> Result<(), PortApError<PortError<X>>> {
        self.client
            .transmit(
                TxMpdu::whole(frame),
                KeySelector::Plaintext,
                WmmAccessCategory::Voice,
                self.profile.management_rate,
                self.profile.coex,
            )
            .await?;
        Ok(())
    }

    /// Stamp the beacon and send it once, unacknowledged, on the voice
    /// queue.
    async fn publish_beacon(&mut self, now: Instant) -> Result<(), PortApError<PortError<X>>> {
        self.advertise_current_protection()?;
        let sequence = self.service.next_management_sequence();
        let bitmap = self
            .service
            .unicast_tim_bitmap()
            .map_err(PortApError::Tim)?;
        let group_pending = self.service.group_traffic_pending();
        let Self {
            client,
            beacon,
            profile,
            counters,
            ..
        } = self;
        let frame = beacon
            .prepare(now, sequence, group_pending, bitmap.partial())
            .ok_or(PortApError::Beacon)?;
        let dtim_beacon = dtim(frame).is_some_and(|(_, count, _)| count == 0);
        client
            .transmit(
                TxMpdu::whole(frame),
                KeySelector::Plaintext,
                WmmAccessCategory::Voice,
                profile.management_rate,
                profile.coex,
            )
            .await?;
        counters.beacons = counters.beacons.saturating_add(1);
        // The group frames a DTIM beacon announced follow it.
        if dtim_beacon && group_pending {
            self.release_group().await?;
        }
        Ok(())
    }

    /// Send every group frame held for this DTIM, oldest first, More Data
    /// set on all but the last.
    async fn release_group(&mut self) -> Result<(), PortApError<PortError<X>>> {
        while let Some(release) = self.service.begin_buffered_group_release()? {
            let Some(held) = self.buffered.take_oldest(HeldFrame::is_group) else {
                self.service
                    .complete_buffered_group_release(release, false)?;
                break;
            };
            self.transmit_data(held.frame, release.more_data()).await?;
            self.counters.released = self.counters.released.saturating_add(1);
            self.service
                .complete_buffered_group_release(release, true)?;
        }
        Ok(())
    }

    /// Send `count` frames held for `peer`, or every one, More Data set
    /// while more remain.
    async fn release_unicast(
        &mut self,
        peer: [u8; 6],
        mut count: Option<usize>,
    ) -> Result<(), PortApError<PortError<X>>> {
        let Some(identity) = self
            .service
            .peer_status(peer)
            .map(|status| status.association_identity())
        else {
            return Ok(());
        };
        while count != Some(0) {
            let Ok(Some(release)) = self.service.begin_buffered_unicast_release(identity) else {
                break;
            };
            self.send_release(peer, release).await?;
            count = count.map(|count| count - 1);
        }
        Ok(())
    }

    /// Send the frame one release reserved.
    async fn send_release(
        &mut self,
        peer: [u8; 6],
        release: ApBufferedUnicastRelease,
    ) -> Result<(), PortApError<PortError<X>>> {
        let Some(held) = self.buffered.take_oldest(|held| held.destination() == peer) else {
            self.service
                .complete_buffered_unicast_release(release, false)?;
            return Ok(());
        };
        self.transmit_data(held.frame, release.more_data()).await?;
        self.counters.released = self.counters.released.saturating_add(1);
        self.service
            .complete_buffered_unicast_release(release, true)?;
        Ok(())
    }

    /// The next frame to send: the head of the destination after the last
    /// one served, so every destination with frames gets its turn.
    fn next_frame(&mut self) -> Option<PortApFrame<X>> {
        let (destination, _) = self.frames.next_head_after(self.cursor)?;
        self.cursor = Some(destination);
        self.frames.try_take_for(destination)
    }

    /// Send one frame now, with the frames for its peer that follow it as an
    /// A-MPDU where one applies, or hold it for a dozing peer or for the next
    /// DTIM while any authorized peer dozes. A frame for no authorized
    /// destination is dropped, its owner returned to the network.
    async fn dispatch(&mut self, frame: PortApFrame<X>) -> Result<(), PortApError<PortError<X>>> {
        let Some(destination) = destination(frame.ethernet()) else {
            self.counters.data_dropped = self.counters.data_dropped.saturating_add(1);
            return Ok(());
        };
        let hold = if destination[0] & 1 != 0 {
            if self.service.authorized_count() == 0 {
                self.counters.data_dropped = self.counters.data_dropped.saturating_add(1);
                return Ok(());
            }
            (self.service.group_downlink_disposition() == ApDownlinkDisposition::Buffer)
                .then_some(None)
        } else {
            match self.service.admit_downlink(destination) {
                Ok(admission) if admission.disposition() == ApDownlinkDisposition::Buffer => {
                    Some(Some(admission.identity()))
                }
                Ok(_) => None,
                Err(_) => {
                    self.counters.data_dropped = self.counters.data_dropped.saturating_add(1);
                    return Ok(());
                }
            }
        };
        let Some(identity) = hold else {
            if destination[0] & 1 == 0 {
                // An offer a failed attempt left goes with the peer's data.
                let now = self.timer.now();
                self.send_due_tx_block_ack_offer(destination, now).await?;
                if let Some(run) = self.aggregate_run(destination, &frame) {
                    return self.transmit_aggregate(destination, frame, run).await;
                }
            }
            return self.transmit_data(frame, false).await;
        };
        if self.buffered.hold(frame).is_err() {
            self.counters.held_dropped = self.counters.held_dropped.saturating_add(1);
            return Ok(());
        }
        match identity {
            Some(identity) => self.service.commit_buffered_unicast(identity)?,
            None => self.service.commit_buffered_group()?,
        };
        self.counters.held = self.counters.held.saturating_add(1);
        Ok(())
    }

    /// Apply one power-management edge of an authorized peer: a peer that
    /// wakes gets every frame held for it, a PS-Poll one. A PS-Poll of a
    /// peer the service does not hold as dozing is the peer's error, not the
    /// access point's.
    async fn power_save(
        &mut self,
        observation: ApPowerSaveObservation,
        now: Instant,
    ) -> Result<(), PortApError<PortError<X>>> {
        match self.service.observe_power_save(observation, now) {
            Ok(ApPowerSaveAction::StateChanged {
                peer,
                state: ApPeerPowerState::Active,
                buffered_frames,
            }) if buffered_frames > 0 => self.release_unicast(peer, None).await,
            Ok(ApPowerSaveAction::ReleaseOne(release)) => {
                self.send_release(release.peer(), release).await
            }
            Ok(_) | Err(_) => Ok(()),
        }
    }

    async fn receive(
        &mut self,
        received: PortFrame<PortRxBuffer<X>>,
        now: Instant,
        deliver: &mut impl FnMut(PortMsdu<'_, PortRxBuffer<X>>),
    ) -> Result<(), PortApError<PortError<X>>> {
        let meta = received.meta();
        let frame = received.bytes();
        let power_save = observe_ap_power_save_for_access_point(frame, self.service.address())
            .filter(|observation| {
                let peer = match *observation {
                    ApPowerSaveObservation::Sleeping { peer }
                    | ApPowerSaveObservation::Active { peer }
                    | ApPowerSaveObservation::PsPoll { peer, .. } => peer,
                };
                self.service.is_authorized(peer)
            });
        // Any frame of an associated peer keeps it; an authentication that
        // never associates ends at its own deadline.
        if let Some(sender) = frame
            .get(10..16)
            .and_then(|address| address.try_into().ok())
            && self.service.peer_status(sender).is_some_and(|status| {
                matches!(
                    status.phase,
                    ApPeerPhase::Securing | ApPeerPhase::Authorized
                )
            })
        {
            self.service.observe_activity(sender, now)?;
        }
        if frame
            .first()
            .is_some_and(|control| (control >> 2) & 0b11 == 2)
        {
            self.receive_data(received, now, deliver).await?;
            if let Some(observation) = power_save {
                self.power_save(observation, now).await?;
            }
            return Ok(());
        }
        // A BlockAckReq of an authorized peer moves its agreement's window.
        if frame.len() >= 20
            && frame[0] == 0x84
            && frame.get(4..10) == Some(self.service.address().as_slice())
        {
            let peer = [
                frame[10], frame[11], frame[12], frame[13], frame[14], frame[15],
            ];
            let tid = frame[17] >> 4;
            let start =
                SequenceNumber::from_sequence_control(u16::from_le_bytes([frame[18], frame[19]]));
            if self.service.is_authorized(peer)
                && let Some(release) = self.reorder.move_window(peer, tid, start)
            {
                self.release(peer, &release, None, deliver);
            }
        }
        if let Some(observation) = power_save {
            return self.power_save(observation, now).await;
        }
        let address = self.client.config().address;
        let retry = frame.get(1).is_some_and(|flags| flags & 0x08 != 0);
        match parse_ap_management_request(self.profile.advertisement, frame, address) {
            Some(ApManagementRequest::Probe { peer, ssid }) => self.probe(peer, ssid).await,
            Some(ApManagementRequest::OpenAuthentication { peer }) => {
                self.authenticate(peer, retry, now).await
            }
            Some(ApManagementRequest::Association {
                peer,
                security,
                maximum_legacy_rate_500kbps,
                ht_capabilities,
                qos_supported,
                short_preamble,
            }) => {
                let capabilities = ApAssociationCapabilities {
                    maximum_legacy_rate_500kbps,
                    short_preamble,
                    ht: ht_capabilities,
                    qos_supported,
                };
                self.associate(peer, security, capabilities, link_metric(meta), now)
                    .await
            }
            Some(
                ApManagementRequest::Disassociation { peer, .. }
                | ApManagementRequest::Deauthentication { peer, .. },
            ) => {
                // A peer the access point is closing ends by its own
                // teardown.
                if self
                    .service
                    .peer_status(peer)
                    .is_some_and(|status| status.phase != ApPeerPhase::Closing)
                {
                    self.remove_peer(peer)?;
                    self.counters.peers_left = self.counters.peers_left.saturating_add(1);
                }
                Ok(())
            }
            Some(ApManagementRequest::SaeAuthentication {
                peer,
                transaction,
                status,
                body,
            }) if self.service.security_policy() == ApSecurityPolicy::Wpa3Personal => {
                let frame = ApSaeFrame {
                    peer,
                    transaction,
                    status,
                    body,
                };
                if self.sae.submit(frame, now) {
                    self.counters.sae_frames = self.counters.sae_frames.saturating_add(1);
                } else {
                    self.counters.sae_dropped = self.counters.sae_dropped.saturating_add(1);
                }
                Ok(())
            }
            Some(ApManagementRequest::BlockAck { peer, action })
                if self.service.is_authorized(peer) =>
            {
                self.block_ack_action(peer, action).await
            }
            Some(
                ApManagementRequest::SaeAuthentication { .. }
                | ApManagementRequest::BlockAck { .. },
            ) => {
                self.counters.unserved = self.counters.unserved.saturating_add(1);
                Ok(())
            }
            None => Ok(()),
        }
    }

    /// Answer a Probe Request for the BSS's SSID, or any SSID, with the
    /// current advertisement, at most once per [`PROBE_RESPONSE_INTERVAL`].
    async fn probe(&mut self, peer: [u8; 6], ssid: &[u8]) -> Result<(), PortApError<PortError<X>>> {
        let now = self.timer.now();
        if !probe::matches_ssid(self.beacon.advertisement(), ssid) || now < self.next_probe_response
        {
            self.counters.probes_ignored = self.counters.probes_ignored.saturating_add(1);
            return Ok(());
        }
        self.advertise_current_protection()?;
        let sequence = self.service.next_management_sequence();
        let mut response = [0; AP_BEACON_CAPACITY];
        let length = probe::write_response(
            self.beacon.advertisement(),
            peer,
            sequence,
            now.as_micros(),
            &mut response,
        )
        .map_err(PortApError::Probe)?;
        self.next_probe_response = now.saturating_add(PROBE_RESPONSE_INTERVAL);
        self.send_management(&response[..length]).await?;
        self.counters.probe_responses = self.counters.probe_responses.saturating_add(1);
        Ok(())
    }

    /// Apply one SAE output: an accepted exchange authenticates the station
    /// with its PMK (ending an earlier pairwise-key epoch), a failed one
    /// forgets a station that never associated; then the replies go out in
    /// order.
    async fn sae_output(
        &mut self,
        peer: [u8; 6],
        output: ApSaeOutput,
        now: Instant,
    ) -> Result<(), PortApError<PortError<X>>> {
        match output.result {
            ApSaeResult::Accepted { pmk, pmkid } => {
                if self.service.peer_status(peer).is_some() {
                    self.close_link(peer)?;
                }
                self.service
                    .authenticate_sae(peer, Pmk::from_bytes(pmk), pmkid, now)?;
                self.counters.sae_accepted = self.counters.sae_accepted.saturating_add(1);
            }
            ApSaeResult::Failed { .. } => {
                if self
                    .service
                    .peer_status(peer)
                    .is_some_and(|status| status.phase == ApPeerPhase::Authenticated)
                {
                    self.remove_peer(peer)?;
                }
            }
            ApSaeResult::Continue => {}
        }
        for reply in output.replies() {
            let sequence = self.service.next_management_sequence();
            let mut frame = [0; AP_BEACON_CAPACITY];
            let length = write_sae_authentication(
                &mut frame,
                self.service.address(),
                reply.peer,
                reply.transaction,
                reply.status,
                reply.body(),
                sequence,
            )?;
            self.send_management(&frame[..length]).await?;
        }
        Ok(())
    }

    /// Answer an Open System authentication. A retransmission from a peer
    /// already past authentication is answered again without resetting it
    /// (its response's acknowledgement was lost); a new authentication
    /// starts the peer over.
    async fn authenticate(
        &mut self,
        peer: [u8; 6],
        retry: bool,
        now: Instant,
    ) -> Result<(), PortApError<PortError<X>>> {
        let repeated = retry
            && self
                .service
                .peer_status(peer)
                .is_some_and(|status| status.phase != ApPeerPhase::Authenticated);
        let status = if repeated {
            0
        } else {
            // A new authentication ends the peer's earlier pairwise-key
            // epoch before the service starts it over.
            self.close_link(peer)?;
            let ApMlmeAction::AuthenticationResponse { status, .. } =
                self.service.authenticate_open(peer, now)
            else {
                return Err(PortApError::Service(ApServiceError::WrongPeerPhase));
            };
            status
        };
        let sequence = self.service.next_management_sequence();
        let mut response = [0; AP_BEACON_CAPACITY];
        let length = write_open_authentication_response(
            &mut response,
            self.service.address(),
            peer,
            status,
            sequence,
        )?;
        self.send_management(&response[..length]).await?;
        self.counters.authentications = self.counters.authentications.saturating_add(1);
        Ok(())
    }

    /// Answer an association of an authenticated peer; a repeated request
    /// of an associated peer (in its handshake, or authorized in an Open
    /// BSS) is answered again with its association and starts no second
    /// handshake. A successful association in a protected BSS begins the
    /// four-way handshake.
    async fn associate(
        &mut self,
        peer: [u8; 6],
        security: oer_ieee80211_mac::ap::ApAssociationSecurityObservation<'_>,
        capabilities: ApAssociationCapabilities,
        link_metric: Option<i8>,
        now: Instant,
    ) -> Result<(), PortApError<PortError<X>>> {
        let Some(status) = self.service.peer_status(peer) else {
            return Ok(());
        };
        let protected = self.service.link_protection() == LinkProtection::Ccmp;
        let repeated = self.service.matches_association_security(security)
            && (status.phase == ApPeerPhase::Securing
                || (!protected && status.phase == ApPeerPhase::Authorized));
        let (result, association_id, ht) = if repeated {
            (0, status.association_id, status.ht)
        } else if status.phase == ApPeerPhase::Authenticated {
            let action = if protected {
                let (nonce, replay_counter) = self.authenticator.handshake_material();
                self.service.associate_rsn(
                    peer,
                    security,
                    capabilities,
                    nonce,
                    replay_counter,
                    now,
                )?
            } else {
                self.service
                    .associate_open(peer, security, capabilities, now)?
            };
            let ApMlmeAction::AssociationResponse {
                status,
                association_id,
                ..
            } = action
            else {
                return Err(PortApError::Service(ApServiceError::WrongPeerPhase));
            };
            (status, association_id.unwrap_or(0), capabilities.ht)
        } else {
            return Ok(());
        };
        let sequence = self.service.next_management_sequence();
        let mut response = [0; AP_BEACON_CAPACITY];
        let length = write_ht_association_response_frame_for_security(
            self.profile.advertisement,
            &mut response,
            self.service.address(),
            peer,
            result,
            association_id,
            sequence,
            self.profile.channel,
            ht,
            self.service.security_policy(),
            self.required_protection(),
        )?;
        self.send_management(&response[..length]).await?;
        self.counters.associations = self.counters.associations.saturating_add(1);
        if protected && !repeated && association_id != 0 {
            let message1: EapolFrame = self.service.begin_wpa2_frame(peer)?;
            self.send_eapol(peer, &message1, false).await?;
        }
        // A new association opens the peer's link; an Open BSS's peer is
        // authorized by it.
        if !repeated && association_id != 0 {
            self.close_link(peer)?;
            self.open_link(peer, ht, link_metric)?;
        }
        Ok(())
    }

    /// Send one EAPOL-Key frame to a peer in its handshake, unprotected and
    /// acknowledged, and arm its retransmission.
    async fn send_eapol(
        &mut self,
        peer: [u8; 6],
        frame: &EapolFrame,
        retransmission: bool,
    ) -> Result<(), PortApError<PortError<X>>> {
        let mut mpdu = [0; RSN_HANDSHAKE_EAPOL_CAPACITY + 64];
        let sequence_number = self.service.current_data_sequence();
        let length = ApDataFrame {
            access_point: self.service.address(),
            destination: peer,
            sequence_number,
            ether_type: EAPOL_ETHER_TYPE,
            payload: frame.as_bytes(),
        }
        .encode(&mut mpdu)?;
        // The sequence number is spent only once the frame encoded.
        self.service.next_data_sequence();
        let report = self
            .client
            .transmit(
                TxMpdu::whole(&mpdu[..length]),
                KeySelector::Plaintext,
                WmmAccessCategory::Voice,
                self.profile.management_rate,
                self.profile.coex,
            )
            .await?;
        let acknowledged =
            matches!(report, TxReport::Mpdu(status) if status.acknowledged == Some(true));
        self.counters.eapol_sent = self.counters.eapol_sent.saturating_add(1);
        self.service
            .observe_wpa2_transmit(peer, retransmission, acknowledged, self.timer.now())?;
        Ok(())
    }

    /// Send one network frame: to an authorized peer under its pairwise key,
    /// or to the group under the group key, as QoS data to a QoS peer in a
    /// protected BSS, its payload handed to the port by ownership. A frame
    /// for no authorized destination is dropped.
    async fn transmit_data(
        &mut self,
        frame: PortApFrame<X>,
        more_data: bool,
    ) -> Result<(), PortApError<PortError<X>>> {
        let Some(destination) = frame
            .ethernet()
            .get(..6)
            .and_then(|bytes| <[u8; 6]>::try_from(bytes).ok())
        else {
            self.counters.data_dropped = self.counters.data_dropped.saturating_add(1);
            return Ok(());
        };
        let group = destination[0] & 1 != 0;
        let authorized = if group {
            self.service.authorized_count() > 0
        } else {
            self.service.is_authorized(destination) && self.link_mut(destination).is_some()
        };
        if !authorized {
            self.counters.data_dropped = self.counters.data_dropped.saturating_add(1);
            return Ok(());
        }
        let access_point = self.service.address();
        let header = split_ethernet(frame.ethernet()).0;
        let mut mpdu = [0; PORT_MPDU_HEADER_CAPACITY];
        let (length, key) = if self.service.link_protection() == LinkProtection::Open {
            let sequence_number = self.service.current_data_sequence();
            let length = ApUnprotectedDataFrame {
                access_point,
                peer: destination,
                sequence_number,
                more_data,
                ethernet: header,
            }
            .encode(&mut mpdu)?;
            self.service.next_data_sequence();
            (length, KeySelector::Plaintext)
        } else {
            let peer_qos = !group
                && self
                    .service
                    .peer_status(destination)
                    .is_some_and(|status| status.qos_supported);
            let (length, key, _) =
                self.encode_protected(destination, peer_qos, more_data, header, &mut mpdu)?;
            (length, KeySelector::Key(key))
        };
        let rate = if group {
            self.profile.management_rate
        } else {
            self.link_mut(destination)
                .ok_or(PortApError::Service(ApServiceError::UnknownPeer))?
                .rate
                .mpdu_rate()
        };
        let report = self
            .client
            .transmit(
                TxMpdu {
                    header: &mpdu[..length],
                    body: Some(NetworkBody(frame)),
                },
                key,
                WmmAccessCategory::BestEffort,
                rate,
                self.profile.coex,
            )
            .await?;
        self.counters.data_sent = self.counters.data_sent.saturating_add(1);
        if !group
            && let TxReport::Mpdu(status) = report
            && let Some(link) = self.link_mut(destination)
        {
            link.rate.observe_mpdu(
                status.attempts,
                status.acknowledged == Some(true),
                status.ack_snr_db,
            );
        }
        Ok(())
    }

    /// Encode the MPDU header of the Ethernet header `ethernet` for
    /// `destination`, a peer or the group, under its
    /// key with its next packet number and sequence number, as QoS data of
    /// TID 0 to a QoS peer: its length, key and sequence number.
    fn encode_protected(
        &mut self,
        destination: [u8; 6],
        peer_qos: bool,
        more_data: bool,
        ethernet: &[u8],
        mpdu: &mut [u8],
    ) -> Result<(usize, KeyHandle, SequenceNumber), PortApError<PortError<X>>> {
        let sequence_number = if peer_qos {
            self.service
                .current_qos_sequence(destination, AP_TX_BLOCK_ACK_TID)
                .ok_or(PortApError::Service(ApServiceError::UnknownPeer))?
        } else {
            self.service.current_data_sequence()
        };
        // The frame encodes before a packet number is spent.
        let length = ApProtectedDataFrame {
            access_point: self.service.address(),
            peer: destination,
            sequence_number,
            user_priority: 0,
            peer_qos,
            more_data,
            ccmp_header: [0; CCMP_HEADER_LEN],
            ethernet,
        }
        .encode(mpdu)?;
        let (header, key) = if destination[0] & 1 != 0 {
            let key_id = self.service.gtk()?.key_id();
            let key = self.group_key.ok_or(PortApError::KeysFull)?;
            let header = self
                .group_transmit
                .next_header(CcmpKeyId::new(key_id).ok_or(PortApError::KeysFull)?)
                .map_err(|_| PortApError::PacketNumbers)?;
            (header, key)
        } else {
            let link = self.link_mut(destination).ok_or(PortApError::KeysFull)?;
            let key = link.key.ok_or(PortApError::KeysFull)?;
            let header = link
                .transmit
                .next_header(CcmpKeyId::new(0).ok_or(PortApError::KeysFull)?)
                .map_err(|_| PortApError::PacketNumbers)?;
            (header, key)
        };
        let offset = if peer_qos {
            IEEE80211_QOS_DATA_HEADER_LEN
        } else {
            IEEE80211_LEGACY_DATA_HEADER_LEN
        };
        mpdu[offset..offset + CCMP_HEADER_LEN].copy_from_slice(&header);
        if peer_qos {
            self.service
                .next_qos_sequence(destination, AP_TX_BLOCK_ACK_TID);
        } else {
            self.service.next_data_sequence();
        }
        Ok((length, key, sequence_number))
    }

    /// The A-MPDU run that `head` for `destination` begins, its head and the
    /// next frame the network queued for the peer admitted: where the peer's
    /// operational TX Block Ack agreement, the port, the peer's HT A-MPDU
    /// Parameters and the Best Effort TXOP limit the BSS advertises admit
    /// two frames at the peer's A-MPDU rate. `None` sends `head` alone.
    fn aggregate_run(&self, destination: [u8; 6], head: &PortApFrame<X>) -> Option<AmpduRun> {
        let status = self.service.peer_status(destination)?;
        if !status.qos_supported || self.service.link_protection() != LinkProtection::Ccmp {
            return None;
        }
        let agreement = status.tx_block_ack?;
        let ht = status.ht?;
        let port = <X::Aggregation as PortAggregation<X>>::capabilities(self.client.port())?;
        let link = self
            .links
            .iter()
            .flatten()
            .find(|link| link.peer == destination)?;
        let txop = self
            .profile
            .advertisement
            .wmm
            .access_category(WmmAccessCategory::BestEffort)
            .txop_limit_units_32_us;
        let mut run = AmpduLimits {
            window: agreement.window,
            port,
            peer_ampdu_parameters: ht.ampdu_parameters(),
            txop_limit_micros: (txop != 0).then_some(u32::from(txop) * 32),
            rate: link.rate.ampdu_rate(),
        }
        .begin()?;
        let next = self.frames.head_for(destination)?;
        (run.admit(head.ethernet().len()) && run.admit(next.ethernet_bytes)).then_some(run)
    }

    /// Send `head` and the frames the network queued for `destination` that
    /// `run` admits after it, taken one at a time, as one A-MPDU of TID 0
    /// under the peer's pairwise key, at the peer's A-MPDU rate. Each
    /// subframe is its encoded header and its owner's payload; the owners go
    /// back to the network when the exchange ends. A frame the run does not
    /// admit stays in the network's queue.
    async fn transmit_aggregate(
        &mut self,
        destination: [u8; 6],
        head: PortApFrame<X>,
        mut run: AmpduRun,
    ) -> Result<(), PortApError<PortError<X>>> {
        let spacing = self
            .service
            .peer_status(destination)
            .and_then(|status| status.ht)
            .map_or(0, |ht| (ht.ampdu_parameters() >> 2) & 0x07);
        self.subframes.clear();
        let mut first_sequence = None;
        let mut header = [0_u8; PORT_MPDU_HEADER_CAPACITY];
        // The run admitted the head and the next frame already.
        let mut admitted = 1_usize;
        let mut frame = Some(head);
        while let Some(owner) = frame.take() {
            let (length, key, sequence) = self.encode_protected(
                destination,
                true,
                false,
                split_ethernet(owner.ethernet()).0,
                &mut header,
            )?;
            first_sequence.get_or_insert(sequence);
            if self
                .subframes
                .push(NetworkBody(owner), &header[..length], KeySelector::Key(key))
                .is_err()
            {
                // The loop stops at a full aggregate: only a header beyond a
                // subframe's capacity is refused.
                self.subframes.clear();
                return Err(PortApError::Data(ApDataFrameError::OutputTooSmall {
                    required: length,
                }));
            }
            if self.subframes.is_full() {
                break;
            }
            let admitted_next = if admitted == 1 {
                true
            } else {
                self.frames
                    .head_for(destination)
                    .is_some_and(|next| run.admit(next.ethernet_bytes))
            };
            if admitted_next {
                admitted += 1;
                frame = self.frames.try_take_for(destination);
            }
        }
        let run = self.subframes.len();
        let first_sequence =
            first_sequence.ok_or(PortApError::Service(ApServiceError::UnknownPeer))?;
        let committed_at = self
            .client
            .port()
            .now()
            .map_err(|error| PortApError::Client(PortClientError::Port(error)))?;
        let ampdu = self
            .subframes
            .request(AP_TX_BLOCK_ACK_TID, first_sequence, committed_at)
            .ok_or(PortApError::Service(ApServiceError::UnknownPeer))?;
        let config = *self.client.config();
        let initial_rate = self
            .link_mut(destination)
            .ok_or(PortApError::Service(ApServiceError::UnknownPeer))?
            .rate
            .ampdu_rate();
        let request = TxRequest {
            access_category: WmmAccessCategory::BestEffort,
            initial_rate,
            receiver: TxReceiver::Individual,
            power: config.power,
            coex: self.profile.coex,
            mpdu_retry_limit: config.retry_limit,
            body: TxBody::Ampdu(ampdu),
        };
        let mut headers = [&[][..]; PORT_AMPDU_SUBFRAMES];
        let frames = self.subframes.frames(&mut headers, spacing);
        let report =
            <X::Aggregation as PortAggregation<X>>::send(&mut self.client, frames, request).await;
        // The exchange ended: the owners go back to the network.
        self.subframes.clear();
        let report = report?;
        self.counters.aggregates = self.counters.aggregates.saturating_add(1);
        self.counters.data_sent = self.counters.data_sent.saturating_add(run as u32);
        if let TxReport::Ampdu(status) = report {
            self.counters.aggregated_acknowledged = self
                .counters
                .aggregated_acknowledged
                .saturating_add(u32::from(status.block_acknowledged_subframes));
            let now = self.timer.now();
            if let Some(link) = self.link_mut(destination) {
                link.rate.observe_ampdu(
                    now,
                    status.original_subframes,
                    status.block_acknowledged_subframes,
                    status.ack_snr_db,
                );
            }
        }
        Ok(())
    }

    /// Serve one received data MPDU: EAPOL of a peer in its handshake, or
    /// data of an authorized peer to the distribution system, checked for
    /// duplicates and, in a protected BSS, for its decryption and packet
    /// number, its MSDUs handed to the application.
    async fn receive_data(
        &mut self,
        received: PortFrame<PortRxBuffer<X>>,
        now: Instant,
        deliver: &mut impl FnMut(PortMsdu<'_, PortRxBuffer<X>>),
    ) -> Result<(), PortApError<PortError<X>>> {
        let bytes = received.bytes();
        let Some(peer) = bytes
            .get(10..16)
            .and_then(|address| <[u8; 6]>::try_from(address).ok())
        else {
            return Ok(());
        };
        let phase = self.service.peer_status(peer).map(|status| status.phase);
        if phase == Some(ApPeerPhase::Securing) {
            return self.receive_eapol(bytes, now).await;
        }
        // To the distribution system, from an authorized peer of this BSS.
        if bytes.get(1).is_none_or(|flags| flags & 0x03 != 0x01)
            || bytes.get(4..10) != Some(self.service.address().as_slice())
            || phase != Some(ApPeerPhase::Authorized)
        {
            self.counters.rx_rejected = self.counters.rx_rejected.saturating_add(1);
            return Ok(());
        }
        // Null Data carries only its power-management bit.
        if bytes[0] & 0x40 != 0 {
            return Ok(());
        }
        let qos = bytes[0] & 0x80 != 0;
        let (Some(sequence), Some(tid)) = (
            bytes
                .get(22..24)
                .map(|field| u16::from_le_bytes([field[0], field[1]])),
            if qos {
                bytes.get(24).map(|control| Some(control & 0x0f))
            } else {
                Some(None)
            },
        ) else {
            self.counters.malformed = self.counters.malformed.saturating_add(1);
            return Ok(());
        };
        let retry = bytes[1] & 0x08 != 0;
        let protected_bss = self.service.link_protection() == LinkProtection::Ccmp;
        let protected = bytes[1] & 0x40 != 0;
        let meta = received.meta();
        // An associated peer's link exists before its handshake ends; only
        // an authorized peer's data reaches the distribution system.
        let authorized = self.service.is_authorized(peer);
        let Some(link) = self.link_mut(peer).filter(|_| authorized) else {
            self.counters.rx_rejected = self.counters.rx_rejected.saturating_add(1);
            return Ok(());
        };
        if link.duplicates.is_duplicate(retry, sequence, tid) {
            self.counters.duplicates = self.counters.duplicates.saturating_add(1);
            return Ok(());
        }
        match (protected_bss, protected) {
            (false, false) => {}
            (true, true) if decrypted(meta) => {}
            _ => {
                self.counters.rx_rejected = self.counters.rx_rejected.saturating_add(1);
                return Ok(());
            }
        }
        match tid.filter(|tid| self.reorder.is_active(peer, *tid)) {
            Some(tid) => self.reorder_mpdu(peer, tid, received, deliver),
            None => self.deliver_mpdu(peer, received, deliver),
        }
        Ok(())
    }

    /// Offer one MPDU of a peer's Block Ack agreement to its reorder
    /// window: an MPDU it releases at once goes from the port's buffer, one
    /// it keeps is copied into the storage.
    fn reorder_mpdu(
        &mut self,
        peer: [u8; 6],
        tid: u8,
        frame: PortFrame<PortRxBuffer<X>>,
        deliver: &mut impl FnMut(PortMsdu<'_, PortRxBuffer<X>>),
    ) {
        let mut make_room = true;
        loop {
            match self.reorder.offer(peer, tid, frame.bytes(), make_room) {
                Offer::MakeRoom(release) => {
                    self.release(peer, &release, None, deliver);
                    make_room = false;
                }
                Offer::Released { release, current } => {
                    self.release(peer, &release, current.then_some(frame), deliver);
                    return;
                }
                Offer::Duplicate => {
                    self.counters.duplicates = self.counters.duplicates.saturating_add(1);
                    return;
                }
                Offer::Behind => {
                    self.counters.behind_window = self.counters.behind_window.saturating_add(1);
                    return;
                }
                Offer::Unbuffered => {
                    self.counters.unbuffered = self.counters.unbuffered.saturating_add(1);
                    return;
                }
                Offer::NoAgreement | Offer::Dropped => return,
            }
        }
    }

    /// Deliver a released run of `peer`'s window in order.
    fn release(
        &mut self,
        peer: [u8; 6],
        release: &ReorderRelease,
        mut current: Option<PortFrame<PortRxBuffer<X>>>,
        deliver: &mut impl FnMut(PortMsdu<'_, PortRxBuffer<X>>),
    ) {
        for mpdu in release.iter() {
            if mpdu.slot == CURRENT_SLOT {
                if let Some(frame) = current.take() {
                    self.deliver_mpdu(peer, frame, deliver);
                }
            } else if let Some(stored) = self.reorder.take(mpdu.slot) {
                self.deliver_stored(peer, stored.bytes(), deliver);
            }
        }
    }

    /// The payload of one in-order data MPDU of `peer` after its CCMP
    /// replay check in a protected BSS; `None` for a replay or a malformed
    /// MPDU, which it counts.
    fn checked_payload(&mut self, peer: [u8; 6], bytes: &[u8]) -> Option<(usize, usize)> {
        let qos = bytes.first()? & 0x80 != 0;
        let header = if qos {
            IEEE80211_QOS_DATA_HEADER_LEN
        } else {
            IEEE80211_LEGACY_DATA_HEADER_LEN
        };
        let offset = if self.service.link_protection() == LinkProtection::Ccmp {
            let tid = qos
                .then(|| bytes.get(24).map(|control| control & 0x0f))
                .flatten();
            let Some(ccmp) = bytes
                .get(header..header + CCMP_HEADER_LEN)
                .and_then(|ccmp| <[u8; CCMP_HEADER_LEN]>::try_from(ccmp).ok())
                .and_then(|ccmp| CcmpHeader::parse(ccmp).ok())
                .filter(|ccmp| ccmp.key_id().value() == 0)
            else {
                self.counters.malformed = self.counters.malformed.saturating_add(1);
                return None;
            };
            let lane = tid.map_or(CcmpReplayLane::NonQos, CcmpReplayLane::Tid);
            let link = self.link_mut(peer)?;
            if link
                .replay
                .commit_immediate(lane, ccmp.packet_number())
                .is_err()
            {
                self.counters.replayed = self.counters.replayed.saturating_add(1);
                return None;
            }
            header + CCMP_HEADER_LEN
        } else {
            header
        };
        let Some(length) = bytes.len().checked_sub(offset) else {
            self.counters.malformed = self.counters.malformed.saturating_add(1);
            return None;
        };
        Some((offset, length))
    }

    /// Check one in-order MPDU and hand its MSDUs on: the only MSDU of an
    /// MPDU in the port's buffer, an A-MSDU's as parts.
    fn deliver_mpdu(
        &mut self,
        peer: [u8; 6],
        frame: PortFrame<PortRxBuffer<X>>,
        deliver: &mut impl FnMut(PortMsdu<'_, PortRxBuffer<X>>),
    ) {
        let bytes = frame.bytes();
        let Some((offset, length)) = self.checked_payload(peer, bytes) else {
            return;
        };
        match plan_data_decapsulation(DataInterfaceRole::AccessPoint, bytes, offset, length) {
            Ok(plan) => {
                let payload = plan.payload_offset..plan.payload_offset + plan.payload_length;
                if bytes.get(payload.clone()).is_none() || plan.ether_type == EAPOL_ETHER_TYPE {
                    return;
                }
                self.counters.delivered = self.counters.delivered.saturating_add(1);
                deliver(PortMsdu::Buffer {
                    buffer: frame.into_buffer(),
                    destination: plan.destination,
                    source: plan.source,
                    ether_type: plan.ether_type,
                    payload,
                });
            }
            Err(DataDecapError::AmsduUnsupported) => {
                self.deliver_parts(bytes, offset, length, deliver)
            }
            Err(_) => self.counters.malformed = self.counters.malformed.saturating_add(1),
        }
    }

    /// Check one MPDU a window kept and hand its MSDUs on as parts.
    fn deliver_stored(
        &mut self,
        peer: [u8; 6],
        bytes: &[u8],
        deliver: &mut impl FnMut(PortMsdu<'_, PortRxBuffer<X>>),
    ) {
        if let Some((offset, length)) = self.checked_payload(peer, bytes) {
            self.deliver_parts(bytes, offset, length, deliver);
        }
    }

    fn deliver_parts(
        &mut self,
        bytes: &[u8],
        offset: usize,
        length: usize,
        deliver: &mut impl FnMut(PortMsdu<'_, PortRxBuffer<X>>),
    ) {
        let Ok(frames) =
            decapsulate_data_frames(DataInterfaceRole::AccessPoint, bytes, offset, length)
        else {
            self.counters.malformed = self.counters.malformed.saturating_add(1);
            return;
        };
        for parts in frames {
            match parts {
                Ok(parts) if parts.ether_type == EAPOL_ETHER_TYPE => {}
                Ok(parts) => {
                    self.counters.delivered = self.counters.delivered.saturating_add(1);
                    deliver(PortMsdu::Parts(parts));
                }
                Err(_) => {
                    self.counters.malformed = self.counters.malformed.saturating_add(1);
                    return;
                }
            }
        }
    }

    /// Answer a peer's Block Ack action: accept an ADDBA Request whose
    /// window the port and the storage can hold (or decline it), end a
    /// receive agreement the peer ends, and apply the peer's ADDBA Response
    /// to, or DELBA of, the access point's own TX agreement.
    async fn block_ack_action(
        &mut self,
        peer: [u8; 6],
        action: BlockAckAction,
    ) -> Result<(), PortApError<PortError<X>>> {
        match action {
            BlockAckAction::AddbaRequest {
                dialog_token,
                tid,
                window,
                starting_sequence,
                ..
            } => {
                let capabilities = self.client.port().capabilities();
                let window = window
                    .min(capabilities.rx_block_ack_max_window)
                    .min(PORT_REORDER_WINDOW as u16);
                let vif = self.client.config().vif;
                let accepted = tid <= capabilities.rx_block_ack_max_tid
                    && window != 0
                    && self.reorder.accept(peer, tid, starting_sequence, window)
                    && if self
                        .client
                        .apply(LowerMacSetting::AddRxBlockAck(RxBlockAckAgreement {
                            vif,
                            peer,
                            tid,
                            start_sequence: starting_sequence,
                            window,
                        }))
                        .is_ok()
                    {
                        true
                    } else {
                        self.reorder.stop(peer, tid);
                        false
                    };
                let mut body = [0_u8; ADDBA_ACTION_BODY_LEN];
                if accepted {
                    self.counters.rx_agreements = self.counters.rx_agreements.saturating_add(1);
                    write_successful_addba_response(&mut body, dialog_token, tid, window)
                } else {
                    write_declined_addba_response(&mut body, dialog_token, tid & 0x0f, window)
                }
                .map_err(|_| PortApError::Service(ApServiceError::WrongPeerPhase))?;
                self.send_action(peer, &body).await.map(|_| ())
            }
            BlockAckAction::Delba {
                tid,
                initiator: true,
                ..
            } => self.stop_rx_agreement(peer, tid),
            action @ (BlockAckAction::AddbaResponse { .. }
            | BlockAckAction::Delba {
                initiator: false, ..
            }) => {
                match self.service.on_tx_block_ack_action(peer, action)? {
                    Some(TxBlockAckResponse::Operational(_)) => {
                        self.counters.tx_agreements = self.counters.tx_agreements.saturating_add(1);
                    }
                    Some(TxBlockAckResponse::Rejected(_)) => {
                        self.counters.tx_agreements_failed =
                            self.counters.tx_agreements_failed.saturating_add(1);
                    }
                    None => {}
                }
                Ok(())
            }
        }
    }

    /// Send one Block Ack action `body` to `peer`; whether the peer
    /// acknowledged it.
    async fn send_action(
        &mut self,
        peer: [u8; 6],
        body: &[u8],
    ) -> Result<bool, PortApError<PortError<X>>> {
        let sequence_number = self.service.next_management_sequence();
        let mut frame = [0_u8; 64];
        let length = ApActionFrame {
            access_point: self.service.address(),
            peer,
            sequence_number,
            body,
        }
        .encode(&mut frame)?;
        let report = self
            .client
            .transmit(
                TxMpdu::whole(&frame[..length]),
                KeySelector::Plaintext,
                WmmAccessCategory::Voice,
                self.profile.management_rate,
                self.profile.coex,
            )
            .await?;
        Ok(matches!(report, TxReport::Mpdu(status) if status.acknowledged == Some(true)))
    }

    /// Queue a newly authorized `peer`'s TX Block Ack offer with the
    /// profile's retry policy, where the BSS is protected and the peer an
    /// HT QoS station, and send its first attempt at once.
    async fn offer_tx_block_ack(
        &mut self,
        peer: [u8; 6],
        now: Instant,
    ) -> Result<(), PortApError<PortError<X>>> {
        if self
            .service
            .queue_tx_block_ack(peer, self.profile.tx_block_ack_retry)?
        {
            self.send_due_tx_block_ack_offer(peer, now).await?;
        }
        Ok(())
    }

    /// Send `peer`'s TX Block Ack offer when one is due at `now`: an ADDBA
    /// Request, whose attempt a missing acknowledgement ends, the next one
    /// due after the retry interval.
    async fn send_due_tx_block_ack_offer(
        &mut self,
        peer: [u8; 6],
        now: Instant,
    ) -> Result<(), PortApError<PortError<X>>> {
        let Some(request) = self.service.take_tx_block_ack_offer(peer, now)? else {
            return Ok(());
        };
        self.counters.tx_agreements_offered = self.counters.tx_agreements_offered.saturating_add(1);
        if !self.send_action(peer, &request.body).await? {
            self.service.tx_block_ack_offer_failed(peer, now)?;
            self.counters.tx_agreements_failed =
                self.counters.tx_agreements_failed.saturating_add(1);
        }
        Ok(())
    }

    /// End at most one TX Block Ack negotiation whose response is overdue at
    /// `now`, its next attempt due after the retry interval while attempts
    /// remain; `true` when one was due.
    fn expire_tx_block_ack(&mut self, now: Instant) -> bool {
        let expired = self.service.expire_tx_block_ack(now).is_some();
        if expired {
            self.counters.tx_agreements_failed =
                self.counters.tx_agreements_failed.saturating_add(1);
        }
        expired
    }

    /// End `peer`'s receive agreement of `tid` here and in the port.
    fn stop_rx_agreement(
        &mut self,
        peer: [u8; 6],
        tid: u8,
    ) -> Result<(), PortApError<PortError<X>>> {
        if self.reorder.stop(peer, tid) {
            let vif: VifId = self.client.config().vif;
            self.client
                .apply(LowerMacSetting::RemoveRxBlockAck { vif, peer, tid })?;
        }
        Ok(())
    }

    /// End every receive agreement of `peer`.
    fn stop_rx_agreements(&mut self, peer: [u8; 6]) -> Result<(), PortApError<PortError<X>>> {
        let mut tids = [None; 16];
        for (slot, tid) in tids.iter_mut().zip(self.reorder.agreements(peer)) {
            *slot = Some(tid);
        }
        for tid in tids.into_iter().flatten() {
            self.stop_rx_agreement(peer, tid)?;
        }
        Ok(())
    }

    /// Release the buffered run of every window whose gap timed out, then
    /// time the gaps of the windows that keep an MPDU.
    fn expire_reorder_gaps(
        &mut self,
        now: Instant,
        deliver: &mut impl FnMut(PortMsdu<'_, PortRxBuffer<X>>),
    ) {
        // The window's peer is the release's; releases of a removed peer
        // were dropped with its agreements.
        while let Some((peer, release)) = self.expire_due_gap(now) {
            self.counters.reorder_gap_timeouts =
                self.counters.reorder_gap_timeouts.saturating_add(1);
            self.release(peer, &release, None, deliver);
        }
        self.reorder.arm_gaps(now, self.profile.rx_reorder_gap);
    }

    fn expire_due_gap(&mut self, now: Instant) -> Option<([u8; 6], ReorderRelease)> {
        self.reorder.expire_due_gap(now)
    }

    /// Serve one EAPOL-Key frame of a peer in its handshake: answer
    /// Message 2 with Message 3; on Message 4 install the peer's pairwise
    /// key and authorize it; close a peer the handshake rejected. Every
    /// other data frame, and EAPOL of a peer outside its handshake, is
    /// ignored.
    async fn receive_eapol(
        &mut self,
        mpdu: &[u8],
        now: Instant,
    ) -> Result<(), PortApError<PortError<X>>> {
        let header = if mpdu[0] & 0x80 != 0 {
            IEEE80211_QOS_DATA_HEADER_LEN
        } else {
            IEEE80211_LEGACY_DATA_HEADER_LEN
        };
        let Some(length) = mpdu.len().checked_sub(header) else {
            return Ok(());
        };
        let Ok(plan) =
            plan_data_decapsulation(DataInterfaceRole::AccessPoint, mpdu, header, length)
        else {
            return Ok(());
        };
        let peer = plan.source;
        if plan.ether_type != EAPOL_ETHER_TYPE
            || plan.destination != self.service.address()
            || !self
                .service
                .peer_status(peer)
                .is_some_and(|status| status.phase == ApPeerPhase::Securing)
        {
            return Ok(());
        }
        let Some(payload) =
            mpdu.get(plan.payload_offset..plan.payload_offset + plan.payload_length)
        else {
            return Ok(());
        };
        let Ok(frame) = OwnedEapolFrame::<RSN_HANDSHAKE_EAPOL_CAPACITY>::try_copy(
            RsnInterface::AccessPoint,
            peer,
            payload,
        ) else {
            return Ok(());
        };
        match self.service.on_eapol(peer, frame)? {
            ApWpa2Progress::None => Ok(()),
            ApWpa2Progress::Transmit(frame) => self.send_eapol(peer, &frame, false).await,
            ApWpa2Progress::AuthorizePeer => {
                self.authorize(peer, now)?;
                self.offer_tx_block_ack(peer, now).await
            }
            ApWpa2Progress::DeauthenticatePeer => {
                let close = self.service.begin_wpa2_failure_close(peer)?;
                self.close_peer(close).await
            }
        }
    }

    /// Install the verified handshake's pairwise key in the port, on the
    /// link the peer's association opened, then open the peer's controlled
    /// port.
    fn authorize(&mut self, peer: [u8; 6], now: Instant) -> Result<(), PortApError<PortError<X>>> {
        if self.link_mut(peer).is_none() {
            return Err(PortApError::Service(ApServiceError::UnknownPeer));
        }
        let key = *self.service.pending_ptk(peer)?.temporal_key();
        let handle = self.install_key(KeyScope::Pairwise { peer }, &key)?;
        if let Some(link) = self.link_mut(peer) {
            link.key = Some(handle);
        }
        self.service.authorize(peer, now)?;
        self.counters.handshakes = self.counters.handshakes.saturating_add(1);
        Ok(())
    }

    /// Close a peer the service began closing: a Disassociation when it was
    /// associated, then a Deauthentication, then its removal.
    async fn close_peer(&mut self, close: ApPeerClose) -> Result<(), PortApError<PortError<X>>> {
        if close.was_associated {
            let reason = if close.kind == ApPeerCloseKind::InactivityTimeout {
                REASON_INACTIVITY
            } else {
                REASON_AUTHENTICATION_INVALID
            };
            self.send_disconnect(close.peer, ApPeerDisconnectKind::Disassociation, reason)
                .await?;
        }
        self.send_disconnect(
            close.peer,
            ApPeerDisconnectKind::Deauthentication,
            REASON_AUTHENTICATION_INVALID,
        )
        .await?;
        self.remove_peer(close.peer)?;
        self.counters.peers_closed = self.counters.peers_closed.saturating_add(1);
        Ok(())
    }

    async fn send_disconnect(
        &mut self,
        peer: [u8; 6],
        kind: ApPeerDisconnectKind,
        reason: u16,
    ) -> Result<(), PortApError<PortError<X>>> {
        let sequence = self.service.next_management_sequence();
        let mut frame = [0; AP_BEACON_CAPACITY];
        let length = write_ap_peer_disconnect(
            &mut frame,
            self.service.address(),
            peer,
            kind,
            reason,
            sequence,
        )?;
        self.send_management(&frame[..length]).await
    }
}
