//! The optional operations of a lower-MAC backend on the owner shape.
//!
//! A structural feature a backend may lack is an extension trait of the
//! part that performs it, as on the base contract: an upper layer that
//! needs the feature requires the bound, and a backend without it does not
//! implement the trait. Every rule a caller controls is a type here, as in
//! the base contract: an aggregate's rate, traffic identifier and size;
//! only the world's outcome (the air, a poisoned backend) stays a result.
//!
//! - [`AmpduClient`]: one attempt sends one A-MPDU, built in an aggregate
//!   buffer whose capacity is the backend's limit, so an aggregate the
//!   backend cannot send cannot be built.
//! - [`BeaconTimingClient`]: an interface's TSF and TBTT schedule; its TBTT
//!   events come through [`LowerMacEvents::tbtt`](super::LowerMacEvents::tbtt).
//! - [`MonitorControl`]: monitor reception, a radio-wide operation of the
//!   phase control.

use core::future::Future;

use oer_ieee80211_mac::tsf::{TsfDuration, TsfInstant};
use oer_ieee80211_port::{ClockError, Work};
use oer_radio_port::{Poisoned, PortResult, Seat};

use super::{
    AckHeader, Ieee80211Capabilities, LowerMacClient, LowerMacControl, PhaseRate, SizedFrame,
    SupportedBackoff, SupportedCoex, SupportedPower, Tid,
};
use crate::{AmpduCapabilities, Protection, TsfSample};

/// A rate an A-MPDU attempt is sent at: a phase rate of a format the
/// backend aggregates in ([`AmpduCapabilities::formats`]).
#[derive(Debug)]
pub struct AmpduRate<'p>(PhaseRate<'p>);

impl<'p> AmpduRate<'p> {
    /// The phase rate.
    pub const fn rate(&self) -> &PhaseRate<'p> {
        &self.0
    }
}

/// The aggregate an [`AmpduClient`] lends: memory for the subframes of one
/// A-MPDU of one traffic identifier at one rate, whose capacity is the
/// backend's limit ([`AmpduCapabilities::max_subframes`] and
/// [`AmpduCapabilities::max_length`]), so the backend never refuses a
/// submitted aggregate by its size. Dropped before a submission, it gives
/// its permit back and drops its bodies.
pub trait PhaseAmpduBuffer {
    /// The owner of a subframe's body.
    type Body;
    /// The backend's capabilities marker.
    type Caps: Ieee80211Capabilities;

    /// Append the subframe `frame`, a QoS data frame of the aggregate's
    /// TID, ending with `body`, which the buffer takes; its octets between
    /// the header and the body for the caller to write.
    ///
    /// # Errors
    ///
    /// The frame and its body, when the aggregate holds no further
    /// subframe of its length: the aggregate is full, and the caller sends
    /// it as it is.
    #[allow(clippy::type_complexity, reason = "the subframe given back")]
    fn push(
        &mut self,
        frame: SizedFrame<AckHeader, Self::Caps>,
        body: Option<Self::Body>,
    ) -> Result<&mut [u8], (SizedFrame<AckHeader, Self::Caps>, Option<Self::Body>)>;

    /// The subframes appended so far.
    fn subframes(&self) -> usize;
}

/// A Minimum MPDU Start Spacing, the IEEE encoding 0-7 of an HT
/// Capabilities A-MPDU Parameters field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MpduStartSpacing(u8);

impl MpduStartSpacing {
    /// `encoding`, when it is at most 7.
    pub const fn new(encoding: u8) -> Option<Self> {
        if encoding <= 7 {
            Some(Self(encoding))
        } else {
            None
        }
    }

    /// The IEEE encoding.
    pub const fn get(self) -> u8 {
        self.0
    }
}

/// One A-MPDU attempt of a running phase, answered by a BlockAck: the queue
/// it occupies, its aggregate and the checked values of its attempt.
pub struct PhaseAmpdu<'p, 'id: 'p, C: AmpduClient<'id> + 'p> {
    pub queue: C::Queue<'p>,
    pub aggregate: C::AmpduBuffer<'p>,
    pub protection: Protection,
    /// The installed pairwise key that protects every subframe, or none.
    pub key: Option<&'p C::PairwiseKey>,
    pub power: SupportedPower<C::Caps>,
    pub backoff: SupportedBackoff<C::Caps>,
    pub coex: SupportedCoex<C::Caps>,
    /// The recipient's Minimum MPDU Start Spacing.
    pub min_mpdu_start_spacing: MpduStartSpacing,
}

/// A-MPDU transmission: one attempt sends one aggregate and reports the
/// recipient's BlockAck in its completion's
/// [`block_ack`](crate::TxCompletion::block_ack); the completion carries
/// the bodies back by subframe. The attempt occupies its access category's
/// queue like a single MPDU; which MPDUs a later aggregate resends stays
/// above the port unless the backend reports
/// [`HardwareServices::AMPDU_RETRY_SELECTION`](crate::HardwareServices::AMPDU_RETRY_SELECTION).
pub trait AmpduClient<'id>: LowerMacClient<'id> {
    /// The permit kind of the backend's aggregate buffers.
    type AmpduKind;
    /// The aggregate buffer the client lends.
    type AmpduBuffer<'c>: PhaseAmpduBuffer<Body = Self::TxBody, Caps = Self::Caps>
    where
        Self: 'c,
        'id: 'c;

    /// The limits of the backend's aggregates.
    fn ampdu_capabilities(&self) -> AmpduCapabilities;

    /// The rate, when the backend aggregates in its format.
    ///
    /// # Errors
    ///
    /// The rate, when it does not.
    fn ampdu_rate<'p>(&self, rate: PhaseRate<'p>) -> Result<AmpduRate<'p>, PhaseRate<'p>> {
        if self.ampdu_capabilities().formats.contains_rate(rate.get()) {
            Ok(AmpduRate(rate))
        } else {
            Err(rate)
        }
    }

    /// An empty aggregate of `tid` at `rate`, once an aggregate buffer is
    /// free: the pool is waited for in its line with `seat`, never refused.
    fn ampdu_buffer<'c, 's>(
        &'c self,
        tid: Tid,
        rate: AmpduRate<'c>,
        seat: &'s mut Seat<'id, Self::AmpduKind>,
    ) -> impl Future<Output = Self::AmpduBuffer<'c>> + 's
    where
        'id: 'c,
        'c: 's;

    /// Admit one A-MPDU attempt; its permits stay with the backend until it
    /// takes the attempt's terminal.
    ///
    /// # Errors
    ///
    /// A poisoned backend, which keeps the attempt.
    fn submit_ampdu<'p>(
        &'p self,
        attempt: PhaseAmpdu<'p, 'id, Self>,
    ) -> Result<Work<'p, Self>, Poisoned<Self::Fault>>
    where
        'id: 'p;
}

/// One target beacon transmission time schedule of the client's interface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TbttPlan {
    /// A target beacon transmission time of the schedule, in the
    /// interface's TSF.
    pub next: TsfInstant,
    /// The beacon interval.
    pub beacon_interval: TsfDuration,
    /// How long before each TBTT its event is reported.
    pub lead: TsfDuration,
}

/// Beacon timing of the client's interface: its TSF, a paired reading of
/// it with the port's radio clock, and its TBTT schedule, whose events the
/// phase's event consumer reports
/// ([`LowerMacEvents::tbtt`](super::LowerMacEvents::tbtt)). Which roles
/// each operation serves is the backend's
/// [`BeaconTimingCapabilities`](crate::BeaconTimingCapabilities); a client
/// of a role it does not serve does not implement the trait.
pub trait BeaconTimingClient<'id>: LowerMacClient<'id> {
    /// The interface's TSF now.
    ///
    /// # Errors
    ///
    /// The clock could not be read now, or a poisoned backend.
    fn tsf(&self) -> PortResult<TsfInstant, ClockError, Self::Fault>;

    /// A paired reading of the interface's TSF and the radio clock.
    ///
    /// # Errors
    ///
    /// The clock could not be read now, or a poisoned backend.
    fn tsf_sample(&self) -> PortResult<TsfSample, ClockError, Self::Fault>;

    /// Set the interface's TSF: a station's follows its access point; an
    /// access point's restarts from zero.
    ///
    /// # Errors
    ///
    /// A poisoned backend.
    fn set_tsf(&self, at: TsfInstant) -> Result<(), Poisoned<Self::Fault>>;

    /// Report the TBTTs of `plan`, replacing any earlier schedule.
    ///
    /// # Errors
    ///
    /// A poisoned backend.
    fn set_tbtt(&self, plan: TbttPlan) -> Result<(), Poisoned<Self::Fault>>;

    /// Stop reporting TBTTs.
    ///
    /// # Errors
    ///
    /// A poisoned backend.
    fn stop_tbtt(&self) -> Result<(), Poisoned<Self::Fault>>;
}

/// Monitor reception of the phase: every frame with a valid FCS the
/// receiver hears, beside the interfaces' own receptions only where the
/// backend's [`MonitorCapabilities`](crate::MonitorCapabilities) say so.
pub trait MonitorControl: LowerMacControl {
    /// The cause a poisoned backend reports.
    type Fault: Copy + core::fmt::Debug;

    /// Start or stop monitor reception.
    ///
    /// # Errors
    ///
    /// A poisoned backend.
    fn set_monitor(&mut self, enabled: bool) -> Result<(), Poisoned<Self::Fault>>;
}
