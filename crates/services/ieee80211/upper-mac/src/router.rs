//! The one consumer of a lower-MAC port's events.
//!
//! A port has exactly one consumer of [`Ieee80211LowerMacPort::next_event`].
//! [`EventRouter::run`] is that consumer: it takes every event and
//! dispatches it to the owner that waits for it, so several exchanges, a
//! receive path and a lifecycle owner share one port:
//!
//! - an attempt completion goes to the exchange that registered its
//!   [`TxId`] ([`EventRouter::completion`]);
//! - received frames and oversize-drop reports go to the bounded receive
//!   queue of the interface they belong to ([`EventRouter::received`]), and
//!   extension events (a TBTT) to the station's ([`EventRouter::extension`]):
//!   every client of the port attaches its interface
//!   ([`EventRouter::attach`]), and with a station and an access point on
//!   one port a frame goes where its addresses put it
//!   ([`classify_sta_ap_rx`]);
//! - lifecycle terminals go to a queue of their own, which the port's owner
//!   reads ([`EventRouter::lifecycle`]);
//! - [`EventsLost`] marks every registered exchange whose completion has not
//!   arrived, which recovers by cancelling its attempt, and is reported in
//!   order by each queue;
//! - the terminal [`Poisoned`] event ends the router and every wait.
//!
//! A bounded queue that overflows reports its own [`EventsLost`] in place of
//! the first dropped entry. A completion no exchange registered, and a frame
//! no attached interface owns, is counted
//! ([`EventRouter::unclaimed_completions`], [`EventRouter::unrouted_frames`])
//! and dropped.
//!
//! The router owns no executor: [`EventRouter::run`] and the waits are
//! futures that share the router by reference on one executor.

use core::{
    cell::RefCell,
    future::{Future, poll_fn},
    pin::pin,
    task::{Poll, Waker},
};

use oer_ieee80211_lower_mac::{
    CorrelationIds, EventsLost, Ieee80211LowerMacPort, LifecycleEvent, LowerMacEvent, MacAddress,
    Poisoned, TxCompletion, TxId, VifId, VifRole,
};
use oer_ieee80211_mac::vif::{StaApRxAddresses, StaApRxRoute, StaApVif, classify_sta_ap_rx};

/// How a wait for an attempt's completion ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Awaited {
    /// The attempt completed.
    Completed(TxCompletion),
    /// Events were lost before the completion arrived: the attempt may have
    /// ended in the gap. Cancel it by its identity, then wait again or
    /// [`EventRouter::resolve`] it.
    Lost,
    /// The port is poisoned.
    Poisoned,
}

/// Every completion slot is registered.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RouterFull;

/// Interfaces one router serves: a station and an access point.
pub const ROUTER_VIFS: usize = 2;

/// Exchanges of one port that wait for a completion at once: two for each
/// interface's client.
pub const PORT_EXCHANGES: usize = 2 * ROUTER_VIFS;

/// Received frames the router keeps for each interface.
pub const PORT_BACKLOG: usize = 4;

/// The event router of a port all its clients share: every client of one
/// port, a station and an access point beside it, takes this one.
pub type PortRouter<'p, P> = EventRouter<'p, P, PORT_EXCHANGES, PORT_BACKLOG>;

/// Why an interface could not attach to the router.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttachError {
    /// The interface is beyond [`ROUTER_VIFS`].
    UnknownVif,
    /// Another client holds the interface.
    Taken,
}

/// The receive identity of an attached interface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Route {
    role: VifRole,
    address: MacAddress,
    /// The BSS a station joined; an access point's is its own address.
    bssid: Option<MacAddress>,
}

/// The interface index `routes` sends `frame` to, if any: the only attached
/// interface takes every frame; a station and an access point split them by
/// their addresses, a station that has not joined a BSS taking what neither
/// proves is the access point's, as it scans.
fn route(routes: &[Option<Route>; ROUTER_VIFS], frame: &[u8]) -> Option<usize> {
    let mut attached = routes
        .iter()
        .enumerate()
        .filter_map(|(index, route)| Some((index, (*route)?)));
    let first = attached.next()?;
    let Some(second) = attached.next() else {
        return Some(first.0);
    };
    let (station, access_point) = match (first.1.role, second.1.role) {
        (VifRole::Station, VifRole::AccessPoint) => (first, second),
        (VifRole::AccessPoint, VifRole::Station) => (second, first),
        // Two interfaces of one role: the frame's receiver decides.
        _ => {
            let receiver: [u8; 6] = frame.get(4..10)?.try_into().ok()?;
            return [first, second]
                .into_iter()
                .find(|(_, route)| route.address == receiver)
                .map(|(index, _)| index);
        }
    };
    let addresses = StaApRxAddresses {
        station: station.1.address,
        station_bssid: station.1.bssid.unwrap_or([0; 6]),
        access_point: access_point.1.address,
    };
    match classify_sta_ap_rx(frame, addresses) {
        StaApRxRoute::Interface(StaApVif::Station) => Some(station.0),
        StaApRxRoute::Interface(StaApVif::AccessPoint) => Some(access_point.0),
        StaApRxRoute::Foreign if station.1.bssid.is_none() => Some(station.0),
        StaApRxRoute::Foreign | StaApRxRoute::Ambiguous | StaApRxRoute::Malformed => None,
    }
}

struct Waiter {
    id: TxId,
    completion: Option<TxCompletion>,
    lost: bool,
    waker: Option<Waker>,
}

/// A bounded queue with a loss marker in place of the first dropped entry.
struct Ring<T, const N: usize> {
    entries: [Option<Result<T, EventsLost>>; N],
    head: usize,
    len: usize,
    /// An entry was dropped and its marker is not queued yet.
    lost: bool,
    waker: Option<Waker>,
}

impl<T, const N: usize> Ring<T, N> {
    fn new() -> Self {
        Self {
            entries: core::array::from_fn(|_| None),
            head: 0,
            len: 0,
            lost: false,
            waker: None,
        }
    }

    fn try_push(&mut self, entry: Result<T, EventsLost>) -> bool {
        if self.len == N {
            return false;
        }
        self.entries[(self.head + self.len) % N] = Some(entry);
        self.len += 1;
        true
    }

    fn push(&mut self, entry: T) {
        if self.lost {
            if !self.try_push(Err(EventsLost)) {
                return;
            }
            self.lost = false;
        }
        if !self.try_push(Ok(entry)) {
            self.lost = true;
        }
        self.wake();
    }

    /// Record a loss of the port that may have taken an entry of this queue.
    fn mark_lost(&mut self) {
        if !self.try_push(Err(EventsLost)) {
            self.lost = true;
        }
        self.wake();
    }

    fn take(&mut self) -> Option<Result<T, EventsLost>> {
        if self.len == 0 {
            return core::mem::take(&mut self.lost).then_some(Err(EventsLost));
        }
        let entry = self.entries[self.head].take();
        self.head = (self.head + 1) % N;
        self.len -= 1;
        entry
    }

    fn wake(&mut self) {
        if let Some(waker) = self.waker.take() {
            waker.wake();
        }
    }
}

/// Lifecycle terminals and extension events the router holds.
const SMALL_QUEUE: usize = 4;

struct State<E, const WAITERS: usize, const RX: usize> {
    waiters: [Option<Waiter>; WAITERS],
    routes: [Option<Route>; ROUTER_VIFS],
    received: [Ring<E, RX>; ROUTER_VIFS],
    lifecycle: Ring<LifecycleEvent, SMALL_QUEUE>,
    extension: [Ring<E, SMALL_QUEUE>; ROUTER_VIFS],
    /// Received frames no attached interface owns.
    unrouted: u32,
    poisoned: bool,
    /// How often the router found the port's queue empty.
    idle: u32,
    /// Waits that need the router to look at the port again.
    router: Option<Waker>,
    ids: CorrelationIds,
    unclaimed: u32,
}

/// The single consumer of a lower-MAC port's events, dispatching them by
/// identity. `WAITERS` bounds the exchanges waiting for a completion at once
/// and `RX` the received frames waiting for the receive path.
pub struct EventRouter<'p, P: Ieee80211LowerMacPort, const WAITERS: usize, const RX: usize> {
    port: &'p P,
    state: RefCell<State<P::Event, WAITERS, RX>>,
}

impl<'p, P: Ieee80211LowerMacPort, const WAITERS: usize, const RX: usize>
    EventRouter<'p, P, WAITERS, RX>
{
    /// A router of `port`'s events whose attempt identities start at
    /// `first_id`. Nothing else may take the port's events.
    pub fn new(port: &'p P, first_id: u32) -> Self {
        Self {
            port,
            state: RefCell::new(State {
                waiters: core::array::from_fn(|_| None),
                routes: [None; ROUTER_VIFS],
                received: core::array::from_fn(|_| Ring::new()),
                lifecycle: Ring::new(),
                extension: core::array::from_fn(|_| Ring::new()),
                unrouted: 0,
                poisoned: false,
                idle: 0,
                router: None,
                ids: CorrelationIds::starting_at(first_id),
                unclaimed: 0,
            }),
        }
    }

    /// The port whose events the router takes.
    pub const fn port(&self) -> &'p P {
        self.port
    }

    /// A fresh attempt identity outside the backend-reserved range, unique
    /// among the router's users until it wraps.
    pub fn next_id(&self) -> TxId {
        self.state.borrow_mut().ids.next()
    }

    /// Whether the port reported its terminal [`Poisoned`] event.
    pub fn poisoned(&self) -> bool {
        self.state.borrow().poisoned
    }

    /// Completions that arrived for no registered identity.
    pub fn unclaimed_completions(&self) -> u32 {
        self.state.borrow().unclaimed
    }

    /// Received frames no attached interface owned.
    pub fn unrouted_frames(&self) -> u32 {
        self.state.borrow().unrouted
    }

    /// Attach interface `vif` of `role` and `address`: from now on the
    /// router queues the frames that belong to it, until the returned
    /// attachment drops.
    pub fn attach(
        &self,
        vif: VifId,
        role: VifRole,
        address: MacAddress,
    ) -> Result<Attachment<'_, 'p, P, WAITERS, RX>, AttachError> {
        let mut state = self.state.borrow_mut();
        let slot = state
            .routes
            .get_mut(usize::from(vif.0))
            .ok_or(AttachError::UnknownVif)?;
        if slot.is_some() {
            return Err(AttachError::Taken);
        }
        *slot = Some(Route {
            role,
            address,
            bssid: (role == VifRole::AccessPoint).then_some(address),
        });
        Ok(Attachment { router: self, vif })
    }

    /// Take and dispatch the port's events until it reports its terminal
    /// [`Poisoned`] event. Poll it for as long as the port is used, beside
    /// the exchanges and the backend's own runner.
    pub async fn run(&self) -> Poisoned {
        loop {
            let event = {
                let mut next = pin!(self.port.next_event());
                poll_fn(|context| match next.as_mut().poll(context) {
                    Poll::Ready(event) => Poll::Ready(event),
                    Poll::Pending => {
                        self.idle(context.waker());
                        Poll::Pending
                    }
                })
                .await
            };
            if self.dispatch(event).is_err() {
                return Poisoned;
            }
        }
    }

    /// The port has nothing queued: every event produced so far is routed.
    fn idle(&self, waker: &Waker) {
        let mut state = self.state.borrow_mut();
        state.idle = state.idle.wrapping_add(1);
        state.router = Some(waker.clone());
        for waiter in state.waiters.iter_mut().flatten() {
            if let Some(waker) = waiter.waker.take() {
                waker.wake();
            }
        }
    }

    fn dispatch(&self, event: Result<P::Event, EventsLost>) -> Result<(), Poisoned> {
        let mut state = self.state.borrow_mut();
        let event = match event {
            Ok(event) => event,
            Err(EventsLost) => {
                for waiter in state.waiters.iter_mut().flatten() {
                    if waiter.completion.is_none() {
                        waiter.lost = true;
                        if let Some(waker) = waiter.waker.take() {
                            waker.wake();
                        }
                    }
                }
                for queue in &mut state.received {
                    queue.mark_lost();
                }
                state.lifecycle.mark_lost();
                for queue in &mut state.extension {
                    queue.mark_lost();
                }
                return Ok(());
            }
        };
        match P::view(&event) {
            LowerMacEvent::TxCompleted(completion) => {
                match state
                    .waiters
                    .iter_mut()
                    .flatten()
                    .find(|waiter| waiter.id == completion.id)
                {
                    Some(waiter) => {
                        waiter.completion = Some(completion);
                        if let Some(waker) = waiter.waker.take() {
                            waker.wake();
                        }
                    }
                    None => state.unclaimed = state.unclaimed.saturating_add(1),
                }
            }
            LowerMacEvent::Received { frame, .. } => match route(&state.routes, frame) {
                Some(index) => state.received[index].push(event),
                None => state.unrouted = state.unrouted.saturating_add(1),
            },
            // An oversize report names no interface: the station's, which
            // counts it, or the only one attached.
            LowerMacEvent::RxTooLong { .. } => match state.station() {
                Some(index) => state.received[index].push(event),
                None => state.unrouted = state.unrouted.saturating_add(1),
            },
            LowerMacEvent::Lifecycle(lifecycle) => state.lifecycle.push(lifecycle),
            // A TBTT is the station's.
            LowerMacEvent::Extension => match state.station() {
                Some(index) => state.extension[index].push(event),
                None => state.unrouted = state.unrouted.saturating_add(1),
            },
            LowerMacEvent::Poisoned(Poisoned) => {
                state.poisoned = true;
                for waiter in state.waiters.iter_mut().flatten() {
                    if let Some(waker) = waiter.waker.take() {
                        waker.wake();
                    }
                }
                for queue in &mut state.received {
                    queue.wake();
                }
                state.lifecycle.wake();
                for queue in &mut state.extension {
                    queue.wake();
                }
                return Err(Poisoned);
            }
        }
        Ok(())
    }

    /// Register `id` before submitting its attempt, so that a completion
    /// that arrives at once is kept for it. The registration ends with the
    /// returned guard.
    pub fn register(&self, id: TxId) -> Result<Registration<'_, 'p, P, WAITERS, RX>, RouterFull> {
        let mut state = self.state.borrow_mut();
        let slot = state
            .waiters
            .iter_mut()
            .find(|slot| slot.is_none())
            .ok_or(RouterFull)?;
        *slot = Some(Waiter {
            id,
            completion: None,
            lost: false,
            waker: None,
        });
        Ok(Registration { router: self, id })
    }

    /// Wait for the completion of a registered attempt. [`Awaited::Lost`]
    /// is reported once per loss.
    pub async fn completion(&self, id: TxId) -> Awaited {
        poll_fn(|context| {
            let mut state = self.state.borrow_mut();
            let poisoned = state.poisoned;
            let Some(waiter) = state
                .waiters
                .iter_mut()
                .flatten()
                .find(|waiter| waiter.id == id)
            else {
                return Poll::Ready(Awaited::Lost);
            };
            if let Some(completion) = waiter.completion.take() {
                return Poll::Ready(Awaited::Completed(completion));
            }
            if poisoned {
                return Poll::Ready(Awaited::Poisoned);
            }
            if core::mem::take(&mut waiter.lost) {
                return Poll::Ready(Awaited::Lost);
            }
            waiter.waker = Some(context.waker().clone());
            Poll::Pending
        })
        .await
    }

    /// Decide an attempt whose cancellation was refused as not running
    /// after a loss: its completion, if the router still takes it from the
    /// port, or `None` once the port's queue was empty without it.
    pub async fn resolve(&self, id: TxId) -> Option<TxCompletion> {
        let start = {
            let mut state = self.state.borrow_mut();
            // Make the router look at the port again.
            if let Some(router) = state.router.take() {
                router.wake();
            }
            state.idle
        };
        poll_fn(|context| {
            let mut state = self.state.borrow_mut();
            let idle = state.idle;
            let Some(waiter) = state
                .waiters
                .iter_mut()
                .flatten()
                .find(|waiter| waiter.id == id)
            else {
                return Poll::Ready(None);
            };
            if let Some(completion) = waiter.completion.take() {
                return Poll::Ready(Some(completion));
            }
            if idle != start {
                return Poll::Ready(None);
            }
            waiter.waker = Some(context.waker().clone());
            if let Some(router) = state.router.take() {
                router.wake();
            }
            Poll::Pending
        })
        .await
    }

    /// The next received frame or oversize-drop report of interface `vif`,
    /// viewed through the port; [`EventsLost`] when frames were dropped.
    /// `None` once the port is poisoned and the queue is empty, and for an
    /// interface beyond [`ROUTER_VIFS`].
    pub async fn received(&self, vif: VifId) -> Option<Result<P::Event, EventsLost>> {
        poll_fn(|context| {
            let mut state = self.state.borrow_mut();
            let poisoned = state.poisoned;
            let Some(queue) = state.received.get_mut(usize::from(vif.0)) else {
                return Poll::Ready(None);
            };
            match queue.take() {
                Some(entry) => Poll::Ready(Some(entry)),
                None if poisoned => Poll::Ready(None),
                None => {
                    queue.waker = Some(context.waker().clone());
                    Poll::Pending
                }
            }
        })
        .await
    }

    /// The next lifecycle terminal; `None` once the port is poisoned and
    /// the queue is empty.
    pub async fn lifecycle(&self) -> Option<Result<LifecycleEvent, EventsLost>> {
        poll_fn(|context| {
            let mut state = self.state.borrow_mut();
            let poisoned = state.poisoned;
            let queue = &mut state.lifecycle;
            match queue.take() {
                Some(entry) => Poll::Ready(Some(entry)),
                None if poisoned => Poll::Ready(None),
                None => {
                    queue.waker = Some(context.waker().clone());
                    Poll::Pending
                }
            }
        })
        .await
    }

    /// The next extension event (a TBTT) of interface `vif`, read through
    /// the extension's view; `None` once the port is poisoned and the queue
    /// is empty, and for an interface beyond [`ROUTER_VIFS`].
    pub async fn extension(&self, vif: VifId) -> Option<Result<P::Event, EventsLost>> {
        poll_fn(|context| {
            let mut state = self.state.borrow_mut();
            let poisoned = state.poisoned;
            let Some(queue) = state.extension.get_mut(usize::from(vif.0)) else {
                return Poll::Ready(None);
            };
            match queue.take() {
                Some(entry) => Poll::Ready(Some(entry)),
                None if poisoned => Poll::Ready(None),
                None => {
                    queue.waker = Some(context.waker().clone());
                    Poll::Pending
                }
            }
        })
        .await
    }
}

impl<E, const WAITERS: usize, const RX: usize> State<E, WAITERS, RX> {
    /// The station interface attached, or the only interface.
    fn station(&self) -> Option<usize> {
        let mut attached = self
            .routes
            .iter()
            .enumerate()
            .filter_map(|(index, route)| Some((index, (*route)?)));
        let first = attached.next()?;
        match attached.next() {
            None => Some(first.0),
            Some(second) => [first, second]
                .into_iter()
                .find(|(_, route)| route.role == VifRole::Station)
                .map(|(index, _)| index),
        }
    }
}

/// An interface attached to the router; dropping it detaches the interface
/// and discards the frames queued for it.
pub struct Attachment<'r, 'p, P: Ieee80211LowerMacPort, const WAITERS: usize, const RX: usize> {
    router: &'r EventRouter<'p, P, WAITERS, RX>,
    vif: VifId,
}

impl<P: Ieee80211LowerMacPort, const WAITERS: usize, const RX: usize>
    Attachment<'_, '_, P, WAITERS, RX>
{
    /// The attached interface.
    pub const fn vif(&self) -> VifId {
        self.vif
    }

    /// The BSS the station joined, or `None` while it has none; the frames
    /// of that BSS are its own from now on.
    pub fn set_bssid(&self, bssid: Option<MacAddress>) {
        let mut state = self.router.state.borrow_mut();
        if let Some(Some(route)) = state.routes.get_mut(usize::from(self.vif.0))
            && route.role == VifRole::Station
        {
            route.bssid = bssid;
        }
    }
}

impl<P: Ieee80211LowerMacPort, const WAITERS: usize, const RX: usize> Drop
    for Attachment<'_, '_, P, WAITERS, RX>
{
    fn drop(&mut self) {
        let mut state = self.router.state.borrow_mut();
        let index = usize::from(self.vif.0);
        if let Some(route) = state.routes.get_mut(index) {
            *route = None;
        }
        if let Some(queue) = state.received.get_mut(index) {
            while queue.take().is_some() {}
        }
        if let Some(queue) = state.extension.get_mut(index) {
            while queue.take().is_some() {}
        }
    }
}

/// A registered attempt identity; dropping it ends the registration and
/// discards a completion nobody took.
pub struct Registration<'r, 'p, P: Ieee80211LowerMacPort, const WAITERS: usize, const RX: usize> {
    router: &'r EventRouter<'p, P, WAITERS, RX>,
    id: TxId,
}

impl<P: Ieee80211LowerMacPort, const WAITERS: usize, const RX: usize>
    Registration<'_, '_, P, WAITERS, RX>
{
    /// The registered identity.
    pub const fn id(&self) -> TxId {
        self.id
    }
}

impl<P: Ieee80211LowerMacPort, const WAITERS: usize, const RX: usize> Drop
    for Registration<'_, '_, P, WAITERS, RX>
{
    fn drop(&mut self) {
        let mut state = self.router.state.borrow_mut();
        for slot in &mut state.waiters {
            if slot.as_ref().is_some_and(|waiter| waiter.id == self.id) {
                *slot = None;
            }
        }
    }
}
