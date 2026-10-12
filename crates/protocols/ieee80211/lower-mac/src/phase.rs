//! A running lower-MAC phase: one object under `&mut`
//! ([`PortPhase`](oer_ieee80211_port::PortPhase)), whose event stream and
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
//! | a transmit buffer of a class ([`TxBufferClass`]), its memory ([`TxBufferMemory`]) | [`InterfacePhase::tx_buffer`], once the caller encoded its frame in it, then the submission | [`LowerMacEvent::TxCompleted`], every buffer of the attempt |
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
//! # Compile-fail tests
//!
//! No caller mints a token: a gate, a ticket or a buffer comes only from
//! the install's resources.
//!
//! ```compile_fail,E0451
//! # use oer_ieee80211_lower_mac::GateOpen;
//! # use core::marker::PhantomData;
//! fn forge<'id>() -> GateOpen<'id> {
//!     oer_radio_port::Slot { index: 0, brand: PhantomData, kind: PhantomData }
//! }
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
//! An attempt holds its ticket and its buffer, so a submitted ticket is
//! gone until its terminal:
//!
//! ```compile_fail,E0382
//! # use oer_ieee80211_lower_mac::*;
//! # use oer_ieee80211_mac::qos::WmmAccessCategory;
//! # use oer_radio_port::Slot;
//! fn two<'p, 'id: 'p, I: InterfacePhase<'p, 'id>>(
//!     interface: &mut I,
//!     ticket: Slot<'id, <I::Types as PhaseTypes<'id>>::TicketKind>,
//!     frame: I::TxBuffer,
//!     other: I::TxBuffer,
//!     power: SupportedPower<I::Caps>,
//!     coex: SupportedCoex<I::Caps>,
//! ) where
//!     SupportedPower<I::Caps>: Copy,
//!     SupportedCoex<I::Caps>: Copy,
//! {
//!     let attempt = |frame| Attempt {
//!         ticket,
//!         frame,
//!         body: None,
//!         access_category: WmmAccessCategory::BestEffort,
//!         protection: Protection::None,
//!         key: None,
//!         power,
//!         coex,
//!     };
//!     let _ = interface.submit(attempt(frame));
//!     let _ = interface.submit(attempt(other));
//! }
//! ```
//!
//! Its compiling twin submits each ticket once:
//!
//! ```
//! # use oer_ieee80211_lower_mac::*;
//! # use oer_ieee80211_mac::qos::WmmAccessCategory;
//! # use oer_radio_port::Slot;
//! fn one<'p, 'id: 'p, I: InterfacePhase<'p, 'id>>(
//!     interface: &mut I,
//!     ticket: Slot<'id, <I::Types as PhaseTypes<'id>>::TicketKind>,
//!     frame: I::TxBuffer,
//!     power: SupportedPower<I::Caps>,
//!     coex: SupportedCoex<I::Caps>,
//! ) {
//!     let _ = interface.submit(Attempt {
//!         ticket,
//!         frame,
//!         body: None,
//!         access_category: WmmAccessCategory::BestEffort,
//!         protection: Protection::None,
//!         key: None,
//!         power,
//!         coex,
//!     });
//! }
//! ```
//!
//! # Poisoned
//!
//! A poisoned backend ends the phase: `next_event` returns
//! [`Poisoned`](oer_radio_port::Poisoned), and the tokens the port holds
//! are not given back. The body ends the station whole and leaves through
//! the poisoned owner; every token ends with the install.
//!
//! [owner decision on #563]: https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6128926225
//! [owner decisions on #563]: https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6129037560

use oer_ieee80211_port::PortPhase;
use oer_radio_port::{Poisoned, Slot};

use crate::{
    Channel, PhyRate, Protection, RxMeta, TbttEvent, TxCompletion, VifRole,
    owner::{
        AccessPoint, BssColor, Ccmp128Key, GroupKeyId, Ieee80211Capabilities, IndividualAddress,
        InterfaceRole, PhaseRate, Station, StationReceive, SupportedBlockAck,
        SupportedCoex, SupportedEdca, SupportedFilter, SupportedNoAck, SupportedPower,
        TxFrame,
    },
};

/// The kind of the open gate's token.
pub enum GateOpenKind {}

/// The kind of the closed gate's token.
pub enum GateClosedKind {}

/// The open transmit gate: one per install, which the body holds while the
/// gate is open.
pub type GateOpen<'id> = Slot<'id, GateOpenKind>;

/// The closed transmit gate: it proves this port publishes no attempt, not
/// that the air is quiet. The backend holds it while the gate is open.
pub type GateClosed<'id> = Slot<'id, GateClosedKind>;

/// An event of a running lower-MAC phase, in the order the events
/// occurred; every token an operation took comes back in one.
pub enum LowerMacEvent<'id, P: PhaseTypes<'id>> {
    /// One received MPDU and its metadata.
    Received {
        /// The frame, from its header to the end of its body, without the
        /// FCS.
        frame: P::RxBuffer,
        /// What the hardware observed of it.
        meta: RxMeta,
    },
    /// The terminal of one attempt: its ticket, its buffers and the bodies
    /// it carried.
    TxCompleted {
        /// The attempt's ticket.
        ticket: Slot<'id, P::TicketKind>,
        /// The buffers of the attempt's frames.
        buffers: P::TxBuffers,
        /// How the attempt ended.
        completion: TxCompletion,
        /// The bodies the attempt carried, each with its subframe index.
        bodies: P::TxBodies,
    },
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
    /// A removed key's slot, once no attempt in flight uses the key.
    KeyRemoved(P::RemovedKey),
}

/// The types of one backend's phase that its events carry.
pub trait PhaseTypes<'id> {
    /// The kind of the transmit tickets.
    type TicketKind;
    /// The buffers a completion gives back, of their classes.
    type TxBuffers: IntoIterator;
    /// The memory of one received MPDU, which goes back through
    /// [`LowerMacPhase::recycle`].
    type RxBuffer: crate::RxBuffer;
    /// The bodies a completion carries, each with its subframe index.
    type TxBodies: IntoIterator;
    /// A removed key's slot, of the role and kind of the key.
    type RemovedKey;
}

/// A running lower-MAC phase of the install `'id`, lent for `'p`.
pub trait LowerMacPhase<'p, 'id: 'p>:
    PortPhase<Event = LowerMacEvent<'id, Self::Types>> + Sized
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

    /// Give a received frame's buffer back to reception. A buffer never
    /// given back only shrinks reception; in tests and debug builds a
    /// dropped one panics.
    fn recycle(&mut self, buffer: <Self::Types as PhaseTypes<'id>>::RxBuffer);

    /// Close the transmit gate of every queue: admission of new attempts
    /// stops at once, and [`LowerMacEvent::GateClosed`] follows once no
    /// attempt is published.
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

/// One attempt, every value of it checked: its ticket, its MPDU in a
/// buffer, its body, its protection, key, power and coexistence level.
/// The frame's kind, fixed when the buffer was lent, sets its response and
/// its retry plan ([`RetryPlan`]) or one rate; its access category picks
/// its queue, whose contention the backend draws.
pub struct Attempt<'id, 'k, T, B, O, P, G, K> {
    /// The attempt's place in flight.
    pub ticket: Slot<'id, T>,
    /// The frame in its buffer, whose kind fixes its rate and response.
    pub frame: B,
    /// The body the frame carries after its header, owned by the attempt
    /// until its terminal.
    pub body: Option<O>,
    /// The access category the attempt contends as.
    pub access_category: oer_ieee80211_mac::qos::WmmAccessCategory,
    /// The control exchange that protects its first transmission; a
    /// retry plan's stages name their own.
    pub protection: Protection,
    /// The installed key that protects the frame, or none.
    pub key: Option<AttemptKey<'k, P, G>>,
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

/// The attempt of the interface `I`.
pub type InterfaceAttempt<'k, 'p, 'id, I> = Attempt<
    'id,
    'k,
    <<I as InterfacePhase<'p, 'id>>::Types as PhaseTypes<'id>>::TicketKind,
    <I as InterfacePhase<'p, 'id>>::TxBuffer,
    <I as InterfacePhase<'p, 'id>>::TxBody,
    <I as InterfacePhase<'p, 'id>>::PairwiseKey,
    <I as InterfacePhase<'p, 'id>>::GroupKey,
    <I as InterfacePhase<'p, 'id>>::Caps,
>;

/// What one interface of a running phase does: its own frames, keys and
/// Block Ack agreements.
pub trait InterfacePhase<'p, 'id: 'p> {
    /// The backend's capabilities marker.
    type Caps: Ieee80211Capabilities;
    /// The types the phase's events carry.
    type Types: PhaseTypes<'id>;
    /// The interface's role.
    type Role: InterfaceRole;
    /// What poisons the backend.
    type Fault: Copy + core::fmt::Debug;
    /// One frame's memory, which holds its buffer token, with its header,
    /// kind, rate and response written.
    type TxBuffer: crate::owner::TxFrameBody;
    /// The owner of an MPDU's body.
    type TxBody: crate::TxBody;
    /// The kind of the interface's pairwise key slots (an access point's:
    /// its association identifiers).
    type PairwiseKeyKind;
    /// The kind of the interface's group key slots.
    type GroupKeyKind;
    /// An installed pairwise key, which holds its slot.
    type PairwiseKey;
    /// An installed group key, which holds its slot.
    type GroupKey;
    /// The kind of the receive Block Ack slots: one pool the backend's
    /// hardware shares between its roles (owner decision, Claude Code chat
    /// with common, 2026-10-12).
    type BlockAckKind;
    /// A running receive Block Ack agreement, which holds its slot.
    type BlockAck;

    /// The interface's own address, which the install gave it.
    fn address(&self) -> IndividualAddress;

    /// The frame `buffer` holds, the caller's encoding from its header on,
    /// bound to `frame`, its kind, which the buffer's class fits
    /// ([`ClassFrame`]): the backend writes the header from `frame` (one
    /// source of its QoS Ack Policy) and sends the octets after it as
    /// written. Dropped unsubmitted, the buffer is lost until the next
    /// install.
    fn tx_buffer<C>(
        &mut self,
        buffer: <Self::Types as TxBufferPool<'id, C>>::Buffer,
        frame: ClassFrame<'p, C, Self::Caps>,
    ) -> Self::TxBuffer
    where
        C: TxBufferClass,
        Self::Types: TxBufferPool<'id, C>;

    /// Admit `attempt`: the backend publishes it once its queue is free,
    /// and its ticket and buffers come back with its terminal.
    ///
    /// # Errors
    ///
    /// A poisoned backend; the attempt's ticket ends with the install.
    fn submit(
        &mut self,
        attempt: InterfaceAttempt<'_, 'p, 'id, Self>,
    ) -> Result<(), Poisoned<Self::Fault>>;

    /// Install `key` for frames with `peer` in `slot`.
    fn install_pairwise_key(
        &mut self,
        slot: Slot<'id, Self::PairwiseKeyKind>,
        peer: IndividualAddress,
        key: Ccmp128Key,
    ) -> Self::PairwiseKey;

    /// Install `key` for group frames under `key_id` in `slot`.
    fn install_group_key(
        &mut self,
        slot: Slot<'id, Self::GroupKeyKind>,
        key_id: GroupKeyId,
        key: Ccmp128Key,
    ) -> Self::GroupKey;

    /// Remove `key`: an attempt that uses it and that the closed gate holds
    /// ends as aborted at once, a published one is waited for, and
    /// [`LowerMacEvent::KeyRemoved`] gives its slot back.
    fn remove_pairwise_key(&mut self, key: Self::PairwiseKey);

    /// Remove `key`, as [`Self::remove_pairwise_key`].
    fn remove_group_key(&mut self, key: Self::GroupKey);

    /// Start acknowledging A-MPDUs of `agreement` in `slot`; the agreement
    /// holds the slot. A caller holds one agreement per peer and TID: it
    /// replaces one through [`Self::replace_block_ack`].
    #[must_use = "an agreement holds its slot; dropping it loses the slot until the next install"]
    fn add_block_ack(
        &mut self,
        slot: Slot<'id, Self::BlockAckKind>,
        agreement: SupportedBlockAck<Self::Caps>,
    ) -> Self::BlockAck;

    /// Replace `running` with `agreement` of the same peer and TID, in its
    /// slot.
    #[must_use = "an agreement holds its slot; dropping it loses the slot until the next install"]
    fn replace_block_ack(
        &mut self,
        running: Self::BlockAck,
        agreement: SupportedBlockAck<Self::Caps>,
    ) -> Self::BlockAck;

    /// End `agreement`, giving its slot back.
    #[must_use = "an ended agreement's slot is capacity; dropping it loses it until the next install"]
    fn remove_block_ack(&mut self, agreement: Self::BlockAck) -> Slot<'id, Self::BlockAckKind>;
}

/// The station interface of a running phase.
pub trait StationPhase<'p, 'id: 'p>: InterfacePhase<'p, 'id, Role = Station> {
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
}

/// The memory of a transmit buffer the body holds: the caller encodes a
/// frame into it, from its header on, before it hands the buffer in.
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
    stages: [Option<RetryStage<R>>; RETRY_STAGES],
}

impl<R> RetryPlan<R> {
    /// The plan of `first` alone.
    pub fn new(first: RetryStage<R>) -> Self {
        let mut stages = [const { None }; RETRY_STAGES];
        stages[0] = Some(first);
        Self { stages }
    }

    /// The plan with `stage` after its last.
    ///
    /// # Errors
    ///
    /// The plan and the stage, when the plan holds [`RETRY_STAGES`].
    pub fn then(mut self, stage: RetryStage<R>) -> Result<Self, (Self, RetryStage<R>)> {
        match self.stages.iter_mut().find(|slot| slot.is_none()) {
            Some(slot) => {
                *slot = Some(stage);
                Ok(self)
            }
            None => Err((self, stage)),
        }
    }

    /// The stages in order.
    pub fn stages(&self) -> impl Iterator<Item = &RetryStage<R>> {
        self.stages.iter().map_while(Option::as_ref)
    }

    /// The transmissions the plan makes at most.
    pub fn transmissions(&self) -> u16 {
        self.stages()
            .map(|stage| u16::from(stage.transmissions.get()))
            .sum()
    }
}

