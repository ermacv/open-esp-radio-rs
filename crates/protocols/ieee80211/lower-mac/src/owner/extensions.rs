//! The optional operations of a lower-MAC phase.
//!
//! A structural feature a backend may lack is an extension trait of the
//! phase or interface that performs it: an upper layer that needs the
//! feature requires the bound, and a backend without it does not implement
//! the trait. Every rule a caller controls is a type here, as in the base
//! contract: an aggregate's rate, traffic identifier and size; only the
//! world's outcome stays a result. No operation awaits: a setting takes
//! effect before the next submission, and what the hardware answers
//! arrives as an event.
//!
//! - [`AmpduPhase`]: one attempt sends one A-MPDU, built in an aggregate
//!   whose subframe count is the backend's limit, each subframe in a buffer
//!   of its own; the planner above the port resends the subframes the
//!   Block Ack report misses.
//! - [`BeaconTimingPhase`]: an interface's TSF and TBTT schedule; its TBTT
//!   events and TSF samples come in the phase's stream.
//! - [`AirReservationPhase`]: a gate's closing that reserves the access
//!   point's air for an absence of the station beside it.
//! - [`MonitorPhase`]: monitor reception, a radio-wide setting.
//!
//! [owner decisions on #563]: https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6129677418

use oer_ieee80211_mac::tsf::{TsfDuration, TsfInstant};
use oer_radio_port::Slot;

use super::{Ieee80211Capabilities, PhaseRate, SupportedCoex, SupportedPower, Tid};
use crate::{
    AmpduCapabilities, PreparationRefused, Protection, SubmitError,
    phase::{AttemptKey, GateOpen, InterfacePhase, LowerMacPhase, PhaseTypes},
};

/// A rate the backend aggregates at, checked by [`AmpduPhase::ampdu_rate`].
#[derive(Clone, Copy, Debug)]
pub struct AmpduRate<'p>(PhaseRate<'p>);

impl<'p> AmpduRate<'p> {
    /// The rate.
    pub const fn rate(&self) -> &PhaseRate<'p> {
        &self.0
    }
}

/// An aggregate being built: its subframes, each in a buffer of its own.
pub trait PhaseAggregate<'p, 'id: 'p> {
    /// The owner of a subframe's body.
    type Body: crate::TxBody;
    /// The backend's capabilities marker.
    type Caps: Ieee80211Capabilities;
    /// The phase types whose buffers the subframes take.
    type Types: PhaseTypes<'id, TxBody = Self::Body>;
    /// The interface role that may submit this aggregate.
    type Role: crate::RoleKeys<'id, Self::Types>;

    /// Append one prepared individual MPDU, fresh or retransmitted. The
    /// pool already checked its class and layout; the aggregate retains
    /// its phase, install and role proof through submission.
    ///
    /// # Errors
    ///
    /// The unchanged prepared MPDU if the aggregate is full or its response
    /// kind cannot be aggregated. No ownership is lost on refusal.
    fn push(
        &mut self,
        mpdu: <Self::Types as PhaseTypes<'id>>::Prepared<'p, Self::Role, Self::Caps>,
    ) -> Result<
        (),
        PreparationRefused<<Self::Types as PhaseTypes<'id>>::Prepared<'p, Self::Role, Self::Caps>>,
    >;

    /// The subframes pushed.
    fn subframes(&self) -> usize;

    /// Cancel preparation and recover every buffer and external body in
    /// subframe order, including after a refused `submit_ampdu`. Numbered
    /// MPDUs retain their own bodies until retry or renewal. This consumes
    /// no ticket and creates no TX terminal.
    #[allow(clippy::type_complexity, reason = "the aggregate's owned resources")]
    fn into_parts(
        self,
    ) -> (
        <Self::Types as PhaseTypes<'id>>::TxBuffers,
        <Self::Types as PhaseTypes<'id>>::TxBodies,
    );
}

/// The minimum MPDU start spacing of an aggregate, as the 802.11 encoding
/// 0 to 7.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MpduStartSpacing(u8);

impl MpduStartSpacing {
    /// `encoding`, when it is 0 to 7.
    pub const fn new(encoding: u8) -> Option<Self> {
        if encoding <= 7 {
            Some(Self(encoding))
        } else {
            None
        }
    }

    /// The encoding.
    pub const fn get(self) -> u8 {
        self.0
    }
}

/// One A-MPDU attempt: its ticket, its aggregate and how it goes.
pub struct AggregateAttempt<'id, 'k, T, A, P, K, X> {
    /// The caller's context for all subframes, returned with their terminal.
    pub context: X,
    /// The attempt's place in flight.
    pub ticket: Slot<'id, T>,
    /// The aggregate, whose subframes' buffers come back with the terminal.
    pub aggregate: A,
    /// The control exchange that protects it.
    pub protection: Protection,
    /// The pairwise key that protects every subframe, or none.
    pub key: Option<&'k P>,
    /// Its transmit power.
    pub power: SupportedPower<K>,
    /// Its coexistence level.
    pub coex: SupportedCoex<K>,
    /// The recipient's minimum MPDU start spacing.
    pub min_mpdu_start_spacing: MpduStartSpacing,
}

/// The aggregate attempt of this interface, including its context and
/// every subframe. A preparation refusal returns this same value.
pub type InterfaceAggregateAttempt<'k, 'p, 'id, I> = AggregateAttempt<
    'id,
    'k,
    <<I as InterfacePhase<'p, 'id>>::Types as PhaseTypes<'id>>::TicketKind,
    <I as AmpduPhase<'p, 'id>>::Aggregate,
    <crate::InterfaceKeys<'p, 'id, I> as crate::KeyTypes<'id>>::PairwiseKey,
    <I as InterfacePhase<'p, 'id>>::Caps,
    <<I as InterfacePhase<'p, 'id>>::Types as PhaseTypes<'id>>::Context,
>;

/// An interface that sends A-MPDUs.
pub trait AmpduPhase<'p, 'id: 'p>: InterfacePhase<'p, 'id> {
    /// An aggregate being built.
    type Aggregate: PhaseAggregate<
            'p,
            'id,
            Body = Self::TxBody,
            Caps = Self::Caps,
            Types = Self::Types,
            Role = Self::Role,
        >;

    /// What the backend aggregates.
    fn ampdu_capabilities(&self) -> AmpduCapabilities;

    /// The rate, when the backend aggregates at it.
    ///
    /// # Errors
    ///
    /// The rate, when it does not.
    fn ampdu_rate(&self, rate: PhaseRate<'p>) -> Result<AmpduRate<'p>, PhaseRate<'p>> {
        if self.ampdu_capabilities().formats.contains_rate(rate.get()) {
            Ok(AmpduRate(rate))
        } else {
            Err(rate)
        }
    }

    /// An empty aggregate of `tid` at `rate`.
    fn aggregate(&mut self, tid: Tid, rate: AmpduRate<'p>) -> Self::Aggregate;

    /// Admit `attempt`, as [`InterfacePhase::submit`]: its ticket and its
    /// subframes' buffers come back with its terminal, whose Block Ack
    /// report names the subframes the recipient acknowledged.
    ///
    /// # Errors
    ///
    /// A preparation refusal returns this entire attempt before admission:
    /// a missing or mismatched key, a retry tied to a different key install,
    /// or an empty aggregate. Poisoning retains the fail-stop contract.
    #[allow(clippy::type_complexity, reason = "the attempt's own types")]
    fn submit_ampdu<'k>(
        &mut self,
        attempt: InterfaceAggregateAttempt<'k, 'p, 'id, Self>,
    ) -> Result<(), SubmitError<InterfaceAggregateAttempt<'k, 'p, 'id, Self>, Self::Fault>>;
}

/// The TBTT schedule of an interface: the wake plan the backend carries
/// out at each TBTT ([owner decisions on #563](https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6129037560)).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TbttPlan {
    /// The TSF of the next TBTT.
    pub next: TsfInstant,
    /// The beacon interval.
    pub beacon_interval: TsfDuration,
    /// How far ahead of each TBTT its event fires.
    pub lead: TsfDuration,
    /// How far ahead of each TBTT the radio wakes to receive its beacon.
    pub wake_ahead: TsfDuration,
    /// The window the radio keeps open for the beacon, and the
    /// coexistence priority it asks for the air with; `None` keeps no
    /// window.
    pub beacon_window: Option<BeaconWindow>,
}

/// The window after a TBTT the radio keeps open for the beacon.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BeaconWindow {
    /// How long the window stays open after the TBTT.
    pub duration: TsfDuration,
    /// The priority the window asks the radio's coexistence for.
    pub priority: oer_radio_coex::CoexPriority,
}

/// An interface with a TSF and a TBTT schedule. Its settings take effect
/// before the next submission; [`Self::sample_tsf`]'s answer and each
/// TBTT arrive as events.
pub trait BeaconTimingPhase<'p, 'id: 'p>: InterfacePhase<'p, 'id> {
    /// Ask for a paired reading of the interface's TSF and the port's radio
    /// clock, which [`LowerMacEvent::TsfSampled`](crate::LowerMacEvent::TsfSampled)
    /// answers.
    fn sample_tsf(&mut self);

    /// Set the interface's TSF to `at`.
    fn set_tsf(&mut self, at: TsfInstant);

    /// Follow `plan`, in place of the earlier one.
    fn set_tbtt(&mut self, plan: TbttPlan);

    /// Stop the TBTT schedule.
    fn stop_tbtt(&mut self);
}

/// A phase whose gate's closing can take one last frame that reserves the
/// air: a CTS-to-self of the access point, which its peers honour by
/// keeping quiet while the station beside it is away ([owner decisions on
/// #563]).
pub trait AirReservationPhase<'p, 'id: 'p>: LowerMacPhase<'p, 'id> {
    /// Close the gate as [`LowerMacPhase::close_gate`] does, and after the
    /// published attempts end, before
    /// [`LowerMacEvent::GateClosed`](crate::LowerMacEvent::GateClosed),
    /// send the access point's CTS-to-self reserving the air until `until`:
    /// its Duration is `min(until − now, MAX_RESERVATION)` when it goes,
    /// and no frame goes once no time is left.
    fn close_gate_reserving(&mut self, gate: GateOpen<'id>, until: crate::Ieee80211Instant);
}

/// The longest reservation a CTS-to-self's Duration field carries.
pub const MAX_RESERVATION_MICROS: u16 = 32_767;

/// A phase that receives as a monitor.
pub trait MonitorPhase<'p, 'id: 'p>: LowerMacPhase<'p, 'id> {
    /// Receive every frame the radio hears, or only the interfaces' own.
    fn set_monitor(&mut self, enabled: bool);
}

/// An attempt key of a pairwise key alone, for an aggregate.
pub type AggregateKey<'k, P> = AttemptKey<'k, P, core::convert::Infallible>;
