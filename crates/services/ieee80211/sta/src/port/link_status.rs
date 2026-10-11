//! The station's link as its applications read it: one stream of link
//! states with a single owned publisher.
//!
//! [`StationLinkStatus`] is the storage, a `static` the composition places
//! where every reader finds it. The station's owner takes the one
//! [`StationLinkPublisher`] ([`StationLinkStatus::publisher`]) and reports
//! each edge through it. Dropping the publisher closes the stream, however
//! the owner ended (a finished run, a cancelled task, a lost port), so a
//! reader never takes the last state of a producer that is gone for the
//! current one: every read is a [`StationLinkRead`], `Open` while a
//! publisher lives and `Closed` with the last snapshot once it is gone, so
//! an edge published just before the drop (a fault, most often) still
//! reaches a waiter, inside the closure. A closed stream reopens only with
//! the next publisher.

use core::{
    cell::RefCell,
    future::poll_fn,
    task::{Context, Poll},
};

use embassy_sync::{
    blocking_mutex::{Mutex, raw::RawMutex},
    waitqueue::MultiWakerRegistration,
};
use oer_ieee80211_sta::station::StaFaultReason;

use super::{PortConnectionConfig, PortDisconnect};

/// Readers that may wait at once.
pub const STATION_LINK_WAITERS: usize = 4;

/// The state of the station's link.
#[derive(Clone, Copy, Debug)]
pub enum StationLinkState {
    /// Not associated: never yet in this run, or since the association
    /// ended.
    Disconnected(Option<PortDisconnect>),
    /// Associated with this configuration.
    Connected(PortConnectionConfig),
    /// The station failed and its owner keeps what it could not return. It
    /// ends its run: a later run, once the port's owner restored the port,
    /// opens with `Disconnected(None)`.
    Faulted(StaFaultReason),
}

/// The link at one revision.
#[derive(Clone, Copy, Debug)]
pub struct StationLinkSnapshot {
    /// Advances with every change, across publishers.
    pub revision: u64,
    pub state: StationLinkState,
    /// Bit per QoS TID whose TX Block Ack agreement is operational; zero
    /// outside an association.
    pub tx_block_ack_operational_tids: u8,
}

/// One read of the stream.
#[derive(Clone, Copy, Debug)]
pub enum StationLinkRead {
    /// A publisher lives: the link now.
    Open(StationLinkSnapshot),
    /// No publisher lives: it was dropped, or none was taken yet. The
    /// snapshot is the last one, which no producer stands behind.
    Closed(StationLinkSnapshot),
}

impl StationLinkRead {
    /// The snapshot either way.
    pub const fn snapshot(&self) -> &StationLinkSnapshot {
        match self {
            Self::Open(snapshot) | Self::Closed(snapshot) => snapshot,
        }
    }

    /// The link of a live producer; `None` once the stream closed.
    pub const fn open(self) -> Option<StationLinkSnapshot> {
        match self {
            Self::Open(snapshot) => Some(snapshot),
            Self::Closed(_) => None,
        }
    }
}

struct Inner {
    snapshot: StationLinkSnapshot,
    /// A publisher exists.
    open: bool,
    waiters: MultiWakerRegistration<STATION_LINK_WAITERS>,
}

impl Inner {
    const fn read(&self) -> StationLinkRead {
        if self.open {
            StationLinkRead::Open(self.snapshot)
        } else {
            StationLinkRead::Closed(self.snapshot)
        }
    }
}

/// The storage of the station's link stream.
pub struct StationLinkStatus<M: RawMutex> {
    inner: Mutex<M, RefCell<Inner>>,
}

impl<M: RawMutex> StationLinkStatus<M> {
    /// A closed stream at revision 0, before any publisher.
    pub const fn new() -> Self {
        Self {
            inner: Mutex::new(RefCell::new(Inner {
                snapshot: StationLinkSnapshot {
                    revision: 0,
                    state: StationLinkState::Disconnected(None),
                    tx_block_ack_operational_tids: 0,
                },
                open: false,
                waiters: MultiWakerRegistration::new(),
            })),
        }
    }

    /// The one publisher, opening the stream disconnected at a new
    /// revision; `None` while another publisher lives.
    pub fn publisher(&self) -> Option<StationLinkPublisher<'_, M>> {
        let opened = self.update(|inner| {
            if inner.open {
                return false;
            }
            inner.open = true;
            inner.snapshot = StationLinkSnapshot {
                revision: inner.snapshot.revision + 1,
                state: StationLinkState::Disconnected(None),
                tx_block_ack_operational_tids: 0,
            };
            true
        });
        opened.then_some(StationLinkPublisher { status: self })
    }

    /// The stream now.
    pub fn read(&self) -> StationLinkRead {
        self.inner.lock(|inner| inner.borrow().read())
    }

    /// Wait for a revision after `seen` or the closure, whichever comes
    /// first. A closed stream answers at once with its last snapshot, which
    /// may still be a revision after `seen` the reader has not taken (a
    /// fault published just before the drop).
    pub async fn changed_since(&self, seen: u64) -> StationLinkRead {
        poll_fn(|context: &mut Context<'_>| {
            self.inner.lock(|inner| {
                let mut inner = inner.borrow_mut();
                if inner.snapshot.revision > seen || !inner.open {
                    return Poll::Ready(inner.read());
                }
                inner.waiters.register(context.waker());
                Poll::Pending
            })
        })
        .await
    }

    /// Change the stream under its lock and wake its waiters.
    fn update<R>(&self, change: impl FnOnce(&mut Inner) -> R) -> R {
        self.inner.lock(|inner| {
            let mut inner = inner.borrow_mut();
            let result = change(&mut inner);
            inner.waiters.wake();
            result
        })
    }
}

impl<M: RawMutex> Default for StationLinkStatus<M> {
    fn default() -> Self {
        Self::new()
    }
}

/// The one producer of a [`StationLinkStatus`]. It is neither `Copy` nor
/// `Clone`; dropping it closes the stream.
#[must_use = "dropping the publisher closes the station's link stream"]
pub struct StationLinkPublisher<'s, M: RawMutex> {
    status: &'s StationLinkStatus<M>,
}

impl<M: RawMutex> StationLinkPublisher<'_, M> {
    /// A new run of the station, after a fault: nothing associated.
    pub fn restarted(&mut self) {
        self.publish(StationLinkState::Disconnected(None));
    }

    /// The station associated.
    pub fn connected(&mut self, config: PortConnectionConfig) {
        self.publish(StationLinkState::Connected(config));
    }

    /// The association ended.
    pub fn disconnected(&mut self, reason: PortDisconnect) {
        self.publish(StationLinkState::Disconnected(Some(reason)));
    }

    /// The station failed: the last edge of its run.
    pub fn faulted(&mut self, reason: StaFaultReason) {
        self.publish(StationLinkState::Faulted(reason));
    }

    /// The TIDs whose TX Block Ack agreement is operational, one bit each;
    /// a new revision only when they changed.
    pub fn tx_block_ack(&mut self, operational_tids: u8) {
        self.status.update(|inner| {
            if inner.snapshot.tx_block_ack_operational_tids != operational_tids {
                inner.snapshot.tx_block_ack_operational_tids = operational_tids;
                inner.snapshot.revision += 1;
            }
        });
    }

    fn publish(&mut self, state: StationLinkState) {
        self.status.update(|inner| {
            inner.snapshot = StationLinkSnapshot {
                revision: inner.snapshot.revision + 1,
                state,
                tx_block_ack_operational_tids: 0,
            };
        });
    }
}

impl<M: RawMutex> Drop for StationLinkPublisher<'_, M> {
    /// The producer is gone: the stream closes at its last revision.
    fn drop(&mut self) {
        self.status.update(|inner| inner.open = false);
    }
}

#[cfg(test)]
mod tests {
    use core::{
        future::Future,
        pin::pin,
        task::{Context, Poll, Waker},
    };

    use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    use oer_ieee80211_sta::station::{StaFaultCause, StaFaultPhase, StaFaultReason};

    use super::*;

    type Status = StationLinkStatus<NoopRawMutex>;

    fn poll<F: Future>(future: core::pin::Pin<&mut F>) -> Poll<F::Output> {
        future.poll(&mut Context::from_waker(Waker::noop()))
    }

    fn open(read: StationLinkRead) -> StationLinkSnapshot {
        read.open().expect("the stream is open")
    }

    fn closed(read: StationLinkRead) -> StationLinkSnapshot {
        let StationLinkRead::Closed(snapshot) = read else {
            panic!("the stream is open");
        };
        snapshot
    }

    #[test]
    fn the_stream_is_closed_until_a_publisher_opens_it() {
        let status = Status::new();
        assert_eq!(closed(status.read()).revision, 0);
        let publisher = status.publisher().unwrap();
        let snapshot = open(status.read());
        assert_eq!(snapshot.revision, 1);
        assert!(matches!(
            snapshot.state,
            StationLinkState::Disconnected(None)
        ));
        // One producer at a time.
        assert!(status.publisher().is_none());
        drop(publisher);
    }

    #[test]
    fn dropping_the_publisher_closes_the_stream_and_wakes_its_waiters() {
        let status = Status::new();
        let mut publisher = status.publisher().unwrap();
        publisher.disconnected(PortDisconnect::BeaconLoss);
        let seen = open(status.read()).revision;
        let mut waiter = pin!(status.changed_since(seen));
        assert!(poll(waiter.as_mut()).is_pending());
        drop(publisher);
        let Poll::Ready(read) = poll(waiter.as_mut()) else {
            panic!("the closure wakes the waiter");
        };
        assert_eq!(closed(read).revision, seen);
        let last = closed(status.read());
        assert_eq!(last.revision, seen);
        assert!(matches!(
            last.state,
            StationLinkState::Disconnected(Some(PortDisconnect::BeaconLoss))
        ));
        // The next publisher reopens it at a later revision: a reader never
        // takes a state of the producer that is gone.
        let _publisher = status.publisher().unwrap();
        assert_eq!(open(status.read()).revision, seen + 1);
    }

    #[test]
    fn a_fault_is_an_edge_and_closure_is_not() {
        let status = Status::new();
        let mut publisher = status.publisher().unwrap();
        let reason = StaFaultReason::new(StaFaultPhase::Connected, StaFaultCause::PortLost);
        let mut waiter = pin!(status.changed_since(1));
        assert!(poll(waiter.as_mut()).is_pending());
        publisher.faulted(reason);
        let Poll::Ready(read) = poll(waiter.as_mut()) else {
            panic!("the fault is an edge of the open stream");
        };
        let snapshot = open(read);
        assert!(matches!(snapshot.state, StationLinkState::Faulted(r) if r == reason));
        drop(publisher);
        let last = closed(status.read());
        assert_eq!(last.revision, snapshot.revision);
        assert!(matches!(last.state, StationLinkState::Faulted(r) if r == reason));
    }

    #[test]
    fn a_fault_published_just_before_the_drop_still_reaches_a_waiter() {
        let status = Status::new();
        let mut publisher = status.publisher().unwrap();
        let reason = StaFaultReason::new(StaFaultPhase::Attempt, StaFaultCause::PortLost);
        let mut waiter = pin!(status.changed_since(1));
        assert!(poll(waiter.as_mut()).is_pending());
        // The lifecycle faults and its task ends before the reader runs.
        publisher.faulted(reason);
        drop(publisher);
        let Poll::Ready(read) = poll(waiter.as_mut()) else {
            panic!("the closure is ready");
        };
        // The closure carries the unread fault: no producer stands behind
        // it, and the reader still learns why the station ended.
        let snapshot = closed(read);
        assert_eq!(snapshot.revision, 2);
        assert!(matches!(snapshot.state, StationLinkState::Faulted(r) if r == reason));
    }

    #[test]
    fn block_ack_changes_advance_the_revision_only_when_they_change() {
        let status = Status::new();
        let mut publisher = status.publisher().unwrap();
        publisher.tx_block_ack(0b1);
        let snapshot = open(status.read());
        assert_eq!(
            (snapshot.revision, snapshot.tx_block_ack_operational_tids),
            (2, 0b1)
        );
        publisher.tx_block_ack(0b1);
        assert_eq!(open(status.read()).revision, 2);
        // A new link state starts with no agreement.
        publisher.disconnected(PortDisconnect::Local);
        assert_eq!(open(status.read()).tx_block_ack_operational_tids, 0);
    }
}
