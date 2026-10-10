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
//! channel. [`CurrentChannel`] needs no optional absence capability and
//! carries no off-channel policy; its [`UpstreamLossPolicy`] bounds the
//! wait: after it, the access point stops, the station scans alone and
//! rejoins, and the access point starts again. [`ProtectedSearch`] additionally executes
//! the coordinator's protected, receive-only absences, requiring both
//! [`LowerMacAirReservation`] and [`LowerMacLiveRetune`].
//! A found upstream is joined without a probe, after the access point's
//! CSA when a move is needed. [`PortStaAp::reconnect`] remains an explicit
//! current-channel discovery and retry.

use core::{
    cell::Cell,
    future::{Future, poll_fn},
    pin::pin,
    task::Poll,
};

use oer_ieee80211_ap::coordinator::{
    ApFollowPolicy, ApSearchPolicy, ChannelCoordinator, CoordinatorAction, CoordinatorActions,
    UpstreamLossPolicy,
};
use oer_ieee80211_ap_service::port::{PortAccessPoint, PortApEnv, PortApError, PortApEvent};
use oer_ieee80211_lower_mac::{Channel, LowerMacAirReservation, LowerMacLiveRetune};
use oer_ieee80211_sta::attempt::AssociationAttemptOutcome;
use oer_ieee80211_sta_service::port::{
    PortAttemptError, PortDisconnect, PortLinkError, PortStation, PortStationEnv, PortStationEvent,
};
use oer_ieee80211_upper_mac_service::absence::{
    AbsenceError, AbsenceState, AbsenceWindow, PortAbsence,
};
use oer_ieee80211_upper_mac_service::client::{PortFault, PortMsdu, PortRxBuffer};
use oer_time::{Instant, Timer};

/// Why [`PortStaAp::run_until`] returned before its deadline. Each is an
/// outcome the pair's policy expects: the pair stays whole and the next
/// `run_until` carries on.
pub enum PortStaApEvent<S: PortStationEnv> {
    /// The station's association ended; the access point runs on.
    StationEnded(PortDisconnect),
    /// The coordinator's policy made the station leave the upstream.
    UpstreamLeft,
    /// An attempt the pair made on its own to join the upstream, a passive
    /// rejoin or the [`UpstreamLossPolicy`]'s scan, did not join it, for
    /// this reason. The access point runs and the station is kept for the
    /// next attempt.
    UpstreamNotJoined(PortAttemptError<S>),
}

impl<S: PortStationEnv> core::fmt::Debug for PortStaApEvent<S>
where
    PortAttemptError<S>: core::fmt::Debug,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::StationEnded(reason) => f.debug_tuple("StationEnded").field(reason).finish(),
            Self::UpstreamLeft => f.write_str("UpstreamLeft"),
            Self::UpstreamNotJoined(error) => {
                f.debug_tuple("UpstreamNotJoined").field(error).finish()
            }
        }
    }
}

impl<S: PortStationEnv> PartialEq for PortStaApEvent<S>
where
    PortAttemptError<S>: PartialEq,
{
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::StationEnded(first), Self::StationEnded(second)) => first == second,
            (Self::UpstreamLeft, Self::UpstreamLeft) => true,
            (Self::UpstreamNotJoined(first), Self::UpstreamNotJoined(second)) => first == second,
            _ => false,
        }
    }
}

/// Why an operation of the pair failed.
pub enum PortStaApError<S: PortStationEnv, A: PortApEnv> {
    Station(PortLinkError<PortFault<S>>),
    /// The station's join failed; the station is kept for the next one.
    Join(PortAttemptError<S>),
    AccessPoint(PortApError<PortFault<A>>),
    Absence(AbsenceError<PortFault<S>>),
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
    PortLinkError<PortFault<S>>: core::fmt::Debug,
    PortAttemptError<S>: core::fmt::Debug,
    PortApError<PortFault<A>>: core::fmt::Debug,
    AbsenceError<PortFault<S>>: core::fmt::Debug,
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

/// Observe the upstream only on the AP's operating channel. No CTS or
/// enabled-port retune extension is needed; there is no off-channel policy.
/// The station cannot search elsewhere while the access point serves, so
/// `upstream_loss` states what happens when the upstream does not come back.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CurrentChannel {
    pub upstream_loss: UpstreamLossPolicy,
}

/// Search in the coordinator's protected receive-only windows. Requires
/// both CTS-to-self and temporary retuning without a lifecycle transition.
pub struct ProtectedSearch;

/// The common home-channel loop stops here so only the protected mode
/// needs to instantiate the absence driver and its backend bounds, and only
/// the current-channel mode carries out its upstream loss policy.
enum SyncReport<S: PortStationEnv> {
    Finished(Option<PortStaApEvent<S>>),
    /// The upstream loss policy is due.
    UpstreamLoss,
    Absence {
        channel: Channel,
        start: Instant,
        until: Instant,
    },
}

/// The owner of a port a station and an access point share; see the module
/// documentation.
pub struct PortStaAp<'p, S, A, T, Search = CurrentChannel>
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
    search: Search,
}

impl<'p, S, A, T, Search> PortStaAp<'p, S, A, T, Search>
where
    S: PortStationEnv,
    A: PortApEnv<Port = S::Port>,
    T: Timer,
{
    fn from_parts(
        station: PortStation<'p, S>,
        access_point: PortAccessPoint<'p, A>,
        coordinator: ChannelCoordinator<'p>,
        search: Search,
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
            search,
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
            .await?
            .map_err(PortStaApError::Join)
    }

    /// Finish a join without abandoning either client's current exchange:
    /// the inner `Err` is a failed attempt, which keeps the station.
    async fn join_station(
        &mut self,
        channel: Channel,
        observed: bool,
        access_point_deliver: &mut impl FnMut(PortMsdu<'_, PortRxBuffer<A>>),
    ) -> Result<Result<(), PortAttemptError<S>>, PortStaApError<S, A>> {
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
                Err(error)
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
        if let Err(error) = joined {
            return Ok(Err(error));
        }
        let channel = self
            .station()
            .connection()
            .ok_or(PortStaApError::NoStation)?
            .config()
            .channel;
        let actions = self.coordinator.station_connected(channel);
        self.act(actions).await.map(Ok)
    }

    /// Complete home-channel work until a report, a receive-only window or
    /// the `upstream_loss` policy's due instant.
    async fn run_until_sync(
        &mut self,
        deadline: Instant,
        upstream_loss: Option<UpstreamLossPolicy>,
        station_deliver: &mut impl FnMut(PortMsdu<'_, PortRxBuffer<S>>),
        access_point_deliver: &mut impl FnMut(PortMsdu<'_, PortRxBuffer<A>>),
    ) -> Result<SyncReport<S>, PortStaApError<S, A>> {
        self.check_absence()?;
        loop {
            let now = self.timer.now();
            if let Some(retune) = self.retune.filter(|retune| retune.at <= now) {
                self.retune = None;
                self.move_port(retune.channel, false).await?;
                continue;
            }
            if now >= deadline {
                return Ok(SyncReport::Finished(None));
            }
            if let Some(channel) = self.join_channel.take() {
                if let Err(error) = self
                    .join_station(channel, true, access_point_deliver)
                    .await?
                {
                    return Ok(SyncReport::Finished(Some(
                        PortStaApEvent::UpstreamNotJoined(error),
                    )));
                }
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
            if let Some(due) =
                upstream_loss.and_then(|policy| self.coordinator.upstream_loss_due(policy))
            {
                if now >= due {
                    return Ok(SyncReport::UpstreamLoss);
                }
                sync = sync.min(due);
            }
            if let Some(CoordinatorAction::Absence {
                channel,
                start,
                until,
            }) = self.coordinator.poll(now)
            {
                return Ok(SyncReport::Absence {
                    channel,
                    start,
                    until,
                });
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
            let (station_report, access_point_report) =
                if *running && station.connection().is_none() {
                    // A restarted access point has no schedule until its first
                    // beacon; the station observes from then on.
                    if let (true, Some(home)) = (searching, home) {
                        let (observed, served) = join(
                            station.observe_on_until(home, sync),
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
                        return Ok(SyncReport::Finished(Some(event)));
                    }
                }
                Some(PortStationEvent::Ended(reason)) => {
                    self.coordinator.upstream_lost(now);
                    return Ok(SyncReport::Finished(Some(PortStaApEvent::StationEnded(
                        reason,
                    ))));
                }
                None => {}
            }
            if let Some(channel) = found {
                let actions = self.coordinator.upstream_found(channel);
                if let Some(event) = self.act_reporting(actions).await? {
                    return Ok(SyncReport::Finished(Some(event)));
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
    ) -> Result<Option<PortStaApEvent<S>>, PortStaApError<S, A>> {
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
            .retune(channel)
            .await
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

impl<'p, S, A, T> PortStaAp<'p, S, A, T, CurrentChannel>
where
    S: PortStationEnv,
    A: PortApEnv<Port = S::Port>,
    T: Timer,
{
    /// Build the explicitly current-channel pair. Its coordinator has no
    /// off-channel search list; neither optional absence capability is
    /// required. Permanent channel moves still follow `policy`, and
    /// `upstream_loss` bounds the wait for a lost upstream.
    pub fn new(
        station: PortStation<'p, S>,
        access_point: PortAccessPoint<'p, A>,
        policy: ApFollowPolicy,
        upstream_loss: UpstreamLossPolicy,
        timer: T,
    ) -> Self {
        Self::from_parts(
            station,
            access_point,
            ChannelCoordinator::new(policy, ApSearchPolicy::new(&[])),
            CurrentChannel { upstream_loss },
            timer,
        )
    }

    /// Serve both interfaces until `deadline`, following permanent channel
    /// moves. After `StationEnded`, listen passively on the AP's channel
    /// and join an observed upstream there without another scan or probe.
    /// When the upstream stays lost for the [`UpstreamLossPolicy`]'s
    /// `after`, the access point stops, the station scans alone and joins
    /// the upstream, and the access point starts on the upstream's
    /// channel; a scan that joins none starts it again on its previous
    /// channel and ends this call with
    /// [`PortStaApEvent::UpstreamNotJoined`], and the next attempt follows
    /// `after` later.
    /// Started exchanges and joins complete before returning, so they can
    /// extend the caller's deadline while still home. A failed join ends the
    /// call with [`PortStaApEvent::UpstreamNotJoined`] and keeps the station
    /// for the next one.
    pub async fn run_until(
        &mut self,
        deadline: Instant,
        station_deliver: &mut impl FnMut(PortMsdu<'_, PortRxBuffer<S>>),
        access_point_deliver: &mut impl FnMut(PortMsdu<'_, PortRxBuffer<A>>),
    ) -> Result<Option<PortStaApEvent<S>>, PortStaApError<S, A>> {
        loop {
            match self
                .run_until_sync(
                    deadline,
                    Some(self.search.upstream_loss),
                    station_deliver,
                    access_point_deliver,
                )
                .await?
            {
                SyncReport::Finished(event) => return Ok(event),
                SyncReport::UpstreamLoss => {
                    if let Some(event) = self.rescan_upstream().await? {
                        return Ok(Some(event));
                    }
                }
                // Only the constructors can install a coordinator, and this
                // mode's constructor installs an empty off-channel list.
                SyncReport::Absence {
                    channel,
                    start,
                    until,
                } => {
                    return Err(PortStaApError::Unsupported(CoordinatorAction::Absence {
                        channel,
                        start,
                        until,
                    }));
                }
            }
        }
    }

    /// Carry out the upstream loss policy: stop the access point, scan and
    /// join alone on the port, and start the access point on the upstream's
    /// channel, or on its previous one when no upstream was joined.
    async fn rescan_upstream(&mut self) -> Result<Option<PortStaApEvent<S>>, PortStaApError<S, A>> {
        let stop = self.coordinator.upstream_rescan();
        self.act(stop).await?;
        let mut station = self.station.take().ok_or(PortStaApError::NoStation)?;
        // A full scan, not the candidate the lost upstream left.
        station.set_refresh(true);
        match station.connect().await {
            AssociationAttemptOutcome::Connected { connected, .. } => {
                let channel = connected
                    .connection()
                    .map(|connection| connection.config().channel);
                self.station = Some(connected);
                let channel = channel.ok_or(PortStaApError::NoStation)?;
                let start = self.coordinator.station_connected(channel);
                self.act(start).await.map(|()| None)
            }
            AssociationAttemptOutcome::Failed(failure) => {
                let (station, _, _, error, _) = failure.into_parts();
                self.station = Some(station);
                let restart = self.coordinator.upstream_rescan_failed(self.timer.now());
                self.act(restart).await?;
                Ok(Some(PortStaApEvent::UpstreamNotJoined(error)))
            }
        }
    }
}

impl<'p, S, A, T> PortStaAp<'p, S, A, T, ProtectedSearch>
where
    S: PortStationEnv,
    A: PortApEnv<Port = S::Port>,
    T: Timer,
    S::Port: LowerMacAirReservation + LowerMacLiveRetune,
{
    /// Build the explicitly protected-window pair. Both backend extensions
    /// are required at construction and on this mode's run path. An empty
    /// search list is allowed, but never substitutes for a missing extension.
    ///
    /// Air reservation alone cannot construct the protected mode:
    ///
    /// ```compile_fail
    /// use oer_ieee80211_ap::coordinator::ChannelCoordinator;
    /// use oer_ieee80211_ap_service::port::{PortAccessPoint, PortApEnv};
    /// use oer_ieee80211_lower_mac::LowerMacAirReservation;
    /// use oer_ieee80211_sta_service::port::{PortStation, PortStationEnv};
    /// use oer_ieee80211_sta_ap_service::PortStaAp;
    /// use oer_time::Timer;
    /// fn build<'p, S, A, T>(station: PortStation<'p, S>, ap: PortAccessPoint<'p, A>, coordinator: ChannelCoordinator<'p>, timer: T)
    /// where S: PortStationEnv, A: PortApEnv<Port = S::Port>, T: Timer, S::Port: LowerMacAirReservation {
    ///     let _ = PortStaAp::new_with_search(station, ap, coordinator, timer);
    /// }
    /// ```
    ///
    /// Live retuning alone cannot construct it either:
    ///
    /// ```compile_fail
    /// use oer_ieee80211_ap::coordinator::ChannelCoordinator;
    /// use oer_ieee80211_ap_service::port::{PortAccessPoint, PortApEnv};
    /// use oer_ieee80211_lower_mac::LowerMacLiveRetune;
    /// use oer_ieee80211_sta_service::port::{PortStation, PortStationEnv};
    /// use oer_ieee80211_sta_ap_service::PortStaAp;
    /// use oer_time::Timer;
    /// fn build<'p, S, A, T>(station: PortStation<'p, S>, ap: PortAccessPoint<'p, A>, coordinator: ChannelCoordinator<'p>, timer: T)
    /// where S: PortStationEnv, A: PortApEnv<Port = S::Port>, T: Timer, S::Port: LowerMacLiveRetune {
    ///     let _ = PortStaAp::new_with_search(station, ap, coordinator, timer);
    /// }
    /// ```
    pub fn new_with_search(
        station: PortStation<'p, S>,
        access_point: PortAccessPoint<'p, A>,
        coordinator: ChannelCoordinator<'p>,
        timer: T,
    ) -> Self {
        Self::from_parts(station, access_point, coordinator, ProtectedSearch, timer)
    }

    /// Serve both interfaces until `deadline`. After `StationEnded`, listen
    /// on the AP's channel and in the coordinator's protected passive
    /// windows. Join a discovered upstream without a probe, after CSA if
    /// needed. A failed join ends the call with
    /// [`PortStaApEvent::UpstreamNotJoined`] and retains the station and
    /// original search density.
    /// Started exchanges complete before a sync point; a late exchange or
    /// CTS skips a window rather than extending it. An expired window or
    /// one reaching the next TBTT is skipped. Finishing an already started
    /// exchange or join can extend the caller's deadline while still home.
    pub async fn run_until(
        &mut self,
        deadline: Instant,
        station_deliver: &mut impl FnMut(PortMsdu<'_, PortRxBuffer<S>>),
        access_point_deliver: &mut impl FnMut(PortMsdu<'_, PortRxBuffer<A>>),
    ) -> Result<Option<PortStaApEvent<S>>, PortStaApError<S, A>> {
        loop {
            match self
                .run_until_sync(deadline, None, station_deliver, access_point_deliver)
                .await?
            {
                SyncReport::Finished(event) => return Ok(event),
                // This mode carries no upstream loss policy.
                SyncReport::UpstreamLoss => unreachable!("no upstream loss policy"),
                SyncReport::Absence {
                    channel,
                    start,
                    until,
                } => {
                    if let Some(channel) = self
                        .search_absence(channel, start, until.min(deadline))
                        .await?
                    {
                        let actions = self.coordinator.upstream_found(channel);
                        if let Some(event) = self.act_reporting(actions).await? {
                            return Ok(Some(event));
                        }
                    }
                }
            }
        }
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
        if until <= start
            || until.saturating_duration_since(start) > self.coordinator.search_policy().dwell
        {
            return Err(PortStaApError::Absence(AbsenceError::InvalidWindow));
        }
        if self.timer.now() >= until || until >= schedule.next_tbtt {
            return Ok(None);
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
