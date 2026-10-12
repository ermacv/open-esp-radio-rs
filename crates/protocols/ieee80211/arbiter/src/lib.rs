#![no_std]
#![forbid(unsafe_code)]

//! The arbiter of the channel one radio's station and access point share:
//! the third sans-IO machine of their loop ([owner decision on #563]).
//!
//! Only the arbiter changes the port's channel: it closes the transmit
//! gate, retunes the port and opens the gate again. The station asks it
//! for a channel and holds the [`Lease`] it answers with; the access point
//! tells it its schedule and follows its commands. A station alone has an
//! arbiter of the same shape, which leases at once.
//!
//! - **The answer is a return value.** [`Arbiter::request`] answers in the
//!   same step, with a lease or a command to leave, so no request goes
//!   unanswered. Only a lease's start is later: its moment `from` is the
//!   arbiter's own deadline.
//! - **A lease is a named value** the station holds and gives back
//!   ([`Arbiter::release`]); [`StationNotice`]s name it, so a notice of an
//!   earlier lease is never taken for the current one.
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
//!   starts once it closed: the attempts in flight end first. Its `until`
//!   is absolute, so the access point's beacons and air reservation hold;
//!   a lease whose gate closed only after `until` is cancelled
//!   ([`CancelReason::Late`]).
//! - **A refused move** onto a lease's channel cancels that lease
//!   ([`CancelReason::Refused`]) and the port returns; a failed return, or
//!   a poisoned port, stops the station and the access point
//!   ([`LeaveReason::PortFailed`]).
//! - **The air reservation** is a policy option, off by default, for
//!   absences no longer than the Duration field's limit
//!   ([`MAX_RESERVATION`]).
//! - **Beside an access point**, the station leaves the access point's
//!   channel only in absences after the access point's TBTTs, which the
//!   access point covers with a reservation of its air.
//!
//! [`coordinator`]'s [`ChannelCoordinator`](coordinator::ChannelCoordinator)
//! keeps the access point's follow, search and loss policies.
//!
//! [owner decision on #563]: https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6129596602

#[cfg(test)]
extern crate std;

pub mod channel;
pub mod coordinator;

use oer_ieee80211_lower_mac::{Ieee80211Capabilities, SupportedChannel};
use oer_time::{Duration, Instant};

/// The name of one lease.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeaseId(u32);

/// The port's channel, leased to the station from `from` until `until`
/// (`None`: until the station gives it back). Only the arbiter makes one,
/// and the station gives it back by value.
#[derive(Debug)]
#[must_use = "a lease goes back to the arbiter through `Arbiter::release`"]
pub struct Lease<K> {
    id: LeaseId,
    channel: SupportedChannel<K>,
    from: Instant,
    until: Option<Instant>,
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

    /// When it ends; `None` until it is given back.
    pub const fn until(&self) -> Option<Instant> {
        self.until
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
    /// Join an access point, and stay.
    Connect,
    /// Look for the lost upstream access point for `dwell`.
    Search {
        /// How long to listen.
        dwell: Duration,
    },
}

/// A station's request for a channel.
#[derive(Clone, Copy, Debug)]
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
    /// The channel, leased.
    Lease(Lease<K>),
    /// The station must leave its network.
    Leave(LeaveReason),
}

/// Why the station must leave its network.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeaveReason {
    /// The upstream moved where the access point may not follow, and the
    /// policy keeps the access point.
    Unservable,
    /// The upstream stayed lost past the policy's bound.
    UpstreamLost,
    /// The port could not return to the access point's channel, or it is
    /// poisoned: the station and the access point stop.
    PortFailed,
}

/// Why a lease ended before it started.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CancelReason {
    /// The station was not ready at the lease's start.
    NotReady,
    /// The station gave it back.
    Released,
    /// The upstream announced a switch.
    Switch,
    /// The upstream was lost.
    UpstreamLost,
    /// The access point stopped.
    AccessPointStopped,
    /// The port refused the move onto the lease's channel; it returns to
    /// the access point's.
    Refused,
    /// The gate closed only after the lease's end: the attempts in flight
    /// outlasted the absence.
    Late,
}

/// What the arbiter tells the station.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationNotice<K> {
    /// The lease started: the port runs on its channel.
    LeaseStarted(LeaseId),
    /// The lease ended: the port left its channel.
    LeaseEnded(LeaseId),
    /// The lease ended before it started.
    LeaseCancelled(LeaseId, CancelReason),
    /// The port moved without the station's request.
    Moved(SupportedChannel<K>),
    /// The station must leave its network.
    Leave(LeaveReason),
}

/// What the arbiter tells the access point.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessPointCommand<K> {
    /// Reserve the air from `from` until `until` (a CTS-to-self), so the
    /// access point's peers keep quiet while the station is away.
    ReserveAir {
        /// The reservation's start.
        from: Instant,
        /// Its end.
        until: Instant,
    },
    /// Start on `channel`.
    Start(SupportedChannel<K>),
    /// Stop: release the peers and close the BSS.
    Stop,
}

/// The access point's facts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessPointFact<K> {
    /// It runs on `channel`, its next TBTT at `next_tbtt`, one every
    /// `beacon_interval`.
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
}

/// The tokens of the port's transmit gate the arbiter holds.
pub trait GateTokens {
    /// The open gate.
    type Open;
    /// The closed gate.
    type Closed;
}

/// One action of the arbiter.
#[derive(Debug)]
pub enum ArbiterAction<G: GateTokens, K> {
    /// Tell the station.
    Station(StationNotice<K>),
    /// Tell the access point.
    AccessPoint(AccessPointCommand<K>),
    /// Close the port's transmit gate ([`Arbiter::gate_closed`] follows).
    CloseGate(G::Open),
    /// Move the port to the channel: disable, set it, enable
    /// ([`Arbiter::retuned`] follows).
    Retune(SupportedChannel<K>),
    /// Open the port's transmit gate ([`Arbiter::gate_opened`] takes the
    /// open gate back).
    OpenGate(G::Closed),
}

/// The longest air reservation: the Duration field's limit, in
/// microseconds.
pub const MAX_RESERVATION: Duration = Duration::from_micros(32_767);

/// The arbiter's policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ArbiterPolicy {
    /// Whether the access point reserves its air for an absence; an absence
    /// longer than [`MAX_RESERVATION`] goes without one.
    pub reserve_air: bool,
    /// The window a station that announces its absence has to tell its
    /// access point.
    pub preparation: Duration,
    /// How long after the access point's TBTT an absence starts.
    pub guard: Duration,
    /// The longest absence from the access point's channel.
    pub absence: Duration,
}

impl ArbiterPolicy {
    /// No air reservation, a 10 ms preparation, absences 2 ms after the
    /// TBTT, at most 20 ms.
    pub const DEFAULT: Self = Self {
        reserve_air: false,
        preparation: Duration::from_millis(10),
        guard: Duration::from_millis(2),
        absence: Duration::from_millis(20),
    };
}

/// The access point's schedule.
#[derive(Clone, Copy, Debug)]
struct Schedule<K> {
    channel: SupportedChannel<K>,
    next_tbtt: Instant,
    beacon_interval: Duration,
}

impl<K> Schedule<K> {
    /// The first absence slot that starts at or after `earliest`.
    fn slot(&self, earliest: Instant, policy: &ArbiterPolicy) -> Instant {
        let mut tbtt = self.next_tbtt;
        loop {
            let start = tbtt.saturating_add(policy.guard);
            if start >= earliest || self.beacon_interval.as_micros() == 0 {
                return start;
            }
            tbtt = tbtt.saturating_add(self.beacon_interval);
        }
    }
}

/// The station's lease and where it stands.
#[derive(Clone, Copy, Debug)]
struct Granted<K> {
    id: LeaseId,
    channel: SupportedChannel<K>,
    from: Instant,
    until: Option<Instant>,
    ready: bool,
    step: Step,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Step {
    /// Before its start.
    Waiting,
    /// The gate closes for the move onto its channel.
    Leaving,
    /// The port moves onto its channel.
    Moving,
    /// Running on its channel.
    Running,
    /// The gate closes for the move back.
    Returning,
    /// The port moves back.
    Back,
}

/// The arbiter.
pub struct Arbiter<G: GateTokens, K: Ieee80211Capabilities> {
    policy: ArbiterPolicy,
    /// The channel the port runs on.
    channel: SupportedChannel<K>,
    open: Option<G::Open>,
    closed: Option<G::Closed>,
    access_point: Option<Schedule<K>>,
    lease: Option<Granted<K>>,
    next_id: u32,
    /// A notice owed to the station, then a command owed to the access
    /// point: at most one of each between two inputs.
    notice: Option<StationNotice<K>>,
    command: Option<AccessPointCommand<K>>,
    /// The gate is closing.
    closing: bool,
    /// The port is moving to this channel.
    retune: Option<SupportedChannel<K>>,
    retuning: bool,
    /// The gate opens once the port is back.
    reopen: bool,
}

impl<G: GateTokens, K: Ieee80211Capabilities> Arbiter<G, K> {
    /// The arbiter of a port on `channel`, holding its open gate.
    pub fn new(policy: ArbiterPolicy, channel: SupportedChannel<K>, gate: G::Open) -> Self {
        Self {
            policy,
            channel,
            open: Some(gate),
            closed: None,
            access_point: None,
            lease: None,
            next_id: 0,
            notice: None,
            command: None,
            closing: false,
            retune: None,
            retuning: false,
            reopen: false,
        }
    }

    /// Answer the station's request at `now`, in this step.
    pub fn request(&mut self, now: Instant, request: ChannelRequest<K>) -> Answer<K> {
        let id = LeaseId(self.next_id);
        self.next_id = self.next_id.wrapping_add(1);
        let earliest = if request.announces_absence {
            now.saturating_add(self.policy.preparation)
        } else {
            now
        };
        let dwell = match request.purpose {
            Purpose::Scan { dwell } | Purpose::Search { dwell } => Some(dwell),
            Purpose::Connect => None,
        };
        let away = request.channel.channel() != self.home().channel();
        let (from, until) = match (&self.access_point, away) {
            // Beside an access point, away from its channel only in an
            // absence after its TBTT, which its air reservation covers.
            (Some(schedule), true) => {
                let from = schedule.slot(earliest, &self.policy);
                let length = match dwell {
                    Some(dwell) if dwell < self.policy.absence => dwell,
                    _ => self.policy.absence,
                };
                let until = from.saturating_add(length);
                if self.policy.reserve_air && length <= MAX_RESERVATION {
                    self.command = Some(AccessPointCommand::ReserveAir { from, until });
                }
                (from, Some(until))
            }
            _ => (earliest, dwell.map(|dwell| earliest.saturating_add(dwell))),
        };
        self.lease = Some(Granted {
            id,
            channel: request.channel,
            from,
            until,
            ready: !request.announces_absence,
            step: Step::Waiting,
        });
        Answer::Lease(Lease {
            id,
            channel: request.channel,
            from,
            until,
        })
    }

    /// The channel the port returns to: the access point's, or the one it
    /// runs on.
    fn home(&self) -> SupportedChannel<K> {
        self.access_point
            .as_ref()
            .map_or(self.channel, |schedule| schedule.channel)
    }

    /// The station is ready for `lease`.
    pub fn ready(&mut self, lease: LeaseId) {
        if let Some(granted) = &mut self.lease
            && granted.id == lease
        {
            granted.ready = true;
        }
    }

    /// The station gives `lease` back.
    pub fn release(&mut self, lease: Lease<K>) {
        let Some(granted) = self.lease.take_if(|granted| granted.id == lease.id) else {
            return;
        };
        match granted.step {
            Step::Waiting => {
                self.notice = Some(StationNotice::LeaseCancelled(
                    granted.id,
                    CancelReason::Released,
                ));
            }
            // A running lease ends with the port's move back, if it moved.
            _ => {
                if granted.channel.channel() != self.home().channel() {
                    self.lease = Some(Granted {
                        until: None,
                        step: Step::Returning,
                        ..granted
                    });
                    self.close_gate_for_move(self.home());
                }
            }
        }
    }

    /// The access point's fact.
    pub fn access_point(&mut self, fact: AccessPointFact<K>) {
        match fact {
            AccessPointFact::Running {
                channel,
                next_tbtt,
                beacon_interval,
            } => {
                self.access_point = Some(Schedule {
                    channel,
                    next_tbtt,
                    beacon_interval,
                });
            }
            AccessPointFact::Stopped => self.access_point = None,
        }
    }

    /// The port's gate closed.
    pub fn gate_closed(&mut self, closed: G::Closed) {
        self.closed = Some(closed);
        self.closing = false;
        if self.retune.is_none() {
            self.reopen = true;
        }
    }

    /// The port's gate opened, which gives the open gate back.
    pub fn gate_opened(&mut self, open: G::Open) {
        self.open = Some(open);
    }

    /// The port runs on `channel`.
    pub fn retuned(&mut self, channel: SupportedChannel<K>) {
        self.channel = channel;
        self.retuning = false;
        self.reopen = true;
        if let Some(granted) = &mut self.lease {
            match granted.step {
                Step::Moving => {
                    granted.step = Step::Running;
                    self.notice = Some(StationNotice::LeaseStarted(granted.id));
                }
                Step::Back => {
                    let id = granted.id;
                    self.lease = None;
                    self.notice = Some(StationNotice::LeaseEnded(id));
                }
                _ => {}
            }
        }
    }

    /// The port refused the move it was asked for: a move onto a lease's
    /// channel cancels that lease and the port returns; a failed return
    /// stops the station and the access point.
    pub fn retune_refused(&mut self) {
        self.retuning = false;
        let home = self.home();
        let Some(granted) = self.lease.take() else {
            return;
        };
        match granted.step {
            Step::Moving => {
                self.notice = Some(StationNotice::LeaseCancelled(granted.id, CancelReason::Refused));
                // The port is on no channel it can trust: back home, the
                // gate still closed.
                self.retune = Some(home);
            }
            _ => {
                self.notice = Some(StationNotice::Leave(LeaveReason::PortFailed));
                self.command = Some(AccessPointCommand::Stop);
            }
        }
    }

    fn close_gate_for_move(&mut self, channel: SupportedChannel<K>) {
        self.retune = Some(channel);
        self.closing = true;
    }

    /// Move the lease along at `now`.
    fn advance(&mut self, now: Instant) {
        let home = self.home();
        let Some(granted) = &mut self.lease else {
            return;
        };
        match granted.step {
            Step::Waiting if now >= granted.from => {
                if !granted.ready {
                    let id = granted.id;
                    self.lease = None;
                    self.notice = Some(StationNotice::LeaseCancelled(id, CancelReason::NotReady));
                    return;
                }
                if granted.channel.channel() == self.channel.channel() {
                    // Already there: the gate still closes, so every
                    // earlier terminal comes before the start.
                    granted.step = Step::Leaving;
                    self.retune = None;
                    self.closing = true;
                } else {
                    granted.step = Step::Leaving;
                    let channel = granted.channel;
                    self.close_gate_for_move(channel);
                }
            }
            Step::Leaving if !self.closing && self.closed.is_some() => {
                // The lease's end is absolute: it keeps the access point's
                // beacons and its air reservation. An absence that starts
                // late is shorter; one the gate closed after is cancelled.
                if granted.until.is_some_and(|until| now >= until) {
                    let id = granted.id;
                    self.lease = None;
                    self.retune = None;
                    self.reopen = true;
                    self.notice = Some(StationNotice::LeaseCancelled(id, CancelReason::Late));
                    return;
                }
                if self.retune.is_some() {
                    granted.step = Step::Moving;
                } else {
                    granted.step = Step::Running;
                    self.notice = Some(StationNotice::LeaseStarted(granted.id));
                }
            }
            Step::Running if granted.until.is_some_and(|until| now >= until) => {
                granted.step = Step::Returning;
                self.close_gate_for_move(home);
            }
            Step::Returning if !self.closing && self.closed.is_some() => {
                granted.step = Step::Back;
                if home.channel() == self.channel.channel() {
                    let id = granted.id;
                    self.lease = None;
                    self.retune = None;
                    self.reopen = true;
                    self.notice = Some(StationNotice::LeaseEnded(id));
                }
            }
            _ => {}
        }
    }

    /// The next action at `now`, if any. The service takes every action
    /// before it waits again.
    pub fn poll_action(&mut self, now: Instant) -> Option<ArbiterAction<G, K>> {
        self.advance(now);
        if let Some(command) = self.command.take() {
            return Some(ArbiterAction::AccessPoint(command));
        }
        if self.closing
            && let Some(open) = self.open.take()
        {
            return Some(ArbiterAction::CloseGate(open));
        }
        if !self.closing
            && self.closed.is_some()
            && !self.retuning
            && let Some(channel) = self.retune.take()
        {
            self.retuning = true;
            return Some(ArbiterAction::Retune(channel));
        }
        if let Some(notice) = self.notice.take() {
            // The station hears a start or an end once the gate is open
            // again; a cancellation at once.
            if matches!(notice, StationNotice::LeaseCancelled(..)) || !self.reopen {
                return Some(ArbiterAction::Station(notice));
            }
            self.notice = Some(notice);
        }
        if self.reopen
            && !self.retuning
            && let Some(closed) = self.closed.take()
        {
            self.reopen = false;
            self.advance(now);
            return Some(ArbiterAction::OpenGate(closed));
        }
        self.notice.take().map(ArbiterAction::Station)
    }

    /// When the arbiter next needs [`Self::poll_action`], even without an
    /// input.
    pub fn poll_deadline(&self) -> Option<Instant> {
        let granted = self.lease.as_ref()?;
        match granted.step {
            Step::Waiting => Some(granted.from),
            Step::Running => granted.until,
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;
