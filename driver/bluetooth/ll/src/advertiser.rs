//! Affine lifecycle for the first portable LE advertising role.
//!
//! This layer owns protocol state, not an executor or hardware ticket. A chip
//! backend must retain [`LegacyAdvertiserEventInFlight`] together with its
//! sealed event owner and may report completion only after that exact hardware
//! transaction returns. The identity check prevents a stale event completion
//! from advancing newer protocol state.

use crate::advertising::{
    AdvertisingDelay, LegacyAdvertisingEncodeError, LegacyAdvertisingEventComplete,
    LegacyNonconnectableAdvertisingEvent, LegacyNonconnectableAdvertisingSet,
    LegacyPreparedAdvertisingEvent, PrimaryAdvertisingChannelMap, ScheduledLegacyAdvertisingEvent,
};
use crate::advertising_lifecycle::{
    LegacyAdvertisingEventIdentity, LegacyAdvertisingEventSequence, LegacyAdvertisingGeneration,
    LegacyAdvertisingGenerationAllocator,
};

/// Disabled portable advertiser retaining the next unique enable generation.
#[derive(Debug, Eq, PartialEq)]
pub struct LegacyAdvertiserStandby {
    generations: LegacyAdvertisingGenerationAllocator,
}

impl LegacyAdvertiserStandby {
    /// Construct a fresh advertiser lifecycle.
    pub const fn new() -> Self {
        Self {
            generations: LegacyAdvertisingGenerationAllocator::new(),
        }
    }

    /// Install a complete immutable configuration snapshot.
    pub const fn configure<'a>(
        self,
        set: LegacyNonconnectableAdvertisingSet<'a>,
    ) -> LegacyAdvertiserConfigured<'a> {
        LegacyAdvertiserConfigured { standby: self, set }
    }
}

impl Default for LegacyAdvertiserStandby {
    fn default() -> Self {
        Self::new()
    }
}

/// Disabled advertiser with a validated configuration available for enable.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "enable, reconfigure, or retain the advertiser"]
pub struct LegacyAdvertiserConfigured<'a> {
    standby: LegacyAdvertiserStandby,
    set: LegacyNonconnectableAdvertisingSet<'a>,
}

impl<'a> LegacyAdvertiserConfigured<'a> {
    /// Replace the disabled configuration without minting a new generation.
    pub const fn reconfigure(self, set: LegacyNonconnectableAdvertisingSet<'a>) -> Self {
        Self {
            standby: self.standby,
            set,
        }
    }

    /// Begin a fresh enable epoch and its first advertising event.
    pub fn enable(self) -> Result<LegacyAdvertiserEnabled<'a>, LegacyAdvertiserEnableError<'a>> {
        let LegacyAdvertiserConfigured { standby, set } = self;
        let (generations, identity) = match standby.generations.begin_enable() {
            Ok(allocated) => allocated,
            Err(generations) => {
                return Err(LegacyAdvertiserEnableError {
                    configured: LegacyAdvertiserConfigured {
                        standby: LegacyAdvertiserStandby { generations },
                        set,
                    },
                });
            }
        };
        Ok(LegacyAdvertiserEnabled {
            standby: LegacyAdvertiserStandby { generations },
            generation: identity.generation(),
            event_sequence: identity.event(),
            event: set.begin_event(),
        })
    }

    /// Remove the configuration and return to standby.
    pub fn into_standby(self) -> LegacyAdvertiserStandby {
        self.standby
    }
}

/// Generation space was exhausted before an enable epoch could begin.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the configured advertiser remains recoverable"]
pub struct LegacyAdvertiserEnableError<'a> {
    configured: LegacyAdvertiserConfigured<'a>,
}

impl<'a> LegacyAdvertiserEnableError<'a> {
    pub fn into_configured(self) -> LegacyAdvertiserConfigured<'a> {
        self.configured
    }
}

/// Enabled advertiser with no backend transmission in flight.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "prepare work, disable, or retain the enabled advertiser"]
pub struct LegacyAdvertiserEnabled<'a> {
    standby: LegacyAdvertiserStandby,
    generation: LegacyAdvertisingGeneration,
    event_sequence: LegacyAdvertisingEventSequence,
    event: LegacyNonconnectableAdvertisingEvent<'a>,
}

impl<'a> LegacyAdvertiserEnabled<'a> {
    pub const fn generation(&self) -> LegacyAdvertisingGeneration {
        self.generation
    }

    pub const fn event_sequence(&self) -> LegacyAdvertisingEventSequence {
        self.event_sequence
    }

    /// Prepare one complete primary-channel event while retaining its continuation.
    pub fn prepare_event(self) -> LegacyAdvertiserEventPrepared<'a> {
        let prepared = self.event.prepare();
        let identity = LegacyAdvertisingEventIdentity::new(self.generation, self.event_sequence);
        LegacyAdvertiserEventPrepared {
            standby: self.standby,
            identity,
            prepared,
        }
    }

    /// Disable immediately because no hardware transmission is in flight.
    pub fn disable(self) -> LegacyAdvertiserConfigured<'a> {
        LegacyAdvertiserConfigured {
            standby: self.standby,
            set: self.event.into_set(),
        }
    }
}

/// Prepared protocol work which has not yet been accepted by a backend.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "submit, cancel, disable, or retain the prepared transmission"]
pub struct LegacyAdvertiserEventPrepared<'a> {
    standby: LegacyAdvertiserStandby,
    identity: LegacyAdvertisingEventIdentity,
    prepared: LegacyPreparedAdvertisingEvent<'a>,
}

impl<'a> LegacyAdvertiserEventPrepared<'a> {
    pub const fn identity(&self) -> LegacyAdvertisingEventIdentity {
        self.identity
    }

    /// Complete ordered primary-channel selection for the hardware event.
    pub const fn channels(&self) -> PrimaryAdvertisingChannelMap {
        self.prepared.channels()
    }

    /// Encode the PDU without changing protocol state.
    pub fn encode(&self, destination: &mut [u8]) -> Result<usize, LegacyAdvertisingEncodeError> {
        self.prepared.encode(destination)
    }

    /// Return to the exact enabled event after lower admission was rejected.
    pub fn cancel(self) -> LegacyAdvertiserEnabled<'a> {
        LegacyAdvertiserEnabled {
            standby: self.standby,
            generation: self.identity.generation(),
            event_sequence: self.identity.event(),
            event: self.prepared.cancel(),
        }
    }

    /// Disable before hardware accepted the transmission.
    pub fn disable(self) -> LegacyAdvertiserConfigured<'a> {
        self.cancel().disable()
    }

    /// Mark the point where a lower backend accepted this exact work.
    ///
    /// Production chip code must keep this value private and pair it with its
    /// non-forgeable hardware ticket.
    pub fn into_submitted(self) -> LegacyAdvertiserEventInFlight<'a> {
        LegacyAdvertiserEventInFlight { prepared: self }
    }
}

/// Protocol continuation waiting for one exact backend TX completion.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "complete, request stop, or retain the in-flight transmission"]
pub struct LegacyAdvertiserEventInFlight<'a> {
    prepared: LegacyAdvertiserEventPrepared<'a>,
}

impl<'a> LegacyAdvertiserEventInFlight<'a> {
    pub const fn identity(&self) -> LegacyAdvertisingEventIdentity {
        self.prepared.identity
    }

    /// Advance only if the backend returned the exact generation/event identity.
    pub fn complete(
        self,
        observed: LegacyAdvertisingEventIdentity,
    ) -> LegacyAdvertiserEventCompletion<'a> {
        let expected = self.identity();
        if observed != expected {
            return LegacyAdvertiserEventCompletion::Mismatch {
                error: LegacyAdvertisingEventCompletionMismatch { expected, observed },
                in_flight: self,
            };
        }

        LegacyAdvertiserEventCompletion::Completed(self.complete_exact())
    }

    /// Advance after an affine backend owner proves this exact event completed.
    ///
    /// This transition records consumption of every scheduled channel item,
    /// not an assertion that RF energy reached the air. A chip backend must
    /// keep its completion statuses alongside the returned continuation.
    pub fn complete_exact(self) -> LegacyAdvertiserEventComplete<'a> {
        let LegacyAdvertiserEventPrepared {
            standby,
            identity,
            prepared,
        } = self.prepared;
        LegacyAdvertiserEventComplete {
            standby,
            generation: identity.generation(),
            event_sequence: identity.event(),
            complete: prepared.into_event_completed(),
        }
    }

    /// Request disable while hardware still owns the submitted transaction.
    pub fn request_disable(self) -> LegacyAdvertiserStopping<'a> {
        LegacyAdvertiserStopping { in_flight: self }
    }
}

/// Result of matching one TX-completion observation.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "retain a mismatch or continue the exact advertiser state"]
pub enum LegacyAdvertiserEventCompletion<'a> {
    Completed(LegacyAdvertiserEventComplete<'a>),
    Mismatch {
        error: LegacyAdvertisingEventCompletionMismatch,
        in_flight: LegacyAdvertiserEventInFlight<'a>,
    },
}

/// Expected and observed identities for a stale or cross-wired completion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyAdvertisingEventCompletionMismatch {
    pub expected: LegacyAdvertisingEventIdentity,
    pub observed: LegacyAdvertisingEventIdentity,
}

/// Closed event awaiting a fresh random delay before its successor is armed.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "schedule the next event, disable, or retain the completed event"]
pub struct LegacyAdvertiserEventComplete<'a> {
    standby: LegacyAdvertiserStandby,
    generation: LegacyAdvertisingGeneration,
    event_sequence: LegacyAdvertisingEventSequence,
    complete: LegacyAdvertisingEventComplete<'a>,
}

impl<'a> LegacyAdvertiserEventComplete<'a> {
    /// Pair a fresh delay with the next event without waiting in this crate.
    pub fn schedule_next(
        self,
        delay: AdvertisingDelay,
    ) -> Result<LegacyAdvertiserScheduled<'a>, LegacyAdvertisingEventSequenceExhausted<'a>> {
        let identity = LegacyAdvertisingEventIdentity::new(self.generation, self.event_sequence);
        let Some(next_identity) = identity.next_event() else {
            return Err(LegacyAdvertisingEventSequenceExhausted { complete: self });
        };
        Ok(LegacyAdvertiserScheduled {
            standby: self.standby,
            generation: next_identity.generation(),
            event_sequence: next_identity.event(),
            scheduled: self.complete.schedule_next(delay),
        })
    }

    /// Disable between events while no timer or TX is owned by a backend.
    pub fn disable(self) -> LegacyAdvertiserConfigured<'a> {
        LegacyAdvertiserConfigured {
            standby: self.standby,
            set: self.complete.into_set(),
        }
    }
}

/// Event-sequence space was exhausted without losing the completed owner.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the completed advertiser owner remains recoverable"]
pub struct LegacyAdvertisingEventSequenceExhausted<'a> {
    complete: LegacyAdvertiserEventComplete<'a>,
}

impl<'a> LegacyAdvertisingEventSequenceExhausted<'a> {
    pub fn into_complete(self) -> LegacyAdvertiserEventComplete<'a> {
        self.complete
    }
}

/// Next event carrying its relative schedule before backend admission.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "wait, disable, or retain the scheduled event"]
pub struct LegacyAdvertiserScheduled<'a> {
    standby: LegacyAdvertiserStandby,
    generation: LegacyAdvertisingGeneration,
    event_sequence: LegacyAdvertisingEventSequence,
    scheduled: ScheduledLegacyAdvertisingEvent<'a>,
}

impl<'a> LegacyAdvertiserScheduled<'a> {
    pub const fn generation(&self) -> LegacyAdvertisingGeneration {
        self.generation
    }

    pub const fn event_sequence(&self) -> LegacyAdvertisingEventSequence {
        self.event_sequence
    }

    pub const fn start_offset_micros(&self) -> u64 {
        self.scheduled.start_offset_micros()
    }

    /// Transfer the scheduled event to a backend which will own its deadline.
    pub fn into_event(self) -> LegacyAdvertiserEnabled<'a> {
        LegacyAdvertiserEnabled {
            standby: self.standby,
            generation: self.generation,
            event_sequence: self.event_sequence,
            event: self.scheduled.into_event(),
        }
    }

    /// Disable while the deadline has not been published to a backend.
    pub fn disable(self) -> LegacyAdvertiserConfigured<'a> {
        LegacyAdvertiserConfigured {
            standby: self.standby,
            set: self.scheduled.cancel(),
        }
    }
}

/// Disable requested after a backend already accepted the exact TX.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "join the exact TX completion before returning to configured"]
pub struct LegacyAdvertiserStopping<'a> {
    in_flight: LegacyAdvertiserEventInFlight<'a>,
}

impl<'a> LegacyAdvertiserStopping<'a> {
    pub const fn identity(&self) -> LegacyAdvertisingEventIdentity {
        self.in_flight.identity()
    }

    /// Join the accepted hardware event without starting its successor.
    pub fn complete(
        self,
        observed: LegacyAdvertisingEventIdentity,
    ) -> LegacyAdvertiserStopCompletion<'a> {
        let expected = self.identity();
        if observed != expected {
            return LegacyAdvertiserStopCompletion::Mismatch {
                error: LegacyAdvertisingEventCompletionMismatch { expected, observed },
                stopping: self,
            };
        }

        let LegacyAdvertiserEventPrepared {
            standby, prepared, ..
        } = self.in_flight.prepared;
        let set = prepared.into_event_completed().into_set();
        LegacyAdvertiserStopCompletion::Configured(LegacyAdvertiserConfigured { standby, set })
    }
}

/// Result of joining an in-flight TX during disable.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "retain a mismatch or the recovered configured advertiser"]
pub enum LegacyAdvertiserStopCompletion<'a> {
    Configured(LegacyAdvertiserConfigured<'a>),
    Mismatch {
        error: LegacyAdvertisingEventCompletionMismatch,
        stopping: LegacyAdvertiserStopping<'a>,
    },
}

#[cfg(test)]
mod tests {
    use crate::{LeDeviceAddress, LeDeviceAddressKind};

    use super::*;
    use crate::advertising::{
        AdvertisingInterval, LegacyAdvertisingData, LegacyNonconnectableAdvertisement,
        PrimaryAdvertisingChannelMap,
    };

    fn set<'a>(
        data: &'a [u8],
        channels: PrimaryAdvertisingChannelMap,
    ) -> LegacyNonconnectableAdvertisingSet<'a> {
        LegacyNonconnectableAdvertisingSet::new(
            LegacyNonconnectableAdvertisement::new(
                LeDeviceAddress::from_wire_bytes([6, 5, 4, 3, 2, 1], LeDeviceAddressKind::Public),
                LegacyAdvertisingData::new(data).unwrap(),
            ),
            channels,
            AdvertisingInterval::new(AdvertisingInterval::MIN_UNITS).unwrap(),
        )
    }

    #[test]
    fn rejected_admission_returns_the_same_generation_event_and_channel_plan() {
        let enabled = LegacyAdvertiserStandby::new()
            .configure(set(&[1], PrimaryAdvertisingChannelMap::all()))
            .enable()
            .unwrap();
        let prepared = enabled.prepare_event();
        let identity = prepared.identity();

        assert_eq!(identity.generation().get(), 1);
        assert_eq!(identity.event().get(), 0);
        assert_eq!(prepared.channels(), PrimaryAdvertisingChannelMap::all());
        assert_eq!(prepared.cancel().prepare_event().identity(), identity);
    }

    #[test]
    fn stale_completion_retains_in_flight_owner_and_exact_progress() {
        let in_flight = LegacyAdvertiserStandby::new()
            .configure(set(&[1], PrimaryAdvertisingChannelMap::all()))
            .enable()
            .unwrap()
            .prepare_event()
            .into_submitted();
        let expected = in_flight.identity();
        let stale = LegacyAdvertisingEventIdentity::from_parts(expected.generation().get(), 1);
        let LegacyAdvertiserEventCompletion::Mismatch { error, in_flight } =
            in_flight.complete(stale)
        else {
            panic!("stale completion must retain the in-flight owner");
        };
        assert_eq!(error.expected, expected);
        assert_eq!(error.observed, stale);
        let complete = in_flight.complete_exact();
        assert_eq!(complete.event_sequence.get(), 0);
    }

    #[test]
    fn completed_event_requires_fresh_delay_and_advances_event_identity() {
        let in_flight = LegacyAdvertiserStandby::new()
            .configure(set(
                &[1, 2],
                PrimaryAdvertisingChannelMap::new(false, false, true).unwrap(),
            ))
            .enable()
            .unwrap()
            .prepare_event()
            .into_submitted();
        let identity = in_flight.identity();
        let LegacyAdvertiserEventCompletion::Completed(complete) = in_flight.complete(identity)
        else {
            panic!("the exact hardware event must close the portable event");
        };

        let scheduled = complete
            .schedule_next(AdvertisingDelay::from_micros(7_500).unwrap())
            .unwrap();
        assert_eq!(scheduled.generation().get(), 1);
        assert_eq!(scheduled.event_sequence().get(), 1);
        assert_eq!(scheduled.start_offset_micros(), 27_500);
        let next = scheduled.into_event().prepare_event();
        assert_eq!(next.identity().event().get(), 1);
        assert_eq!(
            next.channels(),
            PrimaryAdvertisingChannelMap::new(false, false, true).unwrap()
        );
    }

    #[test]
    fn disable_during_in_flight_joins_exact_tx_and_mints_next_generation() {
        let stopping = LegacyAdvertiserStandby::new()
            .configure(set(&[1], PrimaryAdvertisingChannelMap::all()))
            .enable()
            .unwrap()
            .prepare_event()
            .into_submitted()
            .request_disable();
        let expected = stopping.identity();
        let stale = LegacyAdvertisingEventIdentity::from_parts(2, expected.event().get());
        let LegacyAdvertiserStopCompletion::Mismatch { error, stopping } = stopping.complete(stale)
        else {
            panic!("cross-generation completion must retain stopping");
        };
        assert_eq!(error.expected, expected);

        let LegacyAdvertiserStopCompletion::Configured(configured) = stopping.complete(expected)
        else {
            panic!("exact completion must close stopping");
        };
        assert_eq!(configured.enable().unwrap().generation().get(), 2);
    }

    #[test]
    fn generation_and_event_sequence_exhaustion_retain_their_owners() {
        let standby = LegacyAdvertiserStandby {
            generations: LegacyAdvertisingGenerationAllocator::from_next_generation(Some(u32::MAX)),
        };
        let enabled = standby
            .configure(set(&[], PrimaryAdvertisingChannelMap::all()))
            .enable()
            .unwrap();
        assert_eq!(enabled.generation().get(), u32::MAX);
        let configured = enabled.disable();
        assert!(configured.enable().is_err());

        let in_flight = LegacyAdvertiserStandby::new()
            .configure(set(
                &[],
                PrimaryAdvertisingChannelMap::new(true, false, false).unwrap(),
            ))
            .enable()
            .unwrap()
            .prepare_event()
            .into_submitted();
        let identity = in_flight.identity();
        let LegacyAdvertiserEventCompletion::Completed(mut complete) = in_flight.complete(identity)
        else {
            panic!("the complete hardware event must close the portable event");
        };
        complete.event_sequence = LegacyAdvertisingEventIdentity::from_parts(1, u32::MAX).event();
        let exhausted = complete
            .schedule_next(AdvertisingDelay::from_micros(0).unwrap())
            .unwrap_err();
        let configured = exhausted.into_complete().disable();
        assert_eq!(configured.enable().unwrap().generation().get(), 2);
    }
}
