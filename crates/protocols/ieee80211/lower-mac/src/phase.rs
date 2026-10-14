//! A running lower-MAC phase: one object under `&mut`
//! ([`PortPhase`]), whose event stream and
//! operations a body's one loop drives ([owner decision on #563]).
//!
//! # Tokens
//!
//! Every token carries the install's brand `'id`, is minted once by the
//! install's resources and travels by value: an operation takes it, and
//! the event that ends the operation gives it back. So a portable core
//! holds the tokens of what it may do, and what it holds is what it may
//! do: a transmission it has no ticket for, a key it has no slot for or a
//! gate it has not closed does not compile.
//!
//! | Token | Taken by | Given back by |
//! | --- | --- | --- |
//! | a transmit ticket ([`PhaseTypes::TicketKind`]) | [`InterfacePhase::submit`] | [`LowerMacEvent::TxCompleted`] |
//! | a transmit buffer of a class ([`TxBufferClass`]), its memory ([`TxBufferMemory`]) | [`InterfacePhase::submit`], together with its [`ClassFrame`] | [`LowerMacEvent::TxCompleted`], every buffer of the attempt |
//! | a key slot of a role's kind | [`InterfacePhase::install_pairwise_key`], [`InterfacePhase::install_group_key`] (the installed key holds it) | [`LowerMacEvent::KeyRemoved`], after a removal |
//! | a Block Ack slot | [`InterfacePhase::add_block_ack`] (the agreement holds it) | [`InterfacePhase::remove_block_ack`] |
//! | the open gate ([`GateOpen`]) | [`LowerMacPhase::close_gate`] | [`LowerMacPhase::open_gate`], which takes the closed gate ([`GateClosed`]) that [`LowerMacEvent::GateClosed`] gave |
//! | a received frame's buffer | [`LowerMacEvent::Received`] | [`LowerMacPhase::recycle`] |
//!
//! A ticket and a buffer are different tokens ([owner decisions on #563]).
//! A ticket is a place in flight and a place for its terminal; their
//! number is bounded by the hardware. A buffer is memory of one class,
//! sized by the composition, from a pool the body holds: the caller
//! encodes its frame straight into it. A submission
//! takes one ticket and the buffers of its frames (an A-MPDU: one ticket
//! and a buffer per subframe), and its terminal gives all of them back. So
//! a submission never waits: an empty pool is the caller's own capacity,
//! and what the hardware cannot take at once (an occupied queue) the
//! backend keeps until it can. Queue permits are the backend's own and
//! never reach a caller. How many tickets and buffers each role holds,
//! with a reserve for its management frames, is the composition's
//! choice: the tokens are values it hands out.
//!
//! A setting that takes no token (an interface's receive configuration,
//! EDCA, the BSS color, a TBTT plan, monitor reception) takes effect
//! before the next submission; the latest value of a kind wins.
//!
//! An operation that waits for the air or for the attempts in flight (a
//! gate's closing, an interface's receive configuration, a key's removal)
//! returns at once, and its end arrives as an event of the stream. No
//! operation of the phase awaits.
//!
//! # Order
//!
//! The phase's operations reach the hardware in the order the caller made
//! them, through one queue to the driver: an attempt that names a key is
//! published after that key's install, and a key's removal ends after
//! every attempt that names it. A backend's conformance tests check both.
//!
//! The queue is strictly first in, first out: no command replaces another.
//! Its capacity is the tokens' ([owner decision]): one attempt per ticket,
//! and two commands per key slot and the gate, since each token's install
//! or closing and its removal or opening may wait at once —
//! `tickets + 2 × (key slots + 1)`. Block Ack is per-slot desired state;
//! repeated writes coalesce and never enter the command queue.
//!
//! [owner decision]: https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6130970169
//!
//! # Protected frames
//!
//! Preparation derives the Protected bit from [`TxProtection`] and
//! leaves the 8 octets of its CCMP header after its MAC header; its length
//! counts them, not the MIC. At the first transmission of the MPDU the
//! backend writes the CCMP header with a new packet number of the key into
//! the buffer, and the hardware appends the MIC; the plan's transmissions
//! keep it. The completion gives the buffer back as `Sent`
//! ([`TxBufferPool::Sent`]): a retransmission of the same MPDU
//! ([`TxSource::Retransmission`]) keeps its packet number, and only a
//! renewed buffer ([`TxBufferPool::renew`]) is numbered anew. A received
//! frame whose crypto status is
//! [`RxCryptoStatus::DecryptedAndIntegrityVerified`](crate::RxCryptoStatus)
//! keeps its CCMP header, for the receiver's replay check, and loses its
//! MIC ([owner decision on #563](https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6131631789)).
//! [`HardwareServices::PACKET_NUMBERS`](crate::HardwareServices::PACKET_NUMBERS)
//! means the hardware itself assigns them.
//!
//! # Compile-fail tests
//!
//! No caller mints a token: a gate, a ticket or a buffer comes only from
//! the install's resources.
//!
//! ```compile_fail,E0432
//! use oer_ieee80211_lower_mac::GatePlace;
//! ```
//!
//! Nor copies one:
//!
//! ```compile_fail,E0382
//! # use oer_ieee80211_lower_mac::GateOpen;
//! fn twice<'id>(gate: GateOpen<'id>) -> (GateOpen<'id>, GateOpen<'id>) {
//!     (gate, gate)
//! }
//! ```
//!
//! ```compile_fail,E0599
//! # use oer_radio_port::Slot;
//! fn copy<'id, K>(ticket: &Slot<'id, K>) -> Slot<'id, K> {
//!     ticket.clone()
//! }
//! ```
//!
//! Its compiling twin moves the token on, as an operation takes it:
//!
//! ```
//! # use oer_ieee80211_lower_mac::{GateClosed, GateOpen, LowerMacPhase};
//! fn close<'p, 'id: 'p, P: LowerMacPhase<'p, 'id>>(phase: &mut P, gate: GateOpen<'id>) {
//!     phase.close_gate(gate);
//! }
//! fn open<'p, 'id: 'p, P: LowerMacPhase<'p, 'id>>(phase: &mut P, gate: GateClosed<'id>) -> GateOpen<'id> {
//!     phase.open_gate(gate)
//! }
//! ```
//!
//! The open gate does not open a gate, nor the closed one close it:
//!
//! ```compile_fail,E0308
//! # use oer_ieee80211_lower_mac::{GateOpen, LowerMacPhase};
//! fn reopen<'p, 'id: 'p, P: LowerMacPhase<'p, 'id>>(phase: &mut P, gate: GateOpen<'id>) {
//!     let _ = phase.open_gate(gate);
//! }
//! ```
//!
//! A frame a buffer's class does not fit has no [`ClassFrame`], whose
//! fields no caller sets:
//!
//! ```compile_fail,E0451
//! # use oer_ieee80211_lower_mac::{ClassFrame, TxFrame};
//! fn forge<'p, C, K>(frame: TxFrame<'p, K>) -> ClassFrame<'p, C, K> {
//!     ClassFrame { frame, class: core::marker::PhantomData }
//! }
//! ```
//!
//! Its compiling twin checks the frame against the class:
//!
//! ```
//! # use oer_ieee80211_lower_mac::{ClassFrame, Ieee80211Capabilities, TxBufferClass, TxFrame};
//! fn fit<'p, C: TxBufferClass, K: Ieee80211Capabilities>(
//!     frame: TxFrame<'p, K>,
//! ) -> Result<ClassFrame<'p, C, K>, TxFrame<'p, K>> {
//!     ClassFrame::new(frame)
//! }
//! ```
//!
//! An attempt moves its context, ticket and buffer once:
//!
//! ```compile_fail,E0382
//! # use oer_ieee80211_lower_mac::*;
//! fn twice<'p, 'id: 'p, I>(interface: &mut I, attempt: InterfaceAttempt<'_, 'p, 'id, I>)
//! where I: InterfacePhase<'p, 'id> {
//!     let _ = interface.submit(attempt);
//!     let _ = interface.submit(attempt);
//! }
//! ```
//!
//! Its compiling twin moves each operation once, with no `Clone` bound:
//!
//! ```
//! # use oer_ieee80211_lower_mac::*;
//! fn one<'p, 'id: 'p, I>(interface: &mut I, attempt: InterfaceAttempt<'_, 'p, 'id, I>)
//! where I: InterfacePhase<'p, 'id> {
//!     let _ = interface.submit(attempt);
//! }
//! ```
//!
//! A frame checked on one phase cannot be submitted on an unrelated one:
//!
//! ```compile_fail
//! # use oer_ieee80211_lower_mac::*;
//! fn another_phase<'p, 'q, 'id: 'p + 'q, I, J>(
//!     interface: &mut J, attempt: InterfaceAttempt<'_, 'p, 'id, I>,
//! ) where
//!     I: InterfacePhase<'p, 'id>,
//!     J: InterfacePhase<'q, 'id, Caps = I::Caps, Types = I::Types, Role = I::Role>,
//! {
//!     let _ = interface.submit(attempt);
//! }
//! ```
//!
//! # Poisoned
//!
//! A poisoned backend ends the phase: `next_event` returns
//! [`Poisoned`], and the tokens the port holds
//! are not given back. The body ends the station whole and leaves through
//! the poisoned owner; every token ends with the install.
//!
//! [owner decision on #563]: https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6128926225
//! [owner decisions on #563]: https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6129037560

use oer_ieee80211_mac::ccmp::GroupKeyId;
use oer_ieee80211_port::PortPhase;
use oer_radio_port::{Poisoned, Slot};

use crate::{
    Channel, PhyRate, Protection, TbttEvent, TxOutcome, VifRole,
    owner::{
        AccessPoint, BssColor, Ccmp128Key, Ieee80211Capabilities, InterfaceRole, PhaseRate,
        Station, StationReceive, SupportedBlockAck, SupportedCoex, SupportedEdca, SupportedFilter,
        SupportedNoAck, SupportedPower, TxFrame,
    },
};
use oer_ieee80211_mac::address::IndividualAddress;

enum GateKind {}

/// The gate component of a lower-MAC backend's pool set.
///
/// The backend receives its open gate and issuer together when its one
/// [`oer_radio_port::Mint`] is consumed. The unique place remains private.
///
/// ```compile_fail,E0432
/// use oer_ieee80211_lower_mac::GatePlace;
/// ```
///
/// ```
/// use oer_ieee80211_lower_mac::{GateIssuer, GateOpen, GatePool};
/// use oer_radio_port::Mint;
/// fn gate<'id>(mint: Mint<'id, GatePool>) -> (GateOpen<'id>, GateIssuer<'id>) {
///     mint.into_pools()
/// }
/// ```
pub struct GatePool;

impl oer_radio_port::PoolSet for GatePool {
    type Pools<'id> = (GateOpen<'id>, GateIssuer<'id>);

    fn build<'id>(builder: oer_radio_port::PoolBuilder<'id>) -> Self::Pools<'id> {
        GatePlace::mint(builder.into_pools::<oer_radio_port::Slots<GateKind, 1>>())
    }
}

/// The install's unique gate place, held inside exactly one gate state.
#[must_use = "the unique gate place stays in one gate state"]
struct GatePlace<'id> {
    slot: Slot<'id, GateKind>,
}

impl<'id> GatePlace<'id> {
    fn mint([slot]: [Slot<'id, GateKind>; 1]) -> (GateOpen<'id>, GateIssuer<'id>) {
        (
            GateOpen {
                place: Self { slot },
            },
            GateIssuer {
                brand: core::marker::PhantomData,
            },
        )
    }
}

impl core::fmt::Debug for GatePlace<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("GatePlace")
            .field("slot", &self.slot.index())
            .finish()
    }
}

/// The open transmit gate. Closing transfers its unique place to the backend.
#[derive(Debug)]
#[must_use = "close_gate consumes the open gate and returns its closed evidence"]
pub struct GateOpen<'id> {
    place: GatePlace<'id>,
}

/// Evidence that the backend publishes no attempt; it does not prove quiet air.
/// Only the retained backend issuer constructs this state after its barrier.
///
/// ```compile_fail,E0616
/// # use oer_ieee80211_lower_mac::{GateClosed, GateOpen};
/// fn forge<'id>(gate: GateOpen<'id>) -> GateClosed<'id> {
///     GateClosed { place: gate.place }
/// }
/// ```
#[derive(Debug)]
#[must_use = "open_gate consumes the closed evidence and returns the open gate"]
pub struct GateClosed<'id> {
    place: GatePlace<'id>,
}

/// The backend's authority to convert its one gate place between states.
///
/// One issuer is retained by the backend per install and never reaches the
/// body. Closing is allowed only after the current publication's result and
/// the withdrawal of queued attempts. Opening consumes that very evidence.
///
/// ```compile_fail,E0382
/// # use oer_ieee80211_lower_mac::{GateClosed, GateIssuer, GateOpen};
/// fn both<'id>(issuer: &mut GateIssuer<'id>, gate: GateOpen<'id>)
///     -> (GateOpen<'id>, GateClosed<'id>)
/// {
///     let closed = issuer.close(gate);
///     (gate, closed)
/// }
/// ```
#[must_use = "the backend retains the issuer for the whole install"]
pub struct GateIssuer<'id> {
    brand: core::marker::PhantomData<fn(&'id ()) -> &'id ()>,
}

impl<'id> GateIssuer<'id> {
    /// Attest the completed hardware barrier, retaining the same place.
    pub fn close(&mut self, gate: GateOpen<'id>) -> GateClosed<'id> {
        GateClosed { place: gate.place }
    }

    /// Consume the closed evidence to open the same gate.
    pub fn open(&mut self, gate: GateClosed<'id>) -> GateOpen<'id> {
        GateOpen { place: gate.place }
    }
}

/// An event of a running lower-MAC phase, in the order the events
/// occurred; every token an operation took comes back in one.
pub enum LowerMacEvent<'id, P: PhaseTypes<'id>> {
    /// One received MPDU and its metadata.
    Received {
        /// The frame, from its header to the end of its body, without the
        /// FCS.
        frame: P::RxBuffer,
        /// What the hardware observed of it.
        meta: P::RxMeta,
    },
    /// The terminal preserves the submitted shape and all of its owners.
    TxCompleted(TxCompleted<'id, P>),
    /// A TBTT of an interface's schedule.
    Tbtt(TbttEvent),
    /// The paired TSF reading a
    /// [`BeaconTimingPhase::sample_tsf`](crate::BeaconTimingPhase::sample_tsf)
    /// asked for.
    TsfSampled(crate::TsfSample),
    /// The transmit gate closed: no attempt is published.
    GateClosed(GateClosed<'id>),
    /// An interface's receive configuration took effect.
    Configured(VifRole),
    /// A removed key's slot, of its role and kind, once no attempt in
    /// flight uses the key.
    KeyRemoved(Removed<'id, P>),
    /// A station GTK replacement completed in the same slot. RX before this
    /// terminal belongs to the previous installation; RX after it belongs to
    /// this one. Replay state is owned above the port.
    GroupKeyReplaced(<P::StationKeys as KeyTypes<'id>>::GroupKey),
}

/// An accepted attempt's terminal, with the shape of its submission.
///
/// The backend must return exactly the original context, ticket and MPDU
/// for a single attempt, or the original subframes for an aggregate, once.
/// It moves the opaque context unchanged, including on withdrawal and during
/// disable. Classes may share an AC queue, but may never exchange owners.
/// The Rust shape guarantees cardinality for singles; identity remains a
/// backend obligation, covered by the common backend conformance scenarios.
/// A consumer detecting a context/resource mismatch must fail-stop just as
/// for a poisoned backend. It must not count and continue with swapped owners.
///
/// [Owner decision](https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6155986511).
pub enum TxCompleted<'id, P: PhaseTypes<'id>> {
    /// Exactly one submitted MPDU, never an empty or multi-buffer list.
    Single {
        /// Original opaque context.
        context: P::Context,
        /// Original ticket.
        ticket: Slot<'id, P::TicketKind>,
        /// Original MPDU, fresh if unpublished or retaining its sent image.
        buffer: P::AnyTxBuffer,
        /// A fresh MPDU's external body; sent MPDUs retain theirs internally.
        body: Option<P::TxBody>,
        /// Terminal outcome.
        completion: TxOutcome,
    },
    /// The aggregate's original subframes, in submitted order.
    Aggregate {
        /// Original opaque context.
        context: P::Context,
        /// Original ticket.
        ticket: Slot<'id, P::TicketKind>,
        /// Original subframes.
        buffers: P::TxBuffers,
        /// Fresh subframes' bodies by subframe index; sent MPDUs retain theirs.
        bodies: P::TxBodies,
        /// Terminal outcome, including any Block Ack report.
        completion: TxOutcome,
    },
}

/// The keys of one role and their slots, which a phase's types name: they
/// depend on the install alone, so a caller holds them across the phase's
/// lends ([owner decision on #563](https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6131533566)).
pub trait KeyTypes<'id> {
    /// The role's affine pairwise key slots (an access point's: its
    /// association identifiers).
    type PairwiseSlot: 'id;
    /// The role's affine group key slots, retained through removal.
    type GroupSlot: 'id;
    /// An installed pairwise key, which holds its slot.
    type PairwiseKey: 'id;
    /// An installed group key, which holds its slot.
    type GroupKey: 'id;
    /// Backend identity of one installation, distinct from its reusable slot.
    type Identity;
    /// Stable identity of a group slot within this installation, retained
    /// across replacement. It correlates the consumed key with its terminal.
    type GroupSlotIdentity: Copy + Eq;
    /// Retain the installed pairwise identity while preparing an operation.
    fn pairwise_identity(key: &Self::PairwiseKey) -> Self::Identity;
    /// Retain the installed group identity (transmit-capable on an AP only).
    fn group_identity(key: &Self::GroupKey) -> Self::Identity;
    /// The group key's slot identity, independent of its key installation.
    fn group_slot_identity(key: &Self::GroupKey) -> Self::GroupSlotIdentity;
}

/// A removed key's slot, of its kind.
pub enum RemovedKey<'id, K: KeyTypes<'id>> {
    /// A pairwise key's slot.
    Pairwise(K::PairwiseSlot),
    /// A group key's slot.
    Group(K::GroupSlot),
}

/// A removed key's slot, of its role.
pub enum Removed<'id, P: PhaseTypes<'id>> {
    /// The station's.
    Station(RemovedKey<'id, P::StationKeys>),
    /// The access point's.
    AccessPoint(RemovedKey<'id, P::AccessPointKeys>),
}

/// The keys of a role in the phase types `P`.
pub trait RoleKeys<'id, P: PhaseTypes<'id>>: InterfaceRole {
    /// The role's keys.
    type Keys: KeyTypes<'id, Identity = P::KeyIdentity>;
    /// Keys this role may use for TX. Station has only a pairwise key.
    type TxKey<'k>
    where
        'id: 'k;
    /// Retain an opaque backend identity, without inspecting key material.
    fn tx_identity<'k>(key: &Self::TxKey<'k>) -> P::KeyIdentity
    where
        'id: 'k;

    /// The removed key's slot, as the event names it.
    fn removed(key: RemovedKey<'id, Self::Keys>) -> Removed<'id, P>;
}

impl<'id, P: PhaseTypes<'id>> RoleKeys<'id, P> for Station {
    type Keys = P::StationKeys;
    type TxKey<'k>
        = &'k <P::StationKeys as KeyTypes<'id>>::PairwiseKey
    where
        'id: 'k;
    fn tx_identity<'k>(key: &Self::TxKey<'k>) -> P::KeyIdentity
    where
        'id: 'k,
    {
        P::StationKeys::pairwise_identity(key)
    }

    fn removed(key: RemovedKey<'id, Self::Keys>) -> Removed<'id, P> {
        Removed::Station(key)
    }
}

impl<'id, P: PhaseTypes<'id>> RoleKeys<'id, P> for AccessPoint {
    type Keys = P::AccessPointKeys;
    type TxKey<'k>
        = AttemptKey<
        'k,
        <P::AccessPointKeys as KeyTypes<'id>>::PairwiseKey,
        <P::AccessPointKeys as KeyTypes<'id>>::GroupKey,
    >
    where
        'id: 'k;
    fn tx_identity<'k>(key: &Self::TxKey<'k>) -> P::KeyIdentity
    where
        'id: 'k,
    {
        match key {
            AttemptKey::Pairwise(key) => P::AccessPointKeys::pairwise_identity(key),
            AttemptKey::Group(key) => P::AccessPointKeys::group_identity(key),
        }
    }

    fn removed(key: RemovedKey<'id, Self::Keys>) -> Removed<'id, P> {
        Removed::AccessPoint(key)
    }
}

/// The keys of the interface `I`.
pub type InterfaceKeys<'p, 'id, I> = <<I as InterfacePhase<'p, 'id>>::Role as RoleKeys<
    'id,
    <I as InterfacePhase<'p, 'id>>::Types,
>>::Keys;

/// The types of one backend's phase that its events carry.
pub trait PhaseTypes<'id>: Sized {
    /// The backend fault shared by the event stream and every interface.
    type Fault: Copy + core::fmt::Debug;
    /// The composition's operation context. The backend owns and returns
    /// it unchanged, without interpreting or cloning it. The install bound
    /// retains branded values while excluding references to machines local
    /// to the generic `PortBody::run`. Portable contexts need not be `Send`;
    /// a backend crossing execution contexts adds that bound itself.
    ///
    /// [Owner decision](https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6155221933).
    type Context: 'id;
    /// A prepared MPDU retaining its phase, install and intended interface role.
    type Prepared<'k, 'p, R, K: Ieee80211Capabilities>: PreparedTx<'id, Types = Self>;
    /// Opaque installed-key identity, retained by prepared and sent MPDUs.
    type KeyIdentity;
    /// Aggregate storage with a compile-time empty/nonempty state.
    type Aggregate<'k, 'p, R, K, S>: crate::PhaseAggregate<'id, Types = Self>
    where
        R: RoleKeys<'id, Self>,
        K: Ieee80211Capabilities,
        S: crate::AggregateState,
        'id: 'k;
    /// An external MPDU body. A `Sent` MPDU retains its body internally
    /// for a retry; renewing that MPDU returns the body with its memory.
    type TxBody: crate::TxBody;
    /// The kind of the transmit tickets.
    type TicketKind;
    /// A transmit buffer of any class, as a completion gives it back.
    type AnyTxBuffer;
    /// The buffers a completion gives back.
    type TxBuffers: IntoIterator<Item = Self::AnyTxBuffer>;
    /// The memory of one received MPDU, which goes back through
    /// [`LowerMacPhase::recycle`].
    type RxBuffer: crate::RxBuffer;
    /// The metadata of one received MPDU: the portable fields and what the
    /// backend's type guarantees beyond them.
    type RxMeta: crate::RxMetadata;
    /// External bodies of fresh returned MPDUs, with their subframe index.
    /// A `Sent` MPDU retains its own body until retry or renewal.
    type TxBodies: IntoIterator<Item = (u8, Self::TxBody)>;
    /// The station's keys and their slots.
    type StationKeys: KeyTypes<'id, Identity = Self::KeyIdentity>;
    /// The access point's keys and their slots.
    type AccessPointKeys: KeyTypes<'id, Identity = Self::KeyIdentity>;
    /// The kind of the receive Block Ack slots: one pool the backend's
    /// hardware shares between its roles (owner decision, Claude Code chat
    /// with common, 2026-10-12).
    type BlockAckKind;
    /// A running receive Block Ack agreement, which holds its slot.
    type BlockAck;
}

/// A running lower-MAC phase of the install `'id`, lent for `'p`.
pub trait LowerMacPhase<'p, 'id: 'p>:
    PortPhase<
        Event = LowerMacEvent<'id, Self::Types>,
        Fault = <Self::Types as PhaseTypes<'id>>::Fault,
    > + Sized
{
    /// The backend's capabilities marker.
    type Caps: Ieee80211Capabilities;
    /// The types the phase's events carry.
    type Types: PhaseTypes<'id>;
    /// The station interface of the phase.
    type Station<'v>: StationPhase<'p, 'id, Caps = Self::Caps, Types = Self::Types>
    where
        Self: 'v;
    /// The access-point interface of the phase.
    type AccessPoint<'v>: AccessPointPhase<'p, 'id, Caps = Self::Caps, Types = Self::Types>
    where
        Self: 'v;

    /// The channel the phase runs on.
    fn channel(&self) -> Channel;

    /// The rate on the phase's channel, when the capabilities send it
    /// there. Only this check mints a [`PhaseRate`].
    fn rate(&self, rate: PhyRate) -> Option<PhaseRate<'p>> {
        if Self::Caps::CAPS.supports_rate(rate, self.channel()) {
            Some(PhaseRate::checked(rate))
        } else {
            None
        }
    }

    /// The rate, when the capabilities carry an individual frame without
    /// an ACK at it.
    ///
    /// # Errors
    ///
    /// The rate, when they do not.
    fn no_ack(&self, rate: PhaseRate<'p>) -> Result<SupportedNoAck<'p>, PhaseRate<'p>> {
        if Self::Caps::CAPS.individual_no_ack.contains_rate(rate.get()) {
            Ok(SupportedNoAck(rate))
        } else {
            Err(rate)
        }
    }

    /// Return an owned receive buffer to the backend's receive pool. Keeping
    /// it occupies that capacity; dropping it does not recycle it. The host
    /// model poisons the installation when an unrecycled owner is dropped.
    fn recycle(&mut self, buffer: <Self::Types as PhaseTypes<'id>>::RxBuffer);

    /// Close publication in every queue. A current publication is waited for,
    /// at most the backend's PUBLICATION_BOUND, before GateClosed is emitted.
    /// ACK returns Ended(Delivered); unknown timeout returns Indeterminate.
    /// When a retry would be needed, withdraw the attempt with its Sent MPDU
    /// instead of starting that retry. Queued fresh MPDUs return Fresh.
    /// Terminals retain each AC's order and precede the barrier confirmation.
    fn close_gate(&mut self, gate: GateOpen<'id>);

    /// Open the closed gate, a register write that cannot fail.
    fn open_gate(&mut self, gate: GateClosed<'id>) -> GateOpen<'id>;

    /// The EDCA parameters every queue's attempts contend with.
    fn set_edca(&mut self, parameters: SupportedEdca<Self::Caps>);

    /// The station interface.
    fn station(&mut self) -> Self::Station<'_>;

    /// The access-point interface.
    fn access_point(&mut self) -> Self::AccessPoint<'_>;
}

/// One attempt awaiting preparation: its ticket, its MPDU in a
/// buffer, its context, body, protection, key, power and coexistence level.
/// The frame's kind sets its response and
/// its retry plan ([`RetryPlan`]) or one rate; its access category picks
/// its queue, whose contention the backend draws.
pub struct Attempt<'id, T, M, K, X> {
    /// The original operation's context, returned with its terminal.
    pub context: X,
    /// The attempt's place in flight.
    pub ticket: Slot<'id, T>,
    /// Fresh memory with its checked description and body, or the original
    /// returned MPDU together with this attempt's phase-bound rates.
    pub source: M,
    /// The access category the attempt contends as.
    pub access_category: oer_ieee80211_mac::qos::WmmAccessCategory,
    /// The control exchange that protects its first transmission; a
    /// retry plan's stages name their own.
    pub protection: Protection,
    /// Its transmit power.
    pub power: SupportedPower<K>,
    /// Its coexistence level.
    pub coex: SupportedCoex<K>,
}

/// The installed key an attempt names.
pub enum AttemptKey<'k, P, G> {
    /// A pairwise key.
    Pairwise(&'k P),
    /// A group key.
    Group(&'k G),
}

/// The only source of the MPDU's Protected bit and installed TX key.
/// A station's `Key` is a borrowed pairwise key; it cannot supply a group key.
///
/// A station's receive group key is not a transmit key:
///
/// ```compile_fail,E0308
/// use oer_ieee80211_lower_mac::*;
/// fn group<'k, 'id: 'k, T: PhaseTypes<'id>>(
///     key: &'k <T::StationKeys as KeyTypes<'id>>::GroupKey,
/// ) -> TxProtection<<Station as RoleKeys<'id, T>>::TxKey<'k>> {
///     TxProtection::Ccmp(key)
/// }
/// ```
pub enum TxProtection<Key> {
    /// No encryption; the backend clears Protected even in an encoded input header.
    Open,
    /// CCMP under this installation; the backend sets Protected.
    Ccmp(Key),
}

impl<Key> TxProtection<Key> {
    /// Derive the MAC/CCMP layout from this operation's key selection.
    pub(crate) const fn layout(&self) -> crate::MpduProtection {
        match self {
            Self::Open => crate::MpduProtection::Open,
            Self::Ccmp(_) => crate::MpduProtection::Ccmp,
        }
    }
}

/// The class-specific input to preparation; refusal returns this exact type.
pub type PoolSource<'k, 'p, 'id, T, C, K, R> = TxSource<
    'p,
    <T as TxBufferPool<'id, C>>::Buffer,
    <T as TxBufferPool<'id, C>>::Sent,
    ClassFrame<'p, C, K>,
    <T as PhaseTypes<'id>>::TxBody,
    <R as RoleKeys<'id, T>>::TxKey<'k>,
>;

/// Class-specific source whose portable layout has already been checked.
pub type CheckedPoolSource<'k, 'p, 'id, T, C, K, R> = crate::CheckedTxSource<
    'p,
    <T as TxBufferPool<'id, C>>::Buffer,
    <T as TxBufferPool<'id, C>>::Sent,
    ClassFrame<'p, C, K>,
    <T as PhaseTypes<'id>>::TxBody,
    <R as RoleKeys<'id, T>>::TxKey<'k>,
>;

/// Preparation returns a phase-bound MPDU or its unchanged class-specific source.
pub type PoolPreparation<'k, 'p, 'id, T, C, K, R> = Result<
    <T as PhaseTypes<'id>>::Prepared<'k, 'p, R, K>,
    PreparationRefused<PoolSource<'k, 'p, 'id, T, C, K, R>>,
>;

/// A class's fresh allocation or its retained published MPDU.
pub type PoolMpdu<'id, T, C> =
    ReturnedMpdu<<T as TxBufferPool<'id, C>>::Buffer, <T as TxBufferPool<'id, C>>::Sent>;

/// A backend's closed preparation proof. Cancellation recovers its owners
/// without creating a TX terminal; submission consumes the proof itself.
pub trait PreparedTx<'id> {
    /// The backend's resource family.
    type Types: PhaseTypes<'id>;
    /// Recover the buffer and any external body. A `Sent` MPDU keeps its
    /// body nested until renewal.
    fn into_parts(
        self,
    ) -> (
        <Self::Types as PhaseTypes<'id>>::AnyTxBuffer,
        Option<<Self::Types as PhaseTypes<'id>>::TxBody>,
    );
}

/// The attempt of the interface `I`, after its class pool prepared the MPDU.
/// It remains bound to this phase, install, capabilities and role.
pub type InterfaceAttempt<'k, 'p, 'id, I> = Attempt<
    'id,
    <<I as InterfacePhase<'p, 'id>>::Types as PhaseTypes<'id>>::TicketKind,
    <<I as InterfacePhase<'p, 'id>>::Types as PhaseTypes<'id>>::Prepared<
        'k,
        'p,
        <I as InterfacePhase<'p, 'id>>::Role,
        <I as InterfacePhase<'p, 'id>>::Caps,
    >,
    <I as InterfacePhase<'p, 'id>>::Caps,
    <<I as InterfacePhase<'p, 'id>>::Types as PhaseTypes<'id>>::Context,
>;

/// The MPDU supplied to a submission. Fresh memory travels with its
/// `ClassFrame` and body; a `Sent` MPDU already owns that description and
/// body, and receives only the new attempt's phase-bound rate plan.
pub enum TxSource<'p, B, N, F, O, Key> {
    /// A new MPDU, numbered at its first protected transmission.
    Fresh {
        /// Caller-owned memory, writable only before admission.
        buffer: B,
        /// The checked, phase-bound frame description.
        frame: F,
        /// External octets at the end of the MPDU, retained until its return.
        body: Option<O>,
        /// The role's TX protection; the encoded Protected bit is derived from it.
        protection: TxProtection<Key>,
    },
    /// The same published MPDU again: keep its SN and PN, set Retry.
    Retransmission {
        /// The returned MPDU owns its description, payload and body.
        mpdu: N,
        /// Only the transmission parameters change, not the MPDU.
        plan: RetransmissionPlan<'p>,
    },
}

/// The phase-bound rates of a new attempt of a `Sent` MPDU. The backend
/// checks that its response kind matches the original MPDU's kind.
#[derive(Debug)]
pub enum RetransmissionPlan<'p> {
    /// An individually acknowledged MPDU.
    Individual(RetryPlan<PhaseRate<'p>>),
    /// An individually addressed MPDU with no ACK.
    IndividualNoAck(crate::SupportedNoAck<'p>),
    /// A group-addressed MPDU.
    Group(PhaseRate<'p>),
}

/// Preparation failed before admission; the original operation is returned
/// without allocating a PN or creating a TX terminal.
pub struct PreparationRefused<A> {
    /// The dynamic incompatibility.
    pub reason: PreparationError,
    /// Every resource and parameter of the rejected operation.
    pub operation: A,
}

/// Dynamic preparation failures, distinct from the outcome of accepted TX.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreparationError {
    /// A retry names a different key installation, even in the same slot.
    RetransmissionKey,
    /// The new rate plan requires a different response kind.
    RetransmissionKind,
    /// The aggregate cannot take another subframe.
    AggregateFull,
    /// A subframe lies outside the compressed BA window starting at the
    /// aggregate's first sequence number. Neither source has been published.
    BlockAckWindow,
}

/// What one interface of a running phase does: its own frames, keys and
/// Block Ack agreements.
///
/// The body and fault belong to the phase's resource family, with no
/// independent interface-level declarations:
///
/// ```compile_fail,E0220
/// use oer_ieee80211_lower_mac::InterfacePhase;
/// fn body<'p, 'id: 'p, I: InterfacePhase<'p, 'id>>() -> Option<I::TxBody> { None }
/// ```
///
/// ```compile_fail,E0220
/// use oer_ieee80211_lower_mac::InterfacePhase;
/// fn fault<'p, 'id: 'p, I: InterfacePhase<'p, 'id>>() -> Option<I::Fault> { None }
/// ```
///
/// ```
/// use oer_ieee80211_lower_mac::{InterfaceAttempt, InterfacePhase, PhaseTypes, Poisoned};
/// fn body<'p, 'id: 'p, I: InterfacePhase<'p, 'id>>()
///     -> Option<<I::Types as PhaseTypes<'id>>::TxBody> { None }
/// fn submit<'k, 'p, 'id: 'k + 'p, I: InterfacePhase<'p, 'id>>(
///     interface: &mut I, attempt: InterfaceAttempt<'k, 'p, 'id, I>,
/// ) -> Result<(), Poisoned<<I::Types as PhaseTypes<'id>>::Fault>> {
///     interface.submit(attempt)
/// }
/// ```
pub trait InterfacePhase<'p, 'id: 'p> {
    /// The backend's capabilities marker.
    type Caps: Ieee80211Capabilities;
    /// The types the phase's events carry.
    type Types: PhaseTypes<'id>;
    /// The interface's role, which names its keys among the phase's types.
    type Role: RoleKeys<'id, Self::Types>;
    /// The interface's own address, which the install gave it.
    fn address(&self) -> IndividualAddress;

    /// Admit the pool-prepared `attempt`. Its closed preparation proof
    /// retains phase, install and role; submission has no buffer-class
    /// parameter. A retransmission retains SN, PN and payload. An optional body remains
    /// owned by the attempt until its terminal, even if copied into DMA memory.
    /// The backend publishes the attempt once its queue is free; its
    /// context, ticket, buffers and bodies return together, exactly once.
    ///
    /// # Errors
    ///
    /// Only a poisoned backend refuses submission. Preparation already
    /// established the layout and key; fail-stop retains the operation until
    /// the installation ends, without inventing a terminal.
    fn submit<'k>(
        &mut self,
        attempt: InterfaceAttempt<'k, 'p, 'id, Self>,
    ) -> Result<(), Poisoned<<Self::Types as PhaseTypes<'id>>::Fault>>;

    /// Install `key` for frames with `peer` in `slot`.
    fn install_pairwise_key(
        &mut self,
        slot: <InterfaceKeys<'p, 'id, Self> as KeyTypes<'id>>::PairwiseSlot,
        peer: IndividualAddress,
        key: Ccmp128Key,
    ) -> <InterfaceKeys<'p, 'id, Self> as KeyTypes<'id>>::PairwiseKey;

    /// Install `key` for group frames under `key_id` in `slot`.
    fn install_group_key(
        &mut self,
        slot: <InterfaceKeys<'p, 'id, Self> as KeyTypes<'id>>::GroupSlot,
        key_id: GroupKeyId,
        key: Ccmp128Key,
    ) -> <InterfaceKeys<'p, 'id, Self> as KeyTypes<'id>>::GroupKey;

    /// Remove `key`: queued attempts that use it return as
    /// `Withdrawn(KeyRemoved)`, a published one is waited for, and
    /// [`LowerMacEvent::KeyRemoved`] gives its slot back.
    fn remove_pairwise_key(
        &mut self,
        key: <InterfaceKeys<'p, 'id, Self> as KeyTypes<'id>>::PairwiseKey,
    );

    /// Remove `key`, as [`Self::remove_pairwise_key`].
    fn remove_group_key(&mut self, key: <InterfaceKeys<'p, 'id, Self> as KeyTypes<'id>>::GroupKey);

    /// Start acknowledging A-MPDUs of `agreement` in `slot`; the agreement
    /// holds the slot. A caller holds one agreement per peer and TID: it
    /// replaces one through [`Self::replace_block_ack`]. This writes the
    /// slot's desired state; add, replace and remove never enqueue commands.
    /// The latest value is applied before the driver's next command.
    #[must_use = "an agreement holds its slot; dropping it loses the slot until the next install"]
    fn add_block_ack(
        &mut self,
        slot: Slot<'id, <Self::Types as PhaseTypes<'id>>::BlockAckKind>,
        agreement: SupportedBlockAck<Self::Caps>,
    ) -> <Self::Types as PhaseTypes<'id>>::BlockAck;

    /// Replace `running` with `agreement` of the same peer and TID, in its
    /// slot.
    #[must_use = "an agreement holds its slot; dropping it loses the slot until the next install"]
    fn replace_block_ack(
        &mut self,
        running: <Self::Types as PhaseTypes<'id>>::BlockAck,
        agreement: SupportedBlockAck<Self::Caps>,
    ) -> <Self::Types as PhaseTypes<'id>>::BlockAck;

    /// Clear the desired state of `agreement`, giving its slot back. Reuse
    /// before the driver runs replaces the pending removal with the new
    /// agreement; no intermediate hardware installation is promised.
    #[must_use = "an ended agreement's slot is capacity; dropping it loses it until the next install"]
    fn remove_block_ack(
        &mut self,
        agreement: <Self::Types as PhaseTypes<'id>>::BlockAck,
    ) -> Slot<'id, <Self::Types as PhaseTypes<'id>>::BlockAckKind>;
}

/// The station interface of a running phase.
pub trait StationPhase<'p, 'id: 'p>: InterfacePhase<'p, 'id, Role = Station> {
    /// Consume an installed GTK and enqueue its replacement in command order.
    /// The slot reserves one terminal place at this call, under the same
    /// key-slot capacity bound as removal. No separate removal is possible
    /// while the operation holds the key and slot.
    ///
    /// [`LowerMacEvent::GroupKeyReplaced`] returns the new installed key with
    /// the same slot. Its position in the event stream is the installation
    /// boundary for RX. A hardware write failure poisons the backend; any
    /// internal hardware rollback is not a successful port terminal.
    fn replace_group_key(
        &mut self,
        installed: <InterfaceKeys<'p, 'id, Self> as KeyTypes<'id>>::GroupKey,
        key_id: GroupKeyId,
        key: Ccmp128Key,
    );

    /// Set what the station receives once none of its frames is in flight:
    /// a frame the closed gate holds ends as aborted at once, a published
    /// one is waited for, and [`LowerMacEvent::Configured`] follows.
    fn configure(&mut self, receive: StationReceive<Self::Caps>);

    /// The BSS color of the station's HE BSS.
    fn set_bss_color(&mut self, color: BssColor);
}

/// The access-point interface of a running phase, whose BSSID is its
/// address.
pub trait AccessPointPhase<'p, 'id: 'p>: InterfacePhase<'p, 'id, Role = AccessPoint> {
    /// Set the access point's receive rules, as
    /// [`StationPhase::configure`].
    fn configure(&mut self, receive: SupportedFilter<Self::Caps, AccessPoint>);
}

/// A class of transmit buffers: memory of one size, which the composition
/// sets.
pub trait TxBufferClass {
    /// The longest MPDU a buffer of the class holds, without its FCS.
    const CAPACITY: usize;
}

/// The phase types of a backend that keeps a pool of the buffer class `C`.
pub trait TxBufferPool<'id, C: TxBufferClass>: PhaseTypes<'id> {
    /// A buffer of the class: its memory, which the body holds and writes
    /// while it is not submitted. The backend owns its type and mints it
    /// only in the install's resources.
    type Buffer: TxBufferMemory;

    /// Prepare a source while its buffer class is concrete. The resulting
    /// closed value retains phase, install and role until `submit` or an
    /// aggregate consumes it. A refusal returns the original typed source.
    fn prepare<'k, 'p, R: RoleKeys<'id, Self>, K: Ieee80211Capabilities>(
        source: CheckedPoolSource<'k, 'p, 'id, Self, C, K, R>,
    ) -> PoolPreparation<'k, 'p, 'id, Self, C, K, R>
    where
        'id: 'k;

    /// Prepare and append through this builder's receiver, TID and protection.
    /// Fresh sources have no protection field. Refusal returns both the entire
    /// builder and the unchanged class-specific source, before admission or PN.
    fn push_aggregate<'k, 'p, R, K, S>(
        operation: crate::CheckedPoolAggregate<'k, 'p, 'id, Self, C, R, K, S>,
    ) -> crate::AggregatePushResult<'k, 'p, 'id, Self, C, R, K, S>
    where
        R: RoleKeys<'id, Self>,
        K: Ieee80211Capabilities,
        S: crate::AggregateState,
        'id: 'k + 'p;

    /// The buffer, as any class's.
    fn erase(buffer: Self::Buffer) -> Self::AnyTxBuffer;

    /// A published MPDU, protected or open. It owns the immutable encoded
    /// header, payload, checked description and external body. A protected
    /// MPDU also retains its PN and original key-install identity.
    /// A withdrawal returns Fresh only if never published; otherwise Sent.
    /// Retransmission preserves SN, PN and payload and sets Retry.
    /// Only renewal releases the old MPDU for a different frame.
    type Sent;

    /// The buffer, as any class's.
    fn erase_sent(buffer: Self::Sent) -> Self::AnyTxBuffer;

    /// The fresh buffer of the class `C` a completion gave back.
    ///
    /// # Errors
    ///
    /// The buffer, when it is of another class or `Sent`.
    fn class(buffer: Self::AnyTxBuffer) -> Result<Self::Buffer, Self::AnyTxBuffer>;

    /// The `Sent` buffer of the class `C` a completion gave back.
    ///
    /// # Errors
    ///
    /// The buffer, when it is of another class or fresh.
    fn sent(buffer: Self::AnyTxBuffer) -> Result<Self::Sent, Self::AnyTxBuffer>;

    /// The memory of a `Sent` buffer for a new MPDU: its packet number is
    /// forgotten, and the next protected attempt in it is numbered anew.
    /// The original external body is returned explicitly beside the memory.
    fn renew(buffer: Self::Sent) -> (Self::Buffer, Option<Self::TxBody>);
}

/// A returned MPDU retains whether it has ever been published, independently
/// of encryption. `Sent` owns the immutable MPDU, its description and body.
/// Only the owning machine decides to renew it for a different MPDU.
#[derive(Debug)]
pub enum ReturnedMpdu<B, S> {
    /// Original writable allocation; this MPDU was never published.
    Fresh(B),
    /// Published MPDU, including an unprotected one.
    Sent(S),
}

impl<B, S> ReturnedMpdu<B, S> {
    /// Release the old MPDU through its owner's renewal policy. A fresh
    /// allocation needs no renewal. The operation consumes either owner once.
    pub fn into_fresh(self, renew: impl FnOnce(S) -> B) -> B {
        match self {
            Self::Fresh(buffer) => buffer,
            Self::Sent(mpdu) => renew(mpdu),
        }
    }
}

/// The memory of a transmit buffer the body holds: the caller encodes a
/// frame into it, from its header on, before it hands the buffer in
/// ([owner decision on #563](https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6129930775)).
///
/// A backend's buffer token keeps four rules:
///
/// - it opens only the frame's octets, never the DMA descriptor;
/// - it is written only while the body holds it, and a submission takes
///   it by value;
/// - when its memory is cacheable, the backend makes it coherent at the
///   submission, before the DMA reads it;
/// - it is a handle on pinned memory: moving the token never moves the
///   buffer.
pub trait TxBufferMemory {
    /// The buffer's octets, as long as its class holds.
    fn bytes_mut(&mut self) -> &mut [u8];
}

/// A frame that a buffer of the class `C` fits; only [`ClassFrame::new`]
/// makes one.
pub struct ClassFrame<'p, C, K> {
    frame: TxFrame<'p, K>,
    class: core::marker::PhantomData<C>,
}

impl<'p, C: TxBufferClass, K: Ieee80211Capabilities> ClassFrame<'p, C, K> {
    /// `frame`, when a buffer of the class `C` fits it.
    ///
    /// # Errors
    ///
    /// The frame, when it is longer than the class holds.
    pub fn new(frame: TxFrame<'p, K>) -> Result<Self, TxFrame<'p, K>> {
        if frame.mpdu_len() <= C::CAPACITY {
            Ok(Self {
                frame,
                class: core::marker::PhantomData,
            })
        } else {
            Err(frame)
        }
    }

    /// The frame.
    pub fn into_frame(self) -> TxFrame<'p, K> {
        self.frame
    }

    /// Borrow the checked description for preparation without consuming
    /// the original operation, so a refusal can return it intact.
    pub const fn frame(&self) -> &TxFrame<'p, K> {
        &self.frame
    }
}

/// A phase-independent checked description that fits its buffer class.
/// Portable machines retain it in the operation context. Adding the phase's
/// rates cannot lose or re-check the established size proof.
#[derive(Debug)]
pub struct ClassSizedFrame<C, H, K> {
    frame: crate::SizedFrame<H, K>,
    class: core::marker::PhantomData<C>,
}
impl<C, H: Copy, K> Copy for ClassSizedFrame<C, H, K> {}
impl<C, H: Copy, K> Clone for ClassSizedFrame<C, H, K> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<C: TxBufferClass, H: crate::FrameHeader, K: Ieee80211Capabilities> ClassSizedFrame<C, H, K> {
    /// Establish the class-size proof while constructing the operation.
    pub fn new(frame: crate::SizedFrame<H, K>) -> Result<Self, crate::SizedFrame<H, K>> {
        if frame.len().get() <= C::CAPACITY {
            Ok(Self {
                frame,
                class: core::marker::PhantomData,
            })
        } else {
            Err(frame)
        }
    }
    /// The already checked description.
    pub const fn frame(&self) -> &crate::SizedFrame<H, K> {
        &self.frame
    }
}
impl<'p, C, K> ClassFrame<'p, C, K> {
    /// Add only the phase-bound retry plan to an individually acknowledged MPDU.
    pub fn individual(
        frame: ClassSizedFrame<C, crate::AckHeader, K>,
        plan: RetryPlan<PhaseRate<'p>>,
    ) -> Self {
        Self {
            frame: TxFrame::Individual {
                frame: frame.frame,
                plan,
            },
            class: core::marker::PhantomData,
        }
    }
    /// Add only the phase-bound rate to a group MPDU.
    pub fn group(
        frame: ClassSizedFrame<C, crate::HeaderSpec<crate::GroupAddress>, K>,
        rate: PhaseRate<'p>,
    ) -> Self {
        Self {
            frame: TxFrame::Group {
                frame: frame.frame,
                rate,
            },
            class: core::marker::PhantomData,
        }
    }
}

/// The stages a retry plan holds at most.
pub const RETRY_STAGES: usize = 4;

/// One stage of a retry plan: a rate, the transmissions at it and their
/// protection.
#[derive(Debug)]
pub struct RetryStage<R> {
    /// The rate of the stage's transmissions.
    pub rate: R,
    /// The transmissions at the rate.
    pub transmissions: core::num::NonZeroU8,
    /// The control exchange that protects each of them.
    pub protection: Protection,
}

/// The retry plan of an individual frame: its stages in order, as the
/// rate table mac80211 hands its drivers ([owner decisions on #563]).
/// The backend transmits the head of its queue by the plan before the next
/// attempt of that queue, keeps the frame's sequence number and sets the
/// Retry bit of each retransmission; an exhausted plan ends the attempt
/// unacknowledged. A backend whose hardware retries carries the plan out
/// there ([`HardwareServices::RETRY_POLICY`](crate::HardwareServices::RETRY_POLICY)).
#[derive(Debug)]
pub struct RetryPlan<R> {
    /// The first stage: a plan always has one.
    first: RetryStage<R>,
    /// The initialized stages after the first, contiguous and bounded.
    rest: heapless::Vec<RetryStage<R>, { RETRY_STAGES - 1 }>,
}

impl<R> RetryPlan<R> {
    /// The plan of `first` alone.
    pub fn new(first: RetryStage<R>) -> Self {
        Self {
            first,
            rest: heapless::Vec::new(),
        }
    }

    /// The plan with `stage` after its last.
    ///
    /// # Errors
    ///
    /// The plan and the stage, when the plan holds [`RETRY_STAGES`].
    pub fn then(mut self, stage: RetryStage<R>) -> Result<Self, (Self, RetryStage<R>)> {
        match self.rest.push(stage) {
            Ok(()) => Ok(self),
            Err(stage) => Err((self, stage)),
        }
    }

    /// The first stage.
    pub const fn first(&self) -> &RetryStage<R> {
        &self.first
    }

    /// The first stage, by value: a frame of one rate takes it.
    pub fn into_first(self) -> RetryStage<R> {
        self.first
    }

    /// The stages in order.
    pub fn stages(&self) -> impl Iterator<Item = &RetryStage<R>> {
        core::iter::once(&self.first).chain(self.rest.iter())
    }

    /// The transmissions the plan makes at most.
    pub fn transmissions(&self) -> u16 {
        self.stages()
            .map(|stage| u16::from(stage.transmissions.get()))
            .sum()
    }

    /// The plan's first `transmissions` transmissions: its stages in order,
    /// the last one cut and the ones after it dropped.
    ///
    /// ```
    /// # use core::num::NonZeroU8;
    /// # use oer_ieee80211_lower_mac::{Protection, RetryPlan, RetryStage};
    /// let stage = |rate, transmissions| RetryStage {
    ///     rate,
    ///     transmissions: NonZeroU8::new(transmissions).unwrap(),
    ///     protection: Protection::None,
    /// };
    /// let plan = RetryPlan::new(stage('a', 2)).then(stage('b', 3)).ok().unwrap();
    /// let plan = plan.then(stage('c', 25)).ok().unwrap();
    /// let cut = plan.limit(NonZeroU8::new(4).unwrap());
    /// let left: Vec<_> = cut.stages().map(|s| (s.rate, s.transmissions.get())).collect();
    /// assert_eq!(left, [('a', 2), ('b', 2)]);
    /// assert_eq!(cut.transmissions(), 4);
    /// // Each stage's rate turns; a refused one stops the plan.
    /// let numbers = cut.try_map(|rate| Ok::<_, char>(rate as u8)).unwrap();
    /// assert_eq!(numbers.stages().next().unwrap().rate, b'a');
    /// let refused = numbers.try_map(|rate| if rate == b'b' { Err(rate) } else { Ok(rate) });
    /// assert_eq!(refused.err(), Some(b'b'));
    /// ```
    pub fn limit(mut self, transmissions: core::num::NonZeroU8) -> Self {
        let first = self.first.transmissions.min(transmissions);
        let mut left = first.get()..transmissions.get();
        self.first.transmissions = first;
        self.rest.retain_mut(|stage| {
            let taken = left
                .by_ref()
                .take(usize::from(stage.transmissions.get()))
                .count() as u8;
            match core::num::NonZeroU8::new(taken) {
                Some(taken) => {
                    stage.transmissions = taken;
                    true
                }
                None => false,
            }
        });
        self
    }

    /// Map each rate without changing the nonempty plan or its budget.
    pub fn map<S>(&self, mut rate: impl FnMut(&R) -> S) -> RetryPlan<S> {
        let mut stage = |stage: &RetryStage<R>| RetryStage {
            rate: rate(&stage.rate),
            transmissions: stage.transmissions,
            protection: stage.protection,
        };
        RetryPlan {
            first: stage(&self.first),
            // One output per initialized input, with the same capacity.
            rest: self.rest.iter().map(stage).collect(),
        }
    }

    /// Keep the stages whose rates `rate` accepts, in their original order.
    /// Each retained stage keeps its exact transmission count and protection;
    /// `None` means every stage was refused.
    pub fn filter_map<S>(self, mut rate: impl FnMut(R) -> Option<S>) -> Option<RetryPlan<S>> {
        let mut stages = core::iter::once(self.first)
            .chain(self.rest)
            .filter_map(|stage| {
                Some(RetryStage {
                    rate: rate(stage.rate)?,
                    transmissions: stage.transmissions,
                    protection: stage.protection,
                })
            });
        Some(RetryPlan {
            first: stages.next()?,
            // The input has at most RETRY_STAGES stages. Taking its first
            // accepted stage leaves at most RETRY_STAGES - 1; filtering
            // cannot increase that bound.
            rest: stages.collect(),
        })
    }

    /// The plan with each stage's rate turned by `rate`, the stages and
    /// their transmissions kept.
    ///
    /// # Errors
    ///
    /// The first error `rate` returned.
    pub fn try_map<S, E>(self, mut rate: impl FnMut(R) -> Result<S, E>) -> Result<RetryPlan<S>, E> {
        let mut turn = |stage: RetryStage<R>| {
            Ok(RetryStage {
                rate: rate(stage.rate)?,
                transmissions: stage.transmissions,
                protection: stage.protection,
            })
        };
        Ok(RetryPlan {
            first: turn(self.first)?,
            // At most RETRY_STAGES - 1 inputs, even when a conversion fails.
            rest: self.rest.into_iter().map(turn).collect::<Result<_, _>>()?,
        })
    }
}

#[cfg(test)]
mod tests;
