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
//!
//! The key slots and keys live in the state that uses them: a handshake
//! holds the free slots it installs into, a connected station the
//! installed keys, a leaving one the keys it removes; a leave reports only
//! once every key came back as its slot.
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
    AckHeader, GroupAddress, HeaderSpec, Ieee80211Capabilities, IndividualAddress, MpduLength,
    ReceiveFilter, RxEvidence, RxMeta, SizedFrame, Station as StationRole, StationReceive,
    SupportedChannel, SupportedFilter, TxBufferMemory, TxCompletion, TxStatus,
};
use oer_ieee80211_mac::ccmp::CcmpRxReplayEpoch;
use oer_ieee80211_mac::{
    management::ProbeRequest,
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
use oer_ieee80211_upper_mac::{HePacketPadding, rate_control::LinkMetric};

use crate::association::StaAssociatedPeer;
use oer_time::{Duration, Instant};

use crate::pool::Pool;
use crate::power_save::{PowerSaveDecision, PowerSaveView};

use crate::join::{
    association::{StaAssociationEvent, StaAssociationPoll, StaAssociationRuntime},
    authentication::{StaAuthenticationEvent, StaAuthenticationRuntime},
};

/// The tokens a station holds: the port's, or a host test's.
pub trait StaTokens {
    /// A transmit ticket: a place in flight and a place for its terminal.
    type Ticket;
    /// A transmit buffer of the small class, for management frames: its
    /// memory, which the station encodes into.
    type SmallBuffer: TxBufferMemory;
    /// A pairwise key slot.
    type PairwiseSlot;
    /// A group key slot.
    type GroupSlot;
    /// An installed pairwise key, which holds its slot.
    type PairwiseKey;
    /// An installed group key, which holds its slot.
    type GroupKey;
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
pub enum StaInput<'f, T: StaTokens, K> {
    /// A command of the application.
    Command(StaCommand),
    /// A deadline the station named passed.
    Tick,
    /// A frame the station's interface received.
    Received {
        /// The MPDU, from its header to the end of its body.
        frame: &'f [u8],
        /// What the hardware observed of it.
        meta: RxMeta,
    },
    /// The terminal of a transmission: its ticket and buffer come back.
    TxDone {
        /// The transmission's ticket.
        ticket: T::Ticket,
        /// The transmission's buffer.
        buffer: T::SmallBuffer,
        /// How it ended.
        completion: TxCompletion,
    },
    /// The arbiter's answer to the station's request.
    Answer(Answer<K>),
    /// The arbiter's answer to the station's absence.
    AbsenceAnswer(AbsenceAnswer<K>),
    /// The primary lease of the channel the port moved to, which the
    /// arbiter exchanged for the old one.
    Exchanged(Lease<K>),
    /// A notice of the arbiter.
    Arbiter(StationNotice<K>),
    /// The receive configuration the station set took effect.
    Configured,
    /// The pairwise key the station asked to install, installed.
    PairwiseKeyInstalled(T::PairwiseKey),
    /// The group key the station asked to install, installed.
    GroupKeyInstalled(T::GroupKey),
    /// A removed pairwise key's slot.
    PairwiseKeyRemoved(T::PairwiseSlot),
    /// A removed group key's slot.
    GroupKeyRemoved(T::GroupSlot),
}

/// One action of the station, which the service carries out at once.
pub enum StaAction<'a, T: StaTokens, K> {
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
    Configure(StationReceive<K>),
    /// Install a pairwise key ([`StaInput::PairwiseKeyInstalled`]).
    InstallPairwiseKey {
        /// The slot it takes.
        slot: T::PairwiseSlot,
        /// The access point.
        peer: IndividualAddress,
        /// The temporal key.
        key: [u8; 16],
    },
    /// Install a group key ([`StaInput::GroupKeyInstalled`]).
    InstallGroupKey {
        /// The slot it takes.
        slot: T::GroupSlot,
        /// Its key index.
        key_id: GroupKeyId,
        /// The temporal key.
        key: [u8; 16],
    },
    /// Remove the pairwise key ([`StaInput::PairwiseKeyRemoved`]).
    RemovePairwiseKey(T::PairwiseKey),
    /// Remove a group key ([`StaInput::GroupKeyRemoved`]).
    RemoveGroupKey(T::GroupKey),
    /// Transmit one MPDU ([`StaInput::TxDone`] follows).
    Transmit(StaTransmit<T, K>),
    /// Transmit one MPDU under the pairwise key its link lends: EAPOL
    /// after the handshake, sent only while the link is up
    /// ([`StaInput::TxDone`] follows). It carries the Protected bit and
    /// the CCMP header's room.
    TransmitProtected {
        /// The MPDU.
        transmit: StaTransmit<T, K>,
        /// The pairwise key, as [`Station::link`] lends it.
        key: &'a T::PairwiseKey,
    },
    /// The station associated with `Peer`: the rates of its frames start
    /// from what the association made known, the handshake's EAPOL
    /// included ([owner decision on #563]). A
    /// [`StaNotice::Disconnected`] ends it.
    ///
    /// [owner decision on #563]: https://github.com/ermacv/open-esp-radio-rs/issues/563
    Associated(Peer),
    /// Tell the application what happened.
    Notify(StaNotice),
}

/// One MPDU to transmit: its ticket, the buffer the station encoded it in,
/// its kind, its class and its rates.
pub struct StaTransmit<T: StaTokens, K> {
    /// Its ticket.
    pub ticket: T::Ticket,
    /// The buffer holding the frame, from its header on.
    pub buffer: T::SmallBuffer,
    /// The frame's kind: its receiver, header and length.
    pub frame: StaFrame<K>,
    /// What the frame is for, which the service turns into the port's
    /// coexistence priority.
    pub class: StaFrameClass,
    /// Its rate and retry plan.
    pub plan: StaRatePlan,
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
pub enum StaFrame<K> {
    /// An individual frame that solicits an ACK.
    Individual(SizedFrame<AckHeader, K>),
    /// A group frame.
    Group(SizedFrame<HeaderSpec<GroupAddress>, K>),
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
pub struct LinkView<'a, T: StaTokens> {
    /// The station's own address.
    pub address: [u8; 6],
    /// The access point.
    pub bssid: [u8; 6],
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
        bssid: [u8; 6],
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

/// The longest SSID.
const SSID_CAPACITY: usize = 32;
/// The scan list's channels at most.
const CHANNEL_CAPACITY: usize = 16;
/// The BSSs a scan keeps at most.
const SCAN_CAPACITY: usize = 16;

/// A new association's QoS sequence spaces.
const QOS_SEQUENCES: [StaSequenceCounter; 8] =
    [StaSequenceCounter::new(SequenceNumber::ZERO); 8];

/// The reason codes of the station's Deauthentication (IEEE 802.11-2020
/// Table 9-90).
const REASON_UNSPECIFIED: u16 = 1;
const REASON_LEAVING: u16 = 3;
const REASON_HANDSHAKE_TIMEOUT: u16 = 15;

/// What the station connects to and how.
pub struct StaConfig<K> {
    /// The interface's own address.
    pub address: [u8; 6],
    /// The network's SSID.
    pub ssid: [u8; SSID_CAPACITY],
    /// The SSID's length.
    pub ssid_len: u8,
    /// The channels the scan visits, in order.
    pub channels: [Option<SupportedChannel<K>>; CHANNEL_CAPACITY],
    /// How long the scan listens on each channel.
    pub dwell: Duration,
    /// The consecutive cancelled leases after which a scan ends with a
    /// partial result.
    pub skipped_absences: u8,
    /// How much weaker than the strongest a candidate on the channel of
    /// the access point beside the station may be and still win, in dB:
    /// 6 by the owner's decision on #563.
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
    const MEMBER: SupportedFilter<K, StationRole> =
        match SupportedFilter::new(ReceiveFilter::BSS_MEMBER) {
            Some(filter) => filter,
            None => panic!("a station's port receives its network's frames"),
        };
    /// The network's frames and the other networks' management frames:
    /// the network's own channel in a background scan.
    const WIDENED: SupportedFilter<K, StationRole> = match SupportedFilter::new(
        ReceiveFilter::BSS_MEMBER.union(ReceiveFilter::OTHER_BSS_MANAGEMENT),
    ) {
        Some(filter) => filter,
        None => panic!("a station's port hears the other networks beside its own"),
    };
}

/// The station: its state and its tokens.
pub struct Station<T: StaTokens, K: Ieee80211Capabilities> {
    config: StaConfig<K>,
    state: State<T, K>,
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
    /// The access point of an association the service has yet to hear.
    associated_owed: Option<Peer>,
    sequence: StaSequenceCounter,
    scan: ScanTable<SCAN_CAPACITY>,
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

/// A key slot of one kind, and the key it holds.
enum KeyCell<Key, Slot> {
    /// Free: the station holds the slot.
    Free(Slot),
    /// Its install or its removal has not ended.
    Busy,
    /// Installed: the station holds the key.
    Installed(Key),
}

/// The station's key slots and keys, which its state holds.
struct Keys<T: StaTokens> {
    pairwise: KeyCell<T::PairwiseKey, T::PairwiseSlot>,
    /// The free group slots.
    free_groups: Pool<T::GroupSlot>,
    /// The group key in use, by its index.
    group: Option<(GroupKeyId, T::GroupKey)>,
    /// The group keys a rekey replaced, until their removal.
    retiring: Pool<(GroupKeyId, T::GroupKey)>,
    /// The index of the group key being installed.
    installing: Option<GroupKeyId>,
    /// The group installs and removals that have not ended.
    group_busy: u8,
}

impl<T: StaTokens> Keys<T> {
    fn new<const GROUPS: usize>(pairwise: T::PairwiseSlot, groups: [T::GroupSlot; GROUPS]) -> Self {
        Self {
            pairwise: KeyCell::Free(pairwise),
            free_groups: Pool::new(groups),
            group: None,
            retiring: Pool::new([]),
            installing: None,
            group_busy: 0,
        }
    }

    fn take_pairwise_slot(&mut self) -> Option<T::PairwiseSlot> {
        match core::mem::replace(&mut self.pairwise, KeyCell::Busy) {
            KeyCell::Free(slot) => Some(slot),
            cell => {
                self.pairwise = cell;
                None
            }
        }
    }

    fn take_pairwise_key(&mut self) -> Option<T::PairwiseKey> {
        match core::mem::replace(&mut self.pairwise, KeyCell::Busy) {
            KeyCell::Installed(key) => Some(key),
            cell => {
                self.pairwise = cell;
                None
            }
        }
    }

    /// A free slot for the group key `key_id`.
    fn take_group_slot(&mut self, key_id: GroupKeyId) -> Option<T::GroupSlot> {
        let slot = self.free_groups.take()?;
        self.group_busy += 1;
        self.installing = Some(key_id);
        Some(slot)
    }

    /// A group key is installed: it is the one in use, and the one it
    /// replaces retires.
    fn group_installed(&mut self, key: T::GroupKey) {
        self.group_busy = self.group_busy.saturating_sub(1);
        let Some(key_id) = self.installing.take() else {
            unreachable!("a group key is installed into a slot taken for it");
        };
        if let Some(old) = self.group.replace((key_id, key)) {
            self.retiring.put(old);
        }
    }

    /// A key to remove: a retiring group key first, then the one in use.
    fn take_group_key(&mut self) -> Option<(GroupKeyId, T::GroupKey)> {
        let key = self.retiring.take().or_else(|| self.group.take())?;
        self.group_busy += 1;
        Some(key)
    }

    fn group_removed(&mut self, slot: T::GroupSlot) {
        self.group_busy = self.group_busy.saturating_sub(1);
        self.free_groups.put(slot);
    }

    /// Both keys of a handshake are installed.
    const fn installed(&self) -> bool {
        matches!(self.pairwise, KeyCell::Installed(_))
            && self.group.is_some()
            && self.group_busy == 0
    }

    /// Every slot is free.
    fn clear(&self) -> bool {
        matches!(self.pairwise, KeyCell::Free(_))
            && self.group.is_none()
            && self.retiring.is_empty()
            && self.group_busy == 0
    }
}

/// The station's state; each variant holds its own type, and every one
/// the key slots.
#[expect(
    clippy::large_enum_variant,
    reason = "the station is placed once; a state machine's size is its largest state, and without an allocator a box is no option"
)]
enum State<T: StaTokens, K> {
    /// Not connected and not trying.
    Idle(Keys<T>),
    /// Visiting the scan list.
    Scan(Scan<T, K>),
    /// Joining the chosen access point.
    Join(Join<T, K>),
    /// Running the WPA2 four-way handshake.
    Handshake(Handshake<T, K>),
    /// Connected.
    Connected(Connected<T, K>),
    /// Leaving the network.
    Leave(Leave<T>),
    /// Telling the application why it is not connected, then idle.
    Report(StaDisconnectReason, Keys<T>),
    /// Only between a transition's take and its put.
    Taken,
}

impl<T: StaTokens, K> State<T, K> {
    fn keys_mut(&mut self) -> Option<&mut Keys<T>> {
        match self {
            Self::Idle(keys) | Self::Report(_, keys) => Some(keys),
            Self::Scan(scan) => Some(&mut scan.keys),
            Self::Join(join) => Some(&mut join.keys),
            Self::Handshake(handshake) => Some(&mut handshake.keys),
            Self::Connected(connected) => Some(&mut connected.keys),
            Self::Leave(leave) => Some(&mut leave.keys),
            Self::Taken => None,
        }
    }
}

/// A visit of one scan channel.
struct Scan<T: StaTokens, K> {
    visit: usize,
    /// Consecutive cancelled leases.
    skipped: u8,
    step: ScanStep,
    /// The channel of the access point beside the station, as the last
    /// lease named it.
    access_point: Option<SupportedChannel<K>>,
    keys: Keys<T>,
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
    step: JoinStep,
    keys: Keys<T>,
}

enum JoinStep {
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
    Report {
        association_id: u16,
        peer: Peer,
    },
}

/// The access point as the association made it: what it negotiated, and
/// its signal over the noise floor in the Association Response, in dB,
/// when the port reported both.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Peer {
    /// What the association negotiated.
    pub associated: StaAssociatedPeer,
    /// The access point's signal over the noise floor in the Association
    /// Response, which a rate selection starts from (owner decision on
    /// #563: fresher than the scan record the old ESP32-S31 path took).
    pub link_metric: LinkMetric,
}

struct Authenticate {
    runtime: StaAuthenticationRuntime,
    stage: AuthStage,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AuthStage {
    NeedSend,
    AwaitSent { response_timeout: Duration },
    AwaitResponse { until: Instant },
}

/// The WPA2 four-way handshake with the joined access point.
struct Handshake<T: StaTokens, K> {
    bssid: [u8; 6],
    channel: SupportedChannel<K>,
    association_id: u16,
    qos: bool,
    peer: Peer,
    supplicant: RsnStaSupplicant,
    /// The deadline of the message the station waits for.
    deadline: RsnStaResponseDeadline,
    step: HandshakeStep,
    /// The free slots it installs into.
    keys: Keys<T>,
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
    InstallGroup(RsnStaKeyInstallRequest),
    AwaitInstalled(RsnStaKeyInstallRequest),
    /// Message 4, then connected.
    SendLast(RsnTxFrame, CcmpRxReplayEpoch),
    AwaitLast(CcmpRxReplayEpoch),
}

/// The connected station.
struct Connected<T: StaTokens, K> {
    bssid: [u8; 6],
    channel: SupportedChannel<K>,
    association_id: u16,
    /// The access point's BSS is a QoS BSS (it advertised WMM).
    qos: bool,
    /// The access point as the association made it.
    peer: Peer,
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
    /// The station's one transmission in flight, by what it is for.
    sending: Option<Sending>,
    /// The background scan, if one runs.
    background: Option<Background>,
    /// The application has yet to hear the station connected.
    connected_notice: bool,
    /// The supplicant of a protected link and its group key's handshake.
    rsn: Option<ConnectedRsn>,
    /// The installed keys.
    keys: Keys<T>,
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
/// after Group Message 2; without one the old key is removed first.
enum Rekey {
    None,
    /// The new key to install, into a free slot or after the old key's
    /// removal.
    Install(RsnGroupKeyInstallRequest<DEFAULT_EAPOL_FRAME_CAPACITY>),
    /// The old key's removal frees a slot.
    AwaitFree(RsnGroupKeyInstallRequest<DEFAULT_EAPOL_FRAME_CAPACITY>),
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
struct Leave<T: StaTokens> {
    reason: StaDisconnectReason,
    step: LeaveStep,
    /// The keys it removes.
    keys: Keys<T>,
}

/// A Deauthentication to send.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Deauthentication {
    bssid: [u8; 6],
    reason_code: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LeaveStep {
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
/// The LLC/SNAP header of an EAPOL payload.
const LLC_SNAP_EAPOL: [u8; 8] = [0xaa, 0xaa, 0x03, 0, 0, 0, 0x88, 0x8e];

/// The EAPOL frame a data `frame` from `bssid` to `address` carries.
fn received_eapol(
    frame: &[u8],
    address: [u8; 6],
    bssid: [u8; 6],
) -> Option<OwnedEapolFrame<DEFAULT_EAPOL_FRAME_CAPACITY>> {
    let control = u16::from_le_bytes(frame.get(0..2)?.try_into().ok()?);
    // Data from the DS.
    if (control >> 2) & 0b11 != 2 || control & 0x0300 != 0x0200 {
        return None;
    }
    if frame.get(4..10)? != address || frame.get(10..16)? != bssid {
        return None;
    }
    let qos = control & 0x0080 != 0;
    let payload = if qos { 26 } else { 24 };
    if frame.get(payload..payload + 8)? != LLC_SNAP_EAPOL {
        return None;
    }
    OwnedEapolFrame::try_copy(RsnInterface::Station, bssid, frame.get(payload + 8..)?).ok()
}

/// The frame control of a Deauthentication and of a Disassociation.
const DEAUTHENTICATION: u16 = 0x00c0;
const DISASSOCIATION: u16 = 0x00a0;

/// The disconnect `frame` carries from `bssid`, if it is one.
fn peer_disconnect(frame: &[u8], bssid: [u8; 6]) -> Option<u16> {
    let control = u16::from_le_bytes(frame.get(0..2)?.try_into().ok()?) & 0x00fc;
    if control != DEAUTHENTICATION && control != DISASSOCIATION {
        return None;
    }
    if frame.get(10..16)? != bssid {
        return None;
    }
    Some(u16::from_le_bytes(frame.get(24..26)?.try_into().ok()?))
}

/// A group key's index and temporal key, when its index is a group
/// key's (1 to 3).
fn group_key(install: &RsnKeyInstall) -> Option<(GroupKeyId, [u8; 16])> {
    let RsnKeyKind::Group { key_id, .. } = install.kind() else {
        return None;
    };
    Some((GroupKeyId::new(key_id)?, *install.key().as_bytes()))
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
    if keys
        .group
        .as_ref()
        .is_none_or(|(in_use, _)| *in_use != key_id)
    {
        rsn.replay.remove_group(key_id);
    }
}

/// What ended in the station's world.
enum Done {
    Command(StaCommand),
    Tick,
    /// A transmission ended: acknowledged, or not.
    Tx {
        delivered: bool,
    },
    Configured,
}

/// An action a state owes.
enum Owed<T: StaTokens, K> {
    InstallPairwise {
        slot: T::PairwiseSlot,
        peer: IndividualAddress,
        key: [u8; 16],
    },
    InstallGroup {
        slot: T::GroupSlot,
        key_id: GroupKeyId,
        key: [u8; 16],
    },
    RemovePairwise(T::PairwiseKey),
    RemoveGroup(T::GroupKey),
    Request(ChannelRequest<K>),
    Absence(ChannelRequest<K>),
    Ready(LeaseId),
    Exchange,
    Release,
    Upstream(UpstreamFact<K>),
    Configure(StationReceive<K>),
    Transmit(StaTransmit<T, K>),
    /// Under the pairwise key of the connected link.
    TransmitProtected(StaTransmit<T, K>),
    Notify(StaNotice),
}

impl<T: StaTokens, K: Ieee80211Capabilities> Station<T, K> {
    /// An idle station of `config`, with the tickets, small buffers and key
    /// slots the composition granted it and the arbiter's places. It holds
    /// at most eight tokens of a kind, checked at compile time:
    ///
    /// ```compile_fail,E0080
    /// # use oer_ieee80211_arbiter::{AbsencePlace, LeasePlace};
    /// # use oer_ieee80211_lower_mac::*;
    /// # use oer_ieee80211_mac::phy::{HeMcs, HtMcs};
    /// # use oer_ieee80211_sta::machine::{StaConfig, StaTokens, Station};
    /// # struct Caps;
    /// # impl Ieee80211Capabilities for Caps {
    /// #     const CAPS: LowerMacCapabilities = LowerMacCapabilities {
    /// #         bands: BandSet::GHZ2_4, widths: WidthSet::MHZ20,
    /// #         rates: RateSupport { dsss_cck: true, ofdm: true, ht_max_mcs: HtMcs::new(7),
    /// #             he_max_mcs: HeMcs::new(9), he_max_bandwidth_mhz: 20, he_dcm: false,
    /// #             he_ldpc: false, spatial_streams: 1 },
    /// #         services: HardwareServices::FCS, vifs: 1, tx_queues: 4, max_mpdu_length: 2304,
    /// #         max_backoff_slots: 1023, tx_power_ceiling_min_dbm: None,
    /// #         coex_priorities: CoexPrioritySet::only(CoexPriority::Normal),
    /// #         individual_no_ack: PhyFormatSet::NON_HT,
    /// #         station_receive_filters: ReceiveFilter::BSS_MEMBER
    /// #             .union(ReceiveFilter::OTHER_BSS_MANAGEMENT),
    /// #         access_point_receive_filters: ReceiveFilter::BSS_MEMBER, key_slots: 4,
    /// #         rx_block_ack_agreements: 0, rx_block_ack_max_tid: 0, rx_block_ack_max_window: 0,
    /// #         edca_max_aifsn: 15, edca_max_cw_exponent: 10,
    /// #     };
    /// # }
    /// # struct Buffer;
    /// # impl TxBufferMemory for Buffer { fn bytes_mut(&mut self) -> &mut [u8] { &mut [] } }
    /// # struct Tokens;
    /// # impl StaTokens for Tokens {
    /// #     type Ticket = u8; type SmallBuffer = Buffer; type PairwiseSlot = ();
    /// #     type GroupSlot = (); type PairwiseKey = (); type GroupKey = ();
    /// # }
    /// fn nine(
    ///     config: StaConfig<Caps>,
    ///     place: LeasePlace,
    ///     absence: AbsencePlace,
    /// ) -> Station<Tokens, Caps> {
    ///     Station::new(config, [0; 9], [Buffer], place, absence, (), [()])
    /// }
    /// let nine: fn(_, _, _) -> _ = nine;
    /// core::hint::black_box(nine);
    /// ```
    pub fn new<const TICKETS: usize, const BUFFERS: usize, const GROUPS: usize>(
        config: StaConfig<K>,
        tickets: [T::Ticket; TICKETS],
        buffers: [T::SmallBuffer; BUFFERS],
        place: LeasePlace,
        absence_place: AbsencePlace,
        pairwise: T::PairwiseSlot,
        groups: [T::GroupSlot; GROUPS],
    ) -> Self {
        // The port's receive rules, checked as the type is built.
        let _ = (Rules::<K>::MEMBER, Rules::<K>::WIDENED);
        Self {
            config,
            state: State::Idle(Keys::new(pairwise, groups)),
            tickets: Pool::new(tickets),
            buffers: Pool::new(buffers),
            place: Some(place),
            lease: Held::None,
            absence_place: Some(absence_place),
            absence: None,
            notice_owed: None,
            associated_owed: None,
            sequence: StaSequenceCounter::new(SequenceNumber::ZERO),
            scan: ScanTable::new(),
        }
    }

    /// Whether the station is connected.
    pub const fn is_connected(&self) -> bool {
        matches!(self.state, State::Connected(_))
    }

    /// The access point of the associated station, as its association made
    /// it: during the handshake and once connected.
    pub const fn peer(&self) -> Option<&Peer> {
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
    pub fn link(&mut self) -> Option<LinkView<'_, T>> {
        let State::Connected(connected) = &mut self.state else {
            return None;
        };
        if self.absence.is_some() || connected.power.asleep() {
            return None;
        }
        let pairwise = match &connected.keys.pairwise {
            KeyCell::Installed(key) => Some(key),
            KeyCell::Free(_) | KeyCell::Busy => None,
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
        &self.config.ssid[..usize::from(self.config.ssid_len)]
    }

    fn channel(&self, visit: usize) -> Option<SupportedChannel<K>> {
        self.config.channels.get(visit).copied().flatten()
    }

    fn take_state(&mut self) -> State<T, K> {
        core::mem::replace(&mut self.state, State::Taken)
    }

    /// Take one input at `now`.
    pub fn input(&mut self, now: Instant, input: StaInput<'_, T, K>) {
        let done = match input {
            StaInput::TxDone {
                ticket,
                buffer,
                completion,
            } => {
                // A transmission's tokens come back whatever the state.
                self.tickets.put(ticket);
                self.buffers.put(buffer);
                Done::Tx {
                    delivered: matches!(completion.status, TxStatus::Success),
                }
            }
            StaInput::Command(command) => Done::Command(command),
            StaInput::Tick => Done::Tick,
            StaInput::Configured => Done::Configured,
            StaInput::Received { frame, meta } => {
                self.received(now, frame, meta);
                return;
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
            StaInput::PairwiseKeyInstalled(key) => {
                if let Some(keys) = self.state.keys_mut() {
                    keys.pairwise = KeyCell::Installed(key);
                }
                self.keys_changed();
                return;
            }
            StaInput::GroupKeyInstalled(key) => {
                if let Some(keys) = self.state.keys_mut() {
                    keys.group_installed(key);
                }
                self.group_installed();
                return;
            }
            StaInput::PairwiseKeyRemoved(slot) => {
                if let Some(keys) = self.state.keys_mut() {
                    keys.pairwise = KeyCell::Free(slot);
                }
                return;
            }
            StaInput::GroupKeyRemoved(slot) => {
                if let Some(keys) = self.state.keys_mut() {
                    keys.group_removed(slot);
                }
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
                    (State::Scan(scan), _) => {
                        State::Report(StaDisconnectReason::Refused(refusal), scan.keys)
                    }
                    (State::Join(join), _) => {
                        State::Report(StaDisconnectReason::Refused(refusal), join.keys)
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
            State::Handshake(mut handshake) if handshake.keys.installed() => {
                match core::mem::replace(&mut handshake.step, HandshakeStep::Await) {
                    HandshakeStep::AwaitInstalled(request) => {
                        // The replay check starts from the keys' receive
                        // sequences.
                        let Some(epoch) = replay_epoch(&request) else {
                            return self.set(leave_handshake(handshake, REASON_UNSPECIFIED));
                        };
                        match handshake
                            .supplicant
                            .complete_key_install::<DEFAULT_EAPOL_FRAME_CAPACITY>(request, true)
                        {
                            Ok(RsnStaSupplicantAction::Transmit(message4)) => {
                                handshake.step = HandshakeStep::SendLast(message4, epoch);
                                State::Handshake(handshake)
                            }
                            _ => leave_handshake(handshake, REASON_UNSPECIFIED),
                        }
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
    fn group_installed(&mut self) {
        let state = self.take_state();
        self.state = match state {
            State::Connected(mut connected) => {
                if let Some(rsn) = &mut connected.rsn
                    && let Rekey::AwaitInstalled(request) =
                        core::mem::replace(&mut rsn.rekey, Rekey::None)
                {
                    // The new group key's frontiers start from its receive
                    // sequence once it is installed, beside the old key's.
                    let installed = group_key(request.group()).is_some_and(|(key_id, _)| {
                        rsn.replay
                            .install_group(key_id, *request.group().receive_sequence())
                            .is_ok()
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
                Rekey::AwaitFree(request) => Rekey::Install(request),
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
            StationNotice::AbsenceEnded(_, place) => {
                self.absence_place = Some(place);
                self.absence_over(false);
                return;
            }
            StationNotice::AbsenceCancelled(_, _, place) => {
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
            StationNotice::LeaseEnded(id, place) | StationNotice::LeaseCancelled(id, _, place)
                if self.lease.id() != Some(id) =>
            {
                // A notice of an earlier lease: its place came back long
                // ago, so this one is the arbiter's mistake.
                debug_assert!(false, "a notice names a lease the station does not hold");
                let _ = place;
                state
            }
            StationNotice::LeaseEnded(_, place) => {
                self.lease = Held::None;
                self.place = Some(place);
                self.lease_ended(state, false)
            }
            StationNotice::LeaseCancelled(_, _, place) => {
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
                            handshake.keys,
                            StaDisconnectReason::Arbiter(reason),
                            reason_code.map(|reason_code| Deauthentication { bssid, reason_code }),
                        )
                    }
                    State::Join(join) => {
                        leave_unkeyed(join.keys, StaDisconnectReason::Arbiter(reason))
                    }
                    State::Scan(scan) => {
                        leave_unkeyed(scan.keys, StaDisconnectReason::Arbiter(reason))
                    }
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
                    self.select(true, scan.access_point, scan.keys)
                } else {
                    State::Scan(Scan {
                        skipped,
                        step: ScanStep::NeedLease,
                        ..scan
                    })
                }
            }
            // The visit's listening is over: the next channel.
            State::Scan(scan) => self.visit(scan.visit + 1, 0, scan.access_point, scan.keys),
            State::Leave(leave) => self.report(leave),
            // The lease ended without the station's release: the port left;
            // the station leaves, its keys first.
            State::Connected(connected) => leave_connected(connected, lost, None),
            State::Handshake(handshake) => leave_keyed(handshake.keys, lost, None),
            State::Join(join) => leave_unkeyed(join.keys, lost),
            state => state,
        }
    }

    /// A leave reports once every key came back; until then it removes
    /// them.
    fn report(&mut self, leave: Leave<T>) -> State<T, K> {
        if leave.keys.clear() {
            State::Report(leave.reason, leave.keys)
        } else {
            State::Leave(Leave {
                step: LeaveStep::RemoveKeys(None),
                ..leave
            })
        }
    }

    /// The state after `done` in `state`.
    fn step(&mut self, state: State<T, K>, now: Instant, done: Done) -> State<T, K> {
        match (state, done) {
            (State::Idle(keys) | State::Report(_, keys), Done::Command(StaCommand::Connect)) => {
                self.scan.clear();
                self.visit(0, 0, None, keys)
            }
            (State::Connected(connected), Done::Command(StaCommand::Disconnect)) => {
                leave_connected(
                    connected,
                    StaDisconnectReason::Requested,
                    Some(REASON_LEAVING),
                )
            }
            (State::Scan(scan), Done::Command(StaCommand::Disconnect)) => {
                leave_unkeyed(scan.keys, StaDisconnectReason::Requested)
            }
            (State::Join(join), Done::Command(StaCommand::Disconnect)) => {
                leave_unkeyed(join.keys, StaDisconnectReason::Requested)
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
                (HandshakeStep::AwaitSent, Done::Tx { .. }) => {
                    handshake.step = HandshakeStep::Await;
                    State::Handshake(handshake)
                }
                // Message 4 went: connected, the supplicant's keys held.
                (HandshakeStep::AwaitLast(_), Done::Tx { .. }) => connect(handshake),
                (HandshakeStep::Await, Done::Tick)
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
                    (LeaveStep::AwaitDeauthentication, Done::Tx { .. }) => LeaveStep::NeedConfigure,
                    (LeaveStep::AwaitConfigured, Done::Configured) => LeaveStep::NeedRelease,
                    (step, _) => step,
                };
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
        if let Done::Tx { delivered } = done {
            match connected.sending.take() {
                Some(Sending::Wake) => {
                    connected
                        .power
                        .null_sent(StaPowerManagement::Active, delivered);
                    if let Some(background) = &mut connected.background
                        && background.step == BackgroundStep::AwaitWake
                    {
                        self.next_visit(background);
                    }
                }
                Some(Sending::Sleep) => {
                    connected
                        .power
                        .null_sent(StaPowerManagement::PowerSave, delivered);
                    if let Some(background) = &mut connected.background
                        && background.step == BackgroundStep::AwaitSleep
                    {
                        // Not delivered: no readiness; the arbiter cancels
                        // the absence at its start.
                        background.step = if delivered {
                            BackgroundStep::NeedReady
                        } else {
                            BackgroundStep::AwaitStart
                        };
                    }
                }
                Some(Sending::GroupResponse) => {
                    if let Some(rsn) = &mut connected.rsn
                        && matches!(rsn.rekey, Rekey::AwaitSent)
                    {
                        rsn.rekey = if connected.keys.retiring.is_empty() {
                            Rekey::None
                        } else {
                            Rekey::Retire
                        };
                    }
                }
                Some(Sending::Probe) | None => {}
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
            background.visit + 1
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
        keys: Keys<T>,
    ) -> State<T, K> {
        match self.channel(visit) {
            Some(_) => State::Scan(Scan {
                visit,
                skipped,
                step: ScanStep::NeedLease,
                access_point,
                keys,
            }),
            None => self.select(false, access_point, keys),
        }
    }

    /// The join of the best access point of the network the scan found:
    /// the strongest, unless one on the channel of the access point beside
    /// the station is within the margin of it.
    fn select(
        &mut self,
        partial: bool,
        access_point_channel: Option<SupportedChannel<K>>,
        keys: Keys<T>,
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
            return State::Report(StaDisconnectReason::NotFound { partial }, keys);
        };
        let margin = i16::from(self.config.access_point_margin_db);
        let beside = access_point_channel.and_then(|channel| {
            candidates()
                .filter(|record| record.channel == channel.channel().number())
                .filter(|record| i16::from(record.rssi) >= i16::from(strongest.rssi) - margin)
                .max_by_key(|record| record.rssi)
                .copied()
        });
        let access_point = beside.unwrap_or(strongest);
        let Ok(rsn) = select_association_rsn(&access_point, policy) else {
            return State::Report(StaDisconnectReason::NotFound { partial }, keys);
        };
        let Some(channel) = self
            .config
            .channels
            .iter()
            .flatten()
            .copied()
            .find(|channel| channel.channel().number() == access_point.channel)
        else {
            return State::Report(StaDisconnectReason::NotFound { partial }, keys);
        };
        State::Join(Join {
            access_point,
            rsn,
            channel,
            step: JoinStep::NeedLease,
            keys,
        })
    }

    fn join_step(&mut self, mut join: Join<T, K>, now: Instant, done: Done) -> State<T, K> {
        join.step = match (
            core::mem::replace(&mut join.step, JoinStep::NeedLease),
            done,
        ) {
            (JoinStep::AwaitConfigured, Done::Configured) => JoinStep::Authenticate(Authenticate {
                runtime: StaAuthenticationRuntime::new(
                    self.config.address,
                    join.access_point.bssid,
                ),
                stage: AuthStage::NeedSend,
            }),
            (JoinStep::Authenticate(mut auth), done) => {
                let timed_out = match (auth.stage, done) {
                    (AuthStage::AwaitSent { response_timeout }, Done::Tx { delivered: true }) => {
                        auth.stage = AuthStage::AwaitResponse {
                            until: now.checked_add(response_timeout).unwrap_or(now),
                        };
                        false
                    }
                    // Unacknowledged or unanswered: as a timeout.
                    (AuthStage::AwaitSent { .. }, Done::Tx { delivered: false }) => true,
                    (AuthStage::AwaitResponse { until }, Done::Tick) => now >= until,
                    _ => false,
                };
                if timed_out {
                    match auth.runtime.response_timed_out() {
                        Ok(StaAuthenticationEvent::Retry { .. }) => {
                            auth.stage = AuthStage::NeedSend
                        }
                        _ => {
                            return leave_unkeyed(
                                join.keys,
                                StaDisconnectReason::AuthenticationFailed,
                            );
                        }
                    }
                }
                JoinStep::Authenticate(auth)
            }
            (step, _) => step,
        };
        State::Join(join)
    }

    /// Take a received frame.
    fn received(&mut self, now: Instant, frame: &[u8], meta: RxMeta) {
        let rssi = match meta.rssi_dbm {
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
            State::Join(mut join) => match &mut join.step {
                JoinStep::Authenticate(auth) => {
                    let _ = auth.runtime.observe_received_frame();
                    match auth.runtime.observe_management_frame(frame) {
                        Ok(StaAuthenticationEvent::Authenticated { .. }) => {
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
                            ));
                            State::Join(join)
                        }
                        Ok(StaAuthenticationEvent::Retry { .. }) => {
                            auth.stage = AuthStage::NeedSend;
                            State::Join(join)
                        }
                        Ok(StaAuthenticationEvent::Failed { .. }) | Err(_) => {
                            leave_unkeyed(join.keys, StaDisconnectReason::AuthenticationFailed)
                        }
                        Ok(StaAuthenticationEvent::Irrelevant) => State::Join(join),
                    }
                }
                JoinStep::Associate(runtime) => {
                    let _ = runtime.observe_received_frame();
                    match runtime.observe_management_frame(frame, now) {
                        Ok(StaAssociationEvent::Associated { response, .. }) => {
                            // The rate selection starts from the response's
                            // signal over the noise floor: a port that did not
                            // report both fails the association.
                            let derived = StaAssociatedPeer::derive(
                                &join.access_point,
                                &response,
                                self.config.phy,
                                self.config.he_packet_padding,
                            );
                            match (derived, LinkMetric::of(&meta)) {
                                (Ok(associated), Some(link_metric)) => {
                                    join.step = JoinStep::Report {
                                        association_id: response.association_id,
                                        peer: Peer {
                                            associated,
                                            link_metric,
                                        },
                                    };
                                    State::Join(join)
                                }
                                _ => {
                                    leave_unkeyed(join.keys, StaDisconnectReason::AssociationFailed)
                                }
                            }
                        }
                        Ok(StaAssociationEvent::Failed { .. }) | Err(_) => {
                            leave_unkeyed(join.keys, StaDisconnectReason::AssociationFailed)
                        }
                        Ok(StaAssociationEvent::Irrelevant) => State::Join(join),
                    }
                }
                _ => State::Join(join),
            },
            State::Handshake(handshake) => self.handshake_received(handshake, now, frame),
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
        if let Some(eapol) = received_eapol(frame, self.config.address, connected.bssid) {
            return self.group_message(connected, eapol);
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
        if let Ok(beacon) = parse_sta_beacon(frame, connected.bssid, connected.association_id)
            && connected.power.beacon(beacon.tim) == PowerSaveDecision::SendActive
            && connected.sending != Some(Sending::Wake)
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

    /// The next action to carry out at `now`, if any. The service takes
    /// every action before it waits again.
    pub fn poll_action(&mut self, now: Instant) -> Option<StaAction<'_, T, K>> {
        if let Some(peer) = self.associated_owed.take() {
            return Some(StaAction::Associated(peer));
        }
        if let Some(notice) = self.notice_owed.take() {
            return Some(StaAction::Notify(notice));
        }
        let state = self.take_state();
        let (state, owed) = self.owe(state, now);
        self.state = state;
        Some(match owed? {
            Owed::Absence(request) => {
                let (Some(place), Held::Lease(home)) = (self.absence_place.take(), &self.lease)
                else {
                    unreachable!("an absence holds its place and names the primary lease")
                };
                StaAction::RequestAbsence {
                    request,
                    home,
                    place,
                }
            }
            Owed::Ready(id) => StaAction::Ready(id),
            Owed::InstallPairwise { slot, peer, key } => {
                StaAction::InstallPairwiseKey { slot, peer, key }
            }
            Owed::InstallGroup { slot, key_id, key } => {
                StaAction::InstallGroupKey { slot, key_id, key }
            }
            Owed::RemovePairwise(key) => StaAction::RemovePairwiseKey(key),
            Owed::RemoveGroup(key) => StaAction::RemoveGroupKey(key),
            Owed::Exchange => match core::mem::replace(&mut self.lease, Held::None) {
                Held::Lease(lease) => StaAction::ExchangeLease(lease),
                held => {
                    self.lease = held;
                    unreachable!("an exchange holds the lease")
                }
            },
            Owed::Request(request) => {
                let Some(place) = self.place.take() else {
                    unreachable!("a request holds the place")
                };
                StaAction::RequestChannel(request, place)
            }
            Owed::Release => match core::mem::replace(&mut self.lease, Held::None) {
                Held::Lease(lease) => {
                    self.lease = Held::Released(lease.id());
                    StaAction::ReleaseChannel(lease)
                }
                held => {
                    self.lease = held;
                    unreachable!("a release holds the lease")
                }
            },
            Owed::Upstream(fact) => StaAction::Upstream(fact),
            Owed::Configure(receive) => StaAction::Configure(receive),
            Owed::Transmit(transmit) => StaAction::Transmit(transmit),
            // Owed only while the link is up in this very step, so the key
            // is there; without it the frame does not go and its tokens
            // come back.
            Owed::TransmitProtected(transmit) => match &self.state {
                State::Connected(Connected {
                    keys:
                        Keys {
                            pairwise: KeyCell::Installed(key),
                            ..
                        },
                    ..
                }) => StaAction::TransmitProtected { transmit, key },
                _ => {
                    self.tickets.put(transmit.ticket);
                    self.buffers.put(transmit.buffer);
                    return None;
                }
            },
            Owed::Notify(notice) => StaAction::Notify(notice),
        })
    }

    /// The action `state` owes, and the state that awaits its end.
    fn owe(&mut self, state: State<T, K>, now: Instant) -> (State<T, K>, Option<Owed<T, K>>) {
        match state {
            State::Report(reason, keys) => (
                State::Idle(keys),
                Some(Owed::Notify(StaNotice::Disconnected(reason))),
            ),
            State::Scan(mut scan) => {
                let owed = match scan.step {
                    ScanStep::NeedLease if self.place.is_some() => {
                        self.channel(scan.visit).map(|channel| {
                            scan.step = ScanStep::AwaitAnswer;
                            Owed::Request(ChannelRequest {
                                channel,
                                purpose: Purpose::Scan {
                                    dwell: self.config.dwell,
                                },
                            })
                        })
                    }
                    ScanStep::NeedConfigure => {
                        scan.step = ScanStep::AwaitConfigured;
                        Some(Owed::Configure(StationReceive::OtherBssManagement))
                    }
                    // No token or no encoding: listen passively.
                    ScanStep::NeedProbe => {
                        scan.step = ScanStep::Listen;
                        self.encode_probe().map(Owed::Transmit)
                    }
                    _ => None,
                };
                (State::Scan(scan), owed)
            }
            State::Join(join) => self.owe_join(join, now),
            State::Handshake(handshake) => self.owe_handshake(handshake),
            State::Connected(connected) => self.owe_connected(connected),
            State::Leave(leave) => self.owe_leave(leave),
            state => (state, None),
        }
    }

    /// The action the leave owes.
    fn owe_leave(&mut self, mut leave: Leave<T>) -> (State<T, K>, Option<Owed<T, K>>) {
        let owed = match leave.step {
            LeaveStep::RemoveKeys(deauthentication) => {
                if let Some(key) = leave.keys.take_pairwise_key() {
                    Some(Owed::RemovePairwise(key))
                } else if let Some((_, key)) = leave.keys.take_group_key() {
                    Some(Owed::RemoveGroup(key))
                } else if leave.keys.clear() {
                    // Every key is back: the next step owes the action.
                    leave.step = match deauthentication {
                        Some(deauthentication) => LeaveStep::NeedDeauthentication(deauthentication),
                        None => LeaveStep::NeedConfigure,
                    };
                    return self.owe_leave(leave);
                } else {
                    None
                }
            }
            LeaveStep::NeedDeauthentication(Deauthentication { bssid, reason_code }) => {
                match self.encode_management(
                    StaManagementSubtype::Deauthentication,
                    bssid,
                    &reason_code.to_le_bytes(),
                ) {
                    Some(transmit) => {
                        leave.step = LeaveStep::AwaitDeauthentication;
                        Some(Owed::Transmit(transmit))
                    }
                    None => {
                        leave.step = LeaveStep::NeedConfigure;
                        None
                    }
                }
            }
            LeaveStep::NeedConfigure => {
                leave.step = LeaveStep::AwaitConfigured;
                Some(Owed::Configure(StationReceive::Off))
            }
            // No lease to give back: done.
            LeaveStep::NeedRelease if !matches!(self.lease, Held::Lease(_)) => {
                return (self.report(leave), None);
            }
            LeaveStep::NeedRelease => {
                leave.step = LeaveStep::AwaitEnd;
                Some(Owed::Release)
            }
            _ => None,
        };
        (State::Leave(leave), owed)
    }

    /// The handshake's state after a received frame.
    fn handshake_received(
        &mut self,
        mut handshake: Handshake<T, K>,
        now: Instant,
        frame: &[u8],
    ) -> State<T, K> {
        if let Some(reason_code) = peer_disconnect(frame, handshake.bssid) {
            return leave_keyed(
                handshake.keys,
                StaDisconnectReason::Peer { reason_code },
                None,
            );
        }
        let Some(eapol) = received_eapol(frame, self.config.address, handshake.bssid) else {
            return State::Handshake(handshake);
        };
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
                handshake.step = HandshakeStep::Send(message);
                if let Some(deadline) =
                    RsnStaResponseDeadline::start(RsnStaResponseWait::Message3, now)
                {
                    handshake.deadline = deadline;
                }
            }
            RsnStaSupplicantAction::InstallKeys(request) => {
                handshake.step = HandshakeStep::Install(request);
            }
            RsnStaSupplicantAction::Deauthenticate => {
                return leave_handshake(handshake, REASON_UNSPECIFIED);
            }
            RsnStaSupplicantAction::None | RsnStaSupplicantAction::UnwrapKeyData(_) => {}
        }
        State::Handshake(handshake)
    }

    /// The action the handshake owes.
    fn owe_handshake(
        &mut self,
        mut handshake: Handshake<T, K>,
    ) -> (State<T, K>, Option<Owed<T, K>>) {
        let (step, owed) = match core::mem::replace(&mut handshake.step, HandshakeStep::Await) {
            HandshakeStep::Send(message) => {
                match self.encode_eapol(handshake.bssid, message.as_bytes()) {
                    Ok(transmit) => (HandshakeStep::AwaitSent, Some(Owed::Transmit(transmit))),
                    Err(Unsent::NoTokens) => (HandshakeStep::Send(message), None),
                    Err(Unsent::Encode) => {
                        return (leave_handshake(handshake, REASON_UNSPECIFIED), None);
                    }
                }
            }
            HandshakeStep::Install(request) => {
                let Some(peer) = IndividualAddress::new(handshake.bssid) else {
                    return (leave_handshake(handshake, REASON_UNSPECIFIED), None);
                };
                match handshake.keys.take_pairwise_slot() {
                    Some(slot) => {
                        let key = *request.pairwise().key().as_bytes();
                        (
                            HandshakeStep::InstallGroup(request),
                            Some(Owed::InstallPairwise { slot, peer, key }),
                        )
                    }
                    None => (HandshakeStep::Install(request), None),
                }
            }
            HandshakeStep::InstallGroup(request) => {
                // A key index that is no group key's fails the handshake.
                let Some((key_id, key)) = group_key(request.group()) else {
                    return (leave_handshake(handshake, REASON_UNSPECIFIED), None);
                };
                match handshake.keys.take_group_slot(key_id) {
                    Some(slot) => (
                        HandshakeStep::AwaitInstalled(request),
                        Some(Owed::InstallGroup { slot, key_id, key }),
                    ),
                    None => (HandshakeStep::InstallGroup(request), None),
                }
            }
            HandshakeStep::SendLast(message, epoch) => {
                match self.encode_eapol(handshake.bssid, message.as_bytes()) {
                    Ok(transmit) => (
                        HandshakeStep::AwaitLast(epoch),
                        Some(Owed::Transmit(transmit)),
                    ),
                    Err(Unsent::NoTokens) => (HandshakeStep::SendLast(message, epoch), None),
                    Err(Unsent::Encode) => {
                        return (leave_handshake(handshake, REASON_UNSPECIFIED), None);
                    }
                }
            }
            step => (step, None),
        };
        handshake.step = step;
        (State::Handshake(handshake), owed)
    }

    /// An EAPOL frame to `bssid` in a data frame, in a buffer.
    fn encode_eapol(&mut self, bssid: [u8; 6], eapol: &[u8]) -> Result<StaTransmit<T, K>, Unsent> {
        let address = self.config.address;
        let sequence_number = self.sequence.take();
        self.encode_individual(
            |frame| {
                oer_ieee80211_mac::station::StaDataFrame {
                    source: address,
                    bssid,
                    destination: bssid,
                    sequence_number,
                    ether_type: EAPOL_ETHER_TYPE,
                    payload: eapol,
                }
                .encode(frame)
                .ok()
            },
            StaFrameClass::Connection,
        )
    }

    /// The action the connected state owes.
    fn owe_connected(
        &mut self,
        mut connected: Connected<T, K>,
    ) -> (State<T, K>, Option<Owed<T, K>>) {
        if connected.connected_notice {
            connected.connected_notice = false;
            let notice = StaNotice::Connected {
                bssid: connected.bssid,
                channel: connected.channel.channel().number(),
            };
            return (State::Connected(connected), Some(Owed::Notify(notice)));
        }
        if let Some(fact) = connected.report.take() {
            return (State::Connected(connected), Some(Owed::Upstream(fact)));
        }
        if connected.exchange && matches!(self.lease, Held::Lease(_)) {
            connected.exchange = false;
            return (State::Connected(connected), Some(Owed::Exchange));
        }
        if let Some(owed) = self.owe_rekey(&mut connected) {
            return match owed {
                Ok(owed) => (State::Connected(connected), Some(owed)),
                Err(reason) => (
                    leave_connected(connected, reason, Some(REASON_UNSPECIFIED)),
                    None,
                ),
            };
        }
        let owed = self.owe_background(&mut connected);
        (State::Connected(connected), owed)
    }

    /// The action of a group key's handshake, if one is owed; a failure's
    /// reason to leave.
    fn owe_rekey(
        &mut self,
        connected: &mut Connected<T, K>,
    ) -> Option<Result<Owed<T, K>, StaDisconnectReason>> {
        let rsn = connected.rsn.as_mut()?;
        let (rekey, owed) = match core::mem::replace(&mut rsn.rekey, Rekey::None) {
            Rekey::Install(request) => {
                let Some((key_id, key)) = group_key(request.group()) else {
                    return Some(Err(StaDisconnectReason::HandshakeFailed));
                };
                if let Some(slot) = connected.keys.take_group_slot(key_id) {
                    (
                        Rekey::AwaitInstalled(request),
                        Some(Owed::InstallGroup { slot, key_id, key }),
                    )
                } else if let Some((old, key)) = connected.keys.take_group_key() {
                    // No free slot: the old key goes first, and its frames
                    // with it.
                    forget_group(rsn, &connected.keys, old);
                    (Rekey::AwaitFree(request), Some(Owed::RemoveGroup(key)))
                } else {
                    (Rekey::Install(request), None)
                }
            }
            // After the handshake, EAPOL goes under the pairwise key, as in
            // the old ESP32-S31 path (`transmit_protected_data`): only while
            // the link is up.
            Rekey::Send(message)
                if connected.sending.is_none()
                    && self.link_up(&connected.power, &connected.keys) =>
            {
                let qos = connected.qos.then_some(&mut connected.qos_sequences);
                match self.encode_protected_eapol(connected.bssid, qos, message.as_bytes()) {
                    Ok(transmit) => {
                        connected.sending = Some(Sending::GroupResponse);
                        (Rekey::AwaitSent, Some(Owed::TransmitProtected(transmit)))
                    }
                    Err(Unsent::NoTokens) => (Rekey::Send(message), None),
                    Err(Unsent::Encode) => return Some(Err(StaDisconnectReason::Encode)),
                }
            }
            Rekey::Retire => match connected.keys.retiring.take() {
                Some((old, key)) => {
                    connected.keys.group_busy += 1;
                    forget_group(rsn, &connected.keys, old);
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
    fn owe_background(&mut self, connected: &mut Connected<T, K>) -> Option<Owed<T, K>> {
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
                        owed = bss_rules(bssid, Rules::<K>::WIDENED).map(Owed::Configure);
                        BackgroundStep::AwaitWidened
                    }
                    Some(channel)
                        if self.absence_place.is_some() && matches!(self.lease, Held::Lease(_)) =>
                    {
                        owed = Some(Owed::Absence(ChannelRequest {
                            channel,
                            purpose: Purpose::Scan {
                                dwell: self.config.dwell,
                            },
                        }));
                        BackgroundStep::AwaitAnswer
                    }
                    _ => step,
                },
                BackgroundStep::NeedSleep if connected.sending.is_none() => {
                    power = Some((StaPowerManagement::PowerSave, Sending::Sleep));
                    BackgroundStep::AwaitSleep
                }
                BackgroundStep::NeedReady => {
                    owed = absence_id.map(Owed::Ready);
                    BackgroundStep::AwaitStart
                }
                BackgroundStep::NeedConfigure => {
                    owed = Some(Owed::Configure(StationReceive::OtherBssManagement));
                    BackgroundStep::AwaitConfigured
                }
                BackgroundStep::NeedProbe if connected.sending.is_none() => {
                    owed = self.probe(&mut connected.sending);
                    BackgroundStep::Listen
                }
                BackgroundStep::NeedHome => {
                    owed = bss_rules(bssid, Rules::<K>::MEMBER).map(Owed::Configure);
                    BackgroundStep::AwaitHome
                }
                BackgroundStep::NeedWake if connected.sending.is_none() => {
                    power = Some((StaPowerManagement::Active, Sending::Wake));
                    BackgroundStep::AwaitWake
                }
                BackgroundStep::HomeProbe { until } if connected.sending.is_none() => {
                    owed = self.probe(&mut connected.sending);
                    BackgroundStep::HomeListen { until }
                }
                BackgroundStep::Narrow => {
                    owed = bss_rules(bssid, Rules::<K>::MEMBER).map(Owed::Configure);
                    BackgroundStep::AwaitNarrowed
                }
                step => step,
            };
        } else if connected.wake && !connected.power.asleep() && connected.sending.is_none() {
            // The access point buffers for an awake station: PM=0 again.
            connected.wake = false;
            power = Some((StaPowerManagement::Active, Sending::Wake));
        }
        if let Some((power_management, sending)) = power {
            match self.encode_null(bssid, power_management) {
                Some(transmit) => {
                    connected.sending = Some(sending);
                    owed = Some(Owed::Transmit(transmit));
                }
                // No token now: the step waits for the next poll.
                None => {
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
            }
        }
        owed
    }

    /// A Probe Request of the background scan, marked in flight.
    fn probe(&mut self, sending: &mut Option<Sending>) -> Option<Owed<T, K>> {
        let transmit = self.encode_probe()?;
        *sending = Some(Sending::Probe);
        Some(Owed::Transmit(transmit))
    }

    /// A Null to `bssid` with `power_management`, in a buffer.
    fn encode_null(
        &mut self,
        bssid: [u8; 6],
        power_management: StaPowerManagement,
    ) -> Option<StaTransmit<T, K>> {
        let address = self.config.address;
        let sequence_number = self.sequence.take();
        let mut transmit = self
            .encode_individual(
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
            )
            .ok()?;
        transmit.plan = StaRatePlan::Null;
        Some(transmit)
    }

    fn owe_join(
        &mut self,
        mut join: Join<T, K>,
        now: Instant,
    ) -> (State<T, K>, Option<Owed<T, K>>) {
        let connect = ChannelRequest {
            channel: join.channel,
            purpose: Purpose::Connect,
        };
        let (step, owed) = match core::mem::replace(&mut join.step, JoinStep::NeedLease) {
            JoinStep::RetryAt(at) if now >= at && self.place.is_some() => {
                (JoinStep::AwaitAnswer, Some(Owed::Request(connect)))
            }
            JoinStep::NeedLease if self.place.is_some() => {
                (JoinStep::AwaitAnswer, Some(Owed::Request(connect)))
            }
            JoinStep::NeedConfigure => match bss_rules(join.access_point.bssid, Rules::<K>::MEMBER)
            {
                Some(receive) => (JoinStep::AwaitConfigured, Some(Owed::Configure(receive))),
                None => return (leave_unkeyed(join.keys, StaDisconnectReason::Encode), None),
            },
            JoinStep::Authenticate(mut auth) if auth.stage == AuthStage::NeedSend => {
                let Ok(attempt) = auth.runtime.begin_attempt(&mut self.sequence) else {
                    return (
                        leave_unkeyed(join.keys, StaDisconnectReason::AuthenticationFailed),
                        None,
                    );
                };
                let bssid = join.access_point.bssid;
                let encoded = self.encode_individual(
                    |frame| {
                        OpenAuthenticationRequest {
                            source: bssid,
                            bssid,
                            sequence_number: attempt.sequence_number,
                        }
                        .encode(frame)
                        .ok()
                    },
                    StaFrameClass::Connection,
                );
                match encoded {
                    Ok(transmit) => {
                        auth.stage = AuthStage::AwaitSent {
                            response_timeout: attempt.response_timeout,
                        };
                        (JoinStep::Authenticate(auth), Some(Owed::Transmit(transmit)))
                    }
                    // No token now: the next poll tries again.
                    Err(Unsent::NoTokens) => (JoinStep::Authenticate(auth), None),
                    Err(Unsent::Encode) => {
                        return (leave_unkeyed(join.keys, StaDisconnectReason::Encode), None);
                    }
                }
            }
            JoinStep::Associate(mut runtime) => match runtime.poll(now, &mut self.sequence) {
                Ok(StaAssociationPoll::Transmit(attempt)) => {
                    let capabilities = self.config.association;
                    let phy = self.config.phy;
                    let address = self.config.address;
                    let access_point = &join.access_point;
                    let rsn = &join.rsn;
                    let encoded = self.encode_individual(
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
                    );
                    match encoded {
                        Ok(transmit) => {
                            (JoinStep::Associate(runtime), Some(Owed::Transmit(transmit)))
                        }
                        Err(Unsent::NoTokens) => (JoinStep::Associate(runtime), None),
                        Err(Unsent::Encode) => {
                            return (leave_unkeyed(join.keys, StaDisconnectReason::Encode), None);
                        }
                    }
                }
                Ok(StaAssociationPoll::Idle) => (JoinStep::Associate(runtime), None),
                Ok(StaAssociationPoll::Failed { .. }) | Err(_) => {
                    return (
                        leave_unkeyed(join.keys, StaDisconnectReason::AssociationFailed),
                        None,
                    );
                }
            },
            // The next state owes its first action at once.
            JoinStep::Report {
                association_id,
                peer,
            } => {
                let state = self.joined(join, association_id, peer, now);
                return self.owe(state, now);
            }
            step => (step, None),
        };
        join.step = step;
        (State::Join(join), owed)
    }

    /// The associated station: its handshake, or, on an open network,
    /// connected.
    fn joined(
        &mut self,
        join: Join<T, K>,
        association_id: u16,
        peer: Peer,
        now: Instant,
    ) -> State<T, K> {
        self.associated_owed = Some(peer);
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
                power: PowerSaveView::awake(),
                wake: false,
                sending: None,
                background: None,
                connected_notice: true,
                rsn: None,
                keys: join.keys,
            });
        }
        let supplicant = RsnStaSupplicant::try_new(
            self.config.address,
            bssid,
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
                keys: join.keys,
            }),
            _ => leave_keyed(
                join.keys,
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

    /// The frame `encode` writes into a buffer, as an individual frame of
    /// `class`.
    fn encode_individual(
        &mut self,
        encode: impl FnOnce(&mut [u8]) -> Option<usize>,
        class: StaFrameClass,
    ) -> Result<StaTransmit<T, K>, Unsent> {
        let (ticket, mut buffer) = self.tokens().ok_or(Unsent::NoTokens)?;
        let frame = encode(buffer.bytes_mut()).and_then(|len| {
            let header = HeaderSpec::decode(&buffer.bytes_mut()[..len], IndividualAddress::new)?;
            SizedFrame::new(AckHeader::new(header)?, MpduLength::new(len)?)
        });
        match frame {
            Some(frame) => Ok(StaTransmit {
                ticket,
                buffer,
                frame: StaFrame::Individual(frame),
                class,
                plan: StaRatePlan::Management,
            }),
            None => {
                self.tickets.put(ticket);
                self.buffers.put(buffer);
                Err(Unsent::Encode)
            }
        }
    }

    /// Whether the connected link is up: the station is home and awake,
    /// and holds its pairwise key ([`Self::link`]'s rule).
    fn link_up(&self, power: &PowerSaveView, keys: &Keys<T>) -> bool {
        self.absence.is_none()
            && !power.asleep()
            && matches!(keys.pairwise, KeyCell::Installed(_))
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
        bssid: [u8; 6],
        qos: Option<&mut [StaSequenceCounter; 8]>,
        eapol: &[u8],
    ) -> Result<StaTransmit<T, K>, Unsent> {
        const PRIORITY: u8 = 7;
        let peer_qos = qos.is_some();
        let sequence_number = match qos {
            Some(sequences) => sequences[usize::from(PRIORITY)].take(),
            None => self.sequence.take(),
        };
        let frame = oer_ieee80211_mac::station::StaProtectedDataFrame {
            source: self.config.address,
            bssid,
            destination: bssid,
            sequence_number,
            user_priority: PRIORITY,
            peer_qos,
            ccmp_header: [0; 8],
            ether_type: EAPOL_ETHER_TYPE,
            payload: eapol,
        };
        self.encode_individual(|buffer| frame.encode(buffer).ok(), StaFrameClass::Connection)
    }

    /// A broadcast Probe Request in a buffer, if the station holds tokens
    /// and it encodes.
    fn encode_probe(&mut self) -> Option<StaTransmit<T, K>> {
        let (ticket, mut buffer) = self.tokens()?;
        let sequence_number = self.sequence.take();
        let rates = self.config.supported_rates;
        let len = ProbeRequest {
            destination: [0xff; 6],
            source: self.config.address,
            bssid: [0xff; 6],
            sequence_number,
            ssid: &self.config.ssid[..usize::from(self.config.ssid_len)],
            supported_rates: &rates,
        }
        .encode(buffer.bytes_mut())
        .ok();
        let frame = len.and_then(|len| {
            let header = HeaderSpec::decode(&buffer.bytes_mut()[..len], GroupAddress::new)?;
            SizedFrame::new(header, MpduLength::new(len)?)
        });
        match frame {
            Some(frame) => Some(StaTransmit {
                ticket,
                buffer,
                frame: StaFrame::Group(frame),
                class: StaFrameClass::Normal,
                plan: StaRatePlan::Group,
            }),
            None => {
                self.tickets.put(ticket);
                self.buffers.put(buffer);
                None
            }
        }
    }

    /// A management frame of `subtype` to `bssid` in a buffer.
    fn encode_management(
        &mut self,
        subtype: StaManagementSubtype,
        bssid: [u8; 6],
        body: &[u8],
    ) -> Option<StaTransmit<T, K>> {
        let address = self.config.address;
        let sequence_number = self.sequence.take();
        self.encode_individual(
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
        )
        .ok()
    }

    /// When the station next needs a [`StaInput::Tick`], even without an
    /// event.
    pub fn poll_deadline(&self) -> Option<Instant> {
        match &self.state {
            State::Join(Join {
                step:
                    JoinStep::Authenticate(Authenticate {
                        stage: AuthStage::AwaitResponse { until },
                        ..
                    }),
                ..
            }) => Some(*until),
            State::Join(Join {
                step: JoinStep::Associate(runtime),
                ..
            }) => runtime.next_deadline(),
            State::Join(Join {
                step: JoinStep::RetryAt(at),
                ..
            }) => Some(*at),
            State::Handshake(Handshake {
                step: HandshakeStep::Await,
                deadline,
                ..
            }) => Some(deadline.deadline()),
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
fn bss_rules<K>(
    bssid: [u8; 6],
    filter: SupportedFilter<K, StationRole>,
) -> Option<StationReceive<K>> {
    Some(StationReceive::Bss {
        bssid: IndividualAddress::new(bssid)?,
        filter,
    })
}

/// Leave a network the station holds no key of: receive off, the lease
/// back.
fn leave_unkeyed<T: StaTokens, K>(keys: Keys<T>, reason: StaDisconnectReason) -> State<T, K> {
    State::Leave(Leave {
        reason,
        step: LeaveStep::NeedConfigure,
        keys,
    })
}

/// Leave a network the station may hold keys of: the keys first, then the
/// Deauthentication, if any.
fn leave_keyed<T: StaTokens, K>(
    keys: Keys<T>,
    reason: StaDisconnectReason,
    deauthentication: Option<Deauthentication>,
) -> State<T, K> {
    State::Leave(Leave {
        reason,
        step: LeaveStep::RemoveKeys(deauthentication),
        keys,
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
        handshake.keys,
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
    leave_keyed(
        connected.keys,
        reason,
        reason_code.map(|reason_code| Deauthentication { bssid, reason_code }),
    )
}

/// The handshake's end: connected, its supplicant and keys held.
fn connect<T: StaTokens, K>(handshake: Handshake<T, K>) -> State<T, K> {
    let Handshake {
        bssid,
        channel,
        association_id,
        qos,
        peer,
        supplicant,
        keys,
        step,
        ..
    } = handshake;
    let HandshakeStep::AwaitLast(replay) = step else {
        return leave_keyed(keys, StaDisconnectReason::HandshakeFailed, None);
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
            power: PowerSaveView::awake(),
            wake: false,
            sending: None,
            background: None,
            connected_notice: true,
            rsn: Some(ConnectedRsn {
                supplicant,
                rekey: Rekey::None,
                replay,
            }),
            keys,
        }),
        Err(_) => leave_keyed(
            keys,
            StaDisconnectReason::HandshakeFailed,
            Some(Deauthentication {
                bssid,
                reason_code: REASON_UNSPECIFIED,
            }),
        ),
    }
}

/// Why a frame was not handed out.
enum Unsent {
    /// The station holds no ticket or buffer now.
    NoTokens,
    /// The frame did not encode.
    Encode,
}

#[cfg(test)]
mod tests;
