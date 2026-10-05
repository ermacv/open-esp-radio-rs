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

use core::{
    future::{Future, poll_fn},
    pin::pin,
    task::Poll,
};

use oer_ieee80211_ap::coordinator::{ChannelCoordinator, CoordinatorAction, CoordinatorActions};
use oer_ieee80211_ap_service::port::{PortAccessPoint, PortApEnv, PortApError, PortApEvent};
use oer_ieee80211_lower_mac::Channel;
use oer_ieee80211_sta::attempt::AssociationAttemptOutcome;
use oer_ieee80211_sta_service::port::{
    PortAttemptError, PortDisconnect, PortLinkError, PortStation, PortStationEnv, PortStationEvent,
};
use oer_ieee80211_upper_mac_service::client::{PortError, PortMsdu, PortRxBuffer};
use oer_time::{Instant, Timer};

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
    /// The station is not connected.
    NoStation,
    /// An action this owner does not carry out yet: the search for a lost
    /// upstream.
    Unsupported(CoordinatorAction),
}

impl<S: PortStationEnv, A: PortApEnv> core::fmt::Debug for PortStaApError<S, A>
where
    PortLinkError<PortError<S>>: core::fmt::Debug,
    PortAttemptError<S>: core::fmt::Debug,
    PortApError<PortError<A>>: core::fmt::Debug,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Station(error) => f.debug_tuple("Station").field(error).finish(),
            Self::Join(error) => f.debug_tuple("Join").field(error).finish(),
            Self::AccessPoint(error) => f.debug_tuple("AccessPoint").field(error).finish(),
            Self::NoStation => f.write_str("NoStation"),
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
    /// The access point runs: started and not stopped since.
    running: bool,
}

impl<'p, S, A, T> PortStaAp<'p, S, A, T>
where
    S: PortStationEnv,
    A: PortApEnv<Port = S::Port>,
    T: Timer,
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
        let station = self.station.take().ok_or(PortStaApError::NoStation)?;
        let station = match station.connect().await {
            AssociationAttemptOutcome::Connected { connected, .. } => connected,
            AssociationAttemptOutcome::Failed(failure) => {
                let (station, _, _, error, _) = failure.into_parts();
                self.station = Some(station);
                return Err(PortStaApError::Join(error));
            }
        };
        let channel = station
            .connection()
            .map(|connection| connection.config().channel)
            .ok_or(PortStaApError::NoStation)?;
        self.station = Some(station);
        self.coordinator.want_access_point();
        let actions = self.coordinator.station_connected(channel);
        self.act(actions).await
    }

    /// Serve both interfaces until `deadline`: the station's Ethernet frames
    /// go to `station_deliver`, the access point's to `access_point_deliver`.
    pub async fn run_until(
        &mut self,
        deadline: Instant,
        station_deliver: &mut impl FnMut(PortMsdu<'_, PortRxBuffer<S>>),
        access_point_deliver: &mut impl FnMut(PortMsdu<'_, PortRxBuffer<A>>),
    ) -> Result<Option<PortStaApEvent>, PortStaApError<S, A>> {
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
            let mut sync = deadline;
            if let Some(retune) = self.retune {
                sync = sync.min(retune.at);
            }
            if let Some(schedule) = self.access_point.schedule() {
                self.coordinator.access_point_running(schedule);
                sync = sync.min(schedule.next_tbtt);
            }
            let Self {
                station,
                access_point,
                running,
                ..
            } = self;
            let station = station.as_mut().ok_or(PortStaApError::NoStation)?;
            let (station_report, access_point_report) = if *running {
                let (station_report, access_point_report) = join(
                    station.run_until(sync, station_deliver),
                    access_point.run_until(sync, access_point_deliver),
                )
                .await;
                (station_report, Some(access_point_report))
            } else {
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
                    return Ok(Some(PortStaApEvent::StationEnded(reason)));
                }
                None => {}
            }
        }
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
                } => self
                    .access_point
                    .announce_channel_switch(target, mode, count)
                    .map_err(PortStaApError::AccessPoint)?,
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
                }
                action @ (CoordinatorAction::JoinUpstream { .. }
                | CoordinatorAction::Absence { .. }) => {
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
