//! The lower-MAC port on the shared owner shape of `oer-radio-port`
//! ([`Owner`](oer_radio_port::Owner), [`PortBody`](oer_radio_port::PortBody)).
//!
//! Every refusal of [`Ieee80211LowerMacPort`](crate::Ieee80211LowerMacPort)
//! that a caller's own state explains is a type here
//! ([ownership and refusals]); what the world controls (the air, time, the
//! hardware) stays a typed outcome. The protocol ports still implement
//! `Ieee80211LowerMacPort`; a backend on this shape implements
//! [`LowerMacBackend`].
//!
//! | Refusal of the port trait | Form here |
//! | --- | --- |
//! | lifecycle refusals, `SubmitError::Disabled`, `SettingError::PortEnabled` | the owner's state: the install gives the port its channel and addresses, [`LowerMacSetup`] changes them only while disabled, clients exist only in a running phase |
//! | `SettingError::UnsupportedChannel` | [`SupportedChannel`] |
//! | `SettingError::UnknownVif`, `SettingError::UnsupportedRole`, `SubmitError::UnknownVif` | a client per configured interface; an attempt names no interface |
//! | the receive rules a role may request | [`SupportedFilter`] |
//! | `SubmitError::UnsupportedRate` | [`PhaseRate`], checked against the phase's channel |
//! | `SubmitError::Unsupported` (backoff, power, coexistence level) | [`SupportedBackoff`], [`SupportedPower`], [`SupportedCoex`] |
//! | `SubmitError::Busy` (the queue holds an attempt) | the queue's grant ([`LowerMacClient::queue`], awaited in the pool's order, a [`QueueGrant`] of the backend's own type): a submission moves it into the backend, which gives the queue back when it takes the attempt's terminal |
//! | `SubmitError::DuplicateId` | the backend mints the [`Work`] of each submission |
//! | `SubmitError::InvalidLength`, a buffer request past `max_mpdu_length`, no free buffer | [`MpduLength`], and a buffer ([`LowerMacClient::tx_buffer`], awaited, never refused) the attempt holds until its terminal event |
//! | `SettingError::NoKeySlot`, `SettingError::UnknownKey`, `SubmitError::UnknownKey` | a key slot ([`Slot`] of the client's own kind) that the install's resources give once, which the caller counts itself; an installed key token the attempt borrows |
//! | `SettingError::InvalidKey` | [`Ccmp128Key`] |
//! | `SettingError::NoBlockAckSlot`, `SettingError::InvalidBlockAck` | a Block Ack [`Slot`] of the client's own kind and [`SupportedBlockAck`]; ending an agreement takes its token |
//! | `SettingError::Busy` (closing the transmit gate) | [`LowerMacControl::close_gate`], awaited, which yields the gate's guard |
//! | `SettingError::Unsupported` (an EDCA record outside the limits, a BSS color above 63) | [`SupportedEdca`], [`BssColor`] |
//!
//! These stay the world's outcomes: [`Poisoned`],
//! [`EventsLost`](oer_radio_port::EventsLost) (only unsolicited
//! reception), a clock that could not be read, a TSF that failed its timing
//! check, and a cancel that finds the work already ended
//! ([`CancelError::NotRunning`](oer_radio_port::CancelError::NotRunning)).
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
//! queue and buffer permits, which the backend keeps, and each role's key
//! and Block Ack slots, which the caller counts itself.
//! An installed key or agreement holds its slot and outlives the phases
//! between; its kind is its client's, so a station's key is not an access
//! point's. The queue and buffer permits stay in the backend's held part of
//! the install (such as `oer-ieee80211-runtime`'s `TxPermits`): a client
//! awaits them as the backend's own guards ([`LowerMacClient::Queue`],
//! [`LowerMacClient::TxBuffer`]) that give
//! them back when dropped before a submission; a submission moves them in,
//! and the backend gives them back when it takes the attempt's terminal,
//! before it hands the event out. No caller holds a submitted attempt's
//! permits, and a dropped handle or event loses none. The
//! backend mints its own tokens (a key, an agreement, a buffer), each
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
use oer_radio_port::{Poisoned, PortBackend, RadioPortClient, RadioPortEvents, Slot, Work};

use crate::{
    Backoff, Channel, CoexPriority, LowerMacCapabilities, MacAddress, PhyRate, ReceiveFilter,
    RxBeaconPriority, TxBody, TxPower, TxResponse, VifRole,
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

/// The receive rules an interface of a role may request of `K`.
#[derive(Debug)]
pub struct SupportedFilter<K> {
    role: VifRole,
    filter: ReceiveFilter,
    brand: PhantomData<fn() -> K>,
}

impl<K: Ieee80211Capabilities> SupportedFilter<K> {
    /// `filter` for an interface of `role`, when `K` admits every rule of it
    /// for that role.
    pub const fn new(role: VifRole, filter: ReceiveFilter) -> Option<Self> {
        if K::CAPS.receive_filters(role).contains(filter) {
            Some(Self {
                role,
                filter,
                brand: PhantomData,
            })
        } else {
            None
        }
    }

    /// The role and its rules.
    pub const fn get(&self) -> (VifRole, ReceiveFilter) {
        (self.role, self.filter)
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
    peer: MacAddress,
    tid: u8,
    start_sequence: SequenceNumber,
    window: u16,
    brand: PhantomData<fn() -> K>,
}

impl<K: Ieee80211Capabilities> SupportedBlockAck<K> {
    /// An agreement with `peer` on `tid`, when `K` acknowledges its TID and
    /// window.
    pub const fn new(
        peer: MacAddress,
        tid: u8,
        start_sequence: SequenceNumber,
        window: u16,
    ) -> Option<Self> {
        if tid <= K::CAPS.rx_block_ack_max_tid
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
    pub const fn get(&self) -> (MacAddress, u8, SequenceNumber, u16) {
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

impl PhaseRate<'_> {
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

/// One attempt of a running phase, every value of it checked: the queue it
/// occupies, its MPDU in a buffer bound to a buffer permit, its body, its
/// rate on the phase's channel, its protection, key, power, backoff and
/// coexistence level.
pub struct PhaseAttempt<'p, 'id: 'p, C: LowerMacClient<'id> + 'p> {
    pub queue: C::Queue<'p>,
    pub frame: C::TxBuffer<'p>,
    pub body: Option<C::TxBody>,
    pub response: TxResponse,
    pub rate: PhaseRate<'p>,
    pub protection: crate::Protection,
    /// The installed key that protects the frame, or none.
    pub key: Option<&'p C::Key>,
    pub power: SupportedPower<C::Caps>,
    pub backoff: SupportedBackoff<C::Caps>,
    pub coex: SupportedCoex<C::Caps>,
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

    /// Set the address of the interface of `role`, in place of the address
    /// the install or an earlier call gave it: every interface the backend
    /// serves has one, and its client exists in every phase.
    fn configure(&mut self, role: VifRole, address: MacAddress);
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
/// address the install or [`LowerMacSetup::configure`] gave it; its type is
/// its role, so a backend gives a role's client only the operations
/// that role has. Its key and Block Ack slots are of its own kinds, so a
/// key or agreement of one role's client is not another role's, by type.
pub trait LowerMacClient<'id>:
    RadioPortClient<Token: oer_radio_port::RadioWork<Id = crate::TxId>> + Sized
{
    /// The backend's capabilities marker.
    type Caps: Ieee80211Capabilities;
    /// The grant of one queue, which [`Self::queue`] awaits.
    type Queue<'c>: QueueGrant
    where
        Self: 'c,
        'id: 'c;
    /// One MPDU's memory, bound to a buffer permit it holds.
    type TxBuffer<'c>
    where
        Self: 'c,
        'id: 'c;
    /// The owner of an MPDU's body.
    type TxBody: TxBody;
    /// The kind of the client's key slots.
    type KeyKind;
    /// An installed key; removing it gives its slot back.
    type Key;
    /// The kind of the client's receive Block Ack slots.
    type BlockAckKind;
    /// A receive Block Ack agreement; ending it gives its slot back.
    type BlockAck;

    /// The channel the phase runs on.
    fn channel(&self) -> Channel;

    /// The rate on the phase's channel, when the capabilities send it
    /// there. Only this check mints a [`PhaseRate`].
    fn rate(&self, rate: PhyRate) -> Option<PhaseRate<'_>> {
        if Self::Caps::CAPS.supports_rate(rate, self.channel()) {
            Some(PhaseRate::checked(rate))
        } else {
            None
        }
    }

    /// The permit of the queue `access_category` occupies, once no attempt
    /// holds it: an occupied queue is waited for, never refused.
    fn queue<'c>(
        &'c self,
        access_category: WmmAccessCategory,
    ) -> impl Future<Output = Self::Queue<'c>> + 'c
    where
        'id: 'c;

    /// A buffer for an MPDU of `len`, once one of the pool is free: a lent
    /// pool is waited for, never refused. Dropped before a submission, it
    /// gives its permit back.
    fn tx_buffer<'c>(
        &'c self,
        len: MpduLength<Self::Caps>,
    ) -> impl Future<Output = Self::TxBuffer<'c>> + 'c
    where
        'id: 'c;

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

    /// Set the interface's BSS and receive rules.
    fn configure(&self, bssid: Option<MacAddress>, receive: SupportedFilter<Self::Caps>);

    /// Install `key` for frames with `peer` (pairwise) or under `key_id`
    /// (group) in `slot`.
    fn install_key(
        &self,
        slot: Slot<'id, Self::KeyKind>,
        scope: crate::KeyScope,
        key: Ccmp128Key,
    ) -> Self::Key;

    /// Remove `key`, giving its slot back.
    fn remove_key(&self, key: Self::Key) -> Slot<'id, Self::KeyKind>;

    /// Start acknowledging A-MPDUs of `agreement` in `slot`.
    fn add_block_ack(
        &self,
        slot: Slot<'id, Self::BlockAckKind>,
        agreement: SupportedBlockAck<Self::Caps>,
    ) -> Self::BlockAck;

    /// End `agreement`, giving its slot back.
    fn remove_block_ack(&self, agreement: Self::BlockAck) -> Slot<'id, Self::BlockAckKind>;

    /// The BSS color of the interface's HE BSS.
    fn set_bss_color(&self, color: BssColor);
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
}
