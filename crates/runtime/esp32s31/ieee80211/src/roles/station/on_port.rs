//! The ESP32-S31 station on the lower-MAC port (#563 step 2).
//!
//! One port body runs the station for the whole life of a radio start:
//! the portable station drivers of `oer-ieee80211-sta-service` (the
//! candidate scan, the join runner, the attempt and the lifecycle) run over
//! the port through the portable ports this module implements
//! ([`StaScanPort`](oer_ieee80211_sta::scan::StaScanPort) here), and the
//! S31 station's own policy (coexistence, power save, the connected
//! control) runs above them as before. Nothing in this module reaches the
//! hardware: it holds the port's [`StationOwner`] and the install's
//! resources, and a channel move is the owner's `disable`, a channel setup
//! and `enable`.

use core::{
    future::Future,
    pin::pin,
    task::{Context, Poll, Waker},
};

use oer_ieee80211_lower_mac::{LowerMacBackend, LowerMacEvent, StationClient, SupportedChannel};
use oer_ieee80211_port::{Disabled, Enabled, Failed, Owner, PhaseParts, PoisonedOwner};
use oer_radio_port::{EventsLost, Poisoned};

pub mod scan;

/// The client of the station interface a backend lends in each phase.
pub trait StationParts: LowerMacBackend {
    /// The permit kind of the backend's transmit queues.
    type QueueKind;
    /// The permit kind of the backend's transmit buffers.
    type BufferKind;
    /// The station's client.
    type Station<'a, 'id>: StationClient<
            'id,
            Caps = Self::Caps,
            Fault = Self::Fault,
            QueueKind = Self::QueueKind,
            BufferKind = Self::BufferKind,
        >
    where
        Self: 'a,
        'id: 'a;

    /// The station's client among a phase's clients.
    fn station<'b, 'a, 'id>(clients: &'b Self::Clients<'a, 'id>) -> &'b Self::Station<'a, 'id>
    where
        Self: 'a,
        'id: 'a;

    /// Tune the next `Enable` to `channel` through the disabled-only
    /// settings.
    fn set_channel<'a, 'id>(
        setup: &mut Self::Setup<'a, 'id>,
        channel: SupportedChannel<Self::Caps>,
    ) where
        Self: 'a;

    /// The portable view of an owned event.
    fn view(event: &Self::Event) -> LowerMacEvent<'_>;
}

/// Why the station lost its port: the backend is poisoned, or the radio
/// refused the retune a channel move needs (the port stays disabled).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PortLost<F, R> {
    Poisoned(F),
    Refused(R),
}

/// The station's authority over its port, in whichever state the last
/// transition left it.
pub enum StationOwner<'p, 'id, B: LowerMacBackend> {
    Disabled(Owner<'p, 'id, B, Disabled>),
    Enabled(Owner<'p, 'id, B, Enabled>),
    /// A poisoned backend: only its uninstall or a fail-stop remain.
    Poisoned(PoisonedOwner<'p, 'id, B>),
    /// Between a transition's start and end; never observed outside one.
    Moving,
}

impl<'p, 'id, B: StationParts> StationOwner<'p, 'id, B> {
    /// Tune the port to `channel` and enable it: a running port is disabled
    /// first, its leftover terminals given to `sink`.
    ///
    /// # Errors
    ///
    /// A poisoned backend, or the radio's refusal of the retune (the port is
    /// disabled on the channel it was tuned to).
    pub async fn tune(
        &mut self,
        channel: SupportedChannel<B::Caps>,
        sink: &mut dyn FnMut(B::Event),
    ) -> Result<(), PortLost<B::Fault, B::Refusal>> {
        let mut disabled = match core::mem::replace(self, Self::Moving) {
            Self::Disabled(owner) => owner,
            Self::Enabled(owner) => match owner.disable(sink).await {
                Ok(owner) => owner,
                Err(poisoned) => {
                    let cause = poisoned.cause();
                    *self = Self::Poisoned(poisoned);
                    return Err(PortLost::Poisoned(cause));
                }
            },
            Self::Poisoned(poisoned) => {
                let cause = poisoned.cause();
                *self = Self::Poisoned(poisoned);
                return Err(PortLost::Poisoned(cause));
            }
            Self::Moving => unreachable!("a transition ends before the next starts"),
        };
        B::set_channel(&mut *disabled.setup(), channel);
        match disabled.enable().await {
            Ok(enabled) => {
                *self = Self::Enabled(enabled);
                Ok(())
            }
            Err(Failed::Refused(owner, refusal)) => {
                *self = Self::Disabled(owner);
                Err(PortLost::Refused(refusal))
            }
            Err(Failed::Poisoned(poisoned)) => {
                let cause = poisoned.cause();
                *self = Self::Poisoned(poisoned);
                Err(PortLost::Poisoned(cause))
            }
        }
    }

    /// The running phase's parts, when the port is enabled.
    #[allow(clippy::type_complexity, reason = "the phase's three parts")]
    pub fn phase(
        &mut self,
    ) -> Option<PhaseParts<'_, B::Events<'_, 'id>, B::Control<'_, 'id>, B::Clients<'_, 'id>>> {
        match self {
            Self::Enabled(owner) => Some(owner.split()),
            _ => None,
        }
    }
}

/// The next event a phase's consumer holds now, without waiting: `None`
/// when none is queued. Dropping the future of
/// [`next_event`](oer_ieee80211_port::RadioPortEvents::next_event) loses no
/// event, so one poll is a non-blocking take.
pub fn try_next_event<E: oer_ieee80211_port::RadioPortEvents>(
    events: &mut E,
) -> Option<Result<Result<E::Event, EventsLost>, Poisoned<E::Fault>>> {
    let mut next = pin!(events.next_event());
    match next.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(event) => Some(event),
        Poll::Pending => None,
    }
}
