#![no_std]
#![forbid(unsafe_code)]

//! The arbiter of the channel one radio's station and access point share:
//! the third sans-IO machine of their loop ([owner decision on #563]).
//!
//! Only the arbiter changes the port's channel: it closes the transmit
//! gate, retunes the port and opens the gate again, one gate's cycle at a
//! time. The station asks it for a channel and holds the [`Lease`] it
//! answers with; the access point tells it its schedule and follows its
//! commands. A station alone has an arbiter of the same shape, which
//! leases at once.
//!
//! - **The answer is a return value.** [`Arbiter::request`] answers in the
//!   same step, with a lease or a command to leave, so no request goes
//!   unanswered. Only a lease's start is later: its moment `from` is the
//!   arbiter's own deadline.
//! - **One lease at a time, by type.** A request takes the station's
//!   [`LeasePlace`], which comes back with the lease's end or cancellation,
//!   so a station never holds two leases. [`StationNotice`]s name the
//!   lease, so a notice of an earlier lease is never taken for the
//!   current one, and every lease ends in exactly one of
//!   [`StationNotice::LeaseEnded`] or [`StationNotice::LeaseCancelled`].
//! - **Readiness.** A station that must tell its access point it leaves (a
//!   Null with PM=1) asks with [`ChannelRequest::announces_absence`]; the
//!   lease then starts a preparation window later, and only once the
//!   station said [`Arbiter::ready`]. A lease not ready at its start is
//!   cancelled ([`CancelReason::NotReady`]); the station has no timer of
//!   its own for it.
//! - **Order.** The gate closes before every move of the port, so every
//!   terminal of the station's attempts comes before the lease's start and
//!   before its end.
//! - **Time.** The gate starts closing at the lease's `from`, and the lease
//!   starts once it closed and the port moved. Its `until` is the moment
//!   the port is back on the access point's channel: the return starts a
//!   retune earlier, and the station listens only until then
//!   ([`Lease::listen_until`]). A lease whose gate closes too late for two
//!   retunes and the policy's shortest listening before `until` is
//!   cancelled ([`CancelReason::Late`]) ([amendments]).
//! - **Beside an access point**, the station leaves the access point's
//!   channel only in absences after the access point's TBTTs. If the
//!   access point stops during an absence, the lease is cancelled
//!   ([`CancelReason::AccessPointStopped`]) and the port returns.
//! - **A refused move** onto a lease's channel cancels that lease
//!   ([`CancelReason::Refused`]) and the port returns; a failed return, or
//!   a poisoned port, stops the station and the access point
//!   ([`LeaveReason::PortFailed`]).
//! - **The air reservation** is the arbiter's type parameter
//!   ([`ReservationMode`]): an arbiter that reserves the access point's air
//!   for an absence closes the gate with one last frame, a CTS-to-self the
//!   driver sends with the Duration left, and only a phase that sends it
//!   ([`AirReservationPhase`](oer_ieee80211_lower_mac::AirReservationPhase))
//!   closes its gates ([`CloseGateWith`], [reservation decision]).
//! - **The upstream** (the access point the station is connected to) is
//!   the [`coordinator`]'s: the station reports its switch announcements,
//!   its loss and where it found it again ([`UpstreamFact`]); the arbiter
//!   moves the port, announces the access point's own switch, stops and
//!   starts the access point, and tells the station where the port went
//!   ([`StationNotice::Moved`]) or that it must leave.
//!
//! # Compile-fail tests
//!
//! A phase without the air reservation closes no gate of a reserving
//! arbiter:
//!
//! ```compile_fail,E0277
//! # use oer_ieee80211_arbiter::{AirReservation, CloseGateWith};
//! # use oer_ieee80211_lower_mac::{GateOpen, Ieee80211Instant, LowerMacPhase};
//! fn close<'p, 'id: 'p, P: LowerMacPhase<'p, 'id>>(phase: &mut P, gate: GateOpen<'id>) {
//!     CloseGateWith::<'p, 'id, AirReservation>::close_gate_with(
//!         phase,
//!         gate,
//!         None,
//!         |_| -> Ieee80211Instant { unreachable!() },
//!     );
//! }
//! ```
//!
//! Its compiling twins: such a phase closes the gates of an arbiter without
//! the reservation, and a phase with it those of a reserving one.
//!
//! ```
//! # use oer_ieee80211_arbiter::{AirReservation, CloseGateWith, NoReservation};
//! # use oer_ieee80211_lower_mac::{AirReservationPhase, GateOpen, Ieee80211Instant, LowerMacPhase};
//! fn plain<'p, 'id: 'p, P: LowerMacPhase<'p, 'id>>(phase: &mut P, gate: GateOpen<'id>) {
//!     CloseGateWith::<'p, 'id, NoReservation>::close_gate_with(
//!         phase,
//!         gate,
//!         (),
//!         |_| -> Ieee80211Instant { unreachable!() },
//!     );
//! }
//! fn reserving<'p, 'id: 'p, P: AirReservationPhase<'p, 'id>>(phase: &mut P, gate: GateOpen<'id>) {
//!     CloseGateWith::<'p, 'id, AirReservation>::close_gate_with(
//!         phase,
//!         gate,
//!         None,
//!         |_| -> Ieee80211Instant { unreachable!() },
//!     );
//! }
//! ```
//!
//! No caller makes a lease place, so no station holds two leases:
//!
//! ```compile_fail,E0451
//! let _ = oer_ieee80211_arbiter::LeasePlace { private: () };
//! ```
//!
//! [owner decision on #563]: https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6129596602
//! [amendments]: https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6129677418
//! [reservation decision]: https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6129750091

#[cfg(test)]
extern crate std;

pub mod channel;
pub mod coordinator;

use core::fmt::Debug;

use oer_ieee80211_lower_mac::{
    AirReservationPhase, GateOpen, Ieee80211Capabilities, Ieee80211Instant, LowerMacPhase,
    SupportedChannel,
};
use oer_ieee80211_mac::channel_switch::ChannelSwitchMode;
use oer_time::{Duration, Instant};

use coordinator::{
    ApFollowPolicy, ApSchedule, ApSearchPolicy, ChannelCoordinator, CoordinatorAction,
    CoordinatorActions, UpstreamLossPolicy,
};

/// The longest air reservation: the Duration field's limit, in
/// microseconds.
pub const MAX_RESERVATION: Duration = Duration::from_micros(32_767);

/// The name of one lease.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeaseId(u32);

/// The station's right to hold one lease: a request takes it, and the
/// lease's end or cancellation gives it back. Only the arbiter makes one.
#[derive(Debug, Eq, PartialEq)]
pub struct LeasePlace {
    private: (),
}

/// The port's channel, leased to the station from `from` until `until`
/// (`None`: until the station gives it back). Only the arbiter makes one,
/// and the station gives it back by value.
#[derive(Debug)]
#[must_use = "a lease goes back through `Arbiter::release`, or ends by its notice"]
pub struct Lease<K> {
    id: LeaseId,
    channel: SupportedChannel<K>,
    from: Instant,
    until: Option<Instant>,
    listen_until: Option<Instant>,
}

impl<K> Lease<K> {
    /// Its name.
    pub const fn id(&self) -> LeaseId {
        self.id
    }

    /// The channel it leases.
    pub const fn channel(&self) -> SupportedChannel<K> {
        self.channel
    }

    /// When it starts, at the earliest.
    pub const fn from(&self) -> Instant {
        self.from
    }

    /// When it ends: the port is back on the access point's channel then;
    /// `None` until it is given back.
    pub const fn until(&self) -> Option<Instant> {
        self.until
    }

    /// Until when the station may listen on the lease's channel: its end,
    /// less the return's retune.
    pub const fn listen_until(&self) -> Option<Instant> {
        self.listen_until
    }
}

/// What the station wants a channel for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Purpose {
    /// Listen for access points for `dwell`.
    Scan {
        /// How long to listen.
        dwell: Duration,
    },
    /// Join an access point on the port's channel, and stay.
    Connect,
    /// Look for the lost upstream for `dwell`.
    Search {
        /// How long to listen.
        dwell: Duration,
    },
}

/// A station's request for a channel.
#[derive(Debug)]
pub struct ChannelRequest<K> {
    /// The channel.
    pub channel: SupportedChannel<K>,
    /// What for.
    pub purpose: Purpose,
    /// The station is connected and tells its access point it leaves
    /// before the lease starts: the lease waits a preparation window and
    /// the station's [`Arbiter::ready`].
    pub announces_absence: bool,
}

/// The arbiter's answer to a request.
#[derive(Debug)]
pub enum Answer<K> {
    /// The channel, leased; the arbiter keeps the place until the lease
    /// ends.
    Lease(Lease<K>),
    /// The station must leave its network, or may not have the channel;
    /// the place comes back.
    Leave(LeaveReason, LeasePlace),
}

/// Why the station must leave its network, or may not join on a channel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeaveReason {
    /// The upstream moved where the access point may not follow, and the
    /// policy keeps the access point.
    Unservable,
    /// A join away from the running access point's channel: the station
    /// reports where it found its upstream ([`UpstreamFact::Found`]), and
    /// the port moves there first.
    AwayFromAccessPoint,
    /// The port could not return to the access point's channel, or it is
    /// poisoned: the station and the access point stop.
    PortFailed,
}

/// Why a lease ended without its own end.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CancelReason {
    /// The station was not ready at the lease's start.
    NotReady,
    /// The station gave it back before its start.
    Released,
    /// The upstream announced a switch.
    Switch,
    /// The upstream was lost.
    UpstreamLost,
    /// The access point stopped; the port returned to its channel.
    AccessPointStopped,
    /// The port refused the move onto the lease's channel; it returns to
    /// the access point's.
    Refused,
    /// The gate closed too late for the lease.
    Late,
    /// The port failed.
    PortFailed,
}

/// What the arbiter tells the station.
#[derive(Debug, Eq, PartialEq)]
pub enum StationNotice<K> {
    /// The lease started: the port runs on its channel.
    LeaseStarted(LeaseId),
    /// The lease ended: the port left its channel; the place comes back.
    LeaseEnded(LeaseId, LeasePlace),
    /// The lease ended without its own end; the place comes back.
    LeaseCancelled(LeaseId, CancelReason, LeasePlace),
    /// The port moved without the station's request.
    Moved(SupportedChannel<K>),
    /// The station must leave its network.
    Leave(LeaveReason),
}

/// What the arbiter tells the access point.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessPointCommand<K> {
    /// Announce a switch to `target` in the next `count` beacons.
    AnnounceSwitch {
        /// The channel.
        target: SupportedChannel<K>,
        /// What its peers do until the switch.
        mode: ChannelSwitchMode,
        /// The beacons left.
        count: u8,
    },
    /// Start on the channel.
    Start(SupportedChannel<K>),
    /// Stop: release the peers and close the BSS.
    Stop,
}

/// The access point's facts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessPointFact<K> {
    /// It runs on `channel`, its next TBTT at `next_tbtt`, one every
    /// `beacon_interval`; repeated at each TBTT.
    Running {
        /// Its channel.
        channel: SupportedChannel<K>,
        /// Its next TBTT.
        next_tbtt: Instant,
        /// Its beacon interval.
        beacon_interval: Duration,
    },
    /// It stopped.
    Stopped,
    /// Its announced switch to the channel is due.
    SwitchDue(SupportedChannel<K>),
}

/// What the station reports of its upstream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UpstreamFact<K> {
    /// It joined its upstream on the channel.
    Connected(SupportedChannel<K>),
    /// Its upstream announced a switch to `target` at `at`.
    SwitchAnnounced {
        /// The channel.
        target: SupportedChannel<K>,
        /// What the station does until the switch.
        mode: ChannelSwitchMode,
        /// When the upstream switches.
        at: Instant,
    },
    /// It lost its upstream without an announcement.
    Lost,
    /// It heard its lost upstream on the channel.
    Found(SupportedChannel<K>),
    /// Its scan, alone on the port, found no upstream.
    NotFound,
}

/// The tokens of the port's transmit gate the arbiter holds.
pub trait GateTokens {
    /// The open gate.
    type Open;
    /// The closed gate.
    type Closed;
}

/// Whether an arbiter reserves the access point's air for an absence: its
/// type parameter, so only a phase that sends the reservation closes its
/// gates ([`CloseGateWith`]).
pub trait ReservationMode {
    /// What a gate's closing carries: the end of the reservation, or
    /// nothing.
    type Reserve: Copy + Debug + Eq;

    /// What a gate's closing carries for an absence that ends at `until`,
    /// `length` long.
    fn reserve(until: Instant, length: Duration) -> Self::Reserve;

    /// What a gate's closing that leaves for no absence carries.
    fn none() -> Self::Reserve;
}

/// An arbiter that reserves no air.
pub enum NoReservation {}

impl ReservationMode for NoReservation {
    type Reserve = ();

    fn reserve(_: Instant, _: Duration) {}

    fn none() {}
}

/// An arbiter that reserves the access point's air for each absence no
/// longer than [`MAX_RESERVATION`].
pub enum AirReservation {}

impl ReservationMode for AirReservation {
    type Reserve = Option<Instant>;

    fn reserve(until: Instant, length: Duration) -> Option<Instant> {
        (length <= MAX_RESERVATION).then_some(until)
    }

    fn none() -> Option<Instant> {
        None
    }
}

/// A phase that closes the gates of an arbiter of the reservation mode
/// `R`: every phase for [`NoReservation`], only one that sends the
/// reservation for [`AirReservation`].
pub trait CloseGateWith<'p, 'id: 'p, R: ReservationMode>: LowerMacPhase<'p, 'id> {
    /// Close the gate as the arbiter's action asks; `radio` turns the
    /// reservation's end into the port's radio time.
    fn close_gate_with(
        &mut self,
        gate: GateOpen<'id>,
        reserve: R::Reserve,
        radio: impl FnOnce(Instant) -> Ieee80211Instant,
    );
}

impl<'p, 'id: 'p, P: LowerMacPhase<'p, 'id>> CloseGateWith<'p, 'id, NoReservation> for P {
    fn close_gate_with(
        &mut self,
        gate: GateOpen<'id>,
        (): (),
        _: impl FnOnce(Instant) -> Ieee80211Instant,
    ) {
        self.close_gate(gate);
    }
}

impl<'p, 'id: 'p, P: AirReservationPhase<'p, 'id>> CloseGateWith<'p, 'id, AirReservation> for P {
    fn close_gate_with(
        &mut self,
        gate: GateOpen<'id>,
        reserve: Option<Instant>,
        radio: impl FnOnce(Instant) -> Ieee80211Instant,
    ) {
        match reserve {
            Some(until) => self.close_gate_reserving(gate, radio(until)),
            None => self.close_gate(gate),
        }
    }
}

/// One action of the arbiter.
#[derive(Debug)]
pub enum ArbiterAction<G: GateTokens, K, R: ReservationMode> {
    /// Tell the station.
    Station(StationNotice<K>),
    /// Tell the access point.
    AccessPoint(AccessPointCommand<K>),
    /// Close the port's transmit gate ([`Arbiter::gate_closed`] follows),
    /// with the reservation of an absence the mode carries.
    CloseGate {
        /// The open gate.
        gate: G::Open,
        /// The air reservation's end, of an arbiter that reserves it.
        reserve: R::Reserve,
    },
    /// Move the port to the channel: disable, set it, enable
    /// ([`Arbiter::retuned`] or [`Arbiter::retune_refused`] follows).
    Retune(SupportedChannel<K>),
    /// Open the port's transmit gate ([`Arbiter::gate_opened`] takes the
    /// open gate back).
    OpenGate(G::Closed),
}

/// The arbiter's policy.
#[derive(Clone, Copy, Debug)]
pub struct ArbiterPolicy<'a> {
    /// The window a station that announces its absence has to tell its
    /// access point.
    pub preparation: Duration,
    /// How long after the access point's TBTT an absence starts.
    pub guard: Duration,
    /// The longest absence from the access point's channel, its two
    /// retunes included.
    pub absence: Duration,
    /// The longest retune of the port.
    pub retune: Duration,
    /// The shortest listening worth an absence.
    pub min_listen: Duration,
    /// How the access point follows its upstream.
    pub follow: ApFollowPolicy,
    /// How the search for a lost upstream spaces its absences.
    pub search: ApSearchPolicy<'a>,
    /// When the pair gives up waiting for a lost upstream.
    pub loss: UpstreamLossPolicy,
}

impl ArbiterPolicy<'static> {
    /// A 10 ms preparation, absences 2 ms after the TBTT, at most 20 ms
    /// with their two retunes of at most 2 ms each, at least 5 ms of
    /// listening, and the coordinator's default policies.
    pub const DEFAULT: Self = Self {
        preparation: Duration::from_millis(10),
        guard: Duration::from_millis(2),
        absence: Duration::from_millis(20),
        retune: Duration::from_millis(2),
        min_listen: Duration::from_millis(5),
        follow: ApFollowPolicy::DEFAULT,
        search: ApSearchPolicy::new(&[]),
        loss: UpstreamLossPolicy::DEFAULT,
    };
}

/// `a + b`, at most the longest duration.
const fn sum(a: Duration, b: Duration) -> Duration {
    Duration::from_micros(a.as_micros().saturating_add(b.as_micros()))
}

/// The port's transmit gate and who holds its token.
enum Gate<G: GateTokens> {
    /// Open: the arbiter holds the open gate.
    Open(G::Open),
    /// Closing: the port holds the gate.
    Closing,
    /// Closed: the arbiter holds the closed gate.
    Closed(G::Closed),
    /// Opening: the port gives the open gate back.
    Opening,
}

/// The port's channel.
enum Port<K> {
    /// On the channel.
    On(SupportedChannel<K>),
    /// Moving to `to`.
    Retuning { to: SupportedChannel<K> },
    /// Its channel is unknown after a refused move: the next cycle moves
    /// it.
    Unknown,
    /// Failed: the arbiter stopped the station and the access point.
    Failed,
}

/// How a lease ends.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LeaseEnd {
    /// At its `until`, or never.
    Own,
    /// As soon as it can, with its own end.
    Released,
    /// As soon as it can, cancelled for the reason.
    Cancelled(CancelReason),
}

/// The station's lease, by its steps.
struct Granted<K> {
    id: LeaseId,
    channel: SupportedChannel<K>,
    from: Instant,
    until: Option<Instant>,
    /// The port returns to the access point's channel at its end.
    returns: bool,
    place: LeasePlace,
    end: LeaseEnd,
    step: LeaseStep,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LeaseStep {
    /// Before its start.
    Waiting { ready: bool },
    /// The gate cycles onto its channel.
    Starting,
    /// Running on its channel.
    Running,
    /// The gate cycles back.
    Ending,
}

/// A move of the port no lease asked for.
struct Move<K> {
    to: SupportedChannel<K>,
    at: Instant,
}

/// One cycle of the gate: closing, the port's move when it must, opening,
/// then what the cycle was for.
struct Cycle<K, Reserve> {
    to: SupportedChannel<K>,
    reserve: Reserve,
    purpose: CyclePurpose,
}

// The channel, the reservation and the purpose are values: copies of
// them name no token, whatever the capabilities marker.
impl<K> Clone for Port<K> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K> Copy for Port<K> {}

impl<K> Clone for Move<K> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K> Copy for Move<K> {}

impl<K, Reserve: Copy> Clone for Cycle<K, Reserve> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K, Reserve: Copy> Copy for Cycle<K, Reserve> {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CyclePurpose {
    LeaseStart,
    LeaseEnd,
    /// A move no lease asked for; the station hears of it.
    Move,
    /// The return after a refused move or a late lease; the station heard
    /// of its lease's end already.
    Return,
}

/// What one transition owes the station and the access point: at most a
/// lease's end and a command to leave, and at most two commands.
struct Outbox<K> {
    notices: [Option<StationNotice<K>>; 2],
    commands: [Option<AccessPointCommand<K>>; 2],
}

impl<K> Outbox<K> {
    const fn new() -> Self {
        Self {
            notices: [None, None],
            commands: [None, None],
        }
    }

    fn tell(&mut self, notice: StationNotice<K>) {
        let slot = self.notices.iter_mut().find(|slot| slot.is_none());
        debug_assert!(slot.is_some(), "a transition owes the station at most two notices");
        if let Some(slot) = slot {
            *slot = Some(notice);
        }
    }

    fn command(&mut self, command: AccessPointCommand<K>) {
        let slot = self.commands.iter_mut().find(|slot| slot.is_none());
        debug_assert!(slot.is_some(), "a transition owes the access point at most two commands");
        if let Some(slot) = slot {
            *slot = Some(command);
        }
    }
}

/// The arbiter.
pub struct Arbiter<'a, G: GateTokens, K: Ieee80211Capabilities, R: ReservationMode = NoReservation>
{
    policy: ArbiterPolicy<'a>,
    coordinator: ChannelCoordinator<'a>,
    gate: Gate<G>,
    port: Port<K>,
    /// The access point's schedule while it runs.
    access_point: Option<ApSchedule>,
    /// The access point's channel, or the port's alone.
    home: SupportedChannel<K>,
    lease: Option<Granted<K>>,
    pending: Option<Move<K>>,
    cycle: Option<Cycle<K, R::Reserve>>,
    /// The upstream is lost since this instant, while the access point
    /// runs.
    lost: Option<Instant>,
    /// The start of the search's last absence.
    last_search: Option<Instant>,
    next_id: u32,
    outbox: Outbox<K>,
}

impl<'a, G: GateTokens, K: Ieee80211Capabilities, R: ReservationMode> Arbiter<'a, G, K, R> {
    /// The arbiter of a port on `channel`, holding its open gate, and the
    /// station's place for its lease.
    pub fn new(
        policy: ArbiterPolicy<'a>,
        channel: SupportedChannel<K>,
        gate: G::Open,
    ) -> (Self, LeasePlace) {
        (
            Self {
                policy,
                coordinator: ChannelCoordinator::new(policy.follow, policy.search),
                gate: Gate::Open(gate),
                port: Port::On(channel),
                access_point: None,
                home: channel,
                lease: None,
                pending: None,
                cycle: None,
                lost: None,
                last_search: None,
                next_id: 0,
                outbox: Outbox::new(),
            },
            LeasePlace { private: () },
        )
    }

    /// The channel the port runs on, when it is not moving.
    pub fn channel(&self) -> Option<SupportedChannel<K>> {
        match self.port {
            Port::On(channel) => Some(channel),
            _ => None,
        }
    }

    /// Answer the station's request at `now`, in this step; the request
    /// takes the station's place.
    pub fn request(
        &mut self,
        now: Instant,
        request: ChannelRequest<K>,
        place: LeasePlace,
    ) -> Answer<K> {
        if matches!(self.port, Port::Failed) {
            return Answer::Leave(LeaveReason::PortFailed, place);
        }
        let away = request.channel != self.home;
        if away && request.purpose == Purpose::Connect && self.access_point.is_some() {
            return Answer::Leave(LeaveReason::AwayFromAccessPoint, place);
        }
        let id = LeaseId(self.next_id);
        self.next_id = self.next_id.wrapping_add(1);
        let mut earliest = if request.announces_absence {
            now.saturating_add(self.policy.preparation)
        } else {
            now
        };
        let dwell = match request.purpose {
            Purpose::Scan { dwell } => Some(dwell),
            Purpose::Search { dwell } => {
                // A lost upstream's search spaces its absences: one per
                // beacon interval while dense, sparser after.
                if let (Some(since), Some(last), Some(schedule)) =
                    (self.lost, self.last_search, self.access_point)
                {
                    let dense = since.saturating_add(self.policy.search.dense_period);
                    let spacing = if last < dense {
                        schedule.beacon_interval
                    } else {
                        self.policy.search.sparse_interval
                    };
                    let next = last.saturating_add(spacing);
                    if next > earliest {
                        earliest = next;
                    }
                }
                Some(dwell)
            }
            Purpose::Connect => None,
        };
        let retune = self.policy.retune;
        let (from, until, returns) = match (&self.access_point, away) {
            // Beside an access point, away from its channel only in an
            // absence after its TBTT: the move there, the listening and
            // the move back, at most the policy's absence.
            (Some(schedule), true) => {
                let from = slot(*schedule, earliest, self.policy.guard);
                let wanted = dwell.map_or(self.policy.absence, |dwell| {
                    sum(sum(dwell, retune), retune)
                });
                let length = if wanted < self.policy.absence {
                    wanted
                } else {
                    self.policy.absence
                };
                (from, Some(from.saturating_add(length)), true)
            }
            // Alone, or on the access point's channel, the port stays.
            _ => (
                earliest,
                dwell.map(|dwell| earliest.saturating_add(retune).saturating_add(dwell)),
                false,
            ),
        };
        if matches!(request.purpose, Purpose::Search { .. }) {
            self.last_search = Some(from);
        }
        let listen_until = until.map(|until| {
            if returns {
                until.checked_sub(retune).unwrap_or(until)
            } else {
                until
            }
        });
        self.lease = Some(Granted {
            id,
            channel: request.channel,
            from,
            until,
            returns,
            place,
            end: LeaseEnd::Own,
            step: LeaseStep::Waiting {
                ready: !request.announces_absence,
            },
        });
        Answer::Lease(Lease {
            id,
            channel: request.channel,
            from,
            until,
            listen_until,
        })
    }

    /// The station is ready for `lease`.
    pub fn ready(&mut self, lease: LeaseId) {
        if let Some(granted) = &mut self.lease
            && granted.id == lease
            && let LeaseStep::Waiting { ready } = &mut granted.step
        {
            *ready = true;
        }
    }

    /// The station gives `lease` back: it ends with
    /// [`StationNotice::LeaseCancelled`] before its start, with
    /// [`StationNotice::LeaseEnded`] after, once the gate's cycle that
    /// runs ended.
    pub fn release(&mut self, lease: Lease<K>) {
        if self.lease.as_ref().is_some_and(|granted| granted.id == lease.id) {
            self.end_lease(LeaseEnd::Released, CancelReason::Released);
        }
    }

    /// End the lease: a waiting one is cancelled for `reason` at once; one
    /// that started ends as `end` once the gate's cycle that runs ended.
    fn end_lease(&mut self, end: LeaseEnd, reason: CancelReason) {
        let Some(granted) = &mut self.lease else {
            return;
        };
        match granted.step {
            LeaseStep::Waiting { .. } => {
                let granted = self.lease.take().expect("a lease waits");
                self.outbox.tell(StationNotice::LeaseCancelled(
                    granted.id,
                    reason,
                    granted.place,
                ));
            }
            LeaseStep::Starting | LeaseStep::Running => granted.end = end,
            LeaseStep::Ending => {}
        }
    }

    /// The access point's fact at `now`.
    pub fn access_point(&mut self, now: Instant, fact: AccessPointFact<K>) {
        match fact {
            AccessPointFact::Running {
                channel,
                next_tbtt,
                beacon_interval,
            } => {
                let schedule = ApSchedule {
                    channel: channel.channel(),
                    next_tbtt,
                    beacon_interval,
                };
                self.access_point = Some(schedule);
                self.home = channel;
                self.coordinator.access_point_running(schedule);
            }
            AccessPointFact::Stopped => {
                self.access_point = None;
                self.coordinator.access_point_stopped();
                // A lease away from its channel is cancelled and returns.
                if self.lease.as_ref().is_some_and(|granted| granted.returns) {
                    let reason = CancelReason::AccessPointStopped;
                    self.end_lease(LeaseEnd::Cancelled(reason), reason);
                }
            }
            AccessPointFact::SwitchDue(target) => {
                let actions = self
                    .coordinator
                    .access_point_switch_due(target.channel(), now);
                self.apply(actions);
            }
        }
    }

    /// The station's fact about its upstream at `now`.
    pub fn upstream(&mut self, now: Instant, fact: UpstreamFact<K>) {
        match fact {
            UpstreamFact::Connected(channel) => {
                self.lost = None;
                self.last_search = None;
                let actions = self.coordinator.station_connected(channel.channel());
                self.apply(actions);
            }
            UpstreamFact::SwitchAnnounced { target, mode, at } => {
                let reason = CancelReason::Switch;
                self.end_lease(LeaseEnd::Cancelled(reason), reason);
                let actions = self
                    .coordinator
                    .upstream_switch_announced(target.channel(), mode, at);
                self.apply(actions);
            }
            UpstreamFact::Lost => {
                let reason = CancelReason::UpstreamLost;
                if self
                    .lease
                    .as_ref()
                    .is_some_and(|granted| matches!(granted.step, LeaseStep::Waiting { .. }))
                {
                    self.end_lease(LeaseEnd::Cancelled(reason), reason);
                }
                self.coordinator.upstream_lost(now);
                self.lost = self.access_point.map(|_| now);
                self.last_search = None;
            }
            UpstreamFact::Found(channel) => {
                let actions = self.coordinator.upstream_found(channel.channel());
                self.apply(actions);
            }
            UpstreamFact::NotFound => {
                let actions = self.coordinator.upstream_rescan_failed(now);
                self.lost = Some(now);
                self.apply(actions);
            }
        }
    }

    /// The composition wants the access point to run.
    pub fn want_access_point(&mut self) {
        let actions = self.coordinator.want_access_point();
        self.apply(actions);
    }

    /// Carry the coordinator's actions out.
    fn apply(&mut self, actions: CoordinatorActions) {
        for action in actions {
            match action {
                CoordinatorAction::AnnounceSwitch {
                    target,
                    mode,
                    count,
                } => match SupportedChannel::new(target) {
                    Some(target) => self.outbox.command(AccessPointCommand::AnnounceSwitch {
                        target,
                        mode,
                        count,
                    }),
                    None => self.outbox.tell(StationNotice::Leave(LeaveReason::Unservable)),
                },
                CoordinatorAction::Retune { channel, at } => match SupportedChannel::new(channel) {
                    Some(to) => self.pending = Some(Move { to, at }),
                    None => self.outbox.tell(StationNotice::Leave(LeaveReason::Unservable)),
                },
                CoordinatorAction::StopAccessPoint => {
                    self.access_point = None;
                    self.outbox.command(AccessPointCommand::Stop);
                }
                CoordinatorAction::StartAccessPoint { channel } => {
                    if let Some(channel) = SupportedChannel::new(channel) {
                        self.outbox.command(AccessPointCommand::Start(channel));
                    }
                }
                CoordinatorAction::LeaveUpstream => {
                    self.outbox.tell(StationNotice::Leave(LeaveReason::Unservable));
                }
                // The station joins on the channel the port moved to; the
                // move's notice told it.
                CoordinatorAction::JoinUpstream { .. } => {}
                // The arbiter's own search leases place the absences.
                CoordinatorAction::Absence { .. } => {}
            }
        }
    }

    /// The port's gate closed.
    pub fn gate_closed(&mut self, closed: G::Closed) {
        self.gate = Gate::Closed(closed);
    }

    /// The port's gate opened, which gives the open gate back and ends the
    /// gate's cycle.
    pub fn gate_opened(&mut self, open: G::Open) {
        self.gate = Gate::Open(open);
        if let Some(cycle) = self.cycle.take() {
            self.finish(cycle);
        }
    }

    /// What a cycle was for, once the gate opened again.
    fn finish(&mut self, cycle: Cycle<K, R::Reserve>) {
        match cycle.purpose {
            CyclePurpose::LeaseStart => {
                if let Some(granted) = &mut self.lease {
                    granted.step = LeaseStep::Running;
                    self.outbox.tell(StationNotice::LeaseStarted(granted.id));
                }
            }
            CyclePurpose::LeaseEnd => {
                if let Some(granted) = self.lease.take() {
                    self.outbox.tell(match granted.end {
                        LeaseEnd::Cancelled(reason) => {
                            StationNotice::LeaseCancelled(granted.id, reason, granted.place)
                        }
                        LeaseEnd::Own | LeaseEnd::Released => {
                            StationNotice::LeaseEnded(granted.id, granted.place)
                        }
                    });
                }
            }
            CyclePurpose::Move => self.outbox.tell(StationNotice::Moved(cycle.to)),
            CyclePurpose::Return => {}
        }
    }

    /// The port runs on `channel`.
    pub fn retuned(&mut self, channel: SupportedChannel<K>) {
        if let Port::Retuning { to } = self.port {
            debug_assert!(to == channel, "the port moved where the arbiter asked");
        }
        self.port = Port::On(channel);
        let actions = self.coordinator.port_moved(channel.channel());
        self.apply(actions);
    }

    /// The port refused the move it was asked for: a move onto a lease's
    /// channel cancels that lease and the port returns; a failed return
    /// stops the station and the access point.
    pub fn retune_refused(&mut self) {
        let starting = self
            .cycle
            .is_some_and(|cycle| cycle.purpose == CyclePurpose::LeaseStart);
        if !starting {
            self.fail();
            return;
        }
        if let Some(granted) = self.lease.take() {
            self.outbox.tell(StationNotice::LeaseCancelled(
                granted.id,
                CancelReason::Refused,
                granted.place,
            ));
        }
        // The port is on no channel it can trust: back home, the gate
        // still closed.
        self.port = Port::Unknown;
        self.cycle = Some(Cycle {
            to: self.home,
            reserve: R::none(),
            purpose: CyclePurpose::Return,
        });
    }

    /// The port is poisoned: the station and the access point stop.
    pub fn poisoned(&mut self) {
        self.fail();
    }

    fn fail(&mut self) {
        self.port = Port::Failed;
        self.cycle = None;
        self.pending = None;
        if let Some(granted) = self.lease.take() {
            self.outbox.tell(StationNotice::LeaseCancelled(
                granted.id,
                CancelReason::PortFailed,
                granted.place,
            ));
        }
        self.outbox
            .tell(StationNotice::Leave(LeaveReason::PortFailed));
        self.outbox.command(AccessPointCommand::Stop);
    }

    /// Start the next gate's cycle due at `now`, if none runs.
    fn plan(&mut self, now: Instant) {
        if self.cycle.is_some() || matches!(self.port, Port::Failed) {
            return;
        }
        // A lost upstream past the loss policy: the access point stops and
        // the station scans alone.
        if let Some(due) = self.coordinator.upstream_loss_due(self.policy.loss)
            && now >= due
        {
            let actions = self.coordinator.upstream_rescan();
            self.apply(actions);
        }
        let home = self.home;
        let retune = self.policy.retune;
        let pending_due = self.pending.is_some_and(|pending| now >= pending.at);
        if let Some(granted) = &mut self.lease {
            match granted.step {
                LeaseStep::Waiting { ready } if now >= granted.from => {
                    if !ready {
                        let reason = CancelReason::NotReady;
                        self.end_lease(LeaseEnd::Cancelled(reason), reason);
                        return;
                    }
                    granted.step = LeaseStep::Starting;
                    let reserve = match (granted.returns, granted.until) {
                        (true, Some(until)) => match until.checked_duration_since(now) {
                            Some(length) => R::reserve(until, length),
                            None => R::none(),
                        },
                        _ => R::none(),
                    };
                    self.cycle = Some(Cycle {
                        to: granted.channel,
                        reserve,
                        purpose: CyclePurpose::LeaseStart,
                    });
                    return;
                }
                LeaseStep::Running => {
                    let ends = granted.until.map(|until| {
                        if granted.returns {
                            until.checked_sub(retune).unwrap_or(until)
                        } else {
                            until
                        }
                    });
                    if granted.end != LeaseEnd::Own
                        || pending_due
                        || ends.is_some_and(|ends| now >= ends)
                    {
                        granted.step = LeaseStep::Ending;
                        self.cycle = Some(Cycle {
                            to: if granted.returns { home } else { granted.channel },
                            reserve: R::none(),
                            purpose: CyclePurpose::LeaseEnd,
                        });
                        return;
                    }
                }
                _ => {}
            }
        }
        if pending_due && let Some(pending) = self.pending.take() {
            self.home = pending.to;
            self.cycle = Some(Cycle {
                to: pending.to,
                reserve: R::none(),
                purpose: CyclePurpose::Move,
            });
        }
    }

    /// The next action at `now`, if any. The service takes every action
    /// before it waits again.
    pub fn poll_action(&mut self, now: Instant) -> Option<ArbiterAction<G, K, R>> {
        if let Some(command) = self.outbox.commands.iter_mut().find_map(Option::take) {
            return Some(ArbiterAction::AccessPoint(command));
        }
        if let Some(notice) = self.outbox.notices.iter_mut().find_map(Option::take) {
            return Some(ArbiterAction::Station(notice));
        }
        self.plan(now);
        if let Some(command) = self.outbox.commands.iter_mut().find_map(Option::take) {
            return Some(ArbiterAction::AccessPoint(command));
        }
        if let Some(notice) = self.outbox.notices.iter_mut().find_map(Option::take) {
            return Some(ArbiterAction::Station(notice));
        }
        let cycle = self.cycle?;
        match core::mem::replace(&mut self.gate, Gate::Closing) {
            Gate::Open(open) => Some(ArbiterAction::CloseGate {
                gate: open,
                reserve: cycle.reserve,
            }),
            Gate::Closing => None,
            Gate::Opening => {
                self.gate = Gate::Opening;
                None
            }
            Gate::Closed(closed) => {
                if cycle.purpose == CyclePurpose::LeaseStart && self.too_late(now) {
                    // The lease no longer fits: cancelled, and the gate
                    // opens where the port is.
                    let granted = self.lease.take().expect("a lease starts");
                    self.outbox.tell(StationNotice::LeaseCancelled(
                        granted.id,
                        CancelReason::Late,
                        granted.place,
                    ));
                    self.cycle = Some(Cycle {
                        to: self.channel().unwrap_or(self.home),
                        purpose: CyclePurpose::Return,
                        ..cycle
                    });
                    self.gate = Gate::Closed(closed);
                    return self.poll_action(now);
                }
                match self.port {
                    Port::On(channel) if channel == cycle.to => {
                        self.gate = Gate::Opening;
                        Some(ArbiterAction::OpenGate(closed))
                    }
                    Port::On(_) | Port::Unknown => {
                        self.gate = Gate::Closed(closed);
                        self.port = Port::Retuning { to: cycle.to };
                        Some(ArbiterAction::Retune(cycle.to))
                    }
                    Port::Retuning { .. } | Port::Failed => {
                        self.gate = Gate::Closed(closed);
                        None
                    }
                }
            }
        }
    }

    /// Whether the starting lease no longer fits two retunes and the
    /// shortest listening before its end.
    fn too_late(&self, now: Instant) -> bool {
        let Some(granted) = &self.lease else {
            return false;
        };
        let Some(until) = granted.until else {
            return false;
        };
        let retunes = if granted.returns { 2 } else { 1 };
        let needed = (0..retunes).fold(self.policy.min_listen, |needed, _| {
            sum(needed, self.policy.retune)
        });
        now.saturating_add(needed) > until
    }

    /// When the arbiter next needs [`Self::poll_action`], even without an
    /// input.
    pub fn poll_deadline(&self) -> Option<Instant> {
        let lease = self.lease.as_ref().and_then(|granted| match granted.step {
            LeaseStep::Waiting { .. } => Some(granted.from),
            LeaseStep::Running if granted.returns => granted
                .until
                .map(|until| until.checked_sub(self.policy.retune).unwrap_or(until)),
            LeaseStep::Running => granted.until,
            _ => None,
        });
        let pending = self.pending.map(|pending| pending.at);
        let loss = self.coordinator.upstream_loss_due(self.policy.loss);
        [lease, pending, loss].into_iter().flatten().min()
    }
}

/// The first absence slot that starts at or after `earliest`: the guard
/// after one of the schedule's TBTTs.
fn slot(schedule: ApSchedule, earliest: Instant, guard: Duration) -> Instant {
    let mut tbtt = schedule.next_tbtt;
    loop {
        let start = tbtt.saturating_add(guard);
        if start >= earliest || schedule.beacon_interval.as_micros() == 0 {
            return start;
        }
        tbtt = tbtt.saturating_add(schedule.beacon_interval);
    }
}

#[cfg(test)]
mod tests;
