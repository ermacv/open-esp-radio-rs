use super::InterruptPublicationSlot;
use crate::interrupt_fault::DurableFirstFault;
use core::{
    cell::Cell,
    task::{Context, Poll, Waker},
};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;

struct Service<'a> {
    drops: &'a Cell<u32>,
    fault: DurableFirstFault<NoopRawMutex, u8>,
}

impl Drop for Service<'_> {
    fn drop(&mut self) {
        self.drops.set(self.drops.get() + 1);
    }
}

fn service(drops: &Cell<u32>) -> Service<'_> {
    Service {
        drops,
        fault: DurableFirstFault::new(),
    }
}

#[test]
fn complete_service_is_visible_before_route_activation_and_removed_after_disable() {
    let drops = Cell::new(0);
    let slot = InterruptPublicationSlot::<NoopRawMutex, _>::new();
    let live = slot
        .bind(service(&drops), || {
            slot.with(|s| s.unwrap().fault.publish(7));
            Ok::<_, ()>(23)
        })
        .ok()
        .unwrap();
    assert_eq!(live.with(|s| s.fault.get()), Some(7));
    let owner = live
        .disable(|route| {
            assert_eq!(route, 23);
            assert!(slot.with(|s| s.is_some()));
            Ok::<_, ((), _)>(())
        })
        .ok()
        .unwrap();
    assert!(slot.with(|s| s.is_none()));
    assert_eq!(owner.fault.get(), Some(7));
    assert_eq!(drops.get(), 0);
    drop(owner);
    assert_eq!(drops.get(), 1);
}

#[test]
fn failed_bind_returns_the_service_and_allows_retry_without_leaking_the_slot() {
    let drops = Cell::new(0);
    let slot = InterruptPublicationSlot::<NoopRawMutex, _>::new();
    let (error, owner) = slot
        .bind(service(&drops), || Err::<(), _>(19))
        .err()
        .unwrap();
    assert_eq!(error, Some(19));
    assert!(slot.with(|s| s.is_none()));
    assert_eq!(drops.get(), 0);
    let live = slot.bind(owner, || Ok::<_, ()>(())).ok().unwrap();
    drop(live.disable(|()| Ok::<_, ((), ())>(())).ok().unwrap());
    assert_eq!(drops.get(), 1);
}

#[test]
fn occupied_publication_rejects_before_touching_routes_or_either_owner() {
    let first_drops = Cell::new(0);
    let second_drops = Cell::new(0);
    let slot = InterruptPublicationSlot::<NoopRawMutex, _>::new();
    let live = slot
        .bind(service(&first_drops), || Ok::<_, ()>(()))
        .ok()
        .unwrap();
    let (error, second) = slot
        .bind(service(&second_drops), || -> Result<(), ()> {
            panic!("an occupied slot must not touch routes")
        })
        .err()
        .unwrap();
    assert_eq!(error, None);
    assert_eq!((first_drops.get(), second_drops.get()), (0, 0));
    drop(second);
    drop(live.disable(|()| Ok::<_, ((), ())>(())).ok().unwrap());
    assert_eq!((first_drops.get(), second_drops.get()), (1, 1));
}

#[test]
fn rejected_disable_retains_routes_fault_and_cancelled_wait_until_a_successful_retry() {
    let drops = Cell::new(0);
    let slot = InterruptPublicationSlot::<NoopRawMutex, _>::new();
    let live = slot.bind(service(&drops), || Ok::<_, ()>(41)).ok().unwrap();
    let mut cx = Context::from_waker(Waker::noop());
    assert_eq!(live.with(|s| s.fault.poll_wait(&mut cx)), Poll::Pending);
    live.with(|s| s.fault.publish(3));
    let (error, live) = live.disable(|routes| Err((5, routes))).err().unwrap();
    assert_eq!(error, 5);
    assert_eq!(drops.get(), 0);
    assert_eq!(live.with(|s| s.fault.poll_wait(&mut cx)), Poll::Ready(3));
    let owner = live
        .disable(|routes| {
            assert_eq!(routes, 41);
            Ok::<_, ((), _)>(())
        })
        .ok()
        .unwrap();
    let rebound = slot.bind(owner, || Ok::<_, ()>(42)).ok().unwrap();
    assert_eq!(rebound.with(|s| s.fault.get()), Some(3));
    drop(rebound.disable(|_| Ok::<_, ((), _)>(())).ok().unwrap());
    let fresh = slot.bind(service(&drops), || Ok::<_, ()>(43)).ok().unwrap();
    assert_eq!(fresh.with(|s| s.fault.get()), None);
    drop(fresh.disable(|_| Ok::<_, ((), _)>(())).ok().unwrap());
    assert_eq!(drops.get(), 2);
}

#[test]
fn dropping_a_live_route_owner_does_not_make_the_slot_reusable() {
    let drops = Cell::new(0);
    let slot = InterruptPublicationSlot::<NoopRawMutex, _>::new();
    let live = slot.bind(service(&drops), || Ok::<_, ()>(())).ok().unwrap();
    drop(live);
    assert_eq!(drops.get(), 0);
    assert!(slot.with(|s| s.is_some()));
    assert!(slot.bind(service(&drops), || Ok::<_, ()>(())).is_err());
}
