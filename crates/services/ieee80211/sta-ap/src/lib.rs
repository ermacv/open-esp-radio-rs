#![no_std]
#![forbid(unsafe_code)]

//! A station and an access point on one lower-MAC port.
//!
//! [`PortStaAp`] is the owner of a port two clients share: the station
//! ([`PortStation`]) on one interface, the access point
//! ([`PortAccessPoint`]) on the other, and the [`ChannelCoordinator`] that
//! decides the port's one channel. Neither client tunes the port while
//! both run; the owner carries out the coordinator's actions:
//!
//! - both clients run at once, each until the next sync point (the access
//!   point's next TBTT, a retune the coordinator asked for, or the
//!   caller's deadline), so neither future is ever dropped mid-exchange;
//! - at a sync point the owner hands each client's report to the
//!   coordinator (the upstream's announced switch, the access point's due
//!   switch) and acts: the access point announces a move, the port moves
//!   once for both interfaces, the access point stops or starts.
//!
//! The station joins its upstream before the access point starts, alone on
//! the port. An upstream the station loses ends [`PortStaAp::run_until`]
//! with [`PortStaApEvent::StationEnded`].
//! Later runs keep serving the access point and passively search on its
//! channel and in the coordinator's protected, receive-only absences.
//! A found upstream is joined without a probe, after the access point's
//! CSA when a move is needed. [`PortStaAp::reconnect`] remains an explicit
//! current-channel discovery and retry.

use core::{
    cell::Cell,
    future::{Future, poll_fn},
    pin::pin,
    task::Poll,
};

use oer_ieee80211_ap::coordinator::{ChannelCoordinator, CoordinatorAction, CoordinatorActions};
use oer_ieee80211_ap_service::port::{PortAccessPoint, PortApEnv, PortApError, PortApEvent};
use oer_ieee80211_lower_mac::{Channel, LowerMacAirReservation, LowerMacSetting};
use oer_ieee80211_sta::attempt::AssociationAttemptOutcome;
use oer_ieee80211_sta_service::port::{
    PortAttemptError, PortDisconnect, PortLinkError, PortStation, PortStationEnv, PortStationEvent,
};
use oer_ieee80211_upper_mac_service::absence::{
    AbsenceError, AbsenceState, AbsenceWindow, PortAbsence,
};
use oer_ieee80211_upper_mac_service::client::{PortError, PortMsdu, PortRxBuffer};
use oer_time::{Duration, Instant, Timer};

/// Why [`PortStaAp::run_until`] returned before its deadline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PortStaApEvent {
    /// The station's association ended; the access point runs on.
    StationEnded(PortDisconnect),
    /// The coordinator's policy made the station leave the upstream.
    UpstreamLeft,
}

/// Why an operation of the pair failed.
pub enum PortStaApError<S: PortStationEnv, A: PortApEnv> {
    Station(PortLinkError<PortError<S>>),
    /// The station's join failed; the station is kept for the next one.
    Join(PortAttemptError<S>),
    AccessPoint(PortApError<PortError<A>>),
    Absence(AbsenceError<PortError<S>>),
    /// The station is not connected.
    NoStation,
    /// A connected station must leave before another attempt.
    AlreadyConnected,
    /// Initial scanning is forbidden while the access point runs.
    AccessPointRunning,
    /// A current-channel retry requires a running access point.
    NoAccessPoint,
    /// Finish the owner's announced move before retrying the join.
    ChannelSwitchPending,
    /// An action received outside the sync point that can execute it.
    Unsupported(CoordinatorAction),
}

impl<S: PortStationEnv, A: PortApEnv> core::fmt::Debug for PortStaApError<S, A>
where
    PortLinkError<PortError<S>>: core::fmt::Debug,
    PortAttemptError<S>: core::fmt::Debug,
    PortApError<PortError<A>>: core::fmt::Debug,
    AbsenceError<PortError<S>>: core::fmt::Debug,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Station(error) => f.debug_tuple("Station").field(error).finish(),
            Self::Join(error) => f.debug_tuple("Join").field(error).finish(),
            Self::AccessPoint(error) => f.debug_tuple("AccessPoint").field(error).finish(),
            Self::Absence(error) => f.debug_tuple("Absence").field(error).finish(),
            Self::NoStation => f.write_str("NoStation"),
            Self::AlreadyConnected => f.write_str("AlreadyConnected"),
            Self::AccessPointRunning => f.write_str("AccessPointRunning"),
            Self::NoAccessPoint => f.write_str("NoAccessPoint"),
            Self::ChannelSwitchPending => f.write_str("ChannelSwitchPending"),
            Self::Unsupported(action) => f.debug_tuple("Unsupported").field(action).finish(),
        }
    }
}

/// A retune the coordinator asked for later.
#[derive(Clone, Copy, Debug)]
struct PendingRetune {
    channel: Channel,
    at: Instant,
}

/// The owner of a port a station and an access point share; see the module
/// documentation.
pub struct PortStaAp<'p, S, A, T>
where
    S: PortStationEnv,
    A: PortApEnv<Port = S::Port>,
{
    /// `None` only while the station joins.
    station: Option<PortStation<'p, S>>,
    access_point: PortAccessPoint<'p, A>,
    coordinator: ChannelCoordinator<'p>,
    timer: T,
    retune: Option<PendingRetune>,
    join_channel: Option<Channel>,
    absence: AbsenceState,
    announcing: bool,
    /// The access point runs: started and not stopped since.
    running: bool,
}

impl<'p, S, A, T> PortStaAp<'p, S, A, T>
where
    S: PortStationEnv,
    A: PortApEnv<Port = S::Port>,
    T: Timer,
    S::Port: LowerMacAirReservation,
{
    pub fn new(
        station: PortStation<'p, S>,
        access_point: PortAccessPoint<'p, A>,
        coordinator: ChannelCoordinator<'p>,
        timer: T,
    ) -> Self {
        Self {
            station: Some(station),
            access_point,
            coordinator,
            timer,
            retune: None,
            join_channel: None,
            absence: AbsenceState::new(),
            announcing: false,
            running: false,
        }
    }

    pub fn station(&self) -> &PortStation<'p, S> {
        self.station
            .as_ref()
            .expect("the station is kept between joins")
    }

    pub fn access_point(&self) -> &PortAccessPoint<'p, A> {
        &self.access_point
    }

    pub fn coordinator(&self) -> &ChannelCoordinator<'p> {
        &self.coordinator
    }

    /// Join the upstream, the station alone on the port, then start the
    /// access point on the upstream's channel when its policy lets it.
    pub async fn connect(&mut self) -> Result<(), PortStaApError<S, A>> {
        self.check_absence()?;
        if self
            .station
            .as_ref()
            .ok_or(PortStaApError::NoStation)?
            .connection()
            .is_some()
        {
            return Err(PortStaApError::AlreadyConnected);
        }
        if self.running {
            return Err(PortStaApError::AccessPointRunning);
        }
        let station = self.station.take().ok_or(PortStaApError::NoStation)?;
        let station = match station.connect().await {
            AssociationAttemptOutcome::Connected { connected, .. } => connected,
            AssociationAttemptOutcome::Failed(failure) => {
                let (station, _, _, error, _) = failure.into_parts();
                self.station = Some(station);
                return Err(PortStaApError::Join(error));
            }
        };
        self.station = Some(station);
        let channel = self
            .station()
            .connection()
            .map(|connection| connection.config().channel)
            .ok_or(PortStaApError::NoStation)?;
        self.coordinator.want_access_point();
        let actions = self.coordinator.station_connected(channel);
        self.act(actions).await
    }

    /// Retry the upstream on the access point's current channel. The station
    /// refreshes its candidate there and never tunes the port. The access
    /// point keeps its peers, publishes beacons and delivers Ethernet frames
    /// throughout the attempt, including a failed attempt. Both futures run
    /// to completion; a failed join retains the station for the next retry.
    /// After the station's attempt ends, the current access-point run finishes
    /// at its next TBTT and completes any exchange in progress before returning.
    /// This can delay the return by about one beacon interval.
    ///
    /// The caller chooses when to retry. Off-channel search windows belong
    /// to the owner and are not performed by this operation.
    pub async fn reconnect(
        &mut self,
        access_point_deliver: &mut impl FnMut(PortMsdu<'_, PortRxBuffer<A>>),
    ) -> Result<(), PortStaApError<S, A>> {
        self.check_absence()?;
        if self
            .station
            .as_ref()
            .ok_or(PortStaApError::NoStation)?
            .connection()
            .is_some()
        {
            return Err(PortStaApError::AlreadyConnected);
        }
        if self.retune.is_some() || self.announcing {
            return Err(PortStaApError::ChannelSwitchPending);
        }
        let schedule = self
            .access_point
            .schedule()
            .ok_or(PortStaApError::NoAccessPoint)?;
        self.coordinator.access_point_running(schedule);
        self.join_channel = None;
        self.join_station(schedule.channel, false, access_point_deliver)
            .await
    }

    /// Finish a join without abandoning either client's current exchange.
    async fn join_station(
        &mut self,
        channel: Channel,
        observed: bool,
        access_point_deliver: &mut impl FnMut(PortMsdu<'_, PortRxBuffer<A>>),
    ) -> Result<(), PortStaApError<S, A>> {
        if !self.running {
            self.move_port(channel, false).await?;
            self.join_channel = None;
        }
        let station = self.station.take().ok_or(PortStaApError::NoStation)?;
        let finished = Cell::new(false);
        let (outcome, served) = join(
            async {
                let outcome = if observed {
                    station.connect_observed_on(channel).await
                } else {
                    station.connect_on(channel).await
                };
                finished.set(true);
                outcome
            },
            async {
                while self.running && !finished.get() {
                    let deadline = self
                        .access_point
                        .schedule()
                        .expect("the access point stays running during the join")
                        .next_tbtt;
                    if let Some(event) = self
                        .access_point
                        .run_until(deadline, access_point_deliver)
                        .await?
                    {
                        return Ok(Some(event));
                    }
                }
                Ok(None)
            },
        )
        .await;
        let joined = match outcome {
            AssociationAttemptOutcome::Connected { connected, .. } => {
                self.station = Some(connected);
                Ok(())
            }
            AssociationAttemptOutcome::Failed(failure) => {
                let (station, _, _, error, _) = failure.into_parts();
                self.station = Some(station);
                self.coordinator.upstream_join_failed();
                Err(PortStaApError::Join(error))
            }
        };
        if let Some(PortApEvent::ChannelSwitch { target }) =
            served.map_err(PortStaApError::AccessPoint)?
        {
            let actions = self
                .coordinator
                .access_point_switch_due(target, self.timer.now());
            self.act(actions).await?;
        }
        joined?;
        let channel = self
            .station()
            .connection()
            .ok_or(PortStaApError::NoStation)?
            .config()
            .channel;
        let actions = self.coordinator.station_connected(channel);
        self.act(actions).await
    }

    /// Serve both interfaces until `deadline`: the station's Ethernet frames
    /// go to `station_deliver`, the access point's to `access_point_deliver`.
    /// After `StationEnded`, later calls serve the access point and execute
    /// passive search windows. A discovered upstream is joined on the
    /// owner's channel, after CSA if needed. No active probe runs during
    /// this search or join. A failed join keeps the station and returns
    /// `Join`; the next run resumes searching with the original density.
    /// Started exchanges complete before a sync point; a late exchange or
    /// CTS can skip a window, never extend it. Finishing an already started
    /// exchange or join can extend the caller's deadline while still home.
    pub async fn run_until(
        &mut self,
        deadline: Instant,
        station_deliver: &mut impl FnMut(PortMsdu<'_, PortRxBuffer<S>>),
        access_point_deliver: &mut impl FnMut(PortMsdu<'_, PortRxBuffer<A>>),
    ) -> Result<Option<PortStaApEvent>, PortStaApError<S, A>> {
        self.check_absence()?;
        loop {
            let now = self.timer.now();
            if let Some(retune) = self.retune.filter(|retune| retune.at <= now) {
                self.retune = None;
                self.move_port(retune.channel, false).await?;
                continue;
            }
            if now >= deadline {
                return Ok(None);
            }
            if let Some(channel) = self.join_channel.take() {
                self.join_station(channel, true, access_point_deliver)
                    .await?;
                continue;
            }
            let mut sync = deadline;
            if let Some(retune) = self.retune {
                sync = sync.min(retune.at);
            }
            if let Some(schedule) = self.access_point.schedule() {
                self.coordinator.access_point_running(schedule);
                sync = sync.min(schedule.next_tbtt);
            }
            if let Some(CoordinatorAction::Absence {
                channel,
                start,
                until,
            }) = self.coordinator.poll(now)
            {
                let found = self
                    .search_absence(channel, start, until.min(deadline))
                    .await?;
                if let Some(channel) = found {
                    let actions = self.coordinator.upstream_found(channel);
                    if let Some(event) = self.act_reporting(actions).await? {
                        return Ok(Some(event));
                    }
                }
                continue;
            }
            if let Some(next) = self.coordinator.next_deadline() {
                sync = sync.min(next);
            }
            let searching = self.coordinator.searching();
            let home = self
                .access_point
                .schedule()
                .map(|schedule| schedule.channel);
            let Self {
                station,
                access_point,
                running,
                ..
            } = self;
            let station = station.as_mut().ok_or(PortStaApError::NoStation)?;
            let mut found = None;
            let (station_report, access_point_report) = if *running
                && station.connection().is_none()
            {
                if searching {
                    let (observed, served) = join(
                        station.observe_on_until(home.expect("the running AP has a channel"), sync),
                        access_point.run_until(sync, access_point_deliver),
                    )
                    .await;
                    found = observed.map_err(PortStaApError::Station)?;
                    (Ok(None), Some(served))
                } else {
                    (
                        Ok(None),
                        Some(access_point.run_until(sync, access_point_deliver).await),
                    )
                }
            } else if *running {
                let (station_report, access_point_report) = join(
                    station.run_until(sync, station_deliver),
                    access_point.run_until(sync, access_point_deliver),
                )
                .await;
                (station_report, Some(access_point_report))
            } else {
                if station.connection().is_none() {
                    return Err(PortStaApError::NoStation);
                }
                (station.run_until(sync, station_deliver).await, None)
            };
            let now = self.timer.now();
            match access_point_report.transpose() {
                Ok(Some(Some(PortApEvent::ChannelSwitch { target }))) => {
                    let actions = self.coordinator.access_point_switch_due(target, now);
                    self.act(actions).await?;
                }
                Ok(_) => {}
                Err(error) => return Err(PortStaApError::AccessPoint(error)),
            }
            match station_report.map_err(PortStaApError::Station)? {
                Some(PortStationEvent::ChannelSwitch(switch)) => {
                    let actions = self.coordinator.upstream_switch_announced(
                        switch.target,
                        switch.mode,
                        switch.at,
                    );
                    if let Some(event) = self.act_reporting(actions).await? {
                        return Ok(Some(event));
                    }
                }
                Some(PortStationEvent::Ended(reason)) => {
                    self.coordinator.upstream_lost(now);
                    return Ok(Some(PortStaApEvent::StationEnded(reason)));
                }
                None => {}
            }
            if let Some(channel) = found {
                let actions = self.coordinator.upstream_found(channel);
                if let Some(event) = self.act_reporting(actions).await? {
                    return Ok(Some(event));
                }
            }
        }
    }

    fn check_absence(&self) -> Result<(), PortStaApError<S, A>> {
        if self.absence.recovery_required() {
            return Err(PortStaApError::Absence(AbsenceError::RecoveryRequired));
        }
        Ok(())
    }

    /// Carry out `actions`.
    async fn act(&mut self, actions: CoordinatorActions) -> Result<(), PortStaApError<S, A>> {
        self.act_reporting(actions).await.map(|_| ())
    }

    /// Carry out `actions`; the event one of them ends the run with.
    async fn act_reporting(
        &mut self,
        actions: CoordinatorActions,
    ) -> Result<Option<PortStaApEvent>, PortStaApError<S, A>> {
        let mut event = None;
        for action in actions {
            match action {
                CoordinatorAction::AnnounceSwitch {
                    target,
                    mode,
                    count,
                } => {
                    self.access_point
                        .announce_channel_switch(target, mode, count)
                        .map_err(PortStaApError::AccessPoint)?;
                    self.announcing = true;
                }
                CoordinatorAction::Retune { channel, at } => {
                    if at <= self.timer.now() {
                        self.move_port(channel, self.running).await?;
                    } else {
                        self.retune = Some(PendingRetune { channel, at });
                    }
                }
                CoordinatorAction::StopAccessPoint => {
                    self.access_point
                        .stop()
                        .await
                        .map_err(PortStaApError::AccessPoint)?;
                    self.running = false;
                    self.announcing = false;
                    self.coordinator.access_point_stopped();
                }
                CoordinatorAction::StartAccessPoint { channel } => {
                    self.access_point
                        .start(channel)
                        .map_err(PortStaApError::AccessPoint)?;
                    self.running = true;
                }
                CoordinatorAction::LeaveUpstream => {
                    self.station
                        .as_mut()
                        .ok_or(PortStaApError::NoStation)?
                        .disconnect()
                        .await
                        .map_err(PortStaApError::Station)?;
                    event = Some(PortStaApEvent::UpstreamLeft);
                    self.coordinator.upstream_lost(self.timer.now());
                }
                CoordinatorAction::JoinUpstream { channel } => {
                    self.join_channel = Some(channel);
                }
                action @ CoordinatorAction::Absence { .. } => {
                    return Err(PortStaApError::Unsupported(action));
                }
            }
        }
        Ok(event)
    }

    /// Both clients have completed their run before this receive-only visit.
    async fn search_absence(
        &mut self,
        channel: Channel,
        start: Instant,
        until: Instant,
    ) -> Result<Option<Channel>, PortStaApError<S, A>> {
        let schedule = self
            .access_point
            .schedule()
            .ok_or(PortStaApError::NoAccessPoint)?;
        if until <= start {
            return Err(PortStaApError::Absence(AbsenceError::InvalidWindow));
        }
        if until.saturating_duration_since(start) > Duration::from_millis(20)
            || until >= schedule.next_tbtt
        {
            return Err(PortStaApError::Absence(AbsenceError::InvalidWindow));
        }
        let rate = self.access_point.management_rate();
        let coex = self.access_point.coex_priority();
        let window = AbsenceWindow {
            home: schedule.channel,
            channel,
            start,
            until,
        };
        let Some(absence) = PortAbsence::begin(
            self.access_point.client_mut(),
            &self.timer,
            &mut self.absence,
            window,
            rate,
            coex,
        )
        .await
        .map_err(PortStaApError::Absence)?
        else {
            return Ok(None);
        };
        let found = self
            .station
            .as_mut()
            .ok_or(PortStaApError::NoStation)?
            .observe_on_until(channel, until)
            .await;
        if found.is_ok() {
            self.timer.wait_until(until).await;
        }
        absence.finish().map_err(PortStaApError::Absence)?;
        found.map_err(PortStaApError::Station)
    }

    /// Move the port to `channel` for both interfaces: the access point's
    /// due switch is done when `access_point_switch` says it was due, and
    /// the station's announced one.
    async fn move_port(
        &mut self,
        channel: Channel,
        access_point_switch: bool,
    ) -> Result<(), PortStaApError<S, A>> {
        let station = self.station.as_mut().ok_or(PortStaApError::NoStation)?;
        station
            .link_mut()
            .apply(LowerMacSetting::Channel(channel))
            .map_err(PortStaApError::Station)?;
        if access_point_switch {
            self.access_point
                .channel_switched()
                .map_err(PortStaApError::AccessPoint)?;
            self.announcing = false;
        }
        if station.connection().is_some() {
            station
                .channel_switched(channel)
                .map_err(PortStaApError::Station)?;
        }
        // A port that moved only ever lets a stopped access point start.
        for action in self.coordinator.port_moved(channel) {
            match action {
                CoordinatorAction::StartAccessPoint { channel } => {
                    self.access_point
                        .start(channel)
                        .map_err(PortStaApError::AccessPoint)?;
                    self.running = true;
                }
                CoordinatorAction::JoinUpstream { channel } => {
                    self.join_channel = Some(channel);
                }
                action => return Err(PortStaApError::Unsupported(action)),
            }
        }
        Ok(())
    }
}

/// Run both futures to their ends at once.
async fn join<A: Future, B: Future>(a: A, b: B) -> (A::Output, B::Output) {
    let mut a = pin!(a);
    let mut b = pin!(b);
    let mut a_out = None;
    let mut b_out = None;
    poll_fn(|context| {
        if a_out.is_none()
            && let Poll::Ready(out) = a.as_mut().poll(context)
        {
            a_out = Some(out);
        }
        if b_out.is_none()
            && let Poll::Ready(out) = b.as_mut().poll(context)
        {
            b_out = Some(out);
        }
        if a_out.is_some() && b_out.is_some() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    })
    .await;
    (a_out.expect("joined"), b_out.expect("joined"))
}
