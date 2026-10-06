//! A protected receive-only visit away from the port's operating channel.
//!
//! The owner stops polling its transmitting clients and completes every
//! exchange already started before beginning the visit. They stay paused
//! until the visit ends. The router continues receiving events. Background
//! scan and channel selection can use this same primitive.
//! CTS-to-self and live retuning are independent backend extensions:
//! [`LowerMacAirReservation`] and [`LowerMacLiveRetune`]. The base channel
//! setting cannot substitute for the latter's preservation guarantees.

use oer_ieee80211_lower_mac::{
    Channel, CoexPriority, LowerMacAirReservation, LowerMacLiveRetune, PhyRate, TxStatus,
};
use oer_time::{Instant, Timer};

use crate::client::{PortClient, PortClientEnv, PortClientError, PortError};

/// The owner keeps this state across receive futures. A failed return
/// home, including during cancellation, prevents clients from resuming
/// until the port is recovered and a new absence state is installed.
#[derive(Default)]
pub struct AbsenceState {
    recovery_required: bool,
}

impl AbsenceState {
    pub const fn new() -> Self {
        Self {
            recovery_required: false,
        }
    }

    pub const fn recovery_required(&self) -> bool {
        self.recovery_required
    }
}

/// A receive-only visit, on the owner's monotonic clock.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AbsenceWindow {
    pub home: Channel,
    pub channel: Channel,
    pub start: Instant,
    pub until: Instant,
}

/// Why a protected visit could not be started or ended.
#[derive(Debug, Eq, PartialEq)]
pub enum AbsenceError<E> {
    Client(PortClientError<E>),
    /// CTS-to-self did not go out successfully: the radio stays home.
    Reservation(TxStatus),
    /// The window is empty or names the operating channel.
    InvalidWindow,
    /// Returning home failed in an earlier visit, possibly when dropped.
    /// The owner must recover the port before polling its clients again.
    RecoveryRequired,
}

/// The port is away, with all transmitting clients paused by its owner.
///
/// The visit must transmit nothing: neither this guard nor its receive
/// operation may leave an attempt in flight. [`Self::finish`] returns the
/// radio home and reports a setting failure. Dropping a receive future also
/// returns it home synchronously; a terminal port fault still requires the
/// owner's recovery. This does not make an in-flight TX future cancellable.
pub struct PortAbsence<'s, 'p, P: LowerMacLiveRetune> {
    port: &'p P,
    state: &'s mut AbsenceState,
    home: Channel,
    away: bool,
}

impl<'s, 'p, P: LowerMacLiveRetune> PortAbsence<'s, 'p, P> {
    /// Reserve the operating channel's air with CTS-to-self, then retune.
    /// `None` means the absolute window expired while still at home. A late
    /// CTS never shifts the window's end. The reservation covers the time
    /// remaining when requested, so contention can only extend its NAV
    /// beyond `until`, never shorten the protection of the visit.
    ///
    /// Every transmitting client must already be at a sync point. Keep
    /// them paused and poll only reception until `window.until`.
    pub async fn begin<X: PortClientEnv<Port = P>, const EXCHANGES: usize, const RX: usize>(
        client: &mut PortClient<'p, X, EXCHANGES, RX>,
        timer: &impl Timer,
        state: &'s mut AbsenceState,
        window: AbsenceWindow,
        rate: PhyRate,
        coex: CoexPriority,
    ) -> Result<Option<Self>, AbsenceError<PortError<X>>>
    where
        P: LowerMacAirReservation,
    {
        if state.recovery_required() {
            return Err(AbsenceError::RecoveryRequired);
        }
        if window.until <= window.start || window.channel == window.home {
            return Err(AbsenceError::InvalidWindow);
        }
        timer.wait_until(window.start).await;
        let now = timer.now();
        if now >= window.until {
            return Ok(None);
        }
        let completion = client
            .reserve_air(window.until.saturating_duration_since(now), rate, coex)
            .await
            .map_err(AbsenceError::Client)?;
        if completion.status != TxStatus::Success {
            return Err(AbsenceError::Reservation(completion.status));
        }
        if timer.now() >= window.until {
            return Ok(None);
        }
        let port = client.port();
        port.retune_live(window.channel)
            .map_err(|error| AbsenceError::Client(PortClientError::Port(error)))?
            .map_err(|error| AbsenceError::Client(PortClientError::Setting(error)))?;
        Ok(Some(Self {
            port,
            state,
            home: window.home,
            away: true,
        }))
    }

    /// Return home before resuming any transmitting client.
    pub fn finish(mut self) -> Result<(), AbsenceError<P::Error>> {
        self.away = false;
        let restored = self
            .port
            .retune_live(self.home)
            .map_err(|error| AbsenceError::Client(PortClientError::Port(error)))
            .and_then(|result| {
                result.map_err(|error| AbsenceError::Client(PortClientError::Setting(error)))
            });
        self.state.recovery_required = restored.is_err();
        restored
    }
}

impl<P: LowerMacLiveRetune> Drop for PortAbsence<'_, '_, P> {
    fn drop(&mut self) {
        if self.away {
            // Drop cannot return an error. Keep the failure in the owner's
            // state rather than let it resume clients on an unknown channel.
            self.state.recovery_required = !matches!(self.port.retune_live(self.home), Ok(Ok(())),);
        }
    }
}
