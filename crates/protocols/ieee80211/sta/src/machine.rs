//! The station: one sans-IO machine for the whole lifecycle of an
//! infrastructure station, from its scan to its disconnect ([owner
//! decision on #563]).
//!
//! The machine takes the port's events, the channel arbiter's answers and
//! notices, its commands and time as [`StaInput`] values, and leaves what
//! the port and the arbiter must do as [`StaAction`] values, in their own
//! terms (`oer-ieee80211-lower-mac`, `oer-ieee80211-arbiter`): a thin
//! service loop feeds it and carries its actions out, and holds no logic
//! of its own:
//!
//! ```text
//! loop {
//!     select(phase.next_event(), timer(deadline), commands)
//!     station.input(now, input)
//!     while let Some(action) = station.poll_action(now) { carry it out }
//! }
//! ```
//!
//! # Tokens
//!
//! The machine is generic over its tokens ([`StaTokens`]): the transmit
//! tickets and buffers the composition hands it at its start, minted by
//! the port, its key slots, and the arbiter's [`LeasePlace`]. A token
//! leaves in an action by value and comes back in an input, so what the
//! machine holds is what it may do, and a host test plays the port and the
//! arbiter with tokens of its own. The station encodes each frame straight
//! into the memory of its buffer token ([`TxBufferMemory`]).
//! Management requests use the configured station address as transmitter;
//! the selected BSSID identifies the receiver and the BSS.
//!
//! Station owns keys and their free slots across protocol transitions. Install
//! actions carry private affine pending tokens (key ID and link epoch for a
//! group key); responses return those tokens with the keys. A stale install
//! is removed normally. Group slots are fungible: removal returns a raw slot
//! to the free pool, whose fullness proves that every group key came back.
//!
//! # Channels
//!
//! The station never changes the port's channel: it asks the arbiter for a
//! lease of each channel it needs ([`StaAction::RequestChannel`]), listens
//! and transmits there only between the lease's start and its end, and
//! gives it back. Beside an access point, its scan goes in absences. A
//! scan the application asked for ends with a partial result after
//! [`StaConfig::skipped_absences`] consecutive cancelled leases. Among the
//! network's access points, one on the channel of the access point beside
//! the station wins within [`StaConfig::access_point_margin_db`] of the
//! strongest, so a join rarely moves that access point ([follow
//! decision]).
//!
//! A connected station's background scan visits each other channel in an
//! absence announced with a Null (PM=1), and its network's own channel
//! without one: it widens its receive rules to the other networks'
//! management frames for the dwell, then narrows them again and waits for
//! the port to confirm ([follow decision]).
//!
//! # States
//!
//! Its state is an enum whose variants hold their own types; a transition
//! takes the state by value and returns the next one, so a value of one
//! state is never read in another. Each state names the action it owes,
//! so [`Station::poll_action`] hands it out and moves to the state that
//! awaits its end: there is no queue of actions to overflow.
//!
//! [owner decision on #563]: https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6128926225
//! [follow decision]: https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6130775558

use core::marker::PhantomData;
use oer_ieee80211_mac::ccmp::GroupKeyId;

use oer_ieee80211_arbiter::{
    AbsenceAnswer, AbsencePlace, Answer, ChannelRequest, Lease, LeaseId, LeasePlace, LeaveReason,
    Purpose, Refusal, StationNotice, UpstreamFact,
};
use oer_ieee80211_lower_mac::{
    AckHeader, GroupAddress, HeaderSpec, Ieee80211Capabilities, MpduLength, ReceiveFilter,
    ReturnedMpdu, RxEvidence, RxMetadata, SizedFrame, Station as StationRole, StationReceive,
    SupportedChannel, SupportedFilter, TxBufferMemory, TxEndStatus, TxOutcome,
};
use oer_ieee80211_mac::address::{Bssid, IndividualAddress, OwnAddress};
use oer_ieee80211_mac::ccmp::CcmpRxReplayEpoch;
use oer_ieee80211_mac::{
    management::{MAX_SSID_LEN, ProbeRequest},
    scan::{ScanRecord, ScanTable},
    security::StaSecurityPolicy,
    sequence::SequenceNumber,
    station::{
        AssociationCapabilities, AssociationRequest, OpenAuthenticationRequest, SelectedRsn,
        StaManagementFrame, StaManagementSubtype, StaSequenceCounter, association::PhyMode,
        select_association_rsn,
    },
    station_beacon::parse_sta_beacon,
    station_power_save::{StaNullDataFrame, StaPowerManagement},
};
use oer_ieee80211_rsn::{
    DEFAULT_EAPOL_FRAME_CAPACITY, OwnedEapolFrame, Pmk, RsnInterface,
    aes::software_aes128_key_unwrap,
    frames::RsnTxFrame,
    keys::{RsnKeyInstall, RsnKeyKind},
    supplicant::{
        RsnConnectedAction, RsnConnectedSupplicant, RsnGroupKeyInstallRequest, RsnStaDeadlineEvent,
        RsnStaKeyInstallRequest, RsnStaResponseDeadline, RsnStaResponseWait, RsnStaSupplicant,
        RsnStaSupplicantAction,
    },
};
use oer_ieee80211_upper_mac::HePacketPadding;

use crate::association::StaAssociatedPeer;
use oer_time::{Duration, Instant};

use crate::epoch::{LinkEpoch, LinkEpochSource};
use crate::pool::Pool;
use oer_radio_port::{PoolPath, Root};
mod group_keys;
use crate::power_save::{NullOutcome, PowerSaveDecision, PowerSaveView};
use group_keys::{GroupKeys, GroupPlace, Installed as InstalledGroup};

use crate::join::{
    association::{StaAssociationEvent, StaAssociationPoll, StaAssociationRuntime},
    authentication::{StaAuthenticationEvent, StaAuthenticationExchange, StaAuthenticationRuntime},
};

/// A refusal before admission, delivered by the service to the owning machine.
/// It is not a radio terminal and never updates transmission statistics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxRefusal {
    /// A data operation's connection epoch or protection no longer matches
    /// the station's link. Its original resources return without submission.
    LinkChanged,
    /// Service rate selection found no rate for the current phase.
    NoRate,
    /// The pool refused preparation and returned the unchanged source.
    Preparation(oer_ieee80211_lower_mac::PreparationError),
    /// Portable placement failed before entering the backend.
    Layout(oer_ieee80211_lower_mac::LayoutError),
}

/// The tokens a station holds: the port's, or a host test's.
pub trait StaTokens {
    /// Component owning this station's connection epoch source.
    type EpochPath: PoolPath;
    /// The checked size class of management frames.
    type Small: oer_ieee80211_lower_mac::TxBufferClass;
    /// A transmit ticket: a place in flight and a place for its terminal.
    type Ticket;
    /// A transmit buffer of the small class, for management frames: its
    /// memory, which the station encodes into.
    type SmallBuffer: TxBufferMemory;
    /// An immutable published MPDU, including its body and original description.
    type Sent;
    /// Release a completed MPDU for a new frame, ending its retained body owner.
    fn renew(mpdu: Self::Sent) -> Self::SmallBuffer;
    /// A pairwise key slot.
    type PairwiseSlot;
    /// A unique, fungible group slot retained by the backend until removal.
    type GroupSlot;
    /// An installed pairwise key, which holds its slot.
    type PairwiseKey;
    /// An installed group key, which holds its slot.
    type GroupKey;
    /// The metadata the port gives a received frame.
    type RxMeta: RxMetadata;
}

/// How the station protects its link.
pub enum StaSecurity {
    /// An open network.
    Open,
    /// WPA2-Personal with the network's PMK, which the composition derives
    /// from the passphrase outside the station's step.
    Wpa2Personal {
        /// The pairwise master key.
        pmk: Pmk,
    },
}

/// What the application asks of the station.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaCommand {
    /// Connect to the configured network.
    Connect,
    /// Leave the network and stay idle.
    Disconnect,
    /// Scan the channel list while connected.
    BackgroundScan,
}

/// One input of the station.
pub enum StaInput<'id, T: StaTokens, K> {
    /// A command of the application.
    Command(StaCommand),
    /// A deadline the station named passed.
    Tick,
    /// The terminal of a transmission: its ticket and buffer come back.
    TxDone {
        /// The original operation, returned by the port.
        context: StaTxContext<'id, T::EpochPath>,
        /// The transmission's ticket.
        ticket: T::Ticket,
        /// The transmission's buffer.
        buffer: ReturnedMpdu<T::SmallBuffer, T::Sent>,
        /// How it ended.
        completion: TxOutcome,
    },
    /// A frame the service did not send (no rate of its plan is on the
    /// port's channel, or it does not fit its buffer class): its ticket and
    /// buffer come back with a typed refusal. This is not an accepted TX
    /// completion and cannot complete the handshake.
    Unsent {
        /// Refused before admission; no transmission terminal exists.
        reason: TxRefusal,
        /// The operation that was not submitted.
        context: StaTxContext<'id, T::EpochPath>,
        /// The frame's ticket.
        ticket: T::Ticket,
        /// The frame's buffer.
        buffer: ReturnedMpdu<T::SmallBuffer, T::Sent>,
    },
    /// The same-step answer consumes the request's unique LeasePlace, either
    /// returning it on refusal or transferring its ownership into a Lease.
    Answer(Answer<K>),
    /// The same-step answer returns the unique AbsencePlace or its Lease.
    /// Neither resource is cloneable or constructible by the service.
    AbsenceAnswer(AbsenceAnswer<K>),
    /// The primary lease of the channel the port moved to, which the
    /// arbiter exchanged for the old one. Exchange consumes the old affine
    /// Lease and returns its replacement in the same service step.
    Exchanged(Lease<K>),
    /// A notice of the arbiter.
    Arbiter(StationNotice<K>),
    /// The receive configuration the station set took effect.
    Configured(PendingConfiguration<'id, T::EpochPath>),
    /// The pairwise key the station asked to install, installed.
    PairwiseKeyInstalled {
        /// The original installation action.
        pending: PendingPairwiseKey<'id, T::EpochPath>,
        /// The installed key.
        key: T::PairwiseKey,
    },
    /// The group key the station asked to install, installed under the
    /// index its action named.
    GroupKeyInstalled {
        /// The installed key.
        key: T::GroupKey,
        /// The original installation action, including epoch and key index.
        pending: PendingGroupKey<'id, T::EpochPath>,
    },
    /// The FIFO replacement terminal for the pending operation's slot.
    GroupKeyReplaced {
        /// The private successor epoch and installation record.
        pending: PendingGroupReplacement<'id, T::EpochPath>,
        /// The successor key, retaining the same physical slot.
        key: T::GroupKey,
    },
    /// A removed pairwise key's slot.
    PairwiseKeyRemoved(T::PairwiseSlot),
    /// A removed group key's slot.
    GroupKeyRemoved(T::GroupSlot),
}

/// One action of the station, which the service carries out at once.
/// Discarding a produced action is diagnosed by `unused_must_use`:
/// ```compile_fail
/// #![deny(unused_must_use)]
/// use oer_ieee80211_sta::machine::{StaAction, StaTokens};
/// fn ignore<'a, 'id, T: StaTokens, K: 'a>(next: impl FnOnce() -> StaAction<'a, 'id, T, K>)
/// where T::PairwiseKey: 'a {
///     next();
/// }
/// ```
/// The consumer may transfer it to the component that performs it:
/// ```
/// #![deny(unused_must_use)]
/// use oer_ieee80211_sta::machine::{StaAction, StaTokens};
/// fn forward<'a, 'id, T: StaTokens, K: 'a>(next: impl FnOnce() -> StaAction<'a, 'id, T, K>)
///     -> StaAction<'a, 'id, T, K>
/// where T::PairwiseKey: 'a {
///     next()
/// }
/// ```
#[must_use = "carry out the action or return its owners through a terminal path"]
pub enum StaAction<'a, 'id, T: StaTokens, K> {
    /// Ask the arbiter for a channel, with the station's place; the service
    /// hands the answer back ([`StaInput::Answer`]) in the same step.
    RequestChannel(ChannelRequest<K>, LeasePlace),
    /// Ask the arbiter for an absence from the primary lease `home`, with
    /// the absence's place; the service hands the answer back
    /// ([`StaInput::AbsenceAnswer`]) in the same step.
    RequestAbsence {
        /// The absence's channel and purpose.
        request: ChannelRequest<K>,
        /// The running primary lease.
        home: &'a Lease<K>,
        /// The absence's place.
        place: AbsencePlace,
    },
    /// Tell the arbiter the station is ready for its absence: its access
    /// point knows it leaves.
    Ready(LeaseId),
    /// Exchange the primary lease after the port moved; the service hands
    /// the new one back ([`StaInput::Exchanged`]).
    ExchangeLease(Lease<K>),
    /// Give a lease back to the arbiter.
    ReleaseChannel(Lease<K>),
    /// Tell the arbiter a fact about the station's upstream.
    Upstream(UpstreamFact<K>),
    /// Set what the station's interface receives ([`StaInput::Configured`]
    /// follows).
    Configure(StationReceive<K>, PendingConfiguration<'id, T::EpochPath>),
    /// Install a pairwise key ([`StaInput::PairwiseKeyInstalled`]).
    InstallPairwiseKey {
        /// The response must return this token.
        pending: PendingPairwiseKey<'id, T::EpochPath>,
        /// The slot it takes.
        slot: T::PairwiseSlot,
        /// The access point.
        peer: IndividualAddress,
        /// The temporal key.
        key: [u8; 16],
    },
    /// Install a group key ([`StaInput::GroupKeyInstalled`]).
    InstallGroupKey {
        /// The response must return this token.
        pending: PendingGroupKey<'id, T::EpochPath>,
        /// The slot it takes.
        slot: T::GroupSlot,
        /// Its key index.
        key_id: GroupKeyId,
        /// The temporal key.
        key: [u8; 16],
    },
    /// Replace the consumed key in one FIFO operation. Its slot reserves the
    /// terminal; only that terminal switches the station's replay epoch.
    ReplaceGroupKey {
        /// Return with the replacement terminal of this installed key's slot.
        pending: PendingGroupReplacement<'id, T::EpochPath>,
        /// The previous installation and its slot.
        installed: T::GroupKey,
        /// The successor's wire ID.
        key_id: GroupKeyId,
        /// The successor's material.
        key: [u8; 16],
    },
    /// Remove the pairwise key ([`StaInput::PairwiseKeyRemoved`]).
    RemovePairwiseKey(T::PairwiseKey),
    /// Remove a group key ([`StaInput::GroupKeyRemoved`]).
    RemoveGroupKey(T::GroupKey),
    /// Transmit one MPDU ([`StaInput::TxDone`] follows).
    Transmit(StaTransmit<'id, T, K>),
    /// Transmit one MPDU under the pairwise key its link lends: EAPOL
    /// after the handshake, sent only while the link is up
    /// ([`StaInput::TxDone`] follows). It carries the Protected bit and
    /// the CCMP header's room.
    TransmitProtected {
        /// The MPDU.
        transmit: StaTransmit<'id, T, K>,
        /// The pairwise key, as [`Station::link`] lends it.
        key: &'a T::PairwiseKey,
    },
    /// The station associated with `Peer`: the rates of its frames start
    /// from what the association made known, the handshake's EAPOL
    /// included ([owner decision on #563]). A
    /// [`StaNotice::Disconnected`] ends it.
    ///
    /// [owner decision on #563]: https://github.com/ermacv/open-esp-radio-rs/issues/563
    Associated(Peer<T::RxMeta>),
    /// Tell the application what happened.
    Notify(StaNotice),
}

/// One MPDU to transmit: its ticket, the buffer the station encoded it in,
/// its kind, its class and its rates.
#[must_use = "submit the operation or return its ticket and buffer"]
pub struct StaTransmit<'id, T: StaTokens, K> {
    /// Its epoch and purpose, moved with the operation through its terminal.
    pub context: StaTxContext<'id, T::EpochPath>,
    /// Its ticket.
    pub ticket: T::Ticket,
    /// The buffer holding the frame, from its header on.
    pub buffer: T::SmallBuffer,
    /// The frame's kind: its receiver, header and length.
    pub frame: StaFrame<K, T::Small>,
    /// What the frame is for, which the service turns into the port's
    /// coexistence priority.
    pub class: StaFrameClass,
    /// Its rate and retry plan.
    pub plan: StaRatePlan,
}

/// Reserve both owners before consuming a sequence or starting an attempt.
struct TxCredit<'id, T: StaTokens> {
    epoch: LinkEpoch<'id, T::EpochPath>,
    ticket: T::Ticket,
    buffer: T::SmallBuffer,
}

/// The identity of one station operation. No frame parsing or current-state
/// guess assigns a completion to a handshake, power transition or link.
#[derive(Debug)]
pub struct StaTxContext<'id, Path: PoolPath = Root> {
    epoch: LinkEpoch<'id, Path>,
    purpose: TxPurpose,
}

impl<Path: PoolPath> StaTxContext<'_, Path> {
    /// Whether this operation used the probe rate record.
    pub const fn is_probe(&self) -> bool {
        matches!(
            self.purpose,
            TxPurpose::Probe | TxPurpose::Connected(Sending::Probe)
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TxPurpose {
    Probe,
    Authentication,
    Association,
    HandshakeResponse,
    HandshakeLast,
    Deauthentication,
    Connected(Sending),
}

/// What a station frame is for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaFrameClass {
    /// A frame of the connection's setup or keys: Authentication,
    /// Association and EAPOL, which a coexisting radio should let through
    /// first.
    Connection,
    /// Any other frame.
    Normal,
}

/// The kind of a station frame, as the port's frame kinds take it.
pub enum StaFrame<K, C> {
    /// An individual frame that solicits an ACK.
    Individual(oer_ieee80211_lower_mac::ClassSizedFrame<C, AckHeader, K>),
    /// A group frame.
    Group(oer_ieee80211_lower_mac::ClassSizedFrame<C, HeaderSpec<GroupAddress>, K>),
}

/// The rates a station frame goes at, which the service turns into the
/// port's retry plan on the phase's channel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaRatePlan {
    /// An individual management frame: the BSS's lowest basic rate,
    /// retried by the station's management limit.
    Management,
    /// A group frame (a broadcast Probe Request): one transmission at the
    /// band's lowest rate.
    Group,
    /// A Null that tells the access point the station's power state:
    /// [`NULL_TRANSMISSIONS`](crate::power_save::NULL_TRANSMISSIONS)
    /// transmissions at the BSS's lowest basic rate.
    Null,
}

/// The station's connected link, as the data machine sees it while data
/// may go.
pub struct LinkView<'a, 'id, T: StaTokens> {
    /// The station's own address.
    pub address: OwnAddress,
    /// The connection identity, unchanged during sleep and channel absences.
    pub epoch: LinkEpoch<'id, T::EpochPath>,
    /// The access point.
    pub bssid: Bssid,
    /// The station's association identifier.
    pub association_id: u16,
    /// How data is sequenced on the BSS.
    pub mode: LinkMode<'a>,
    /// How data is protected.
    pub protection: LinkProtection<'a, T>,
}

/// How data is sequenced on a link: in the station's sequence spaces,
/// which its own frames share.
pub enum LinkMode<'a> {
    /// A QoS BSS: QoS Data, a sequence space per TID.
    Qos(&'a mut [StaSequenceCounter; 8]),
    /// A non-QoS BSS: Data in the station's sequence space, which its
    /// management frames share.
    NonQos(&'a mut StaSequenceCounter),
}

/// How data is protected on a link.
pub enum LinkProtection<'a, T: StaTokens> {
    /// An open network.
    Open,
    /// CCMP: the pairwise key data goes under, and the replay check of the
    /// association's keys that received data passes.
    Ccmp {
        /// The pairwise key.
        key: &'a T::PairwiseKey,
        /// The replay check.
        replay: &'a mut CcmpRxReplayEpoch,
    },
}

/// What the station tells the application.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaNotice {
    /// The station is connected to `bssid` on `channel`.
    Connected {
        /// The access point.
        bssid: Bssid,
        /// Its primary channel number.
        channel: u8,
    },
    /// The station left its network, or failed to join one.
    Disconnected(StaDisconnectReason),
    /// The background scan ended; `partial` when it ended early after
    /// cancelled absences.
    BackgroundScanDone {
        /// It ended early.
        partial: bool,
    },
}

/// Why the station is not connected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaDisconnectReason {
    /// A required frame was refused before admission.
    TransmitRefused(TxRefusal),
    /// The application asked it to leave.
    Requested,
    /// No access point of the network answered the scan; `partial` when
    /// the scan ended early after cancelled absences.
    NotFound {
        /// The scan ended early.
        partial: bool,
    },
    /// The arbiter did not let the station on a channel.
    Refused(Refusal),
    /// The arbiter told the station to leave its network.
    Arbiter(LeaveReason),
    /// The access point refused or never answered the authentication.
    AuthenticationFailed,
    /// The access point refused or never answered the association.
    AssociationFailed,
    /// The access point deauthenticated or disassociated the station.
    Peer {
        /// The reason code it gave.
        reason_code: u16,
    },
    /// A frame the station had to send did not encode.
    Encode,
    /// The four-way handshake, or a group key's handshake, failed or timed
    /// out.
    HandshakeFailed,
}

/// The scan list's channels at most.
const CHANNEL_CAPACITY: usize = 16;
/// The BSSs a scan keeps at most.
const SCAN_CAPACITY: usize = 16;

/// A new association's QoS sequence spaces.
const QOS_SEQUENCES: [StaSequenceCounter; 8] = [StaSequenceCounter::new(SequenceNumber::ZERO); 8];

/// The reason codes of the station's Deauthentication (IEEE 802.11-2020
/// Table 9-90).
const REASON_UNSPECIFIED: u16 = 1;
const REASON_LEAVING: u16 = 3;
const REASON_HANDSHAKE_TIMEOUT: u16 = 15;

/// What the station connects to and how.
///
/// A scan list is a contiguous sequence of checked channels. A hole that
/// could terminate a scan before its later channels is not representable:
///
/// ```compile_fail,E0308
/// use oer_ieee80211_sta::machine::StaConfig;
/// fn holes<K>(config: &mut StaConfig<K>) {
///     config.channels = [None; 16];
/// }
/// ```
///
/// A bounded, contiguous list uses the same checked channel values:
///
/// ```
/// use oer_ieee80211_sta::machine::StaConfig;
/// use oer_ieee80211_lower_mac::SupportedChannel;
/// fn one<K>(config: &mut StaConfig<K>, channel: SupportedChannel<K>) {
///     config.channels = heapless::Vec::from_slice(&[channel]).unwrap();
/// }
/// ```
pub struct StaConfig<K> {
    /// The interface's own address.
    pub address: OwnAddress,
    /// The network's SSID.
    pub ssid: heapless::Vec<u8, MAX_SSID_LEN>,
    /// The channels the scan visits, in order.
    pub channels: heapless::Vec<SupportedChannel<K>, CHANNEL_CAPACITY>,
    /// How long the scan listens on each channel.
    pub dwell: Duration,
    /// The consecutive cancelled leases after which a scan ends with a
    /// partial result.
    pub skipped_absences: u8,
    /// The margin, in dB, within which a candidate on the channel of the
    /// access point beside the station wins over the strongest candidate.
    /// The owner set this parameter to [6 dB].
    ///
    /// [6 dB]: https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6130775558
    pub access_point_margin_db: u8,
    /// The rates the Probe Request offers.
    pub supported_rates: [u8; 8],
    /// The capabilities the Association Request offers.
    pub association: &'static AssociationCapabilities,
    /// The PHY the station associates with.
    pub phy: PhyMode,
    /// How the station protects its link.
    pub security: StaSecurity,
    /// A fresh 32-octet nonce for each handshake.
    pub nonce: fn() -> [u8; 32],
    /// The nominal packet padding the access point's HE Capabilities
    /// element sets: an integrator's policy (the Espressif stack's is
    /// `oer-espressif-ieee80211-policy`'s).
    pub he_packet_padding: fn(&[u8]) -> HePacketPadding,
}

/// The receive rules a station uses, which its port must offer: checked
/// when the station's type is built, so a port without them builds no
/// station.
struct Rules<K>(PhantomData<fn() -> K>);

impl<K: Ieee80211Capabilities> Rules<K> {
    /// The network's frames alone.
    const MEMBER: SupportedFilter<K, StationRole> = {
        #[cfg_attr(not(test), expect(clippy::panic, reason = "evaluated at build"))]
        const fn checked<K: Ieee80211Capabilities>() -> SupportedFilter<K, StationRole> {
            match SupportedFilter::new(ReceiveFilter::BSS_MEMBER) {
                Some(filter) => filter,
                None => panic!("a station's port receives its network's frames"),
            }
        }
        checked::<K>()
    };
    /// The network's frames and the other networks' management frames:
    /// the network's own channel in a background scan.
    const WIDENED: SupportedFilter<K, StationRole> = {
        #[cfg_attr(not(test), expect(clippy::panic, reason = "evaluated at build"))]
        const fn checked<K: Ieee80211Capabilities>() -> SupportedFilter<K, StationRole> {
            match SupportedFilter::new(
                ReceiveFilter::BSS_MEMBER.union(ReceiveFilter::OTHER_BSS_MANAGEMENT),
            ) {
                Some(filter) => filter,
                None => panic!("a station's port hears the other networks beside its own"),
            }
        }
        checked::<K>()
    };
}

/// The station: its state and its tokens.
pub struct Station<'id, T: StaTokens, K: Ieee80211Capabilities> {
    config: StaConfig<K>,
    state: State<T, K>,
    keys: Keys<T>,
    configuration: Option<ConfigurationPlace>,
    tickets: Pool<T::Ticket>,
    buffers: Pool<T::SmallBuffer>,
    /// The station's place for a lease, while it holds none.
    place: Option<LeasePlace>,
    /// The lease the station holds, or gave back and awaits the end of.
    lease: Held<K>,
    /// The station's place for an absence, while it takes none.
    absence_place: Option<AbsencePlace>,
    /// The absence the station holds.
    absence: Option<Lease<K>>,
    /// A notice for the application the station owes outside its state's
    /// steps.
    notice_owed: Option<StaNotice>,
    sequence: StaSequenceCounter,
    scan: ScanTable<SCAN_CAPACITY>,
    /// The connection attempt that owns new TX operations.
    epoch: Option<LinkEpoch<'id, T::EpochPath>>,
    epochs: LinkEpochSource<'id, T::EpochPath>,
}

/// The station's lease.
enum Held<K> {
    /// None: the station holds its place.
    None,
    /// Held, from the arbiter's answer to its end.
    Lease(Lease<K>),
    /// Given back; its end's notice names it.
    Released(LeaseId),
}

impl<K> Held<K> {
    fn id(&self) -> Option<LeaseId> {
        match self {
            Self::None => None,
            Self::Lease(lease) => Some(lease.id()),
            Self::Released(id) => Some(*id),
        }
    }
}

/// A pairwise cell awaiting the response to its one installation.
#[derive(Debug)]
#[must_use = "return this token with the pairwise installation result"]
pub struct PendingPairwiseKey<'id, Path: PoolPath = Root> {
    cell: PairwiseCell,
    epoch: LinkEpoch<'id, Path>,
}

#[derive(Debug)]
struct PairwiseCell;

/// A group-key installation awaiting its response. Only Station creates this value;
/// moving it into the response proves which transition owns the installed key.
///
/// ```compile_fail,E0063
/// use oer_ieee80211_sta::machine::{StaInput, StaTokens};
/// fn forged<'id, T: StaTokens, K>(key: T::GroupKey) -> StaInput<'id, T, K> {
///     StaInput::GroupKeyInstalled { key }
/// }
/// ```
///
/// It cannot be copied to manufacture a second response:
/// ```compile_fail,E0599
/// use oer_ieee80211_sta::machine::PendingGroupKey;
/// fn duplicate<'id>(pending: PendingGroupKey<'id>) -> (PendingGroupKey<'id>, PendingGroupKey<'id>) {
///     (pending.clone(), pending)
/// }
/// ```
///
/// With the original action's token, returning the key is well-typed:
/// ```
/// use oer_ieee80211_sta::machine::{PendingGroupKey, StaInput, StaTokens};
/// fn installed<'id, T: StaTokens, K>(pending: PendingGroupKey<'id, T::EpochPath>, key: T::GroupKey)
///     -> StaInput<'id, T, K> {
///     StaInput::GroupKeyInstalled { pending, key }
/// }
/// ```
#[derive(Debug)]
#[must_use = "return this token with the group installation result"]
pub struct PendingGroupKey<'id, Path: PoolPath = Root> {
    epoch: LinkEpoch<'id, Path>,
    key_id: GroupKeyId,
    material: [u8; 16],
    place: GroupPlace,
}

/// The reserved successor of a GTK. It stays private until the port's
/// replacement terminal; the active replay epoch remains usable meanwhile.
///
/// The pending owner cannot complete two replacements:
/// ```compile_fail,E0382
/// use oer_ieee80211_sta::machine::PendingGroupReplacement;
/// fn duplicate<'id>(pending: PendingGroupReplacement<'id>) {
///     let first = pending;
///     let second = pending;
/// }
/// ```
/// The replacement input must return that owner:
/// ```compile_fail,E0063
/// use oer_ieee80211_sta::machine::{StaInput, StaTokens};
/// fn forged<'id, T: StaTokens, K>(key: T::GroupKey) -> StaInput<'id, T, K> {
///     StaInput::GroupKeyReplaced { key }
/// }
/// ```
/// Its valid terminal moves both values once:
/// ```
/// use oer_ieee80211_sta::machine::{PendingGroupReplacement, StaInput, StaTokens};
/// fn returned<'id, T: StaTokens, K>(pending: PendingGroupReplacement<'id, T::EpochPath>, key: T::GroupKey)
///     -> StaInput<'id, T, K> {
///     StaInput::GroupKeyReplaced { pending, key }
/// }
/// ```
#[must_use = "return this token with the matching group replacement terminal"]
pub struct PendingGroupReplacement<'id, Path: PoolPath = Root> {
    install: PendingGroupKey<'id, Path>,
    previous_id: GroupKeyId,
    replay: oer_ieee80211_mac::ccmp::CcmpRxReplayState,
}

/// The replacement command retains both its station epoch and installed key.
type GroupReplacement<'id, T> = (
    PendingGroupReplacement<'id, <T as StaTokens>::EpochPath>,
    <T as StaTokens>::GroupKey,
);

/// A receive-configuration response must return this action token. Station
/// grants only one at a time; returning an old epoch releases its permission
/// without completing a transition of the current connection.
///
/// ```compile_fail,E0308
/// use oer_ieee80211_sta::machine::{StaInput, StaTokens};
/// fn forged<'id, T: StaTokens, K>() -> StaInput<'id, T, K> {
///     StaInput::Configured
/// }
/// ```
#[derive(Debug)]
#[must_use = "return this token with the receive configuration result"]
pub struct PendingConfiguration<'id, Path: PoolPath = Root> {
    place: ConfigurationPlace,
    epoch: Option<LinkEpoch<'id, Path>>,
}

#[derive(Debug)]
struct ConfigurationPlace;

/// The state of a key cell, independent of the protocol's current state.
enum KeyCell<Key, Slot> {
    Free(Slot),
    Installing,
    Installed(Key),
    Removing,
}

/// Free slots are fungible credits from the installation's declared pool.
/// An install takes one; only its eventual removal returns one. Thus a full
/// pool proves that no group installation or removal remains outstanding.
struct GroupSlots<Slot> {
    free: Pool<Slot>,
    capacity: usize,
}

impl<Slot> GroupSlots<Slot> {
    fn new<const N: usize>(slots: [Slot; N]) -> Self {
        Self {
            free: Pool::new(slots),
            capacity: N,
        }
    }
    fn is_full(&self) -> bool {
        self.free.len() == self.capacity
    }
}

struct Keys<T: StaTokens> {
    pairwise: KeyCell<T::PairwiseKey, T::PairwiseSlot>,
    group_slots: GroupSlots<T::GroupSlot>,
    groups: GroupKeys<T::GroupKey>,
}

impl<T: StaTokens> Keys<T> {
    fn new<const N: usize>(pairwise: T::PairwiseSlot, groups: [T::GroupSlot; N]) -> Self {
        Self {
            pairwise: KeyCell::Free(pairwise),
            group_slots: GroupSlots::new(groups),
            groups: GroupKeys::new(),
        }
    }

    fn take_pairwise_slot<'id>(
        &mut self,
        epoch: LinkEpoch<'id, T::EpochPath>,
    ) -> Option<(T::PairwiseSlot, PendingPairwiseKey<'id, T::EpochPath>)> {
        match core::mem::replace(&mut self.pairwise, KeyCell::Installing) {
            KeyCell::Free(slot) => Some((
                slot,
                PendingPairwiseKey {
                    cell: PairwiseCell,
                    epoch,
                },
            )),
            cell => {
                self.pairwise = cell;
                None
            }
        }
    }

    fn pairwise_installed(
        &mut self,
        pending: PendingPairwiseKey<'_, T::EpochPath>,
        key: T::PairwiseKey,
    ) {
        let PairwiseCell = pending.cell;
        self.pairwise = KeyCell::Installed(key);
    }

    fn take_pairwise_key(&mut self) -> Option<T::PairwiseKey> {
        match core::mem::replace(&mut self.pairwise, KeyCell::Removing) {
            KeyCell::Installed(key) => Some(key),
            cell => {
                self.pairwise = cell;
                None
            }
        }
    }

    fn take_group_slot<'id>(
        &mut self,
        epoch: LinkEpoch<'id, T::EpochPath>,
        key_id: GroupKeyId,
        material: [u8; 16],
    ) -> Option<(T::GroupSlot, PendingGroupKey<'id, T::EpochPath>)> {
        let slot = self.group_slots.free.take()?;
        let Some(place) = self.groups.reserve() else {
            self.group_slots.free.put(slot);
            return None;
        };
        Some((
            slot,
            PendingGroupKey {
                epoch,
                key_id,
                material,
                place,
            },
        ))
    }

    fn group_installed(
        &mut self,
        pending: PendingGroupKey<'_, T::EpochPath>,
        key: T::GroupKey,
        current: bool,
    ) {
        self.groups.install(
            InstalledGroup {
                place: pending.place,
                key_id: pending.key_id,
                material: pending.material,
                key,
            },
            current,
        );
    }

    fn take_retiring_key(&mut self) -> Option<(GroupKeyId, T::GroupKey)> {
        self.groups.take_retiring()
    }

    fn take_group_key(&mut self) -> Option<(GroupKeyId, T::GroupKey)> {
        self.groups.take_any()
    }

    fn group_removed(&mut self, slot: T::GroupSlot) {
        self.group_slots.free.put(slot);
    }

    fn current_group(&self) -> Option<GroupKeyId> {
        self.groups.current_id()
    }

    fn has_retiring(&self) -> bool {
        self.groups.has_retiring()
    }

    fn installed(&self) -> bool {
        matches!(self.pairwise, KeyCell::Installed(_)) && self.current_group().is_some()
    }

    fn clear(&self) -> bool {
        matches!(self.pairwise, KeyCell::Free(_)) && self.group_slots.is_full()
    }
}

/// The protocol state. Key-cell ownership remains in Station while these
/// states change, including while an installation response is outstanding.
#[expect(
    clippy::large_enum_variant,
    reason = "the station is placed once; a state machine's size is its largest state, and without an allocator a box is no option"
)]
enum State<T: StaTokens, K> {
    /// Not connected and not trying.
    Idle,
    /// Visiting the scan list.
    Scan(Scan<K>),
    /// Joining the chosen access point.
    Join(Join<T, K>),
    /// Running the WPA2 four-way handshake.
    Handshake(Handshake<T, K>),
    /// Connected.
    Connected(Connected<T, K>),
    /// Leaving the network.
    Leave(Leave),
    /// Telling the application why it is not connected, then idle.
    Report(StaDisconnectReason),
    /// Only between a transition's take and its put.
    Taken,
}

/// A visit of one scan channel.
struct Scan<K> {
    visit: usize,
    /// Consecutive cancelled leases.
    skipped: u8,
    step: ScanStep,
    /// The channel of the access point beside the station, as the last
    /// lease named it.
    access_point: Option<SupportedChannel<K>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ScanStep {
    /// Ask the arbiter for the visit's channel.
    NeedLease,
    /// The arbiter answers in the same step.
    AwaitAnswer,
    /// The lease is granted; the port moves at its start.
    AwaitStart,
    NeedConfigure,
    AwaitConfigured,
    NeedProbe,
    /// Listening until the lease's end.
    Listen,
}

/// The join of one access point.
struct Join<T: StaTokens, K> {
    access_point: ScanRecord,
    rsn: SelectedRsn,
    channel: SupportedChannel<K>,
    step: JoinStep<T::RxMeta>,
}

enum JoinStep<M> {
    NeedLease,
    AwaitAnswer,
    /// The access point beside the station followed a join too recently:
    /// the station asks again at this instant.
    RetryAt(Instant),
    AwaitStart,
    NeedConfigure,
    AwaitConfigured,
    Authenticate(Authenticate),
    Associate(StaAssociationRuntime),
    NotifyAssociated {
        association_id: u16,
        peer: Peer<M>,
    },
    StartLink {
        association_id: u16,
        peer: Peer<M>,
    },
}

/// The access point as the association made it: what it negotiated, and
/// the metadata its Association Response arrived with, whose type is the
/// port's: a rate selection starts from it (owner decision on #563, fresher
/// than the scan record the old ESP32-S31 path took), and one that needs a
/// signal measurement requires a port whose metadata carries one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Peer<M> {
    /// What the association negotiated.
    pub associated: StaAssociatedPeer,
    /// The Association Response's metadata.
    pub response: M,
}

enum Authenticate {
    NeedCredit(StaAuthenticationRuntime),
    Exchange {
        runtime: StaAuthenticationExchange,
        stage: AuthStage,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AuthStage {
    AwaitSent { response_timeout: Duration },
    AwaitResponse { until: Instant },
}

/// The WPA2 four-way handshake with the joined access point.
struct Handshake<T: StaTokens, K> {
    bssid: Bssid,
    channel: SupportedChannel<K>,
    association_id: u16,
    qos: bool,
    peer: Peer<T::RxMeta>,
    supplicant: RsnStaSupplicant,
    /// The deadline of the message the station waits for.
    deadline: RsnStaResponseDeadline,
    step: HandshakeStep,
}

/// The handshake's step; its values move from one step to the next, so the
/// handshake holds the largest of them in place.
#[expect(
    clippy::large_enum_variant,
    reason = "a step's frame or key request moves into the next step; without an allocator a box is no option"
)]
enum HandshakeStep {
    /// Waiting for the access point's next message.
    Await,
    /// A message to send.
    Send(RsnTxFrame),
    /// Message 2's terminal.
    AwaitSent,
    /// The keys of Message 3 to install: the pairwise key, then the group
    /// key.
    Install(RsnStaKeyInstallRequest),
    InstallGroup(RsnStaKeyInstallRequest, Option<CcmpRxReplayEpoch>),
    AwaitInstalled(
        RsnStaKeyInstallRequest,
        Option<CcmpRxReplayEpoch>,
        Option<GroupKeyId>,
    ),
    /// M4 was not delivered. Only a repeated M3 or the existing deadline
    /// advances this state; both key installations and their replay survive.
    AwaitRetry(CcmpRxReplayEpoch),
    /// Message 4, then connected.
    SendLast(RsnTxFrame, CcmpRxReplayEpoch),
    AwaitLast(CcmpRxReplayEpoch),
}

/// The connected station.
struct Connected<T: StaTokens, K> {
    bssid: Bssid,
    channel: SupportedChannel<K>,
    association_id: u16,
    /// The access point's BSS is a QoS BSS (it advertised WMM).
    qos: bool,
    /// The access point as the association made it.
    peer: Peer<T::RxMeta>,
    /// The association's QoS sequence spaces, one per TID: its EAPOL's and,
    /// lent through the link, its data's.
    qos_sequences: [StaSequenceCounter; 8],
    /// The upstream fact the arbiter has yet to hear.
    report: Option<UpstreamFact<K>>,
    /// The primary lease to exchange after the port moved.
    exchange: bool,
    /// What the access point knows of the station's power state.
    power: PowerSaveView,
    /// A Null with PM=0 the station owes its access point.
    wake: bool,
    /// Admission: at most one connected control transmission at a time.
    sending: bool,
    /// A PM=0 transition is outstanding; beacons need not request another.
    waking: bool,
    /// The background scan, if one runs.
    background: Option<Background>,
    /// The application has yet to hear the station connected.
    connected_notice: bool,
    /// The supplicant of a protected link and its group key's handshake.
    rsn: Option<ConnectedRsn>,
}

/// What the connected station's transmission in flight is for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Sending {
    /// A Null with PM=1 before an absence.
    Sleep,
    /// A Null with PM=0.
    Wake,
    /// A Probe Request of the background scan.
    Probe,
    /// Group Message 2.
    GroupResponse,
}

/// A protected link's supplicant and its group key's handshake.
struct ConnectedRsn {
    supplicant: RsnConnectedSupplicant,
    rekey: Rekey,
    /// The replay check of the association's keys, which the data machine
    /// borrows through the link.
    replay: CcmpRxReplayEpoch,
}

/// A group key's handshake: one algorithm whatever the slots. With a free
/// group slot the new key goes in beside the old one, which is removed
/// after Group Message 2; a full pool replaces one key in a single operation.
enum Rekey {
    None,
    /// The new key to install in a free slot or replace in an occupied slot.
    Install(RsnGroupKeyInstallRequest<DEFAULT_EAPOL_FRAME_CAPACITY>),
    AwaitInstalled(RsnGroupKeyInstallRequest<DEFAULT_EAPOL_FRAME_CAPACITY>),
    /// Group Message 2.
    Send(RsnTxFrame),
    AwaitSent,
    /// The replaced key's removal.
    Retire,
    AwaitRetired,
}

/// A connected station's background scan: one visit per channel.
struct Background {
    visit: usize,
    skipped: u8,
    step: BackgroundStep,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BackgroundStep {
    NeedAbsence,
    AwaitAnswer,
    /// Tell the access point the station leaves.
    NeedSleep,
    AwaitSleep,
    NeedReady,
    AwaitStart,
    NeedConfigure,
    AwaitConfigured,
    NeedProbe,
    Listen,
    /// Back home: the network's receive rules, then PM=0.
    NeedHome,
    AwaitHome,
    NeedWake,
    AwaitWake,
    /// The network's own channel: the widened receive rules, a probe and
    /// the dwell, then the network's rules again.
    AwaitWidened,
    HomeProbe {
        until: Instant,
    },
    HomeListen {
        until: Instant,
    },
    Narrow,
    AwaitNarrowed,
}

/// The station leaving its network, and why.
struct Leave {
    reason: StaDisconnectReason,
    step: LeaveStep,
}

/// A Deauthentication to send.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Deauthentication {
    bssid: Bssid,
    reason_code: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LeaveStep {
    /// The absence's terminal proves the port returned to the home channel.
    AwaitAbsenceEnd(Option<Deauthentication>),
    /// Remove the installed keys; their slots come back with their
    /// removals. Then the Deauthentication, if any.
    RemoveKeys(Option<Deauthentication>),
    NeedDeauthentication(Deauthentication),
    AwaitDeauthentication,
    NeedConfigure,
    AwaitConfigured,
    NeedRelease,
    /// The lease ends with its notice, which gives the place back.
    AwaitEnd,
}

/// The EAPOL ether type.
const EAPOL_ETHER_TYPE: u16 = 0x888e;

/// A checked management disconnect from the selected BSS.
fn peer_disconnect(frame: &[u8], bssid: Bssid) -> Option<u16> {
    let header = oer_ieee80211_mac::frame::DecodedHeader::decode(frame)?;
    if header.control().is_data()
        || !matches!(header.control().subtype(), 10 | 12)
        || header.transmitter() != bssid.get()
        || header.address3() != bssid.get()
        || header.control().protected()
    {
        return None;
    }
    let (reason, _) = header.body().split_first_chunk::<2>()?;
    Some(u16::from_le_bytes(*reason))
}

/// A group key's checked wire ID and temporal key.
fn group_key(install: &RsnKeyInstall) -> Option<(GroupKeyId, [u8; 16])> {
    let RsnKeyKind::Group { key_id, .. } = install.kind() else {
        return None;
    };
    Some((key_id, *install.key().as_bytes()))
}

/// The replay check of the keys a handshake's Message 3 installs.
fn replay_epoch(request: &RsnStaKeyInstallRequest) -> Option<CcmpRxReplayEpoch> {
    let (key_id, _) = group_key(request.group())?;
    CcmpRxReplayEpoch::new(
        *request.pairwise().receive_sequence(),
        key_id,
        *request.group().receive_sequence(),
    )
    .ok()
}

/// The replay check forgets a removed group key, unless the key in use
/// holds its index.
fn forget_group<T: StaTokens>(rsn: &mut ConnectedRsn, keys: &Keys<T>, key_id: GroupKeyId) {
    if keys.current_group() != Some(key_id) {
        rsn.replay.remove_group(key_id);
    }
}

/// What ended in the station's world.
enum Done {
    Unsent {
        purpose: TxPurpose,
        reason: TxRefusal,
    },
    Command(StaCommand),
    Tick,
    /// The complete port terminal, including withdrawals and unknown outcomes.
    Tx {
        purpose: TxPurpose,
        completion: TxOutcome,
    },
    Configured,
}

/// An action a state owes.
enum Owed<'id, T: StaTokens, K> {
    InstallPairwise {
        pending: PendingPairwiseKey<'id, T::EpochPath>,
        slot: T::PairwiseSlot,
        peer: IndividualAddress,
        key: [u8; 16],
    },
    InstallGroup {
        pending: PendingGroupKey<'id, T::EpochPath>,
        slot: T::GroupSlot,
        key_id: GroupKeyId,
        key: [u8; 16],
    },
    ReplaceGroup {
        pending: PendingGroupReplacement<'id, T::EpochPath>,
        installed: T::GroupKey,
        key_id: GroupKeyId,
        key: [u8; 16],
    },
    RemovePairwise(T::PairwiseKey),
    RemoveGroup(T::GroupKey),
    /// A request, with the place it takes.
    Request(ChannelRequest<K>, LeasePlace),
    /// An absence, with the place it takes.
    Absence(ChannelRequest<K>, AbsencePlace),
    Ready(LeaseId),
    /// The primary lease to exchange.
    Exchange(Lease<K>),
    /// The primary lease to give back.
    Release(Lease<K>),
    Upstream(UpstreamFact<K>),
    Configure(StationReceive<K>, PendingConfiguration<'id, T::EpochPath>),
    Transmit(StaTransmit<'id, T, K>),
    /// Under the pairwise key of the connected link.
    TransmitProtected(StaTransmit<'id, T, K>),
    Associated(Peer<T::RxMeta>),
    Notify(StaNotice),
}

/// An action, an internal transition to drain, or an actual wait for input.
enum ActionProgress<A> {
    Action(A),
    Transition,
    Wait,
}

impl<A> From<Option<A>> for ActionProgress<A> {
    fn from(action: Option<A>) -> Self {
        match action {
            Some(action) => Self::Action(action),
            None => Self::Wait,
        }
    }
}

type ActionStep<'id, T, K> = (State<T, K>, ActionProgress<Owed<'id, T, K>>);

/// The affine resources composition grants to one station. Epoch issuance,
/// frame pools, channel admission and key slots move into it together.
pub struct StationResources<
    'id,
    T: StaTokens,
    const TICKETS: usize,
    const BUFFERS: usize,
    const GROUPS: usize,
> {
    /// The installation's only station epoch source.
    pub epochs: LinkEpochSource<'id, T::EpochPath>,
    /// Ticket credits, each reserving a terminal.
    pub tickets: [T::Ticket; TICKETS],
    /// Fresh small allocations for protocol frames.
    pub buffers: [T::SmallBuffer; BUFFERS],
    /// Primary-channel lease admission.
    pub place: LeasePlace,
    /// Temporary channel-absence admission.
    pub absence_place: AbsencePlace,
    /// Pairwise-key installation slot.
    pub pairwise: T::PairwiseSlot,
    /// Receive group-key installation slots.
    pub groups: [T::GroupSlot; GROUPS],
}

/// Protected mode requires a group-key slot. Construction returns every
/// resource and the configuration unchanged when that slot is absent.
#[must_use]
pub struct MissingGroupSlot<
    'id,
    T: StaTokens,
    K: Ieee80211Capabilities,
    const TICKETS: usize,
    const BUFFERS: usize,
    const GROUPS: usize,
> {
    pub config: StaConfig<K>,
    pub resources: StationResources<'id, T, TICKETS, BUFFERS, GROUPS>,
}

impl<'id, T: StaTokens, K: Ieee80211Capabilities> Station<'id, T, K> {
    /// An idle station of `config`, with the tickets, small buffers and key
    /// slots the composition granted it and the arbiter's places. It holds
    /// at most eight tokens of a kind, checked at compile time:
    ///
    #[doc = include_str!("machine/resources_contracts.md")]
    #[allow(
        clippy::result_large_err,
        reason = "construction refusal returns all affine resources"
    )]
    pub fn new<const TICKETS: usize, const BUFFERS: usize, const GROUPS: usize>(
        config: StaConfig<K>,
        resources: StationResources<'id, T, TICKETS, BUFFERS, GROUPS>,
    ) -> Result<Self, MissingGroupSlot<'id, T, K, TICKETS, BUFFERS, GROUPS>> {
        const {
            #[cfg_attr(not(test), expect(clippy::panic, reason = "evaluated at build"))]
            const fn check(tickets: usize, buffers: usize) {
                if tickets < 1 {
                    panic!("a station needs at least one TX ticket");
                }
                if buffers < 1 {
                    panic!("a station needs at least one small buffer");
                }
            }
            check(TICKETS, BUFFERS);
            let _: (
                SupportedFilter<K, StationRole>,
                SupportedFilter<K, StationRole>,
            ) = (Rules::<K>::MEMBER, Rules::<K>::WIDENED);
        }
        if GROUPS == 0 && !matches!(config.security, StaSecurity::Open) {
            return Err(MissingGroupSlot { config, resources });
        }
        let StationResources {
            epochs,
            tickets,
            buffers,
            place,
            absence_place,
            pairwise,
            groups,
        } = resources;
        Ok(Self {
            config,
            state: State::Idle,
            keys: Keys::new(pairwise, groups),
            configuration: Some(ConfigurationPlace),
            tickets: Pool::new(tickets),
            buffers: Pool::new(buffers),
            place: Some(place),
            lease: Held::None,
            absence_place: Some(absence_place),
            absence: None,
            notice_owed: None,
            sequence: StaSequenceCounter::new(SequenceNumber::ZERO),
            scan: ScanTable::new(),
            epoch: None,
            epochs,
        })
    }

    /// Tokens given back beyond the station's own: dropped, counted.
    pub fn lost_tokens(&self) -> u32 {
        self.tickets.lost().combined(self.buffers.lost()).get()
    }

    /// A leave has closed the link even while its deauthentication and key
    /// removals still use the old operation epoch. Taken is a synchronous
    /// transition only; issuing actions uses that transition's owning epoch.
    fn current_epoch(&self) -> Option<LinkEpoch<'id, T::EpochPath>> {
        match self.state {
            State::Idle | State::Leave(_) | State::Report(_) => None,
            State::Scan(_)
            | State::Join(_)
            | State::Handshake(_)
            | State::Connected(_)
            | State::Taken => self.epoch,
        }
    }

    /// Whether an operation still belongs to this connection attempt.
    /// Resource return is unconditional; protocol and rate updates use this guard.
    pub fn is_current(&self, context: &StaTxContext<'id, T::EpochPath>) -> bool {
        Some(context.epoch) == self.epoch
    }

    /// Whether the station is connected.
    pub const fn is_connected(&self) -> bool {
        matches!(self.state, State::Connected(_))
    }

    /// The access point of the associated station, as its association made
    /// it: during the handshake and once connected.
    pub const fn peer(&self) -> Option<&Peer<T::RxMeta>> {
        match &self.state {
            State::Handshake(handshake) => Some(&handshake.peer),
            State::Connected(connected) => Some(&connected.peer),
            _ => None,
        }
    }

    /// The connected link, which the data machine sends and receives on,
    /// while data may go: the station is home and awake, and a protected
    /// link holds its key. It lends the pairwise key, so only the station
    /// removes it, the replay check, and on a non-QoS BSS the station's
    /// sequence space ([owner decision on #563](https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6131495971)).
    pub fn link(&mut self) -> Option<LinkView<'_, 'id, T>> {
        let State::Connected(connected) = &mut self.state else {
            return None;
        };
        if self.absence.is_some() || connected.power.asleep() {
            return None;
        }
        let pairwise = match &self.keys.pairwise {
            KeyCell::Installed(key) => Some(key),
            KeyCell::Free(_) | KeyCell::Installing | KeyCell::Removing => None,
        };
        let replay = match &mut connected.rsn {
            Some(rsn) => Some(&mut rsn.replay),
            None => None,
        };
        let protection = match (pairwise, replay) {
            (Some(key), Some(replay)) => LinkProtection::Ccmp { key, replay },
            // A protected link without its key sends nothing.
            (None, Some(_)) | (Some(_), None) => return None,
            (None, None) => LinkProtection::Open,
        };
        Some(LinkView {
            epoch: self.epoch?,
            address: self.config.address,
            bssid: connected.bssid,
            association_id: connected.association_id,
            mode: if connected.qos {
                LinkMode::Qos(&mut connected.qos_sequences)
            } else {
                LinkMode::NonQos(&mut self.sequence)
            },
            protection,
        })
    }

    /// The security policy the scan selects and the join associates with.
    const fn policy(&self) -> StaSecurityPolicy {
        match self.config.security {
            StaSecurity::Open => StaSecurityPolicy::Open,
            StaSecurity::Wpa2Personal { .. } => StaSecurityPolicy::Wpa2Personal,
        }
    }

    fn ssid(&self) -> &[u8] {
        &self.config.ssid
    }

    fn channel(&self, visit: usize) -> Option<SupportedChannel<K>> {
        self.config.channels.get(visit).copied()
    }

    fn take_state(&mut self) -> State<T, K> {
        core::mem::replace(&mut self.state, State::Taken)
    }

    /// Take one input at `now`.
    pub fn input(&mut self, now: Instant, input: StaInput<'id, T, K>) {
        let done = match input {
            StaInput::TxDone {
                context,
                ticket,
                buffer,
                completion,
            } => {
                // A transmission's tokens come back whatever the state.
                self.tickets.put(ticket);
                self.buffers.put(buffer.into_fresh(T::renew));
                if !self.is_current(&context) {
                    return;
                }
                Done::Tx {
                    purpose: context.purpose,
                    completion,
                }
            }
            StaInput::Unsent {
                reason,
                context,
                ticket,
                buffer,
            } => {
                self.tickets.put(ticket);
                self.buffers.put(buffer.into_fresh(T::renew));
                if !self.is_current(&context) {
                    return;
                }
                Done::Unsent {
                    purpose: context.purpose,
                    reason,
                }
            }
            StaInput::Command(command) => Done::Command(command),
            StaInput::Tick => Done::Tick,
            StaInput::Configured(pending) => {
                self.configuration = Some(pending.place);
                if pending.epoch != self.current_epoch() {
                    return;
                }
                Done::Configured
            }
            StaInput::Answer(answer) => {
                self.answered(answer);
                return;
            }
            StaInput::AbsenceAnswer(answer) => {
                self.absence_answered(answer);
                return;
            }
            StaInput::Exchanged(lease) => {
                self.lease = Held::Lease(lease);
                return;
            }
            StaInput::PairwiseKeyInstalled { pending, key } => {
                let current = self.current_epoch() == Some(pending.epoch);
                self.keys.pairwise_installed(pending, key);
                // Leave waits for every outstanding key installation to return.
                // An old response becomes a removal action there.
                if current {
                    self.keys_changed();
                }
                return;
            }
            StaInput::GroupKeyInstalled { pending, key } => {
                let current = self.current_epoch() == Some(pending.epoch);
                self.keys.group_installed(pending, key, current);
                if current {
                    self.group_installed(None);
                }
                return;
            }
            StaInput::GroupKeyReplaced { pending, key } => {
                let PendingGroupReplacement {
                    install,
                    previous_id,
                    replay,
                } = pending;
                let current = self.current_epoch() == Some(install.epoch);
                let key_id = install.key_id;
                self.keys.group_installed(install, key, current);
                if current {
                    if let State::Connected(Connected { rsn: Some(rsn), .. }) = &mut self.state
                        && previous_id != key_id
                    {
                        rsn.replay.remove_group(previous_id);
                    }
                    self.group_installed(Some(replay));
                }
                return;
            }
            StaInput::PairwiseKeyRemoved(slot) => {
                self.keys.pairwise = KeyCell::Free(slot);
                return;
            }
            StaInput::GroupKeyRemoved(slot) => {
                self.keys.group_removed(slot);
                self.group_removed();
                return;
            }
            StaInput::Arbiter(notice) => {
                self.noticed(notice);
                return;
            }
        };
        let state = self.take_state();
        self.state = self.step(state, now, done);
    }

    /// The arbiter's answer.
    fn answered(&mut self, answer: Answer<K>) {
        let state = self.take_state();
        self.state = match answer {
            Answer::Lease(lease) => {
                let access_point = lease.access_point();
                self.lease = Held::Lease(lease);
                match state {
                    State::Scan(mut scan) => {
                        scan.access_point = access_point;
                        scan.step = ScanStep::AwaitStart;
                        State::Scan(scan)
                    }
                    State::Join(mut join) => {
                        join.step = JoinStep::AwaitStart;
                        State::Join(join)
                    }
                    state => state,
                }
            }
            Answer::Refused(refusal, place) => {
                self.place = Some(place);
                match (state, refusal) {
                    (State::Join(mut join), Refusal::TooSoon { retry_at }) => {
                        join.step = JoinStep::RetryAt(retry_at);
                        State::Join(join)
                    }
                    (State::Scan(_) | State::Join(_), _) => {
                        leave_unkeyed(StaDisconnectReason::Refused(refusal))
                    }
                    (state, _) => state,
                }
            }
        };
    }

    /// A key of the handshake is installed: once both are, Message 4
    /// follows.
    fn keys_changed(&mut self) {
        let state = self.take_state();
        self.state = match state {
            State::Handshake(mut handshake) if self.keys.installed() => {
                match core::mem::replace(&mut handshake.step, HandshakeStep::Await) {
                    HandshakeStep::AwaitInstalled(request, mut replay, replaced) => {
                        if let Some(epoch) = &mut replay {
                            let Some((id, _)) = group_key(request.group()) else {
                                return self.set(leave_handshake(handshake, REASON_UNSPECIFIED));
                            };
                            if let Some(old) = replaced
                                && old != id
                            {
                                epoch.remove_group(old);
                            }
                            if epoch
                                .install_group(id, *request.group().receive_sequence())
                                .is_err()
                            {
                                return self.set(leave_handshake(handshake, REASON_UNSPECIFIED));
                            }
                        }
                        finish_handshake_install(handshake, request, replay)
                    }
                    step => {
                        handshake.step = step;
                        State::Handshake(handshake)
                    }
                }
            }
            state => state,
        };
    }

    /// A group key is installed: the handshake's, or a rekey's.
    fn group_installed(&mut self, new_epoch: Option<oer_ieee80211_mac::ccmp::CcmpRxReplayState>) {
        let state = self.take_state();
        self.state = match state {
            State::Connected(mut connected) => {
                if let Some(rsn) = &mut connected.rsn
                    && let Rekey::AwaitInstalled(request) =
                        core::mem::replace(&mut rsn.rekey, Rekey::None)
                {
                    // The new group key's frontiers start from its receive
                    // sequence once it is installed, beside the old key's.
                    let installed =
                        group_key(request.group()).is_some_and(|(key_id, _)| match new_epoch {
                            Some(epoch) => {
                                rsn.replay.install_group_epoch(key_id, epoch);
                                true
                            }
                            None => rsn
                                .replay
                                .install_group(key_id, *request.group().receive_sequence())
                                .is_ok(),
                        });
                    let completed = rsn.supplicant.complete_group_key_install(request, true);
                    match completed {
                        Ok(response) if installed => rsn.rekey = Rekey::Send(response),
                        _ => {
                            return self.set(leave_connected(
                                connected,
                                StaDisconnectReason::HandshakeFailed,
                                Some(REASON_UNSPECIFIED),
                            ));
                        }
                    }
                }
                State::Connected(connected)
            }
            state => {
                self.state = state;
                self.keys_changed();
                return;
            }
        };
    }

    /// A group key is removed: a rekey's slot is free, or its old key gone.
    fn group_removed(&mut self) {
        if let State::Connected(Connected { rsn: Some(rsn), .. }) = &mut self.state {
            rsn.rekey = match core::mem::replace(&mut rsn.rekey, Rekey::None) {
                Rekey::AwaitRetired => Rekey::None,
                rekey => rekey,
            };
        }
    }

    fn set(&mut self, state: State<T, K>) {
        self.state = state;
    }

    /// The arbiter's answer to the station's absence.
    fn absence_answered(&mut self, answer: AbsenceAnswer<K>) {
        let State::Connected(connected) = &mut self.state else {
            if let AbsenceAnswer::Refused(_, place) = answer {
                self.absence_place = Some(place);
            }
            return;
        };
        let Some(background) = &mut connected.background else {
            return;
        };
        match answer {
            AbsenceAnswer::Lease(lease) => {
                self.absence = Some(lease);
                background.step = BackgroundStep::NeedSleep;
            }
            // Not connected after all: the background scan ends.
            AbsenceAnswer::Refused(_, place) => {
                self.absence_place = Some(place);
                connected.background = None;
            }
        }
    }

    /// The background scan's step after its absence ended, `cancelled` or
    /// not.
    fn absence_over(&mut self, cancelled: bool) {
        self.absence = None;
        let limit = self.config.skipped_absences;
        let State::Connected(connected) = &mut self.state else {
            return;
        };
        let Some(background) = &mut connected.background else {
            return;
        };
        background.step = if cancelled {
            background.skipped = background.skipped.saturating_add(1);
            if connected.power.asleep() {
                // A PM=1 may have reached the access point: wake first.
                BackgroundStep::NeedWake
            } else if background.skipped >= limit {
                connected.background = None;
                self.notice_owed = Some(StaNotice::BackgroundScanDone { partial: true });
                return;
            } else {
                BackgroundStep::NeedAbsence
            }
        } else {
            background.skipped = 0;
            BackgroundStep::NeedHome
        };
    }

    /// A notice of the arbiter.
    fn noticed(&mut self, notice: StationNotice<K>) {
        match notice {
            StationNotice::AbsenceStarted(id) => {
                if self.absence.as_ref().map(Lease::id) == Some(id)
                    && let State::Connected(Connected {
                        background: Some(background),
                        ..
                    }) = &mut self.state
                    && background.step == BackgroundStep::AwaitStart
                {
                    background.step = BackgroundStep::NeedConfigure;
                }
                return;
            }
            StationNotice::AbsenceEnded(place) => {
                self.absence_place = Some(place);
                self.absence_over(false);
                return;
            }
            StationNotice::AbsenceCancelled(_, place) => {
                self.absence_place = Some(place);
                self.absence_over(true);
                return;
            }
            _ => {}
        }
        let state = self.take_state();
        self.state = match notice {
            StationNotice::LeaseStarted(id) => {
                if self.lease.id() != Some(id) {
                    state
                } else {
                    match state {
                        State::Scan(mut scan) if scan.step == ScanStep::AwaitStart => {
                            scan.skipped = 0;
                            scan.step = ScanStep::NeedConfigure;
                            State::Scan(scan)
                        }
                        State::Join(mut join) if matches!(join.step, JoinStep::AwaitStart) => {
                            join.step = JoinStep::NeedConfigure;
                            State::Join(join)
                        }
                        state => state,
                    }
                }
            }
            StationNotice::LeaseEnded(place) => {
                self.lease = Held::None;
                self.place = Some(place);
                self.lease_ended(state, false)
            }
            StationNotice::LeaseCancelled(_, place) => {
                self.lease = Held::None;
                self.place = Some(place);
                self.lease_ended(state, true)
            }
            StationNotice::Moved(channel) => match state {
                // The primary lease moves with the port: the station
                // exchanges it.
                State::Connected(mut connected) => {
                    connected.channel = channel;
                    connected.exchange = true;
                    State::Connected(connected)
                }
                state => state,
            },
            StationNotice::AbsenceStarted(_)
            | StationNotice::AbsenceEnded(..)
            | StationNotice::AbsenceCancelled(..) => state,
            StationNotice::Leave(reason) => {
                let reason_code = match reason {
                    // The port no longer reaches the access point.
                    LeaveReason::PortFailed => None,
                    LeaveReason::Unservable => Some(REASON_LEAVING),
                };
                match state {
                    State::Connected(connected) => leave_connected(
                        connected,
                        StaDisconnectReason::Arbiter(reason),
                        reason_code,
                    ),
                    State::Handshake(handshake) => {
                        let bssid = handshake.bssid;
                        leave_keyed(
                            StaDisconnectReason::Arbiter(reason),
                            reason_code.map(|reason_code| Deauthentication { bssid, reason_code }),
                        )
                    }
                    State::Join(_) => leave_unkeyed(StaDisconnectReason::Arbiter(reason)),
                    State::Scan(_) => leave_unkeyed(StaDisconnectReason::Arbiter(reason)),
                    state => state,
                }
            }
        };
    }

    /// The state after the lease ended, at its own end or `cancelled`.
    fn lease_ended(&mut self, state: State<T, K>, cancelled: bool) -> State<T, K> {
        let lost = StaDisconnectReason::Arbiter(LeaveReason::PortFailed);
        match state {
            // A skipped absence: the same channel again, until too many in
            // a row end the scan with what it found.
            State::Scan(scan) if cancelled => {
                let skipped = scan.skipped.saturating_add(1);
                if skipped >= self.config.skipped_absences {
                    self.select(true, scan.access_point)
                } else {
                    State::Scan(Scan {
                        skipped,
                        step: ScanStep::NeedLease,
                        ..scan
                    })
                }
            }
            // The visit's listening is over: the next channel.
            State::Scan(scan) => self.visit(scan.visit.saturating_add(1), 0, scan.access_point),
            State::Leave(leave) if leave.step == LeaveStep::AwaitEnd => self.report(leave),
            // A terminal may arrive while an earlier leave action is still
            // owed. Only AwaitEnd proves that deauthentication and receive
            // shutdown completed; all preceding steps retain their payload.
            State::Leave(leave) => State::Leave(leave),
            // The lease ended without the station's release: the port left;
            // the station leaves, its keys first.
            State::Connected(connected) => leave_connected(connected, lost, None),
            State::Handshake(_) => leave_keyed(lost, None),
            State::Join(_) => leave_unkeyed(lost),
            state => state,
        }
    }

    /// A leave reports once every key came back; until then it removes
    /// them.
    fn report(&mut self, leave: Leave) -> State<T, K> {
        if self.keys.clear() {
            State::Report(leave.reason)
        } else {
            State::Leave(Leave {
                step: LeaveStep::RemoveKeys(None),
                ..leave
            })
        }
    }

    /// The state after `done` in `state`.
    fn step(&mut self, state: State<T, K>, now: Instant, done: Done) -> State<T, K> {
        if let Done::Unsent { purpose, reason } = done {
            return self.unsent(state, now, purpose, reason);
        }
        match (state, done) {
            (State::Idle | State::Report(_), Done::Command(StaCommand::Connect)) => {
                self.epoch = Some(self.epochs.begin());
                self.scan.clear();
                self.visit(0, 0, None)
            }
            (State::Connected(connected), Done::Command(StaCommand::Disconnect)) => {
                leave_connected(
                    connected,
                    StaDisconnectReason::Requested,
                    Some(REASON_LEAVING),
                )
            }
            (State::Scan(_), Done::Command(StaCommand::Disconnect)) => {
                leave_unkeyed(StaDisconnectReason::Requested)
            }
            (State::Join(_), Done::Command(StaCommand::Disconnect)) => {
                leave_unkeyed(StaDisconnectReason::Requested)
            }
            (State::Connected(mut connected), Done::Command(StaCommand::BackgroundScan))
                if connected.background.is_none() =>
            {
                self.scan.clear();
                if self.channel(0).is_some() {
                    connected.background = Some(Background {
                        visit: 0,
                        skipped: 0,
                        step: BackgroundStep::NeedAbsence,
                    });
                } else {
                    self.notice_owed = Some(StaNotice::BackgroundScanDone { partial: false });
                }
                State::Connected(connected)
            }
            (State::Connected(connected), done) => self.connected_step(connected, now, done),
            (State::Scan(mut scan), Done::Configured) if scan.step == ScanStep::AwaitConfigured => {
                scan.step = ScanStep::NeedProbe;
                State::Scan(scan)
            }
            (State::Join(join), done) => self.join_step(join, now, done),
            (State::Handshake(handshake), Done::Command(StaCommand::Disconnect)) => {
                leave_handshake(handshake, REASON_LEAVING)
            }
            (State::Handshake(mut handshake), done) => match (&handshake.step, done) {
                (
                    HandshakeStep::AwaitSent,
                    Done::Tx {
                        purpose: TxPurpose::HandshakeResponse,
                        ..
                    },
                ) => {
                    handshake.step = HandshakeStep::Await;
                    State::Handshake(handshake)
                }
                // Only acknowledged M4 completes the association.
                (
                    HandshakeStep::AwaitLast(_),
                    Done::Tx {
                        purpose: TxPurpose::HandshakeLast,
                        completion,
                    },
                ) => match completion {
                    TxOutcome::Ended {
                        status: TxEndStatus::Delivered,
                        ..
                    } => connect(handshake, self.keys.has_retiring()),
                    TxOutcome::Withdrawn(oer_ieee80211_lower_mac::WithdrawCause::Disabled) => {
                        leave_keyed(
                            StaDisconnectReason::TransmitRefused(TxRefusal::LinkChanged),
                            None,
                        )
                    }
                    _ => {
                        if let HandshakeStep::AwaitLast(replay) =
                            core::mem::replace(&mut handshake.step, HandshakeStep::Await)
                        {
                            handshake.step = HandshakeStep::AwaitRetry(replay);
                        }
                        State::Handshake(handshake)
                    }
                },
                (_, Done::Tick)
                    if matches!(
                        handshake.deadline.poll(now),
                        RsnStaDeadlineEvent::Expired { .. }
                    ) =>
                {
                    leave_handshake(handshake, REASON_HANDSHAKE_TIMEOUT)
                }
                _ => State::Handshake(handshake),
            },
            (State::Leave(mut leave), done) => {
                leave.step = match (leave.step, done) {
                    (
                        LeaveStep::AwaitDeauthentication,
                        Done::Tx {
                            purpose: TxPurpose::Deauthentication,
                            ..
                        },
                    ) => LeaveStep::NeedConfigure,
                    (LeaveStep::AwaitConfigured, Done::Configured) => LeaveStep::NeedRelease,
                    (step, _) => step,
                };
                State::Leave(leave)
            }
            (state, _) => state,
        }
    }

    /// Refusal is a protocol input of its own. In particular, refusing M4
    /// never executes the transition for an accepted TX completion.
    fn unsent(
        &mut self,
        state: State<T, K>,
        now: Instant,
        purpose: TxPurpose,
        reason: TxRefusal,
    ) -> State<T, K> {
        match (state, purpose) {
            (
                State::Handshake(handshake),
                TxPurpose::HandshakeResponse | TxPurpose::HandshakeLast,
            ) => leave_handshake(handshake, REASON_UNSPECIFIED),
            (State::Join(_), TxPurpose::Authentication | TxPurpose::Association) => {
                leave_unkeyed(StaDisconnectReason::TransmitRefused(reason))
            }
            (State::Scan(_), TxPurpose::Probe) => {
                leave_unkeyed(StaDisconnectReason::TransmitRefused(reason))
            }
            (State::Connected(connected), TxPurpose::Connected(Sending::GroupResponse)) => {
                leave_connected(
                    connected,
                    StaDisconnectReason::HandshakeFailed,
                    Some(REASON_UNSPECIFIED),
                )
            }
            (State::Connected(connected), purpose @ TxPurpose::Connected(_)) => {
                self.connected_step(connected, now, Done::Unsent { purpose, reason })
            }
            (State::Leave(mut leave), TxPurpose::Deauthentication) => {
                if matches!(leave.step, LeaveStep::AwaitDeauthentication) {
                    leave.step = LeaveStep::NeedConfigure;
                }
                State::Leave(leave)
            }
            (state, _) => state,
        }
    }

    /// The connected state after `done`.
    fn connected_step(
        &mut self,
        mut connected: Connected<T, K>,
        now: Instant,
        done: Done,
    ) -> State<T, K> {
        let sent = match done {
            Done::Tx {
                purpose: TxPurpose::Connected(sending),
                completion,
            } => Some((
                sending,
                match completion {
                    TxOutcome::Ended {
                        status: TxEndStatus::Delivered,
                        ..
                    } => NullOutcome::Delivered,
                    _ => NullOutcome::Unconfirmed,
                },
            )),
            Done::Unsent {
                purpose: TxPurpose::Connected(sending),
                ..
            } => Some((sending, NullOutcome::Unconfirmed)),
            _ => None,
        };
        if let Some((sending, outcome)) = sent {
            connected.sending = false;
            match sending {
                Sending::Wake => {
                    connected.waking = false;
                    connected
                        .power
                        .null_sent(StaPowerManagement::Active, outcome);
                    if let Some(background) = &mut connected.background
                        && background.step == BackgroundStep::AwaitWake
                    {
                        self.next_visit(background);
                    }
                }
                Sending::Sleep => {
                    connected
                        .power
                        .null_sent(StaPowerManagement::PowerSave, outcome);
                    if let Some(background) = &mut connected.background
                        && background.step == BackgroundStep::AwaitSleep
                    {
                        // Not delivered: no readiness; the arbiter cancels
                        // the absence at its start.
                        background.step = if outcome == NullOutcome::Delivered {
                            BackgroundStep::NeedReady
                        } else {
                            BackgroundStep::AwaitStart
                        };
                    }
                }
                Sending::GroupResponse => {
                    if let Some(rsn) = &mut connected.rsn
                        && matches!(rsn.rekey, Rekey::AwaitSent)
                    {
                        rsn.rekey = if !self.keys.has_retiring() {
                            Rekey::None
                        } else {
                            Rekey::Retire
                        };
                    }
                }
                Sending::Probe => {}
            }
            if let Some(background) = &connected.background
                && background.step == BackgroundStep::NeedAbsence
                && background.skipped >= self.config.skipped_absences
            {
                connected.background = None;
                self.notice_owed = Some(StaNotice::BackgroundScanDone { partial: true });
            }
            return self.background_done(connected);
        }
        let dwell = self.config.dwell;
        if let Some(background) = &mut connected.background {
            background.step = match (background.step, done) {
                (BackgroundStep::AwaitConfigured, Done::Configured) => BackgroundStep::NeedProbe,
                (BackgroundStep::AwaitHome, Done::Configured) => BackgroundStep::NeedWake,
                (BackgroundStep::AwaitWidened, Done::Configured) => BackgroundStep::HomeProbe {
                    until: now.checked_add(dwell).unwrap_or(now),
                },
                (
                    BackgroundStep::HomeProbe { until } | BackgroundStep::HomeListen { until },
                    Done::Tick,
                ) if now >= until => BackgroundStep::Narrow,
                (BackgroundStep::AwaitNarrowed, Done::Configured) => {
                    self.next_visit(background);
                    background.step
                }
                (step, _) => step,
            };
        }
        self.background_done(connected)
    }

    /// The next visit of the background scan, or its end.
    fn next_visit(&mut self, background: &mut Background) {
        let visit = if background.skipped > 0 {
            background.visit
        } else {
            background.visit.saturating_add(1)
        };
        background.visit = visit;
        background.step = BackgroundStep::NeedAbsence;
    }

    /// End a background scan that ran out of channels or of tries.
    fn background_done(&mut self, mut connected: Connected<T, K>) -> State<T, K> {
        if let Some(background) = &connected.background
            && background.step == BackgroundStep::NeedAbsence
        {
            if background.skipped >= self.config.skipped_absences {
                connected.background = None;
                self.notice_owed = Some(StaNotice::BackgroundScanDone { partial: true });
            } else if self.channel(background.visit).is_none() {
                connected.background = None;
                self.notice_owed = Some(StaNotice::BackgroundScanDone { partial: false });
            }
        }
        State::Connected(connected)
    }

    /// The scan's visit of the channel at `visit`, or its end.
    fn visit(
        &mut self,
        visit: usize,
        skipped: u8,
        access_point: Option<SupportedChannel<K>>,
    ) -> State<T, K> {
        match self.channel(visit) {
            Some(_) => State::Scan(Scan {
                visit,
                skipped,
                step: ScanStep::NeedLease,
                access_point,
            }),
            None => self.select(false, access_point),
        }
    }

    /// The join of the best access point of the network the scan found:
    /// the strongest, unless one on the channel of the access point beside
    /// the station is within the margin of it.
    fn select(
        &mut self,
        partial: bool,
        access_point_channel: Option<SupportedChannel<K>>,
    ) -> State<T, K> {
        let ssid = self.ssid();
        let policy = self.policy();
        let candidates = || {
            self.scan
                .records()
                .iter()
                .filter(move |record| record.ssid_bytes() == ssid)
                .filter(move |record| select_association_rsn(record, policy).is_ok())
        };
        let Some(strongest) = candidates().max_by_key(|record| record.rssi).copied() else {
            return leave_unkeyed(StaDisconnectReason::NotFound { partial });
        };
        let margin = i16::from(self.config.access_point_margin_db);
        let beside = access_point_channel.and_then(|channel| {
            candidates()
                .filter(|record| record.channel == channel.channel().number())
                .filter(|record| {
                    i16::from(record.rssi) >= i16::from(strongest.rssi).saturating_sub(margin)
                })
                .max_by_key(|record| record.rssi)
                .copied()
        });
        let access_point = beside.unwrap_or(strongest);
        let Ok(rsn) = select_association_rsn(&access_point, policy) else {
            return leave_unkeyed(StaDisconnectReason::NotFound { partial });
        };
        let Some(channel) = self
            .config
            .channels
            .iter()
            .copied()
            .find(|channel| channel.channel().number() == access_point.channel)
        else {
            return leave_unkeyed(StaDisconnectReason::NotFound { partial });
        };
        State::Join(Join {
            access_point,
            rsn,
            channel,
            step: JoinStep::NeedLease,
        })
    }

    fn join_step(&mut self, mut join: Join<T, K>, now: Instant, done: Done) -> State<T, K> {
        join.step = match (
            core::mem::replace(&mut join.step, JoinStep::NeedLease),
            done,
        ) {
            (JoinStep::AwaitConfigured, Done::Configured) => {
                JoinStep::Authenticate(Authenticate::NeedCredit(StaAuthenticationRuntime::new(
                    self.config.address,
                    join.access_point.bssid,
                )))
            }
            (JoinStep::Authenticate(Authenticate::Exchange { runtime, mut stage }), done) => {
                let timed_out = match (stage, done) {
                    (
                        AuthStage::AwaitSent { response_timeout },
                        Done::Tx {
                            purpose: TxPurpose::Authentication,
                            completion:
                                TxOutcome::Ended {
                                    status: TxEndStatus::Delivered,
                                    ..
                                },
                        },
                    ) => {
                        stage = AuthStage::AwaitResponse {
                            until: now.checked_add(response_timeout).unwrap_or(now),
                        };
                        false
                    }
                    (
                        AuthStage::AwaitSent { .. },
                        Done::Tx {
                            purpose: TxPurpose::Authentication,
                            ..
                        },
                    ) => true,
                    (AuthStage::AwaitResponse { until }, Done::Tick) => now >= until,
                    _ => false,
                };
                let auth = if timed_out {
                    match runtime.response_timed_out() {
                        StaAuthenticationEvent::Retry { runtime, .. } => {
                            Authenticate::NeedCredit(runtime)
                        }
                        _ => return leave_unkeyed(StaDisconnectReason::AuthenticationFailed),
                    }
                } else {
                    Authenticate::Exchange { runtime, stage }
                };
                JoinStep::Authenticate(auth)
            }
            (step, _) => step,
        };
        State::Join(join)
    }

    /// Admit a received frame once, process station EAPOL/management, and
    /// deliver each remaining MSDU. Refusals retain their typed reason.
    ///
    /// Data cannot be admitted again through the old second RX entry point:
    ///
    /// ```compile_fail,E0425
    /// let _ = oer_ieee80211_sta::data::received;
    /// ```
    ///
    /// ```
    /// use oer_ieee80211_sta::machine::{Station, StaTokens};
    /// use oer_ieee80211_lower_mac::Ieee80211Capabilities;
    /// fn receive<T: StaTokens, K: Ieee80211Capabilities>(
    ///     station: &mut Station<'_, T, K>, now: oer_time::Instant,
    ///     frame: &[u8], meta: T::RxMeta,
    /// ) {
    ///     let _ = station.receive(now, frame, meta, |_| {});
    /// }
    /// ```
    pub fn receive<'frame>(
        &mut self,
        now: Instant,
        frame: &'frame [u8],
        meta: T::RxMeta,
        mut deliver: impl FnMut(oer_ieee80211_mac::data::EthernetFrameParts<'frame>),
    ) -> Result<(), oer_ieee80211_mac::receive::RxDropReason> {
        use oer_ieee80211_mac::{
            frame::DecodedHeader,
            receive::{AdmittedData, RxDropReason},
        };
        let header = DecodedHeader::decode(frame).ok_or(RxDropReason::Header)?;
        if !header.control().is_data() {
            self.received_management(now, frame, meta);
            return Ok(());
        }
        let (bssid, replay) = match &mut self.state {
            State::Handshake(handshake) => (
                handshake.bssid,
                match &mut handshake.step {
                    HandshakeStep::AwaitLast(replay)
                    | HandshakeStep::AwaitRetry(replay)
                    | HandshakeStep::SendLast(_, replay) => Some(replay),
                    HandshakeStep::InstallGroup(_, replay)
                    | HandshakeStep::AwaitInstalled(_, replay, _) => replay.as_mut(),
                    _ => None,
                },
            ),
            State::Connected(connected) => (
                connected.bssid,
                connected.rsn.as_mut().map(|rsn| &mut rsn.replay),
            ),
            _ => return Err(RxDropReason::NotConnected),
        };
        let admitted = AdmittedData::admit(
            header,
            self.config.address,
            bssid,
            crate::data::integrity(meta.common()),
            replay,
        )?;
        for parts in admitted.msdus()? {
            let parts = parts?;
            if parts.ether_type == EAPOL_ETHER_TYPE {
                let eapol =
                    OwnedEapolFrame::try_copy(RsnInterface::Station, bssid.get(), parts.payload)
                        .map_err(|_| RxDropReason::Payload)?;
                let state = self.take_state();
                self.state = match state {
                    State::Handshake(handshake) => self.handshake_eapol(handshake, now, eapol),
                    State::Connected(connected) => self.group_message(connected, eapol),
                    state => state,
                };
            } else {
                if !matches!(self.config.security, StaSecurity::Open) && !admitted.protected() {
                    return Err(RxDropReason::Protection);
                }
                if self.link().is_none() {
                    return Err(RxDropReason::NotConnected);
                }
                deliver(parts);
            }
        }
        Ok(())
    }

    /// Take a received frame.
    fn received_management(&mut self, now: Instant, frame: &[u8], meta: T::RxMeta) {
        let rssi = match meta.common().rssi_dbm {
            RxEvidence::HardwareObserved(rssi) | RxEvidence::ProtocolValidated(rssi) => rssi,
            RxEvidence::Unavailable => i8::MIN,
        };
        let state = self.take_state();
        self.state = match state {
            State::Scan(scan)
                if scan.step == ScanStep::Listen || scan.step == ScanStep::NeedProbe =>
            {
                let channel = self
                    .channel(scan.visit)
                    .map_or(0, |channel| channel.channel().number());
                self.scan
                    .observe_management(frame, channel, rssi, Some(now));
                State::Scan(scan)
            }
            State::Join(mut join) => {
                match core::mem::replace(&mut join.step, JoinStep::NeedLease) {
                    JoinStep::Authenticate(Authenticate::NeedCredit(runtime)) => {
                        join.step = JoinStep::Authenticate(Authenticate::NeedCredit(runtime));
                        State::Join(join)
                    }
                    JoinStep::Authenticate(Authenticate::Exchange { mut runtime, stage }) => {
                        runtime.observe_received_frame();
                        match runtime.observe_management_frame(frame) {
                            StaAuthenticationEvent::Authenticated { .. } => {
                                join.step = JoinStep::Associate(StaAssociationRuntime::new(
                                    self.config.address,
                                    join.access_point.bssid,
                                    match self.config.security {
                                        StaSecurity::Open => {
                                            oer_ieee80211_mac::security::LinkProtection::Open
                                        }
                                        StaSecurity::Wpa2Personal { .. } => {
                                            oer_ieee80211_mac::security::LinkProtection::Ccmp
                                        }
                                    },
                                    now,
                                ));
                                State::Join(join)
                            }
                            StaAuthenticationEvent::Retry { runtime, .. } => {
                                join.step =
                                    JoinStep::Authenticate(Authenticate::NeedCredit(runtime));
                                State::Join(join)
                            }
                            StaAuthenticationEvent::Failed { .. } => {
                                leave_unkeyed(StaDisconnectReason::AuthenticationFailed)
                            }
                            StaAuthenticationEvent::Irrelevant(runtime) => {
                                join.step = JoinStep::Authenticate(Authenticate::Exchange {
                                    runtime,
                                    stage,
                                });
                                State::Join(join)
                            }
                        }
                    }
                    JoinStep::Associate(mut runtime) => {
                        runtime.observe_received_frame();
                        match runtime.observe_management_frame(frame, now) {
                            Ok(StaAssociationEvent::Associated { response, .. }) => {
                                match StaAssociatedPeer::derive(
                                    &join.access_point,
                                    &response,
                                    self.config.phy,
                                    self.config.he_packet_padding,
                                ) {
                                    Ok(associated) => {
                                        join.step = JoinStep::NotifyAssociated {
                                            association_id: response.association_id,
                                            // The rate selection starts from the
                                            // response's metadata.
                                            peer: Peer {
                                                associated,
                                                response: meta,
                                            },
                                        };
                                        State::Join(join)
                                    }
                                    Err(_) => leave_unkeyed(StaDisconnectReason::AssociationFailed),
                                }
                            }
                            Ok(StaAssociationEvent::Failed { .. }) | Err(_) => {
                                leave_unkeyed(StaDisconnectReason::AssociationFailed)
                            }
                            Ok(StaAssociationEvent::Irrelevant) => {
                                join.step = JoinStep::Associate(runtime);
                                State::Join(join)
                            }
                        }
                    }
                    step => {
                        join.step = step;
                        State::Join(join)
                    }
                }
            }
            State::Handshake(handshake) => match peer_disconnect(frame, handshake.bssid) {
                Some(reason_code) => leave_keyed(StaDisconnectReason::Peer { reason_code }, None),
                None => State::Handshake(handshake),
            },
            State::Connected(connected) => self.connected_received(connected, now, frame, rssi),
            state => state,
        };
    }

    /// The connected state after a received frame.
    fn connected_received(
        &mut self,
        mut connected: Connected<T, K>,
        now: Instant,
        frame: &[u8],
        rssi: i8,
    ) -> State<T, K> {
        if let Some(reason_code) = peer_disconnect(frame, connected.bssid) {
            // The access point ended the link: no Deauthentication back.
            return leave_connected(connected, StaDisconnectReason::Peer { reason_code }, None);
        }
        let step = connected
            .background
            .as_ref()
            .map(|background| background.step);
        let away = matches!(
            step,
            Some(BackgroundStep::NeedProbe | BackgroundStep::Listen)
        );
        let home = matches!(
            step,
            Some(BackgroundStep::HomeProbe { .. } | BackgroundStep::HomeListen { .. })
        );
        if away {
            let channel = self
                .absence
                .as_ref()
                .map_or(0, |absence| absence.channel().channel().number());
            self.scan
                .observe_management(frame, channel, rssi, Some(now));
            return State::Connected(connected);
        }
        if home {
            let channel = connected.channel.channel().number();
            self.scan
                .observe_management(frame, channel, rssi, Some(now));
        }
        if let Ok(beacon) = parse_sta_beacon(frame, connected.bssid.get(), connected.association_id)
            && connected.power.beacon(beacon.tim) == PowerSaveDecision::SendActive
            && !connected.waking
        {
            connected.wake = true;
        }
        State::Connected(connected)
    }

    /// A connected station's EAPOL frame: a group key's handshake.
    fn group_message(
        &mut self,
        mut connected: Connected<T, K>,
        eapol: OwnedEapolFrame<DEFAULT_EAPOL_FRAME_CAPACITY>,
    ) -> State<T, K> {
        let Some(rsn) = &mut connected.rsn else {
            return State::Connected(connected);
        };
        if !matches!(rsn.rekey, Rekey::None) {
            return State::Connected(connected);
        }
        rsn.rekey = match rsn.supplicant.on_group_message1(eapol) {
            // The key data, unwrapped in the step: a few AES blocks.
            Ok(RsnConnectedAction::UnwrapGroupKeyData(request)) => {
                let unwrapped =
                    software_aes128_key_unwrap(request.kek(), request.wrapped_key_data());
                match rsn
                    .supplicant
                    .complete_group_key_data_unwrap(request, unwrapped)
                {
                    Ok(install) => Rekey::Install(install),
                    // Unusable key data: nothing changes.
                    Err(_) => Rekey::None,
                }
            }
            Ok(RsnConnectedAction::Retransmit(response)) => Rekey::Send(response),
            // A frame the supplicant rejects changes nothing.
            Err(_) => Rekey::None,
        };
        State::Connected(connected)
    }

    fn configure(&mut self, receive: StationReceive<K>) -> Option<Owed<'id, T, K>> {
        let place = self.configuration.take()?;
        let epoch = if matches!(receive, StationReceive::Off) {
            None
        } else {
            self.epoch
        };
        Some(Owed::Configure(
            receive,
            PendingConfiguration { place, epoch },
        ))
    }

    /// The next action to carry out at `now`, if any. `None` means a wait
    /// for an input, a returned resource, or the state's deadline.
    ///
    /// Internal transitions form an acyclic graph. Join completion enters
    /// Handshake/Connected; preparation failure enters Leave; Leave moves
    /// through RemoveKeys, NeedDeauthentication and NeedConfigure, or from
    /// NeedRelease to Report. The longest path is five steps: connected
    /// failure, an already ended absence, returned key slots, failed frame
    /// encoding, configuration. Every back edge requires an external
    /// completion or a new command. Associated is its own join step before
    /// either Connected or the first handshake action.
    #[must_use = "perform the returned action before waiting for another input"]
    pub fn poll_action(&mut self, now: Instant) -> Option<StaAction<'_, 'id, T, K>> {
        let owed = loop {
            if let Some(notice) = self.notice_owed.take() {
                return Some(StaAction::Notify(notice));
            }
            let state = self.take_state();
            let (state, progress) = self.owe(state, now);
            self.state = state;
            match progress {
                ActionProgress::Action(owed) => break owed,
                ActionProgress::Transition => continue,
                ActionProgress::Wait => return None,
            }
        };
        Some(match owed {
            // Owed only while the station holds its primary lease; without
            // it the place stays, and nothing is asked.
            Owed::Absence(request, place) => match &self.lease {
                Held::Lease(home) => StaAction::RequestAbsence {
                    request,
                    home,
                    place,
                },
                _ => {
                    self.absence_place = Some(place);
                    return None;
                }
            },
            Owed::Ready(id) => StaAction::Ready(id),
            Owed::InstallPairwise {
                pending,
                slot,
                peer,
                key,
            } => StaAction::InstallPairwiseKey {
                pending,
                slot,
                peer,
                key,
            },
            Owed::InstallGroup {
                pending,
                slot,
                key_id,
                key,
            } => StaAction::InstallGroupKey {
                pending,
                slot,
                key_id,
                key,
            },
            Owed::RemovePairwise(key) => StaAction::RemovePairwiseKey(key),
            Owed::ReplaceGroup {
                pending,
                installed,
                key_id,
                key,
            } => StaAction::ReplaceGroupKey {
                pending,
                installed,
                key_id,
                key,
            },
            Owed::RemoveGroup(key) => StaAction::RemoveGroupKey(key),
            Owed::Exchange(lease) => StaAction::ExchangeLease(lease),
            Owed::Request(request, place) => StaAction::RequestChannel(request, place),
            Owed::Release(lease) => StaAction::ReleaseChannel(lease),
            Owed::Upstream(fact) => StaAction::Upstream(fact),
            Owed::Configure(receive, pending) => StaAction::Configure(receive, pending),
            Owed::Transmit(transmit) => StaAction::Transmit(transmit),
            // Owed only while the link is up in this very step, so the key
            // is there; without it the frame does not go and its tokens
            // come back.
            Owed::TransmitProtected(transmit) => match &self.keys.pairwise {
                KeyCell::Installed(key) => StaAction::TransmitProtected { transmit, key },
                _ => {
                    self.tickets.put(transmit.ticket);
                    self.buffers.put(transmit.buffer);
                    return None;
                }
            },
            Owed::Notify(notice) => StaAction::Notify(notice),
            Owed::Associated(peer) => StaAction::Associated(peer),
        })
    }

    /// The action `state` owes, and the state that awaits its end.
    fn owe(&mut self, state: State<T, K>, now: Instant) -> ActionStep<'id, T, K> {
        match state {
            State::Report(reason) => {
                self.epoch = None;
                (
                    State::Idle,
                    ActionProgress::Action(Owed::Notify(StaNotice::Disconnected(reason))),
                )
            }
            State::Scan(mut scan) => {
                let owed = match scan.step {
                    ScanStep::NeedLease if self.place.is_some() => {
                        match (self.channel(scan.visit), self.place.take()) {
                            (Some(channel), Some(place)) => {
                                scan.step = ScanStep::AwaitAnswer;
                                Some(Owed::Request(
                                    ChannelRequest {
                                        channel,
                                        purpose: Purpose::Scan {
                                            dwell: self.config.dwell,
                                        },
                                    },
                                    place,
                                ))
                            }
                            (_, place) => {
                                self.place = place;
                                None
                            }
                        }
                    }
                    ScanStep::NeedConfigure if self.configuration.is_some() => {
                        scan.step = ScanStep::AwaitConfigured;
                        self.configure(StationReceive::OtherBssManagement)
                    }
                    ScanStep::NeedProbe => match self.encode_probe(TxPurpose::Probe) {
                        Ok(transmit) => {
                            scan.step = ScanStep::Listen;
                            Some(Owed::Transmit(transmit))
                        }
                        Err(Unsent::NoTokens) => None,
                        Err(Unsent::Encode) => {
                            return (
                                leave_unkeyed(StaDisconnectReason::Encode),
                                ActionProgress::Transition,
                            );
                        }
                        Err(Unsent::NoConnection) => {
                            return (
                                leave_unkeyed(StaDisconnectReason::TransmitRefused(
                                    TxRefusal::LinkChanged,
                                )),
                                ActionProgress::Transition,
                            );
                        }
                    },
                    _ => None,
                };
                (State::Scan(scan), owed.into())
            }
            State::Join(join) => self.owe_join(join, now),
            State::Handshake(handshake) => self.owe_handshake(handshake),
            State::Connected(connected) => self.owe_connected(connected),
            State::Leave(leave) => self.owe_leave(leave),
            state => (state, ActionProgress::Wait),
        }
    }

    /// The action the leave owes.
    fn owe_leave(&mut self, mut leave: Leave) -> ActionStep<'id, T, K> {
        let owed = match leave.step {
            LeaveStep::AwaitAbsenceEnd(deauthentication) if self.absence.is_none() => {
                leave.step = LeaveStep::RemoveKeys(deauthentication);
                return (State::Leave(leave), ActionProgress::Transition);
            }
            LeaveStep::RemoveKeys(deauthentication) => {
                if let Some(key) = self.keys.take_pairwise_key() {
                    Some(Owed::RemovePairwise(key))
                } else if let Some((_, key)) = self.keys.take_group_key() {
                    Some(Owed::RemoveGroup(key))
                } else if self.keys.clear() {
                    // Every key is back: the next step owes the action.
                    leave.step = match deauthentication {
                        Some(deauthentication) => LeaveStep::NeedDeauthentication(deauthentication),
                        None => LeaveStep::NeedConfigure,
                    };
                    return (State::Leave(leave), ActionProgress::Transition);
                } else {
                    None
                }
            }
            LeaveStep::NeedDeauthentication(Deauthentication { bssid, reason_code }) => {
                match self.encode_management(
                    StaManagementSubtype::Deauthentication,
                    bssid,
                    &reason_code.to_le_bytes(),
                    TxPurpose::Deauthentication,
                ) {
                    Ok(transmit) => {
                        leave.step = LeaveStep::AwaitDeauthentication;
                        Some(Owed::Transmit(transmit))
                    }
                    Err(Unsent::NoTokens) => None,
                    Err(Unsent::Encode | Unsent::NoConnection) => {
                        leave.step = LeaveStep::NeedConfigure;
                        return (State::Leave(leave), ActionProgress::Transition);
                    }
                }
            }
            LeaveStep::NeedConfigure if self.configuration.is_some() => {
                leave.step = LeaveStep::AwaitConfigured;
                self.configure(StationReceive::Off)
            }
            LeaveStep::NeedRelease => match core::mem::replace(&mut self.lease, Held::None) {
                Held::Lease(lease) => {
                    self.lease = Held::Released(lease.id());
                    leave.step = LeaveStep::AwaitEnd;
                    Some(Owed::Release(lease))
                }
                // No lease to give back: done.
                held => {
                    self.lease = held;
                    return (self.report(leave), ActionProgress::Transition);
                }
            },
            _ => None,
        };
        (State::Leave(leave), owed.into())
    }

    /// The handshake's state after a received frame.
    fn handshake_eapol(
        &mut self,
        mut handshake: Handshake<T, K>,
        now: Instant,
        eapol: OwnedEapolFrame<DEFAULT_EAPOL_FRAME_CAPACITY>,
    ) -> State<T, K> {
        if !matches!(
            handshake.step,
            HandshakeStep::Await | HandshakeStep::AwaitSent | HandshakeStep::AwaitRetry(_)
        ) {
            return State::Handshake(handshake);
        }
        if matches!(handshake.step, HandshakeStep::AwaitRetry(_))
            && eapol.key_frame().message() != oer_ieee80211_rsn::EapolKeyMessage::PairwiseMessage3
        {
            return State::Handshake(handshake);
        }
        let StaSecurity::Wpa2Personal { pmk } = &self.config.security else {
            return State::Handshake(handshake);
        };
        let action = match handshake.supplicant.on_frame(eapol, pmk) {
            // Message 3's key data, unwrapped in the step: a few AES blocks.
            Ok(RsnStaSupplicantAction::UnwrapKeyData(request)) => {
                let unwrapped =
                    software_aes128_key_unwrap(request.kek(), request.wrapped_key_data());
                match handshake
                    .supplicant
                    .complete_key_data_unwrap(request, unwrapped)
                {
                    Ok(install) => RsnStaSupplicantAction::InstallKeys(install),
                    Err(_) => return leave_handshake(handshake, REASON_UNSPECIFIED),
                }
            }
            Ok(action) => action,
            // A peer frame the supplicant rejects changes nothing.
            Err(error) if error.is_peer_input_rejection() => RsnStaSupplicantAction::None,
            Err(_) => return leave_handshake(handshake, REASON_UNSPECIFIED),
        };
        match action {
            RsnStaSupplicantAction::Transmit(message) => {
                match core::mem::replace(&mut handshake.step, HandshakeStep::Await) {
                    HandshakeStep::AwaitRetry(replay) => {
                        handshake.step = HandshakeStep::SendLast(message, replay);
                    }
                    _ => {
                        handshake.step = HandshakeStep::Send(message);
                        if let Some(deadline) =
                            RsnStaResponseDeadline::start(RsnStaResponseWait::Message3, now)
                        {
                            handshake.deadline = deadline;
                        }
                    }
                }
            }
            RsnStaSupplicantAction::InstallKeys(request) => {
                handshake.step = match core::mem::replace(&mut handshake.step, HandshakeStep::Await)
                {
                    HandshakeStep::AwaitRetry(replay) => {
                        HandshakeStep::InstallGroup(request, Some(replay))
                    }
                    _ => HandshakeStep::Install(request),
                };
            }
            RsnStaSupplicantAction::Deauthenticate => {
                return leave_handshake(handshake, REASON_UNSPECIFIED);
            }
            RsnStaSupplicantAction::None | RsnStaSupplicantAction::UnwrapKeyData(_) => {}
        }
        State::Handshake(handshake)
    }

    /// The action the handshake owes.
    fn owe_handshake(&mut self, mut handshake: Handshake<T, K>) -> ActionStep<'id, T, K> {
        let (step, owed) = match core::mem::replace(&mut handshake.step, HandshakeStep::Await) {
            HandshakeStep::Send(message) => {
                match self.encode_eapol(
                    handshake.bssid,
                    message.as_bytes(),
                    TxPurpose::HandshakeResponse,
                ) {
                    Ok(transmit) => (HandshakeStep::AwaitSent, Some(Owed::Transmit(transmit))),
                    Err(Unsent::NoTokens) => (HandshakeStep::Send(message), None),
                    Err(Unsent::Encode | Unsent::NoConnection) => {
                        return (
                            leave_handshake(handshake, REASON_UNSPECIFIED),
                            ActionProgress::Transition,
                        );
                    }
                }
            }
            HandshakeStep::Install(request) => {
                let peer = handshake.bssid.individual();
                match self
                    .current_epoch()
                    .and_then(|epoch| self.keys.take_pairwise_slot(epoch))
                {
                    Some((slot, pending)) => {
                        let key = *request.pairwise().key().as_bytes();
                        (
                            HandshakeStep::InstallGroup(request, None),
                            Some(Owed::InstallPairwise {
                                pending,
                                slot,
                                peer,
                                key,
                            }),
                        )
                    }
                    None => (HandshakeStep::Install(request), None),
                }
            }
            HandshakeStep::InstallGroup(request, replay) => {
                let Some((key_id, key)) = group_key(request.group()) else {
                    return (
                        leave_handshake(handshake, REASON_UNSPECIFIED),
                        ActionProgress::Transition,
                    );
                };
                if self.keys.groups.same_material(key_id, &key) {
                    // A repeated M3 acknowledges installed material without
                    // resetting either the PTK or GTK replay frontiers.
                    return (
                        finish_handshake_install(handshake, request, replay),
                        ActionProgress::Transition,
                    );
                }
                if let Some((slot, pending)) = self
                    .current_epoch()
                    .and_then(|epoch| self.keys.take_group_slot(epoch, key_id, key))
                {
                    (
                        HandshakeStep::AwaitInstalled(request, replay, None),
                        Some(Owed::InstallGroup {
                            pending,
                            slot,
                            key_id,
                            key,
                        }),
                    )
                } else {
                    match self.reserve_group_replacement(
                        key_id,
                        key,
                        *request.group().receive_sequence(),
                    ) {
                        Ok(Some((pending, installed))) => {
                            let previous_id = pending.previous_id;
                            (
                                HandshakeStep::AwaitInstalled(request, replay, Some(previous_id)),
                                Some(Owed::ReplaceGroup {
                                    pending,
                                    installed,
                                    key_id,
                                    key,
                                }),
                            )
                        }
                        Ok(None) => (HandshakeStep::InstallGroup(request, replay), None),
                        Err(()) => {
                            return (
                                leave_handshake(handshake, REASON_UNSPECIFIED),
                                ActionProgress::Transition,
                            );
                        }
                    }
                }
            }
            HandshakeStep::SendLast(message, epoch) => {
                match self.encode_eapol(
                    handshake.bssid,
                    message.as_bytes(),
                    TxPurpose::HandshakeLast,
                ) {
                    Ok(transmit) => (
                        HandshakeStep::AwaitLast(epoch),
                        Some(Owed::Transmit(transmit)),
                    ),
                    Err(Unsent::NoTokens) => (HandshakeStep::SendLast(message, epoch), None),
                    Err(Unsent::Encode | Unsent::NoConnection) => {
                        return (
                            leave_handshake(handshake, REASON_UNSPECIFIED),
                            ActionProgress::Transition,
                        );
                    }
                }
            }
            step => (step, None),
        };
        handshake.step = step;
        (State::Handshake(handshake), owed.into())
    }

    /// An EAPOL frame to `bssid` in a data frame, in a buffer.
    fn encode_eapol(
        &mut self,
        bssid: Bssid,
        eapol: &[u8],
        purpose: TxPurpose,
    ) -> Result<StaTransmit<'id, T, K>, Unsent> {
        let credit = self.reserve_transmit()?;
        let address = self.config.address;
        let sequence_number = self.sequence.take();
        Ok(self.encode_reserved(
            credit,
            |frame| {
                oer_ieee80211_mac::station::StaDataFrame {
                    source: address,
                    bssid,
                    destination: bssid.get(),
                    sequence_number,
                    ether_type: EAPOL_ETHER_TYPE,
                    payload: eapol,
                }
                .encode(frame)
                .ok()
            },
            StaFrameClass::Connection,
            purpose,
            StaRatePlan::Management,
        )?)
    }

    /// The action the connected state owes.
    fn owe_connected(&mut self, mut connected: Connected<T, K>) -> ActionStep<'id, T, K> {
        if connected.connected_notice {
            connected.connected_notice = false;
            let notice = StaNotice::Connected {
                bssid: connected.bssid,
                channel: connected.channel.channel().number(),
            };
            return (
                State::Connected(connected),
                ActionProgress::Action(Owed::Notify(notice)),
            );
        }
        if let Some(fact) = connected.report.take() {
            return (
                State::Connected(connected),
                ActionProgress::Action(Owed::Upstream(fact)),
            );
        }
        if connected.exchange {
            match core::mem::replace(&mut self.lease, Held::None) {
                Held::Lease(lease) => {
                    connected.exchange = false;
                    return (
                        State::Connected(connected),
                        ActionProgress::Action(Owed::Exchange(lease)),
                    );
                }
                held => self.lease = held,
            }
        }
        if let Some(owed) = self.owe_rekey(&mut connected) {
            return match owed {
                Ok(owed) => (State::Connected(connected), ActionProgress::Action(owed)),
                Err(reason) => (
                    leave_connected(connected, reason, Some(REASON_UNSPECIFIED)),
                    ActionProgress::Transition,
                ),
            };
        }
        match self.owe_background(&mut connected) {
            Ok(owed) => (State::Connected(connected), owed.into()),
            Err(reason) => (
                leave_connected(connected, reason, Some(REASON_UNSPECIFIED)),
                ActionProgress::Transition,
            ),
        }
    }

    /// The action of a group key's handshake, if one is owed; a failure's
    /// reason to leave.
    fn reserve_group_replacement(
        &mut self,
        key_id: GroupKeyId,
        material: [u8; 16],
        rsc: [u8; 8],
    ) -> Result<Option<GroupReplacement<'id, T>>, ()> {
        let replay = oer_ieee80211_mac::ccmp::CcmpRxReplayState::from_receive_sequence(rsc)
            .map_err(|_| ())?;
        let Some(epoch) = self.current_epoch() else {
            return Ok(None);
        };
        let Some(InstalledGroup {
            place,
            key_id: previous_id,
            key,
            ..
        }) = self
            .keys
            .groups
            .replace(key_id, self.keys.group_slots.capacity == 1)
        else {
            return Ok(None);
        };
        Ok(Some((
            PendingGroupReplacement {
                install: PendingGroupKey {
                    epoch,
                    key_id,
                    material,
                    place,
                },
                previous_id,
                replay,
            },
            key,
        )))
    }

    fn owe_rekey(
        &mut self,
        connected: &mut Connected<T, K>,
    ) -> Option<Result<Owed<'id, T, K>, StaDisconnectReason>> {
        let rsn = connected.rsn.as_mut()?;
        let (rekey, owed) = match core::mem::replace(&mut rsn.rekey, Rekey::None) {
            Rekey::Install(request) => {
                let Some((key_id, key)) = group_key(request.group()) else {
                    return Some(Err(StaDisconnectReason::HandshakeFailed));
                };
                if self.keys.groups.same_material(key_id, &key) {
                    rsn.rekey = match rsn.supplicant.complete_group_key_install(request, true) {
                        Ok(response) => Rekey::Send(response),
                        Err(_) => return Some(Err(StaDisconnectReason::HandshakeFailed)),
                    };
                    return self.owe_rekey(connected);
                }
                if let Some((slot, pending)) = self
                    .current_epoch()
                    .and_then(|epoch| self.keys.take_group_slot(epoch, key_id, key))
                {
                    (
                        Rekey::AwaitInstalled(request),
                        Some(Owed::InstallGroup {
                            pending,
                            slot,
                            key_id,
                            key,
                        }),
                    )
                } else {
                    match self.reserve_group_replacement(
                        key_id,
                        key,
                        *request.group().receive_sequence(),
                    ) {
                        Ok(Some((pending, installed))) => (
                            Rekey::AwaitInstalled(request),
                            Some(Owed::ReplaceGroup {
                                pending,
                                installed,
                                key_id,
                                key,
                            }),
                        ),
                        Ok(None) => (Rekey::Install(request), None),
                        Err(()) => return Some(Err(StaDisconnectReason::HandshakeFailed)),
                    }
                }
            }
            // After the handshake, EAPOL goes under the pairwise key, as in
            // the old ESP32-S31 path (`transmit_protected_data`): only while
            // the link is up.
            Rekey::Send(message)
                if !connected.sending && self.link_up(&connected.power, &self.keys) =>
            {
                let qos = connected.qos.then_some(&mut connected.qos_sequences);
                match self.encode_protected_eapol(connected.bssid, qos, message.as_bytes()) {
                    Ok(transmit) => {
                        connected.sending = true;
                        (Rekey::AwaitSent, Some(Owed::TransmitProtected(transmit)))
                    }
                    Err(Unsent::NoTokens) => (Rekey::Send(message), None),
                    Err(Unsent::Encode) => return Some(Err(StaDisconnectReason::Encode)),
                    Err(Unsent::NoConnection) => {
                        return Some(Err(StaDisconnectReason::TransmitRefused(
                            TxRefusal::LinkChanged,
                        )));
                    }
                }
            }
            Rekey::Retire => match self.keys.take_retiring_key() {
                Some((old, key)) => {
                    forget_group(rsn, &self.keys, old);
                    (Rekey::AwaitRetired, Some(Owed::RemoveGroup(key)))
                }
                None => (Rekey::None, None),
            },
            rekey => (rekey, None),
        };
        rsn.rekey = rekey;
        owed.map(Ok)
    }

    /// The background scan's action, or a Null the access point is owed.
    fn owe_background(
        &mut self,
        connected: &mut Connected<T, K>,
    ) -> Result<Option<Owed<'id, T, K>>, StaDisconnectReason> {
        let bssid = connected.bssid;
        let home = connected.channel;
        let absence_id = self.absence.as_ref().map(Lease::id);
        let mut owed = None;
        let mut power = None;
        if let Some(background) = &mut connected.background {
            let step = background.step;
            background.step = match step {
                BackgroundStep::NeedAbsence => match self.channel(background.visit) {
                    // The network's own channel: no absence.
                    Some(channel) if channel == home => {
                        owed = self.configure(bss_rules(bssid, Rules::<K>::WIDENED));
                        if owed.is_some() {
                            BackgroundStep::AwaitWidened
                        } else {
                            step
                        }
                    }
                    Some(channel)
                        if self.absence_place.is_some() && matches!(self.lease, Held::Lease(_)) =>
                    {
                        owed = self.absence_place.take().map(|place| {
                            Owed::Absence(
                                ChannelRequest {
                                    channel,
                                    purpose: Purpose::Scan {
                                        dwell: self.config.dwell,
                                    },
                                },
                                place,
                            )
                        });
                        BackgroundStep::AwaitAnswer
                    }
                    _ => step,
                },
                BackgroundStep::NeedSleep if !connected.sending => {
                    power = Some((StaPowerManagement::PowerSave, Sending::Sleep));
                    BackgroundStep::AwaitSleep
                }
                BackgroundStep::NeedReady => {
                    owed = absence_id.map(Owed::Ready);
                    BackgroundStep::AwaitStart
                }
                BackgroundStep::NeedConfigure if self.configuration.is_some() => {
                    owed = self.configure(StationReceive::OtherBssManagement);
                    BackgroundStep::AwaitConfigured
                }
                BackgroundStep::NeedProbe if !connected.sending => {
                    owed = self.probe(&mut connected.sending)?;
                    if owed.is_some() {
                        BackgroundStep::Listen
                    } else {
                        BackgroundStep::NeedProbe
                    }
                }
                BackgroundStep::NeedHome if self.configuration.is_some() => {
                    owed = self.configure(bss_rules(bssid, Rules::<K>::MEMBER));
                    BackgroundStep::AwaitHome
                }
                BackgroundStep::NeedWake if !connected.sending => {
                    power = Some((StaPowerManagement::Active, Sending::Wake));
                    BackgroundStep::AwaitWake
                }
                BackgroundStep::HomeProbe { until } if !connected.sending => {
                    owed = self.probe(&mut connected.sending)?;
                    if owed.is_some() {
                        BackgroundStep::HomeListen { until }
                    } else {
                        BackgroundStep::HomeProbe { until }
                    }
                }
                BackgroundStep::Narrow if self.configuration.is_some() => {
                    owed = self.configure(bss_rules(bssid, Rules::<K>::MEMBER));
                    BackgroundStep::AwaitNarrowed
                }
                step => step,
            };
        } else if connected.wake && !connected.power.asleep() && !connected.sending {
            // The access point buffers for an awake station: PM=0 again.
            connected.wake = false;
            power = Some((StaPowerManagement::Active, Sending::Wake));
        }
        if let Some((power_management, sending)) = power {
            match self.encode_null(bssid, power_management) {
                Ok(transmit) => {
                    connected.sending = true;
                    connected.waking = sending == Sending::Wake;
                    owed = Some(Owed::Transmit(transmit));
                }
                // No token now: the step waits for the next poll.
                Err(Unsent::NoTokens) => {
                    if let Some(background) = &mut connected.background {
                        background.step = match background.step {
                            BackgroundStep::AwaitSleep => BackgroundStep::NeedSleep,
                            BackgroundStep::AwaitWake => BackgroundStep::NeedWake,
                            step => step,
                        };
                    } else {
                        connected.wake = true;
                    }
                }
                Err(Unsent::Encode) => return Err(StaDisconnectReason::Encode),
                Err(Unsent::NoConnection) => {
                    return Err(StaDisconnectReason::TransmitRefused(TxRefusal::LinkChanged));
                }
            }
        }
        Ok(owed)
    }

    /// A Probe Request of the background scan, marked in flight.
    fn probe(
        &mut self,
        sending: &mut bool,
    ) -> Result<Option<Owed<'id, T, K>>, StaDisconnectReason> {
        match self.encode_probe(TxPurpose::Connected(Sending::Probe)) {
            Ok(transmit) => {
                *sending = true;
                Ok(Some(Owed::Transmit(transmit)))
            }
            Err(Unsent::NoTokens) => Ok(None),
            Err(Unsent::Encode) => Err(StaDisconnectReason::Encode),
            Err(Unsent::NoConnection) => {
                Err(StaDisconnectReason::TransmitRefused(TxRefusal::LinkChanged))
            }
        }
    }

    /// A Null to `bssid` with `power_management`, in a buffer.
    fn encode_null(
        &mut self,
        bssid: Bssid,
        power_management: StaPowerManagement,
    ) -> Result<StaTransmit<'id, T, K>, Unsent> {
        let credit = self.reserve_transmit()?;
        let address = self.config.address;
        let sequence_number = self.sequence.take();
        let transmit = self.encode_reserved(
            credit,
            |frame| {
                StaNullDataFrame {
                    station_address: address,
                    bssid,
                    sequence_number,
                    power_management,
                }
                .encode(frame)
                .ok()
            },
            StaFrameClass::Normal,
            TxPurpose::Connected(match power_management {
                StaPowerManagement::Active => Sending::Wake,
                StaPowerManagement::PowerSave => Sending::Sleep,
            }),
            StaRatePlan::Null,
        )?;
        Ok(transmit)
    }

    fn owe_join(&mut self, mut join: Join<T, K>, now: Instant) -> ActionStep<'id, T, K> {
        let connect = ChannelRequest {
            channel: join.channel,
            purpose: Purpose::Connect,
        };
        let (step, owed) = match core::mem::replace(&mut join.step, JoinStep::NeedLease) {
            JoinStep::RetryAt(at) if now >= at => match self.place.take() {
                Some(place) => (JoinStep::AwaitAnswer, Some(Owed::Request(connect, place))),
                None => (JoinStep::RetryAt(at), None),
            },
            JoinStep::NeedLease => match self.place.take() {
                Some(place) => (JoinStep::AwaitAnswer, Some(Owed::Request(connect, place))),
                None => (JoinStep::NeedLease, None),
            },
            JoinStep::NeedConfigure if self.configuration.is_some() => {
                let receive = bss_rules(join.access_point.bssid, Rules::<K>::MEMBER);
                (JoinStep::AwaitConfigured, self.configure(receive))
            }
            JoinStep::Authenticate(Authenticate::NeedCredit(runtime)) => {
                let credit = match self.reserve_transmit() {
                    Ok(credit) => credit,
                    Err(Unsent::NoTokens) => {
                        join.step = JoinStep::Authenticate(Authenticate::NeedCredit(runtime));
                        return (State::Join(join), ActionProgress::Wait);
                    }
                    Err(Unsent::Encode) => {
                        return (
                            leave_unkeyed(StaDisconnectReason::Encode),
                            ActionProgress::Transition,
                        );
                    }
                    Err(Unsent::NoConnection) => {
                        return (
                            leave_unkeyed(StaDisconnectReason::TransmitRefused(
                                TxRefusal::LinkChanged,
                            )),
                            ActionProgress::Transition,
                        );
                    }
                };
                let (runtime, attempt) = runtime.begin_attempt(&mut self.sequence);
                let bssid = join.access_point.bssid;
                let address = self.config.address;
                let encoded = self.encode_reserved(
                    credit,
                    |frame| {
                        OpenAuthenticationRequest {
                            source: address,
                            bssid,
                            sequence_number: attempt.sequence_number,
                        }
                        .encode(frame)
                        .ok()
                    },
                    StaFrameClass::Connection,
                    TxPurpose::Authentication,
                    StaRatePlan::Management,
                );
                match encoded {
                    Ok(transmit) => {
                        let auth = Authenticate::Exchange {
                            runtime,
                            stage: AuthStage::AwaitSent {
                                response_timeout: attempt.response_timeout,
                            },
                        };
                        (JoinStep::Authenticate(auth), Some(Owed::Transmit(transmit)))
                    }
                    Err(EncodeError) => {
                        return (
                            leave_unkeyed(StaDisconnectReason::Encode),
                            ActionProgress::Transition,
                        );
                    }
                }
            }
            JoinStep::Associate(mut runtime) => {
                if runtime
                    .response_deadline()
                    .is_some_and(|deadline| now >= deadline)
                {
                    return (
                        leave_unkeyed(StaDisconnectReason::AssociationFailed),
                        ActionProgress::Transition,
                    );
                }
                let credit = match self.reserve_transmit() {
                    Ok(credit) => credit,
                    Err(Unsent::NoTokens) => {
                        join.step = JoinStep::Associate(runtime);
                        return (State::Join(join), ActionProgress::Wait);
                    }
                    Err(Unsent::Encode) => {
                        return (
                            leave_unkeyed(StaDisconnectReason::Encode),
                            ActionProgress::Transition,
                        );
                    }
                    Err(Unsent::NoConnection) => {
                        return (
                            leave_unkeyed(StaDisconnectReason::TransmitRefused(
                                TxRefusal::LinkChanged,
                            )),
                            ActionProgress::Transition,
                        );
                    }
                };
                match runtime.poll(now, &mut self.sequence) {
                    Ok(StaAssociationPoll::Transmit(attempt)) => {
                        let capabilities = self.config.association;
                        let phy = self.config.phy;
                        let address = self.config.address;
                        let access_point = &join.access_point;
                        let rsn = &join.rsn;
                        let encoded = self.encode_reserved(
                            credit,
                            |frame| {
                                AssociationRequest {
                                    source: address,
                                    access_point,
                                    sequence_number: attempt.sequence_number,
                                    listen_interval: 10,
                                    phy,
                                    security: rsn,
                                    power_capability: None,
                                    he_ul_mu_power: None,
                                }
                                .encode(frame, capabilities)
                                .ok()
                            },
                            StaFrameClass::Connection,
                            TxPurpose::Association,
                            StaRatePlan::Management,
                        );
                        match encoded {
                            Ok(transmit) => {
                                (JoinStep::Associate(runtime), Some(Owed::Transmit(transmit)))
                            }
                            Err(EncodeError) => {
                                return (
                                    leave_unkeyed(StaDisconnectReason::Encode),
                                    ActionProgress::Transition,
                                );
                            }
                        }
                    }
                    Ok(StaAssociationPoll::Idle) => {
                        self.return_credit(credit);
                        (JoinStep::Associate(runtime), None)
                    }
                    Ok(StaAssociationPoll::Failed { .. }) | Err(_) => {
                        self.return_credit(credit);
                        return (
                            leave_unkeyed(StaDisconnectReason::AssociationFailed),
                            ActionProgress::Transition,
                        );
                    }
                }
            }
            // The next state owes its first action at once.
            JoinStep::NotifyAssociated {
                association_id,
                peer,
            } => (
                JoinStep::StartLink {
                    association_id,
                    peer,
                },
                Some(Owed::Associated(peer)),
            ),
            JoinStep::StartLink {
                association_id,
                peer,
            } => {
                let state = self.joined(join, association_id, peer, now);
                return (state, ActionProgress::Transition);
            }
            step => (step, None),
        };
        join.step = step;
        (State::Join(join), owed.into())
    }

    /// The associated station: its handshake, or, on an open network,
    /// connected.
    fn joined(
        &mut self,
        join: Join<T, K>,
        association_id: u16,
        peer: Peer<T::RxMeta>,
        now: Instant,
    ) -> State<T, K> {
        let bssid = join.access_point.bssid;
        let channel = join.channel;
        let qos = join.access_point.wmm_ie_len > 0;
        if matches!(self.config.security, StaSecurity::Open) {
            return State::Connected(Connected {
                bssid,
                channel,
                association_id,
                qos,
                peer,
                qos_sequences: QOS_SEQUENCES,
                report: Some(UpstreamFact::Connected(channel)),
                exchange: false,
                power: PowerSaveView::Awake,
                wake: false,
                sending: false,
                waking: false,
                background: None,
                connected_notice: true,
                rsn: None,
            });
        }
        let supplicant = RsnStaSupplicant::try_new(
            self.config.address.get(),
            bssid.get(),
            (self.config.nonce)(),
            join.rsn.as_bytes(),
            join.access_point.rsn_ie_bytes(),
            join.access_point.rsnxe_bytes(),
        );
        let deadline = RsnStaResponseDeadline::start(RsnStaResponseWait::Message1, now);
        match (supplicant, deadline) {
            (Ok(supplicant), Some(deadline)) => State::Handshake(Handshake {
                bssid,
                channel,
                association_id,
                qos,
                peer,
                supplicant,
                deadline,
                step: HandshakeStep::Await,
            }),
            _ => leave_keyed(
                StaDisconnectReason::HandshakeFailed,
                Some(Deauthentication {
                    bssid,
                    reason_code: REASON_UNSPECIFIED,
                }),
            ),
        }
    }

    /// A ticket and a small buffer, if the station holds both.
    fn tokens(&mut self) -> Option<(T::Ticket, T::SmallBuffer)> {
        let ticket = self.tickets.take()?;
        match self.buffers.take() {
            Some(buffer) => Some((ticket, buffer)),
            None => {
                self.tickets.put(ticket);
                None
            }
        }
    }

    fn reserve_transmit(&mut self) -> Result<TxCredit<'id, T>, Unsent> {
        let epoch = self.epoch.ok_or(Unsent::NoConnection)?;
        let (ticket, buffer) = self.tokens().ok_or(Unsent::NoTokens)?;
        Ok(TxCredit {
            epoch,
            ticket,
            buffer,
        })
    }

    fn return_credit(&mut self, credit: TxCredit<'id, T>) {
        self.tickets.put(credit.ticket);
        self.buffers.put(credit.buffer);
    }

    /// Encode with both owners already reserved, after the caller may have
    /// consumed a sequence or started an attempt.
    fn encode_reserved(
        &mut self,
        credit: TxCredit<'id, T>,
        encode: impl FnOnce(&mut [u8]) -> Option<usize>,
        class: StaFrameClass,
        purpose: TxPurpose,
        plan: StaRatePlan,
    ) -> Result<StaTransmit<'id, T, K>, EncodeError> {
        let TxCredit {
            epoch,
            ticket,
            mut buffer,
        } = credit;
        let frame = encode(buffer.bytes_mut()).and_then(|len| {
            let bytes = buffer.bytes_mut().split_at_checked(len)?.0;
            let length = MpduLength::new(len)?;
            if matches!(plan, StaRatePlan::Group) {
                let header = HeaderSpec::decode(bytes, GroupAddress::new)?;
                oer_ieee80211_lower_mac::ClassSizedFrame::new(SizedFrame::new(header, length)?)
                    .ok()
                    .map(StaFrame::Group)
            } else {
                let header = HeaderSpec::decode(bytes, IndividualAddress::new)?;
                oer_ieee80211_lower_mac::ClassSizedFrame::new(SizedFrame::new(
                    AckHeader::new(header)?,
                    length,
                )?)
                .ok()
                .map(StaFrame::Individual)
            }
        });
        match frame {
            Some(frame) => Ok(StaTransmit {
                context: StaTxContext { epoch, purpose },
                ticket,
                buffer,
                frame,
                class,
                plan,
            }),
            None => {
                self.tickets.put(ticket);
                self.buffers.put(buffer);
                Err(EncodeError)
            }
        }
    }

    /// Whether the connected link is up: the station is home and awake,
    /// and holds its pairwise key ([`Self::link`]'s rule).
    fn link_up(&self, power: &PowerSaveView, keys: &Keys<T>) -> bool {
        self.absence.is_none() && !power.asleep() && matches!(keys.pairwise, KeyCell::Installed(_))
    }

    /// An EAPOL frame of the connected station under its pairwise key: the
    /// Protected bit and the CCMP header's room, which the backend fills;
    /// QoS Data at user priority 7 on a QoS BSS, as the old ESP32-S31 path
    /// sent it.
    ///
    /// On a QoS BSS it takes TID 7's number, which data at priority 7
    /// shares, not TID 0's as the old path did: a rekey's EAPOL would leave
    /// holes in TID 0's sequence under its Block Ack (owner decision on
    /// #563).
    fn encode_protected_eapol(
        &mut self,
        bssid: Bssid,
        qos: Option<&mut [StaSequenceCounter; 8]>,
        eapol: &[u8],
    ) -> Result<StaTransmit<'id, T, K>, Unsent> {
        const PRIORITY: u8 = 7;
        let credit = self.reserve_transmit()?;
        let peer_qos = qos.is_some();
        let sequence_number = match qos {
            Some(sequences) => {
                let [_, _, _, _, _, _, _, voice] = sequences;
                voice.take()
            }
            None => self.sequence.take(),
        };
        let frame = oer_ieee80211_mac::station::StaProtectedDataFrame {
            source: self.config.address,
            bssid,
            destination: bssid.get(),
            sequence_number,
            user_priority: PRIORITY,
            peer_qos,
            ccmp_header: [0; 8],
            ether_type: EAPOL_ETHER_TYPE,
            payload: eapol,
        };
        Ok(self.encode_reserved(
            credit,
            |buffer| frame.encode(buffer).ok(),
            StaFrameClass::Connection,
            TxPurpose::Connected(Sending::GroupResponse),
            StaRatePlan::Management,
        )?)
    }

    /// A Probe Request reserves credit before consuming its sequence.
    fn encode_probe(&mut self, purpose: TxPurpose) -> Result<StaTransmit<'id, T, K>, Unsent> {
        let credit = self.reserve_transmit()?;
        let sequence_number = self.sequence.take();
        let rates = self.config.supported_rates;
        let address = self.config.address.get();
        let ssid = self.config.ssid.clone();
        Ok(self.encode_reserved(
            credit,
            |output| {
                ProbeRequest {
                    destination: [0xff; 6],
                    source: address,
                    bssid: [0xff; 6],
                    sequence_number,
                    ssid: &ssid,
                    supported_rates: &rates,
                }
                .encode(output)
                .ok()
            },
            StaFrameClass::Normal,
            purpose,
            StaRatePlan::Group,
        )?)
    }

    /// A management frame of `subtype` to `bssid` in a buffer.
    fn encode_management(
        &mut self,
        subtype: StaManagementSubtype,
        bssid: Bssid,
        body: &[u8],
        purpose: TxPurpose,
    ) -> Result<StaTransmit<'id, T, K>, Unsent> {
        let credit = self.reserve_transmit()?;
        let address = self.config.address;
        let sequence_number = self.sequence.take();
        Ok(self.encode_reserved(
            credit,
            |frame| {
                StaManagementFrame {
                    subtype,
                    source: address,
                    bssid,
                    sequence_number,
                    body,
                }
                .encode(frame)
                .ok()
            },
            StaFrameClass::Normal,
            purpose,
            StaRatePlan::Management,
        )?)
    }

    /// When the station next needs a [`StaInput::Tick`], even without an
    /// event.
    pub fn poll_deadline(&self) -> Option<Instant> {
        match &self.state {
            State::Join(Join {
                step:
                    JoinStep::Authenticate(Authenticate::Exchange {
                        stage: AuthStage::AwaitResponse { until },
                        ..
                    }),
                ..
            }) => Some(*until),
            State::Join(Join {
                step: JoinStep::Associate(runtime),
                ..
            }) => {
                if self.tickets.is_empty() || self.buffers.is_empty() {
                    runtime.response_deadline()
                } else {
                    runtime.next_deadline()
                }
            }
            State::Join(Join {
                step: JoinStep::RetryAt(at),
                ..
            }) => Some(*at),
            State::Handshake(Handshake { deadline, .. }) => Some(deadline.deadline()),
            State::Connected(Connected {
                background: Some(background),
                ..
            }) => match background.step {
                BackgroundStep::HomeProbe { until } | BackgroundStep::HomeListen { until } => {
                    Some(until)
                }
                _ => None,
            },
            _ => None,
        }
    }
}

/// The network's receive rules of `filter` for `bssid`.
fn bss_rules<K>(bssid: Bssid, filter: SupportedFilter<K, StationRole>) -> StationReceive<K> {
    StationReceive::Bss {
        bssid: bssid.individual(),
        filter,
    }
}

/// Leave a network the station holds no key of: receive off, the lease
/// back.
fn leave_unkeyed<T: StaTokens, K>(reason: StaDisconnectReason) -> State<T, K> {
    State::Leave(Leave {
        reason,
        step: LeaveStep::NeedConfigure,
    })
}

/// Leave a network the station may hold keys of: the keys first, then the
/// Deauthentication, if any.
fn leave_keyed<T: StaTokens, K>(
    reason: StaDisconnectReason,
    deauthentication: Option<Deauthentication>,
) -> State<T, K> {
    State::Leave(Leave {
        reason,
        step: LeaveStep::RemoveKeys(deauthentication),
    })
}

/// A failed handshake: the keys, then a Deauthentication with
/// `reason_code`.
fn leave_handshake<T: StaTokens, K>(handshake: Handshake<T, K>, reason_code: u16) -> State<T, K> {
    let reason = if reason_code == REASON_LEAVING {
        StaDisconnectReason::Requested
    } else {
        StaDisconnectReason::HandshakeFailed
    };
    leave_keyed(
        reason,
        Some(Deauthentication {
            bssid: handshake.bssid,
            reason_code,
        }),
    )
}

/// Leave the connected network: the keys, then a Deauthentication with
/// `reason_code`, if any.
fn leave_connected<T: StaTokens, K>(
    connected: Connected<T, K>,
    reason: StaDisconnectReason,
    reason_code: Option<u16>,
) -> State<T, K> {
    let bssid = connected.bssid;
    State::Leave(Leave {
        reason,
        step: LeaveStep::AwaitAbsenceEnd(
            reason_code.map(|reason_code| Deauthentication { bssid, reason_code }),
        ),
    })
}

fn finish_handshake_install<T: StaTokens, K>(
    mut handshake: Handshake<T, K>,
    request: RsnStaKeyInstallRequest,
    retained: Option<CcmpRxReplayEpoch>,
) -> State<T, K> {
    let replay = match retained {
        Some(replay) => replay,
        None => {
            let Some(replay) = replay_epoch(&request) else {
                return leave_handshake(handshake, REASON_UNSPECIFIED);
            };
            replay
        }
    };
    match handshake
        .supplicant
        .complete_key_install::<DEFAULT_EAPOL_FRAME_CAPACITY>(request, true)
    {
        Ok(RsnStaSupplicantAction::Transmit(message)) => {
            handshake.step = HandshakeStep::SendLast(message, replay);
            State::Handshake(handshake)
        }
        _ => leave_handshake(handshake, REASON_UNSPECIFIED),
    }
}

/// The handshake's end: connected, its supplicant and keys held.
fn connect<T: StaTokens, K>(handshake: Handshake<T, K>, retire_groups: bool) -> State<T, K> {
    let Handshake {
        bssid,
        channel,
        association_id,
        qos,
        peer,
        supplicant,
        step,
        ..
    } = handshake;
    let HandshakeStep::AwaitLast(replay) = step else {
        return leave_keyed(StaDisconnectReason::HandshakeFailed, None);
    };
    match supplicant.into_connected() {
        Ok(supplicant) => State::Connected(Connected {
            bssid,
            channel,
            association_id,
            qos,
            peer,
            qos_sequences: QOS_SEQUENCES,
            report: Some(UpstreamFact::Connected(channel)),
            exchange: false,
            power: PowerSaveView::Awake,
            wake: false,
            sending: false,
            waking: false,
            background: None,
            connected_notice: true,
            rsn: Some(ConnectedRsn {
                supplicant,
                rekey: if retire_groups {
                    Rekey::Retire
                } else {
                    Rekey::None
                },
                replay,
            }),
        }),
        Err(_) => leave_keyed(
            StaDisconnectReason::HandshakeFailed,
            Some(Deauthentication {
                bssid,
                reason_code: REASON_UNSPECIFIED,
            }),
        ),
    }
}

/// Why a frame was not handed out.
#[derive(Debug)]
enum Unsent {
    NoConnection,
    /// The station holds no ticket or buffer now.
    NoTokens,
    /// The frame did not encode.
    Encode,
}

/// Encoding can fail after credit is reserved; acquiring credit cannot.
#[derive(Debug)]
struct EncodeError;

impl From<EncodeError> for Unsent {
    fn from(_: EncodeError) -> Self {
        Self::Encode
    }
}

#[cfg(test)]
mod tests;
