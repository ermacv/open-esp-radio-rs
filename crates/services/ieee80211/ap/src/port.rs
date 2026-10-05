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
    AP_MAX_CLIENTS, AccessPointService, ApAssociationCapabilities, ApMlmeAction, ApPeerClose,
    ApPeerCloseKind, ApPeerPhase, ApServiceError, ApWpa2Error, ApWpa2Progress, ApWpa2RetryProgress,
    beacon::ApBeacon,
    sae::{ApSaeFrame, ApSaeOutput, ApSaeRandom, ApSaeResponder, ApSaeResult},
};
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
    protection::ApBssProtection,
    qos::{WmmAccessCategory, WmmUserPriority},
    security::{ApSecurityPolicy, LinkProtection},
    sequence::SequenceNumber,
    ssid::WifiSsid,
    tsf::TsfInstant,
};
use oer_ieee80211_rsn::{
    OwnedEapolFrame, Pmk, RsnInterface, frames::RsnTxFrame, runner::RSN_HANDSHAKE_EAPOL_CAPACITY,
};
use oer_ieee80211_upper_mac::TxReport;
use oer_ieee80211_upper_mac_service::{
    EventRouter,
    client::{
        PortClient, PortClientEnv, PortClientError, PortError, PortFrame, PortInput, PortMsdu,
        PortRxBuffer,
    },
    queue::TxQueue,
};
use oer_time::{Clock, Duration, Instant, Timer};

use oer_ieee80211_ap::{
    ApBufferedUnicastRelease, ApDownlinkDisposition, ApPeerPowerState, ApPowerSaveAction,
};
use oer_ieee80211_mac::ap::{ApPowerSaveObservation, observe_ap_power_save_for_access_point};
use oer_ieee80211_mac::beacon::dtim;
use oer_ieee80211_upper_mac_service::queue::PORT_FRAME_CAPACITY;

use oer_ieee80211_lower_mac::{RxBlockAckAgreement, VifId};
use oer_ieee80211_mac::ap::ApActionFrame;
use oer_ieee80211_mac::block_ack::{
    ADDBA_ACTION_BODY_LEN, BlockAckAction, write_declined_addba_response,
    write_successful_addba_response,
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
}

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
    /// The rate of beacons, management frames and group data.
    pub management_rate: PhyRate,
    /// The rate of data to an associated peer.
    pub data_rate: PhyRate,
    pub coex: CoexPriority,
    /// The step of the CCMP packet numbers the access point sends under.
    pub ccmp_step: CcmpPacketNumberStep,
    /// How long a receive reorder window keeps an MPDU behind a missing
    /// one before it releases its run past the gap.
    pub rx_reorder_gap: Duration,
}

/// The memory of one access point that the composition places: its beacon
/// template, its transmit queue, the `HELD` frames it holds for power save
/// (for every dozing peer and the next DTIM together; the composition sizes
/// it for its peers' traffic and its memory) and its peers' receive
/// reordering.
pub struct PortApStorage<const HELD: usize> {
    beacon: [u8; AP_BEACON_CAPACITY],
    queue: TxQueue,
    held: [Option<HeldFrame>; HELD],
    /// The peers' receive Block Ack agreements, one per peer at most on
    /// average, and their kept MPDUs.
    reorder: RxReorder<AP_MAX_CLIENTS>,
}

impl<const HELD: usize> PortApStorage<HELD> {
    pub const fn new() -> Self {
        Self {
            beacon: [0; AP_BEACON_CAPACITY],
            queue: TxQueue::new(),
            held: [const { None }; HELD],
            reorder: RxReorder::new(),
        }
    }
}

/// One frame held for power save: an Ethernet-II frame for a dozing peer,
/// or for the group until the next DTIM.
pub struct HeldFrame {
    order: u32,
    ethernet: [u8; PORT_FRAME_CAPACITY],
    len: usize,
}

impl HeldFrame {
    fn ethernet(&self) -> &[u8] {
        &self.ethernet[..self.len]
    }

    fn destination(&self) -> [u8; 6] {
        let mut destination = [0; 6];
        destination.copy_from_slice(&self.ethernet[..6]);
        destination
    }

    fn is_group(&self) -> bool {
        self.ethernet[0] & 1 != 0
    }
}

/// The frames held for power save, shared by every dozing peer and the
/// group, each released oldest first for its destination.
struct PowerSaveBuffer<'p> {
    frames: &'p mut [Option<HeldFrame>],
    next_order: u32,
}

impl<'p> PowerSaveBuffer<'p> {
    fn new(frames: &'p mut [Option<HeldFrame>]) -> Self {
        frames.iter_mut().for_each(|slot| *slot = None);
        Self {
            frames,
            next_order: 0,
        }
    }

    /// Hold `ethernet`; `false` when every slot is taken.
    fn hold(&mut self, ethernet: &[u8]) -> bool {
        let Some(slot) = self.frames.iter_mut().find(|slot| slot.is_none()) else {
            return false;
        };
        if ethernet.len() > PORT_FRAME_CAPACITY || ethernet.len() < 6 {
            return false;
        }
        let mut held = HeldFrame {
            order: self.next_order,
            ethernet: [0; PORT_FRAME_CAPACITY],
            len: ethernet.len(),
        };
        held.ethernet[..ethernet.len()].copy_from_slice(ethernet);
        *slot = Some(held);
        self.next_order = self.next_order.wrapping_add(1);
        true
    }

    /// Take the oldest frame `matches` selects.
    fn take_oldest(&mut self, matches: impl Fn(&HeldFrame) -> bool) -> Option<HeldFrame> {
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

impl<const HELD: usize> Default for PortApStorage<HELD> {
    fn default() -> Self {
        Self::new()
    }
}

/// The outcome of offering one frame for transmission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PortApSend {
    /// The frame waits in the transmit queue, which `run_until` sends.
    Queued,
    /// The transmit queue is full, or the frame exceeds its capacity.
    Full,
}

/// A peer's link: its pairwise key, the CCMP packet numbers sent to it and
/// received from it, and its duplicate filter.
struct PeerLink {
    peer: [u8; 6],
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
    /// WPA3 BSS, Block Ack actions.
    pub unserved: u32,
    /// EAPOL-Key frames sent, retransmissions included.
    pub eapol_sent: u32,
    /// SAE frames handed to the executor.
    pub sae_frames: u32,
    /// SAE frames dropped while the executor was busy.
    pub sae_dropped: u32,
    /// Stations SAE authenticated.
    pub sae_accepted: u32,
    /// Data MPDUs sent to a peer or the group.
    pub data_sent: u32,
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
    /// The link of each authorized peer.
    links: [Option<PeerLink>; AP_MAX_CLIENTS],
    /// The packet numbers of group data.
    group_transmit: CcmpTxPacketNumber,
    queue: &'p mut TxQueue,
    buffered: PowerSaveBuffer<'p>,
    reorder: &'p mut RxReorder<AP_MAX_CLIENTS>,
    /// The protection the beacon template carries.
    advertised: ApBssProtection,
    /// Before it no Probe Response goes out.
    next_probe_response: Instant,
    counters: PortApCounters,
}

impl<'p, X: PortApEnv> PortAccessPoint<'p, X> {
    /// An access point of `profile` and `service` over `client`, its beacon
    /// template in `storage`.
    pub fn new<const HELD: usize>(
        client: PortApClient<'p, X>,
        timer: X::Timer,
        authenticator: X::Authenticator,
        sae: X::Sae,
        profile: PortApProfile<'p>,
        service: AccessPointService<'p>,
        storage: &'p mut PortApStorage<HELD>,
    ) -> Result<Self, PortApBuildError> {
        let PortApStorage {
            beacon,
            queue,
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
            group_transmit: CcmpTxPacketNumber::new(profile.ccmp_step),
            queue,
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

    fn link_mut(&mut self, peer: [u8; 6]) -> Option<&mut PeerLink> {
        self.links
            .iter_mut()
            .flatten()
            .find(|link| link.peer == peer)
    }

    /// Open `peer`'s link, under `key` in a protected BSS.
    fn open_link(
        &mut self,
        peer: [u8; 6],
        key: Option<KeyHandle>,
    ) -> Result<(), PortApError<PortError<X>>> {
        let Some(slot) = self.links.iter_mut().find(|slot| slot.is_none()) else {
            return Err(PortApError::KeysFull);
        };
        *slot = Some(PeerLink {
            peer,
            key,
            transmit: CcmpTxPacketNumber::new(self.profile.ccmp_step),
            replay: CcmpRxReplayState::default(),
            duplicates: RxDuplicateFilter::new(),
        });
        Ok(())
    }

    /// Close `peer`'s link, removing its pairwise key from the port.
    fn remove_pairwise_key(&mut self, peer: [u8; 6]) -> Result<(), PortApError<PortError<X>>> {
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

    /// Queue one Ethernet-II frame for its destination, an authorized peer
    /// or the group; `run_until` sends it.
    pub fn send(&mut self, ethernet: &[u8], user_priority: WmmUserPriority) -> PortApSend {
        if self.queue.push(ethernet, user_priority) {
            PortApSend::Queued
        } else {
            PortApSend::Full
        }
    }

    /// Whether the transmit queue has room.
    pub const fn can_queue(&self) -> bool {
        !self.queue.is_full()
    }

    /// Forget a peer: its pairwise key, its SAE session, the frames held for
    /// it, then its state.
    fn remove_peer(&mut self, peer: [u8; 6]) -> Result<(), PortApError<PortError<X>>> {
        self.remove_pairwise_key(peer)?;
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
            if let Some(frame) = self.queue.pop() {
                self.dispatch(frame.ethernet()).await?;
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
            ]
            .into_iter()
            .flatten()
            .fold(deadline, Instant::min);
            let woken = {
                let Self {
                    client, timer, sae, ..
                } = self;
                let mut input = pin!(client.next_input(&*timer, wake));
                poll_fn(|context| {
                    if let Poll::Ready((peer, output)) = sae.poll_output(context) {
                        return Poll::Ready(Wake::Sae(peer, output));
                    }
                    input.as_mut().poll(context).map(Wake::Input)
                })
                .await
            };
            match woken {
                Wake::Sae(peer, output) => self.sae_output(peer, output, now).await?,
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
                frame,
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
                frame,
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
            self.transmit_data(held.ethernet(), release.more_data())
                .await?;
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
        self.transmit_data(held.ethernet(), release.more_data())
            .await?;
        self.counters.released = self.counters.released.saturating_add(1);
        self.service
            .complete_buffered_unicast_release(release, true)?;
        Ok(())
    }

    /// Send one queued frame now, or hold it for a dozing peer or for the
    /// next DTIM while any authorized peer dozes.
    async fn dispatch(&mut self, ethernet: &[u8]) -> Result<(), PortApError<PortError<X>>> {
        let Some(destination) = ethernet
            .get(..6)
            .and_then(|bytes| <[u8; 6]>::try_from(bytes).ok())
        else {
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
            return self.transmit_data(ethernet, false).await;
        };
        if !self.buffered.hold(ethernet) {
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
                self.associate(peer, security, capabilities, now).await
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
                    self.remove_pairwise_key(peer)?;
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
            self.remove_pairwise_key(peer)?;
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
        // An Open BSS's peer is authorized by its association.
        if !protected && !repeated && association_id != 0 && self.link_mut(peer).is_none() {
            self.open_link(peer, None)?;
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
                &mpdu[..length],
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

    /// Send one queued frame: to an authorized peer under its pairwise key,
    /// or to the group under the group key, as QoS data to a QoS peer in a
    /// protected BSS. A frame for no authorized destination is dropped.
    async fn transmit_data(
        &mut self,
        ethernet: &[u8],
        more_data: bool,
    ) -> Result<(), PortApError<PortError<X>>> {
        let Some(destination) = ethernet
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
        let mut mpdu = [0; oer_ieee80211_upper_mac_service::queue::PORT_FRAME_CAPACITY + 64];
        let (length, key, rate) = if self.service.link_protection() == LinkProtection::Open {
            let sequence_number = self.service.current_data_sequence();
            let length = ApUnprotectedDataFrame {
                access_point,
                peer: destination,
                sequence_number,
                more_data,
                ethernet,
            }
            .encode(&mut mpdu)?;
            self.service.next_data_sequence();
            let rate = if group {
                self.profile.management_rate
            } else {
                self.profile.data_rate
            };
            (length, KeySelector::Plaintext, rate)
        } else {
            let peer_qos = !group
                && self
                    .service
                    .peer_status(destination)
                    .is_some_and(|status| status.qos_supported);
            let sequence_number = if peer_qos {
                self.service
                    .current_qos_sequence(destination, oer_ieee80211_ap::AP_TX_BLOCK_ACK_TID)
                    .ok_or(PortApError::Service(ApServiceError::UnknownPeer))?
            } else {
                self.service.current_data_sequence()
            };
            // The frame encodes before a packet number is spent.
            let length = ApProtectedDataFrame {
                access_point,
                peer: destination,
                sequence_number,
                user_priority: 0,
                peer_qos,
                more_data,
                ccmp_header: [0; CCMP_HEADER_LEN],
                ethernet,
            }
            .encode(&mut mpdu)?;
            let (header, key, rate) = if group {
                let key_id = self.service.gtk()?.key_id();
                let key = self.group_key.ok_or(PortApError::KeysFull)?;
                let header = self
                    .group_transmit
                    .next_header(CcmpKeyId::new(key_id).ok_or(PortApError::KeysFull)?)
                    .map_err(|_| PortApError::PacketNumbers)?;
                (header, key, self.profile.management_rate)
            } else {
                let data_rate = self.profile.data_rate;
                let link = self.link_mut(destination).ok_or(PortApError::KeysFull)?;
                let key = link.key.ok_or(PortApError::KeysFull)?;
                let header = link
                    .transmit
                    .next_header(CcmpKeyId::new(0).ok_or(PortApError::KeysFull)?)
                    .map_err(|_| PortApError::PacketNumbers)?;
                (header, key, data_rate)
            };
            let offset = if peer_qos {
                IEEE80211_QOS_DATA_HEADER_LEN
            } else {
                IEEE80211_LEGACY_DATA_HEADER_LEN
            };
            mpdu[offset..offset + CCMP_HEADER_LEN].copy_from_slice(&header);
            if peer_qos {
                self.service
                    .next_qos_sequence(destination, oer_ieee80211_ap::AP_TX_BLOCK_ACK_TID);
            } else {
                self.service.next_data_sequence();
            }
            (length, KeySelector::Key(key), rate)
        };
        self.client
            .transmit(
                &mpdu[..length],
                key,
                WmmAccessCategory::BestEffort,
                rate,
                self.profile.coex,
            )
            .await?;
        self.counters.data_sent = self.counters.data_sent.saturating_add(1);
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
        let Some(link) = self.link_mut(peer) else {
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
    /// window the port and the storage can hold (or decline it), and end an
    /// agreement the peer ends. The access point's own TX agreements are
    /// not served yet.
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
                let sequence_number = self.service.next_management_sequence();
                let mut frame = [0_u8; 64];
                let length = ApActionFrame {
                    access_point: self.service.address(),
                    peer,
                    sequence_number,
                    body: &body,
                }
                .encode(&mut frame)?;
                self.send_management(&frame[..length]).await
            }
            BlockAckAction::Delba {
                tid,
                initiator: true,
                ..
            } => self.stop_rx_agreement(peer, tid),
            _ => {
                self.counters.unserved = self.counters.unserved.saturating_add(1);
                Ok(())
            }
        }
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
            ApWpa2Progress::AuthorizePeer => self.authorize(peer, now),
            ApWpa2Progress::DeauthenticatePeer => {
                let close = self.service.begin_wpa2_failure_close(peer)?;
                self.close_peer(close).await
            }
        }
    }

    /// Install the verified handshake's pairwise key in the port, then open
    /// the peer's controlled port.
    fn authorize(&mut self, peer: [u8; 6], now: Instant) -> Result<(), PortApError<PortError<X>>> {
        if self.links.iter().all(Option::is_some) {
            return Err(PortApError::KeysFull);
        }
        let key = *self.service.pending_ptk(peer)?.temporal_key();
        let handle = self.install_key(KeyScope::Pairwise { peer }, &key)?;
        self.open_link(peer, Some(handle))?;
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
