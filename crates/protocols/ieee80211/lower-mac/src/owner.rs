//! The lower-MAC port on the owner shape of `oer-ieee80211-port`
//! ([`Owner`](oer_ieee80211_port::Owner), [`PortBody`](oer_ieee80211_port::PortBody)).
//!
//! Every refusal of the old lower-MAC port trait that a caller's own state
//! explains is a type here ([ownership and refusals]); what the world
//! controls (the air, time, the hardware) stays a typed outcome. A backend
//! implements [`LowerMacBackend`].
//!
//! | Refusal of the port trait | Form here |
//! | --- | --- |
//! | lifecycle refusals, `SubmitError::Disabled`, `SettingError::PortEnabled` | the owner's state: the install gives the port its channel and addresses, [`LowerMacSetup`] changes the channel only while disabled, a backend whose addresses change implements [`AddressSetup`], clients exist only in a running phase |
//! | `SettingError::UnsupportedChannel` | [`SupportedChannel`] |
//! | `SettingError::UnknownVif`, `SettingError::UnsupportedRole`, `SubmitError::UnknownVif` | a client per interface, its role a type ([`StationClient`], [`AccessPointClient`]) with only that role's operations (the station's HE BSS color); an attempt names no interface |
//! | the receive rules a role may request, a BSSID the role cannot take | [`SupportedFilter`] of the role, and the station's [`StationReceive`]; an access point's BSSID is its address |
//! | `SettingError::Busy` (configuring an interface, removing a key an attempt in flight uses) | [`StationClient::configure`], [`AccessPointClient::configure`] and the key removals wait for the published attempts and abort the ones the closed gate holds, never refused |
//! | `SubmitError::UnsupportedRate` | [`PhaseRate`], checked against the phase's channel; a BlockAckReq takes a [`LegacyPhaseRate`] |
//! | `SubmitError::Unsupported` (backoff, power, coexistence level) | [`SupportedBackoff`], [`SupportedPower`], [`SupportedCoex`] |
//! | `SubmitError::Unsupported` (a response the frame or rate cannot carry: an ACK of a group frame, no ACK at a rate without it or of a frame that does not say so) | the buffer's [`TxFrame`] kind: an [`IndividualAddress`] or [`GroupAddress`] receiver, and for no ACK a [`NoAckHeader`] at a [`SupportedNoAck`] rate; the backend writes the QoS Ack Policy from the kind ([`QosControl`]) |
//! | `SubmitError::Busy` (the queue holds an attempt) | the queue's grant ([`LowerMacClient::queue`], awaited in the pool's order with the seat of the interface's access category, a [`QueueGrant`] of the backend's own type): a submission moves it into the backend, which gives the queue back when it takes the attempt's terminal |
//! | `SubmitError::DuplicateId` | the backend mints the [`Work`] of each submission |
//! | `SubmitError::InvalidLength`, a buffer request past `max_mpdu_length`, no free buffer | [`MpduLength`], and a buffer ([`LowerMacClient::tx_buffer`], awaited with a seat, never refused) the attempt holds until its terminal event |
//! | `SettingError::NoKeySlot`, `SettingError::UnknownKey`, `SubmitError::UnknownKey` | a key slot ([`Slot`] of the client's own kind, pairwise or group; an access point's pairwise slot is an association identifier) that the install's resources give once, which the caller counts itself; an installed key token the attempt borrows ([`AttemptKey`]) |
//! | `SettingError::InvalidKey` | [`Ccmp128Key`], [`GroupKeyId`], an [`IndividualAddress`] peer |
//! | `SettingError::NoBlockAckSlot`, `SettingError::InvalidBlockAck` | a Block Ack [`Slot`] of the client's own kind and [`SupportedBlockAck`]; the client keeps one agreement per peer and [`Tid`], and a new one for the pair replaces the old and gives its slot back |
//! | `SettingError::Busy` (closing the transmit gate) | [`LowerMacControl::close_gate`], awaited, which yields the gate's guard |
//! | `SettingError::Unsupported` (an EDCA record outside the limits, a BSS color above 63) | [`SupportedEdca`], [`BssColor`] |
//!
//! These stay the world's outcomes: [`Poisoned`],
//! [`EventsLost`](oer_radio_port::EventsLost) (only unsolicited
//! reception), a clock that could not be read, a TSF that failed its timing
//! check, a cancel that finds the work already ended
//! ([`CancelError::NotRunning`](oer_ieee80211_port::CancelError::NotRunning)),
//! and hardware that rejects a key or misreads a programmed agreement (a
//! fault).
//!
//! # Capabilities
//!
//! What the hardware supports is the same for every instance of a backend
//! type, so it is an associated constant of a capabilities marker
//! ([`Ieee80211Capabilities`]); a validated value is branded by that
//! marker, minted only by its checks, and a portable core is generic over
//! the marker alone. A literal value is checked at compile time:
//!
//! ```
//! # use oer_ieee80211_lower_mac::{Channel, ChannelWidth, Ieee80211Capabilities, SupportedChannel};
//! fn tune<K: Ieee80211Capabilities>() -> SupportedChannel<K> {
//!     const {
//!         SupportedChannel::<K>::from_const(match Channel::ghz2_4(6, ChannelWidth::Mhz20) {
//!             Ok(channel) => channel,
//!             Err(_) => panic!("channel 6 is a 2.4 GHz channel"),
//!         })
//!     }
//! }
//! ```
//!
//! # Tokens
//!
//! Every token carries the install's brand `'id`. The install's resources
//! give its slots and permits once, through `oer_radio_port::Mint`: the
//! queue and buffer permits, which the backend keeps, each role's key
//! and Block Ack slots, which the caller counts itself, and each
//! interface's transmit seats ([`TxSeats`]): a seat on the queue line and
//! one on the buffer line per access category, which the caller lends to
//! each take and keeps across takes (an A-MPDU's sequential buffers). A
//! queue seat's index names its access category
//! ([`seat_access_category`]), so the queue a take waits for is the one
//! the capabilities assign that access category.
//! An installed key holds its slot and outlives the phases between; a
//! Block Ack agreement's slot stays in its client's table, one per peer and
//! TID, which gives it back when the agreement is replaced or removed. A
//! slot's kind is its client's, so a station's key is not an access
//! point's. The queue and buffer permits stay in the backend's held part of
//! the install (such as `oer-ieee80211-runtime`'s `TxPermits`): a client
//! awaits them as the backend's own guards ([`LowerMacClient::Queue`],
//! [`LowerMacClient::TxBuffer`]) that give
//! them back when dropped before a submission; a submission moves them in,
//! and the backend gives them back when it takes the attempt's terminal,
//! before it hands the event out. No caller holds a submitted attempt's
//! permits, and a dropped handle or event loses none. The
//! backend mints its own tokens (a key, a queue grant, a buffer), each
//! neither `Clone` nor `Copy` and without a public constructor, which it
//! proves with compile-fail tests of its types.
//!
//! # Compile-fail tests
//!
//! A channel the capabilities do not state does not compile as a constant:
//!
//! ```compile_fail,E0080
//! # use oer_ieee80211_lower_mac::*;
//! # use oer_ieee80211_mac::phy::{HeMcs, HtMcs};
//! # struct TwoFourOnly;
//! # impl Ieee80211Capabilities for TwoFourOnly {
//! #     const CAPS: LowerMacCapabilities = LowerMacCapabilities {
//! #         bands: BandSet::GHZ2_4,
//! #         widths: WidthSet::MHZ20,
//! #         rates: RateSupport {
//! #             dsss_cck: true,
//! #             ofdm: true,
//! #             ht_max_mcs: HtMcs::new(7),
//! #             he_max_mcs: HeMcs::new(9),
//! #             he_max_bandwidth_mhz: 20,
//! #             he_dcm: false,
//! #             he_ldpc: false,
//! #             spatial_streams: 1,
//! #         },
//! #         services: HardwareServices::FCS,
//! #         vifs: 2,
//! #         tx_queues: 4,
//! #         max_mpdu_length: 2304,
//! #         max_backoff_slots: 1023,
//! #         tx_power_ceiling_min_dbm: Some(0),
//! #         coex_priorities: CoexPrioritySet::only(CoexPriority::Normal),
//! #         individual_no_ack: PhyFormatSet::NON_HT,
//! #         station_receive_filters: ReceiveFilter::BSS_MEMBER,
//! #         access_point_receive_filters: ReceiveFilter::BSS_MEMBER,
//! #         key_slots: 4,
//! #         rx_block_ack_agreements: 2,
//! #         rx_block_ack_max_tid: 7,
//! #         rx_block_ack_max_window: 64,
//! #         edca_max_aifsn: 15,
//! #         edca_max_cw_exponent: 10,
//! #     };
//! # }
//! const FIVE: SupportedChannel<TwoFourOnly> =
//!     SupportedChannel::from_const(match Channel::ghz5(36, ChannelWidth::Mhz20) {
//!         Ok(channel) => channel,
//!         Err(_) => panic!(),
//!     });
//! # fn main() { let _ = FIVE; }
//! ```
//!
//! Its compiling twin names a channel the capabilities state:
//!
//! ```
//! # use oer_ieee80211_lower_mac::*;
//! # use oer_ieee80211_mac::phy::{HeMcs, HtMcs};
//! # struct TwoFourOnly;
//! # impl Ieee80211Capabilities for TwoFourOnly {
//! #     const CAPS: LowerMacCapabilities = LowerMacCapabilities {
//! #         bands: BandSet::GHZ2_4,
//! #         widths: WidthSet::MHZ20,
//! #         rates: RateSupport {
//! #             dsss_cck: true,
//! #             ofdm: true,
//! #             ht_max_mcs: HtMcs::new(7),
//! #             he_max_mcs: HeMcs::new(9),
//! #             he_max_bandwidth_mhz: 20,
//! #             he_dcm: false,
//! #             he_ldpc: false,
//! #             spatial_streams: 1,
//! #         },
//! #         services: HardwareServices::FCS,
//! #         vifs: 2,
//! #         tx_queues: 4,
//! #         max_mpdu_length: 2304,
//! #         max_backoff_slots: 1023,
//! #         tx_power_ceiling_min_dbm: Some(0),
//! #         coex_priorities: CoexPrioritySet::only(CoexPriority::Normal),
//! #         individual_no_ack: PhyFormatSet::NON_HT,
//! #         station_receive_filters: ReceiveFilter::BSS_MEMBER,
//! #         access_point_receive_filters: ReceiveFilter::BSS_MEMBER,
//! #         key_slots: 4,
//! #         rx_block_ack_agreements: 2,
//! #         rx_block_ack_max_tid: 7,
//! #         rx_block_ack_max_window: 64,
//! #         edca_max_aifsn: 15,
//! #         edca_max_cw_exponent: 10,
//! #     };
//! # }
//! const SIX: SupportedChannel<TwoFourOnly> =
//!     SupportedChannel::from_const(match Channel::ghz2_4(6, ChannelWidth::Mhz20) {
//!         Ok(channel) => channel,
//!         Err(_) => panic!(),
//!     });
//! # fn main() { let _ = SIX; }
//! ```
//!
//! No caller or backend mints a rate past the phase's check:
//!
//! ```compile_fail,E0624
//! # use oer_ieee80211_lower_mac::{PhaseRate, PhyRate};
//! # use oer_ieee80211_mac::phy::LegacyRate;
//! let _ = PhaseRate::checked(PhyRate::Legacy(LegacyRate::Ofdm6M));
//! ```
//!
//! ```
//! # use oer_ieee80211_lower_mac::{LowerMacClient, PhaseRate, PhyRate};
//! # use oer_ieee80211_mac::phy::LegacyRate;
//! fn rate<'id, C: LowerMacClient<'id>>(client: &C) -> Option<PhaseRate<'_>> {
//!     client.rate(PhyRate::Legacy(LegacyRate::Ofdm6M))
//! }
//! ```
//!
//! ```compile_fail,E0451
//! # use oer_ieee80211_lower_mac::BssColor;
//! let _ = BssColor { color: 64 };
//! ```
//!
//! ```
//! # use oer_ieee80211_lower_mac::BssColor;
//! assert!(BssColor::new(64).is_none());
//! assert!(BssColor::new(63).is_some());
//! ```
//!
//! [ownership and refusals]: https://github.com/ermacv/open-esp-radio-rs/blob/main/docs/architecture.md#ownership-and-refusals

use core::{future::Future, marker::PhantomData};

use oer_ieee80211_mac::qos::WmmAccessCategory;
use oer_ieee80211_mac::{extensions::wmm::WmmParameterSet, sequence::SequenceNumber};
use oer_ieee80211_port::{PortBackend, RadioPortClient, RadioPortEvents, Work};
use oer_radio_port::{Line, Mint, Poisoned, Seat, Slot};

use crate::{
    Backoff, Channel, CoexPriority, LowerMacCapabilities, MacAddress, PhyRate, ReceiveFilter,
    RxBeaconPriority, TxBody, TxPower, VifRole,
};

/// What a lower-MAC backend type supports, the same for every instance: the
/// brand of its validated values.
pub trait Ieee80211Capabilities {
    /// The backend's capabilities.
    const CAPS: LowerMacCapabilities;
}

/// A channel the capabilities `K` tune to.
#[derive(Debug)]
pub struct SupportedChannel<K> {
    channel: Channel,
    brand: PhantomData<fn() -> K>,
}

impl<K: Ieee80211Capabilities> SupportedChannel<K> {
    /// `channel`, when `K` tunes to it.
    pub const fn new(channel: Channel) -> Option<Self> {
        if K::CAPS.supports_channel(channel) {
            Some(Self {
                channel,
                brand: PhantomData,
            })
        } else {
            None
        }
    }

    /// `channel`, checked when the image is built: an unsupported channel
    /// does not compile in a constant.
    ///
    /// # Panics
    ///
    /// At compile time in a constant, and at run time otherwise, when `K`
    /// does not tune to `channel`.
    pub const fn from_const(channel: Channel) -> Self {
        match Self::new(channel) {
            Some(supported) => supported,
            None => panic!("the backend does not tune to this channel"),
        }
    }

    /// The channel.
    pub const fn channel(&self) -> Channel {
        self.channel
    }
}

/// An interface role, as a type.
pub trait InterfaceRole {
    /// The role.
    const ROLE: VifRole;
}

/// The station role.
pub enum Station {}

/// The access-point role.
pub enum AccessPoint {}

impl InterfaceRole for Station {
    const ROLE: VifRole = VifRole::Station;
}

impl InterfaceRole for AccessPoint {
    const ROLE: VifRole = VifRole::AccessPoint;
}

/// Receive rules an interface of role `R` may request of `K`.
pub struct SupportedFilter<K, R> {
    filter: ReceiveFilter,
    brand: PhantomData<fn() -> (K, R)>,
}

impl<K: Ieee80211Capabilities, R: InterfaceRole> SupportedFilter<K, R> {
    /// `filter`, when `K` admits every rule of it for role `R`.
    pub const fn new(filter: ReceiveFilter) -> Option<Self> {
        if K::CAPS.receive_filters(R::ROLE).contains(filter) {
            Some(Self {
                filter,
                brand: PhantomData,
            })
        } else {
            None
        }
    }

    /// The rules.
    pub const fn get(&self) -> ReceiveFilter {
        self.filter
    }
}

impl<K, R> Clone for SupportedFilter<K, R> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K, R> Copy for SupportedFilter<K, R> {}

impl<K, R> core::fmt::Debug for SupportedFilter<K, R> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_tuple("SupportedFilter")
            .field(&self.filter)
            .finish()
    }
}

/// What a station receives: nothing, other BSSs' management only (outside
/// a BSS), or a BSS with its rules.
pub enum StationReceive<K> {
    Off,
    OtherBssManagement,
    Bss {
        bssid: IndividualAddress,
        filter: SupportedFilter<K, Station>,
    },
}

impl<K> Clone for StationReceive<K> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K> Copy for StationReceive<K> {}

/// The identifier of a group key, 0 to 3.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GroupKeyId(u8);

impl GroupKeyId {
    /// `key_id`, when it is at most 3.
    pub const fn new(key_id: u8) -> Option<Self> {
        if key_id <= 3 {
            Some(Self(key_id))
        } else {
            None
        }
    }

    /// The identifier.
    pub const fn get(self) -> u8 {
        self.0
    }
}

/// A backoff `K` counts down.
#[derive(Debug)]
pub struct SupportedBackoff<K> {
    backoff: Backoff,
    brand: PhantomData<fn() -> K>,
}

impl<K: Ieee80211Capabilities> SupportedBackoff<K> {
    /// `backoff`, when `K` counts it down.
    pub const fn new(backoff: Backoff) -> Option<Self> {
        if K::CAPS.supports_backoff(backoff) {
            Some(Self {
                backoff,
                brand: PhantomData,
            })
        } else {
            None
        }
    }

    /// The backoff.
    pub const fn get(&self) -> Backoff {
        self.backoff
    }
}

/// A transmit power `K` applies.
#[derive(Debug)]
pub struct SupportedPower<K> {
    power: TxPower,
    brand: PhantomData<fn() -> K>,
}

impl<K: Ieee80211Capabilities> SupportedPower<K> {
    /// `power`, when `K` applies it: its calibrated power, or a ceiling no
    /// lower than its lowest.
    pub const fn new(power: TxPower) -> Option<Self> {
        let supported = match (power, K::CAPS.tx_power_ceiling_min_dbm) {
            (TxPower::Calibrated, _) => true,
            (TxPower::MaxDbm(dbm), Some(lowest)) => dbm >= lowest,
            (TxPower::MaxDbm(_), None) => false,
        };
        if supported {
            Some(Self {
                power,
                brand: PhantomData,
            })
        } else {
            None
        }
    }

    /// The power.
    pub const fn get(&self) -> TxPower {
        self.power
    }
}

/// A coexistence level `K` maps onto its arbitration.
#[derive(Debug)]
pub struct SupportedCoex<K> {
    priority: CoexPriority,
    brand: PhantomData<fn() -> K>,
}

impl<K: Ieee80211Capabilities> SupportedCoex<K> {
    /// `priority`, when `K` states it.
    pub const fn new(priority: CoexPriority) -> Option<Self> {
        if K::CAPS.coex_priorities.contains(priority) {
            Some(Self {
                priority,
                brand: PhantomData,
            })
        } else {
            None
        }
    }

    /// The level.
    pub const fn get(&self) -> CoexPriority {
        self.priority
    }
}

/// The length of one MPDU without FCS and MIC: at least a header, at most
/// what `K`'s buffers hold.
#[derive(Debug)]
pub struct MpduLength<K> {
    len: u16,
    brand: PhantomData<fn() -> K>,
}

/// The shortest MPDU: an ACK frame's header.
const MIN_MPDU: u16 = 10;

impl<K: Ieee80211Capabilities> MpduLength<K> {
    /// `len`, when it is an MPDU's length `K` buffers.
    pub const fn new(len: usize) -> Option<Self> {
        if len >= MIN_MPDU as usize && len <= K::CAPS.max_mpdu_length as usize {
            Some(Self {
                len: len as u16,
                brand: PhantomData,
            })
        } else {
            None
        }
    }

    /// The length.
    pub const fn get(&self) -> usize {
        self.len as usize
    }
}

/// A receive Block Ack agreement within `K`'s limits.
#[derive(Debug)]
pub struct SupportedBlockAck<K> {
    peer: IndividualAddress,
    tid: Tid,
    start_sequence: SequenceNumber,
    window: u16,
    brand: PhantomData<fn() -> K>,
}

impl<K: Ieee80211Capabilities> SupportedBlockAck<K> {
    /// An agreement with `peer` on `tid`, when `K` acknowledges its TID and
    /// window.
    pub const fn new(
        peer: IndividualAddress,
        tid: Tid,
        start_sequence: SequenceNumber,
        window: u16,
    ) -> Option<Self> {
        if tid.get() <= K::CAPS.rx_block_ack_max_tid
            && window >= 1
            && window <= K::CAPS.rx_block_ack_max_window
        {
            Some(Self {
                peer,
                tid,
                start_sequence,
                window,
                brand: PhantomData,
            })
        } else {
            None
        }
    }

    /// The originator, the TID, the starting sequence and the window.
    pub const fn get(&self) -> (IndividualAddress, Tid, SequenceNumber, u16) {
        (self.peer, self.tid, self.start_sequence, self.window)
    }
}

/// An EDCA parameter set whose every record lies within `K`'s limits: an
/// AIFSN and contention-window exponents it applies, `ECWmin` no larger
/// than `ECWmax`.
#[derive(Debug)]
pub struct SupportedEdca<K> {
    parameters: WmmParameterSet,
    brand: PhantomData<fn() -> K>,
}

impl<K: Ieee80211Capabilities> SupportedEdca<K> {
    /// `parameters`, when `K` applies every record of it.
    pub const fn new(parameters: WmmParameterSet) -> Option<Self> {
        let categories = [
            oer_ieee80211_mac::qos::WmmAccessCategory::BestEffort,
            oer_ieee80211_mac::qos::WmmAccessCategory::Background,
            oer_ieee80211_mac::qos::WmmAccessCategory::Video,
            oer_ieee80211_mac::qos::WmmAccessCategory::Voice,
        ];
        let mut index = 0;
        while index < categories.len() {
            let record = parameters.access_category(categories[index]);
            if record.aifsn > K::CAPS.edca_max_aifsn
                || record.ecw_max > K::CAPS.edca_max_cw_exponent
                || record.ecw_min > record.ecw_max
            {
                return None;
            }
            index += 1;
        }
        Some(Self {
            parameters,
            brand: PhantomData,
        })
    }

    /// The parameter set.
    pub const fn get(&self) -> WmmParameterSet {
        self.parameters
    }
}

/// The color of an HE BSS, 0-63.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BssColor {
    color: u8,
}

impl BssColor {
    /// `color`, when it is a BSS color.
    pub const fn new(color: u8) -> Option<Self> {
        if color <= 63 {
            Some(Self { color })
        } else {
            None
        }
    }

    /// The color.
    pub const fn get(self) -> u8 {
        self.color
    }
}

/// A CCMP-128 temporal key. It is not `Copy`, and its `Debug` hides its
/// octets, so a key reaches no log.
#[derive(Clone, Eq, PartialEq)]
pub struct Ccmp128Key(pub [u8; 16]);

impl core::fmt::Debug for Ccmp128Key {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("Ccmp128Key(..)")
    }
}

/// A rate the phase's channel carries: [`LowerMacClient::rate`] checks it
/// against the capabilities and the channel the phase runs on.
#[derive(Debug)]
pub struct PhaseRate<'p> {
    rate: PhyRate,
    loan: PhantomData<fn(&'p ()) -> &'p ()>,
}

impl<'p> PhaseRate<'p> {
    /// A rate [`LowerMacClient::rate`] checked; nothing else mints one.
    const fn checked(rate: PhyRate) -> Self {
        Self {
            rate,
            loan: PhantomData,
        }
    }

    /// The rate.
    pub const fn get(&self) -> PhyRate {
        self.rate
    }

    /// The rate, when it is non-HT, as a BlockAckReq needs.
    ///
    /// # Errors
    ///
    /// The rate, when it is not.
    pub fn legacy(self) -> Result<LegacyPhaseRate<'p>, Self> {
        if matches!(self.rate, PhyRate::Legacy(_)) {
            Ok(LegacyPhaseRate(self))
        } else {
            Err(self)
        }
    }
}

/// The receiver of an individual frame: its address has the group bit
/// clear.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IndividualAddress(MacAddress);

impl IndividualAddress {
    /// `address`, when its group bit is clear.
    pub const fn new(address: MacAddress) -> Option<Self> {
        if address[0] & 1 == 0 {
            Some(Self(address))
        } else {
            None
        }
    }

    /// The address.
    pub const fn get(self) -> MacAddress {
        self.0
    }
}

/// The receiver of a group frame: its address has the group bit set.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GroupAddress(MacAddress);

impl GroupAddress {
    /// `address`, when its group bit is set.
    pub const fn new(address: MacAddress) -> Option<Self> {
        if address[0] & 1 == 1 {
            Some(Self(address))
        } else {
            None
        }
    }

    /// The address.
    pub const fn get(self) -> MacAddress {
        self.0
    }
}

/// A traffic identifier, 0 to 15.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Tid(u8);

impl Tid {
    /// `tid`, when it is at most 15.
    pub const fn new(tid: u8) -> Option<Self> {
        if tid <= 15 { Some(Self(tid)) } else { None }
    }

    /// The identifier.
    pub const fn get(self) -> u8 {
        self.0
    }
}

/// The Frame Control field of a data or management frame of protocol
/// version 0: never a control frame, whose kinds have their own buffers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameControl(u16);

impl FrameControl {
    const DATA: u16 = 2;
    const MANAGEMENT: u16 = 0;

    /// `field`, when it is a data or management frame of version 0.
    pub const fn new(field: u16) -> Option<Self> {
        let kind = (field >> 2) & 0b11;
        if field & 0b11 == 0 && (kind == Self::DATA || kind == Self::MANAGEMENT) {
            Some(Self(field))
        } else {
            None
        }
    }

    /// The field.
    pub const fn get(self) -> u16 {
        self.0
    }

    const fn is_qos_data(self) -> bool {
        (self.0 >> 2) & 0b11 == Self::DATA && self.0 & (1 << 7) != 0
    }

    const fn is_four_address(self) -> bool {
        (self.0 >> 2) & 0b11 == Self::DATA && self.0 & (0b11 << 8) == 0b11 << 8
    }

    /// Whether the frame carries an HT Control field: the Order bit of a
    /// QoS data or management frame; in a non-QoS data frame the bit
    /// requests strictly ordered service instead.
    const fn has_ht_control(self) -> bool {
        self.0 & (1 << 15) != 0 && (self.is_qos_data() || (self.0 >> 2) & 0b11 == Self::MANAGEMENT)
    }
}

/// The QoS Control field of a QoS data frame, but its Ack Policy: the
/// backend writes that from the frame's response
/// (its [`TxFrame`] kind), the one source of it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QosControl {
    pub tid: Tid,
    pub end_of_service_period: bool,
    pub amsdu_present: bool,
    /// The field's high octet (TXOP limit, queue size or mesh control).
    pub high: u8,
}

impl QosControl {
    /// The field, with the Ack Policy of a frame that solicits an ACK
    /// (`no_ack` false: Normal Ack) or none (No Ack).
    pub const fn encode(self, no_ack: bool) -> u16 {
        self.tid.get() as u16
            | (self.end_of_service_period as u16) << 4
            | (no_ack as u16) << 5
            | (self.amsdu_present as u16) << 7
            | (self.high as u16) << 8
    }
}

/// The MAC header of a data or management frame, addressed to `A`
/// ([`IndividualAddress`] or [`GroupAddress`]). Its fields agree with its
/// Frame Control: a fourth address exactly when To DS and From DS are both
/// set, a QoS Control exactly in a QoS data frame, an HT Control exactly
/// when the Order bit is set in a QoS data or management frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeaderSpec<A> {
    frame_control: FrameControl,
    duration: u16,
    receiver: A,
    transmitter: MacAddress,
    address3: MacAddress,
    sequence_control: u16,
    address4: Option<MacAddress>,
    qos: Option<QosControl>,
    ht_control: Option<[u8; 4]>,
}

/// A receiver address of a header: [`IndividualAddress`] or
/// [`GroupAddress`].
pub trait ReceiverAddress: Copy {
    /// The address.
    fn address(self) -> MacAddress;
}

impl ReceiverAddress for IndividualAddress {
    fn address(self) -> MacAddress {
        self.0
    }
}

impl ReceiverAddress for GroupAddress {
    fn address(self) -> MacAddress {
        self.0
    }
}

impl<A: ReceiverAddress> HeaderSpec<A> {
    /// The header, when its fields agree with its Frame Control.
    #[allow(clippy::too_many_arguments, reason = "the fields of a MAC header")]
    pub fn new(
        frame_control: FrameControl,
        duration: u16,
        receiver: A,
        transmitter: MacAddress,
        address3: MacAddress,
        sequence_control: u16,
        address4: Option<MacAddress>,
        qos: Option<QosControl>,
        ht_control: Option<[u8; 4]>,
    ) -> Option<Self> {
        (address4.is_some() == frame_control.is_four_address()
            && qos.is_some() == frame_control.is_qos_data()
            && ht_control.is_some() == frame_control.has_ht_control())
        .then_some(Self {
            frame_control,
            duration,
            receiver,
            transmitter,
            address3,
            sequence_control,
            address4,
            qos,
            ht_control,
        })
    }

    /// The header's length in octets.
    pub fn len(&self) -> usize {
        24 + 6 * usize::from(self.address4.is_some())
            + 2 * usize::from(self.qos.is_some())
            + 4 * usize::from(self.ht_control.is_some())
    }

    /// Whether the header is empty: never.
    pub fn is_empty(&self) -> bool {
        false
    }

    /// Write the header into `out` with the Ack Policy of a frame that
    /// solicits no ACK when `no_ack`; a backend's buffer request calls it.
    ///
    /// # Panics
    ///
    /// `out` is shorter than [`Self::len`].
    pub fn encode(&self, out: &mut [u8], no_ack: bool) -> usize {
        let len = self.len();
        let out = &mut out[..len];
        out[0..2].copy_from_slice(&self.frame_control.get().to_le_bytes());
        out[2..4].copy_from_slice(&self.duration.to_le_bytes());
        out[4..10].copy_from_slice(&self.receiver.address());
        out[10..16].copy_from_slice(&self.transmitter);
        out[16..22].copy_from_slice(&self.address3);
        out[22..24].copy_from_slice(&self.sequence_control.to_le_bytes());
        let mut at = 24;
        if let Some(address4) = self.address4 {
            out[at..at + 6].copy_from_slice(&address4);
            at += 6;
        }
        if let Some(qos) = self.qos {
            out[at..at + 2].copy_from_slice(&qos.encode(no_ack).to_le_bytes());
            at += 2;
        }
        if let Some(ht_control) = self.ht_control {
            out[at..at + 4].copy_from_slice(&ht_control);
            at += 4;
        }
        at
    }
}

/// A rate of the phase that carries an individual frame without an ACK,
/// as the capabilities' individual no-ACK formats state:
/// [`LowerMacClient::no_ack`] checks it.
#[derive(Debug)]
pub struct SupportedNoAck<'p>(PhaseRate<'p>);

impl<'p> SupportedNoAck<'p> {
    /// The rate.
    pub const fn rate(&self) -> &PhaseRate<'p> {
        &self.0
    }
}

/// A non-HT rate of the phase, the only rates a BlockAckReq goes at:
/// [`PhaseRate::legacy`] checks it.
#[derive(Debug)]
pub struct LegacyPhaseRate<'p>(PhaseRate<'p>);

impl<'p> LegacyPhaseRate<'p> {
    /// The rate.
    pub const fn rate(&self) -> &PhaseRate<'p> {
        &self.0
    }
}

/// The header of an individual frame that solicits an ACK: any data or
/// management frame but Action No Ack, which its receiver never
/// acknowledges.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AckHeader(HeaderSpec<IndividualAddress>);

impl AckHeader {
    /// `header`, when it is not an Action No Ack frame.
    pub fn new(header: HeaderSpec<IndividualAddress>) -> Option<Self> {
        (header.frame_control.get() & 0x00fc != NoAckHeader::ACTION_NO_ACK).then_some(Self(header))
    }

    /// The header.
    pub const fn header(&self) -> &HeaderSpec<IndividualAddress> {
        &self.0
    }
}

/// The header of an individual frame that solicits no ACK: one that says
/// so to its receiver, a QoS data frame (whose Ack Policy the backend
/// writes as No Ack) or an Action No Ack frame. Any other individual frame
/// is acknowledged by its receiver whatever its sender expects.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NoAckHeader(HeaderSpec<IndividualAddress>);

impl NoAckHeader {
    /// Frame Control's type and subtype bits of an Action No Ack frame.
    const ACTION_NO_ACK: u16 = 0b1110 << 4;

    /// `header`, when it is a QoS data or an Action No Ack frame.
    pub fn new(header: HeaderSpec<IndividualAddress>) -> Option<Self> {
        let frame_control = header.frame_control;
        (frame_control.is_qos_data() || frame_control.get() & 0x00fc == Self::ACTION_NO_ACK)
            .then_some(Self(header))
    }

    /// The header.
    pub const fn header(&self) -> &HeaderSpec<IndividualAddress> {
        &self.0
    }
}

/// What a buffer request describes: the frame's kind, its header and its
/// rate, from which its response follows. Only the kinds the hardware
/// sends exist, so a frame and its response never disagree.
#[derive(Debug)]
pub enum TxFrame<'p, K> {
    /// An individual data or management frame, which solicits an ACK.
    Individual {
        frame: SizedFrame<AckHeader, K>,
        rate: PhaseRate<'p>,
    },
    /// An individual frame that solicits no ACK and says so, at a rate that
    /// carries one without it.
    IndividualNoAck {
        frame: SizedFrame<NoAckHeader, K>,
        rate: SupportedNoAck<'p>,
    },
    /// A group data or management frame, which solicits no response.
    Group {
        frame: SizedFrame<HeaderSpec<GroupAddress>, K>,
        rate: PhaseRate<'p>,
    },
    /// A BlockAckReq, which solicits a BlockAck, at a non-HT rate.
    BlockAckRequest {
        receiver: IndividualAddress,
        transmitter: MacAddress,
        tid: Tid,
        starting_sequence: SequenceNumber,
        rate: LegacyPhaseRate<'p>,
    },
}

/// A header that knows its length: [`HeaderSpec`], [`AckHeader`] and
/// [`NoAckHeader`].
pub trait FrameHeader {
    /// The header's length in octets.
    fn header_len(&self) -> usize;
}

impl<A: ReceiverAddress> FrameHeader for HeaderSpec<A> {
    fn header_len(&self) -> usize {
        self.len()
    }
}

impl FrameHeader for AckHeader {
    fn header_len(&self) -> usize {
        self.0.len()
    }
}

impl FrameHeader for NoAckHeader {
    fn header_len(&self) -> usize {
        self.0.len()
    }
}

/// A frame's header with the length of its whole MPDU, which holds the
/// header: the buffer a request lends is exactly that long.
#[derive(Debug)]
pub struct SizedFrame<H, K> {
    header: H,
    len: MpduLength<K>,
}

impl<H: FrameHeader, K: Ieee80211Capabilities> SizedFrame<H, K> {
    /// `header` in an MPDU of `len` octets, when they hold the header.
    pub fn new(header: H, len: MpduLength<K>) -> Option<Self> {
        (len.get() >= header.header_len()).then_some(Self { header, len })
    }

    /// The header.
    pub const fn header(&self) -> &H {
        &self.header
    }

    /// The MPDU's length.
    pub const fn len(&self) -> &MpduLength<K> {
        &self.len
    }
}

/// The body of a frame a buffer holds: the octets after its header, which
/// the caller writes.
pub trait TxFrameBody {
    /// The octets after the header.
    fn body_mut(&mut self) -> &mut [u8];
}

/// The grant of the queue an attempt of one access category occupies: a
/// backend's guard over the queue's one permit, which the backend hands out
/// and takes back. The queue it names is the one the capabilities assign
/// its access category. Dropped before a submission, it gives the queue
/// back; a submission moves it into the backend. The backend owns its
/// type, keeps its constructor private and proves with compile-fail tests
/// that no caller forges one.
pub trait QueueGrant {
    /// The queue the attempt occupies.
    fn queue(&self) -> u8;

    /// The access category the attempt contends as.
    fn access_category(&self) -> WmmAccessCategory;
}

/// The seats of one line of transmit permits: one per interface role and
/// access category.
pub const TX_SEATS: usize = 8;

/// The access category of the transmit seat at `index` of its line: an
/// interface's four seats follow the access category index (ACI), the
/// station's seats first, then the access point's ([`TxSeats::mint`]).
pub const fn seat_access_category(index: u8) -> WmmAccessCategory {
    match index % 4 {
        0 => WmmAccessCategory::BestEffort,
        1 => WmmAccessCategory::Background,
        2 => WmmAccessCategory::Video,
        _ => WmmAccessCategory::Voice,
    }
}

/// The transmit seats of one interface: for each access category, its
/// seat on the queue line (of the kind `Q`) and on the buffer line (of the
/// kind `B`), indexed by the access category index. The caller lends a
/// seat to each take and keeps it across takes.
pub struct TxSeats<'id, Q, B> {
    queue: [Seat<'id, Q>; 4],
    buffer: [Seat<'id, B>; 4],
}

impl<'id, Q, B> TxSeats<'id, Q, B> {
    /// The [`TX_SEATS`] seats of the queue line of the kind `Q` and of the
    /// buffer line of the kind `B`, with the lines that size their pools,
    /// and the seats of each interface: the station's, then the access
    /// point's.
    #[allow(
        clippy::type_complexity,
        reason = "the two lines and the seats of each role, minted together"
    )]
    pub fn mint(
        mint: &mut Mint<'id>,
    ) -> (Line<'id, Q, TX_SEATS>, Line<'id, B, TX_SEATS>, [Self; 2]) {
        let (queue_line, [q0, q1, q2, q3, q4, q5, q6, q7]) = mint.seats::<Q, TX_SEATS>();
        let (buffer_line, [b0, b1, b2, b3, b4, b5, b6, b7]) = mint.seats::<B, TX_SEATS>();
        (
            queue_line,
            buffer_line,
            [
                Self {
                    queue: [q0, q1, q2, q3],
                    buffer: [b0, b1, b2, b3],
                },
                Self {
                    queue: [q4, q5, q6, q7],
                    buffer: [b4, b5, b6, b7],
                },
            ],
        )
    }

    /// The seat on the queue line of `access_category`.
    pub fn queue(&mut self, access_category: WmmAccessCategory) -> &mut Seat<'id, Q> {
        &mut self.queue[access_category as usize]
    }

    /// The seat on the buffer line of `access_category`.
    pub fn buffer(&mut self, access_category: WmmAccessCategory) -> &mut Seat<'id, B> {
        &mut self.buffer[access_category as usize]
    }
}

/// One attempt of a running phase, every value of it checked: the queue it
/// occupies, its MPDU in a buffer bound to a buffer permit, its body, its
/// protection, key, power, backoff and coexistence level. The frame's kind,
/// fixed when the buffer was lent, sets its rate and response.
pub struct PhaseAttempt<'p, 'id: 'p, C: LowerMacClient<'id> + 'p> {
    pub queue: C::Queue<'p>,
    /// The frame, whose kind fixes its rate and response.
    pub frame: C::TxBuffer<'p>,
    pub body: Option<C::TxBody>,
    pub protection: crate::Protection,
    /// The installed key that protects the frame, or none.
    pub key: Option<AttemptKey<'p, 'id, C>>,
    pub power: SupportedPower<C::Caps>,
    pub backoff: SupportedBackoff<C::Caps>,
    pub coex: SupportedCoex<C::Caps>,
}

/// The installed key an attempt borrows.
pub enum AttemptKey<'p, 'id: 'p, C: LowerMacClient<'id> + 'p> {
    Pairwise(&'p C::PairwiseKey),
    Group(&'p C::GroupKey),
}

/// A lower-MAC backend on the owner shape. Its disabled-only settings are a
/// [`LowerMacSetup`], its phase control a [`LowerMacControl`], its clients
/// [`LowerMacClient`]s and its events a [`LowerMacEvents`]; its
/// held part of the install keeps the transmit permits,
/// and the body's resources give each role's key and Block Ack slots
/// once.
pub trait LowerMacBackend: PortBackend {
    /// The backend's capabilities marker.
    type Caps: Ieee80211Capabilities;
}

/// The disabled-only settings of a lower-MAC port.
pub trait LowerMacSetup {
    /// The backend's capabilities marker.
    type Caps: Ieee80211Capabilities;

    /// Tune to `channel`, which every interface shares, in place of the
    /// channel the install or an earlier call set: a port always has one,
    /// so no phase runs untuned. The next `Enable` retunes. Nothing refuses
    /// it: the port is disabled and the channel is supported.
    fn set_channel(&mut self, channel: SupportedChannel<Self::Caps>);
}

/// The disabled-only setting of a backend whose interface addresses change
/// after its install; one whose hardware fixes them (the ESP32-S31 station
/// address, set by its cold start) does not implement it.
pub trait AddressSetup: LowerMacSetup {
    /// Set the address of the interface of `role`, in place of the one the
    /// install or an earlier call gave it.
    fn set_address(&mut self, role: VifRole, address: IndividualAddress);
}

/// The guard of a closed transmit gate: it proves this port publishes no
/// attempt, not that the air is quiet; reception runs, and other radios and
/// peers still transmit. Dropping it opens the gate, a synchronous register
/// write that cannot fail, which a poisoned port ignores. An operation that
/// needs no attempt on the air (a retune that keeps the port enabled) is a
/// method of the guard, so it cannot meet a published attempt.
pub trait TxGated {}

/// The control of a running lower-MAC phase: the radio-wide operations that
/// only gate or tune admission, with one holder. None refuses as busy.
pub trait LowerMacControl {
    /// The backend's capabilities marker.
    type Caps: Ieee80211Capabilities;
    /// The guard of the closed transmit gate.
    type Gated<'g>: TxGated
    where
        Self: 'g;

    /// Close the transmit gate of every queue: admission of new attempts
    /// stops at once, and the future ends once no attempt is published,
    /// with the guard that keeps the gate closed. A future dropped before
    /// it yields the guard opens the gate again, so no gate stays closed
    /// without its guard.
    fn close_gate(&mut self) -> impl Future<Output = Self::Gated<'_>>;

    /// The EDCA parameters every queue's attempts contend with.
    fn set_edca(&mut self, parameters: SupportedEdca<Self::Caps>);

    /// The coexistence priority beacons are received with.
    fn set_rx_beacon_priority(&mut self, priority: RxBeaconPriority);
}

/// What a lower-MAC client of one interface does in a running phase of the
/// install `'id`.
///
/// A client exists for each interface the backend serves, each with the
/// address the install or [`AddressSetup::set_address`] gave it; its type is
/// its role, so a backend gives a role's client only the operations
/// that role has. Its key and Block Ack slots are of its own kinds, so a
/// key or agreement of one role's client is not another role's, by type.
pub trait LowerMacClient<'id>:
    RadioPortClient<Token: oer_ieee80211_port::RadioWork<Id = crate::TxId>> + Sized
{
    /// The backend's capabilities marker.
    type Caps: Ieee80211Capabilities;
    /// The kind of the queue line's seats.
    type QueueKind;
    /// The kind of the buffer line's seats.
    type BufferKind;
    /// The grant of one queue, which [`Self::queue`] awaits.
    type Queue<'c>: QueueGrant
    where
        Self: 'c,
        'id: 'c;
    /// One frame's memory, bound to a buffer permit it holds, with its
    /// header, kind, rate and response written by the request.
    type TxBuffer<'c>: TxFrameBody
    where
        Self: 'c,
        'id: 'c;
    /// The owner of an MPDU's body.
    type TxBody: TxBody;
    /// The kind of the client's pairwise key slots (an access point's:
    /// its association identifiers).
    type PairwiseKeyKind;
    /// The kind of the client's group key slots.
    type GroupKeyKind;
    /// An installed pairwise key; removing it gives its slot back.
    type PairwiseKey;
    /// An installed group key; removing it gives its slot back.
    type GroupKey;
    /// The kind of the receive Block Ack slots: one pool the backend's
    /// hardware shares between its roles, so every client of a backend
    /// names the same kind and a slot one role gives back serves the other
    /// (owner decision, Claude Code chat with common, 2026-10-12).
    type BlockAckKind;

    /// The channel the phase runs on.
    fn channel(&self) -> Channel;

    /// The interface's own address, which the install gave it.
    fn address(&self) -> IndividualAddress;

    /// The rate on the phase's channel, when the capabilities send it
    /// there. Only this check mints a [`PhaseRate`].
    fn rate(&self, rate: PhyRate) -> Option<PhaseRate<'_>> {
        if Self::Caps::CAPS.supports_rate(rate, self.channel()) {
            Some(PhaseRate::checked(rate))
        } else {
            None
        }
    }

    /// The permit of the queue the access category of `seat` occupies
    /// ([`seat_access_category`]), once no attempt holds it: an occupied
    /// queue is waited for in its line with `seat`, never refused. The
    /// seat is lent only while the take waits.
    fn queue<'c, 's>(
        &'c self,
        seat: &'s mut Seat<'id, Self::QueueKind>,
    ) -> impl Future<Output = Self::Queue<'c>> + 's
    where
        'id: 'c,
        'c: 's;

    /// The rate, when the capabilities carry an individual frame without
    /// an ACK at it.
    ///
    /// # Errors
    ///
    /// The rate, when they do not.
    fn no_ack<'p>(&self, rate: PhaseRate<'p>) -> Result<SupportedNoAck<'p>, PhaseRate<'p>> {
        if Self::Caps::CAPS.individual_no_ack.contains_rate(rate.get()) {
            Ok(SupportedNoAck(rate))
        } else {
            Err(rate)
        }
    }

    /// A buffer holding `frame`'s header, once one of the pool is free: a
    /// lent pool is waited for in its line with `seat`, never refused. Its
    /// kind fixes its rate and response; the caller writes its body. The
    /// seat is lent only while the take waits, so an A-MPDU takes its
    /// buffers one after another with one seat. Dropped before a
    /// submission, the buffer gives its permit back.
    fn tx_buffer<'c, 's>(
        &'c self,
        frame: TxFrame<'c, Self::Caps>,
        seat: &'s mut Seat<'id, Self::BufferKind>,
    ) -> impl Future<Output = Self::TxBuffer<'c>> + 's
    where
        'id: 'c,
        'c: 's;

    /// Admit `attempt`; its queue and buffer permits stay with the backend
    /// until it takes the attempt's terminal.
    ///
    /// # Errors
    ///
    /// A poisoned backend, which keeps the attempt.
    fn submit<'p>(
        &'p self,
        attempt: PhaseAttempt<'p, 'id, Self>,
    ) -> Result<Work<'p, Self>, Poisoned<Self::Fault>>
    where
        'id: 'p;

    /// Install `key` for frames with `peer` in `slot`.
    fn install_pairwise_key(
        &self,
        slot: Slot<'id, Self::PairwiseKeyKind>,
        peer: IndividualAddress,
        key: Ccmp128Key,
    ) -> Self::PairwiseKey;

    /// Install `key` for group frames under `key_id` in `slot`.
    fn install_group_key(
        &self,
        slot: Slot<'id, Self::GroupKeyKind>,
        key_id: GroupKeyId,
        key: Ccmp128Key,
    ) -> Self::GroupKey;

    /// Remove `key` once no attempt in flight uses it, giving its slot
    /// back.
    /// An attempt that uses it and that the closed transmit gate holds ends
    /// as aborted at once; a published one is waited for, never refused,
    /// so the wait ends also under the gate's guard. Dropped before it
    /// ends, the removal leaves the key installed, and its slot is lost
    /// until the next install.
    fn remove_pairwise_key(
        &self,
        key: Self::PairwiseKey,
    ) -> impl Future<Output = Slot<'id, Self::PairwiseKeyKind>> + '_;

    /// Remove `key` once no attempt in flight uses it, giving its slot
    /// back.
    /// An attempt that uses it and that the closed transmit gate holds ends
    /// as aborted at once; a published one is waited for, never refused,
    /// so the wait ends also under the gate's guard. Dropped before it
    /// ends, the removal leaves the key installed, and its slot is lost
    /// until the next install.
    fn remove_group_key(
        &self,
        key: Self::GroupKey,
    ) -> impl Future<Output = Slot<'id, Self::GroupKeyKind>> + '_;

    /// Start acknowledging A-MPDUs of `agreement` in `slot`. The client
    /// keeps one agreement per peer and TID: one that replaces an earlier
    /// agreement of the pair gives that agreement's slot back.
    #[must_use = "a replaced agreement's slot is capacity; dropping it loses it until the next install"]
    fn add_block_ack(
        &self,
        slot: Slot<'id, Self::BlockAckKind>,
        agreement: SupportedBlockAck<Self::Caps>,
    ) -> Option<Slot<'id, Self::BlockAckKind>>;

    /// End the agreement with `peer` on `tid`, giving its slot back; `None`
    /// when the client keeps none for the pair.
    #[must_use = "an ended agreement's slot is capacity; dropping it loses it until the next install"]
    fn remove_block_ack(
        &self,
        peer: IndividualAddress,
        tid: Tid,
    ) -> Option<Slot<'id, Self::BlockAckKind>>;
}

/// The client of a station interface.
pub trait StationClient<'id>: LowerMacClient<'id> {
    /// Set what the station receives, once none of its frames is in
    /// flight: a frame the closed transmit gate holds ends as aborted at
    /// once, a published one is waited for, never refused.
    fn configure(&self, receive: StationReceive<Self::Caps>) -> impl Future<Output = ()> + '_;

    /// The BSS color of the station's HE BSS.
    fn set_bss_color(&self, color: BssColor);
}

/// The client of an access-point interface, whose BSSID is its address.
pub trait AccessPointClient<'id>: LowerMacClient<'id> {
    /// Set the access point's receive rules, once none of its frames is in
    /// flight: a frame the closed transmit gate holds ends as aborted at
    /// once, a published one is waited for.
    fn configure(
        &self,
        receive: SupportedFilter<Self::Caps, AccessPoint>,
    ) -> impl Future<Output = ()> + '_;
}

/// The one consumer of a running lower-MAC phase's events: received frames,
/// the completions of admitted work with their bodies, and extension
/// events, read through the portable view.
pub trait LowerMacEvents: RadioPortEvents {
    /// The memory of one received MPDU.
    type RxBuffer: crate::RxBuffer;
    /// The bodies a completion carries, each with its subframe index.
    type TxBodies: IntoIterator;

    /// The portable view of an owned event.
    fn view(event: &Self::Event) -> crate::LowerMacEvent<'_>;

    /// Take a received frame out of its event, with its metadata.
    ///
    /// # Errors
    ///
    /// The event, when it is another kind.
    fn into_received(event: Self::Event) -> Result<(Self::RxBuffer, crate::RxMeta), Self::Event>;

    /// Take a completion out of its event, with the bodies of its work;
    /// its [`TxCompletion::id`](crate::TxCompletion::id) is the
    /// [`Work::id`] of the work it ends, whose permits the backend gave
    /// back before the event was handed out.
    ///
    /// # Errors
    ///
    /// The event, when it is another kind.
    fn into_completed(
        event: Self::Event,
    ) -> Result<(crate::TxCompletion, Self::TxBodies), Self::Event>;

    /// The TBTT an event announces, of a client's
    /// [`BeaconTimingClient`] schedule; `None` for another kind, and on a
    /// backend without beacon timing.
    fn tbtt(event: &Self::Event) -> Option<crate::TbttEvent> {
        let _ = event;
        None
    }
}

mod extensions;

pub use extensions::{
    AmpduClient, AmpduRate, BeaconTimingClient, MonitorControl, MpduStartSpacing, PhaseAmpdu,
    PhaseAmpduBuffer, TbttPlan,
};

#[cfg(test)]
mod tests;
