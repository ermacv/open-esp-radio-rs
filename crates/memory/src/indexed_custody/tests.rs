extern crate std;

use super::*;
use core::cell::Cell;
use std::{sync::mpsc, thread};

struct Name(usize);
struct Domain;

#[allow(
    unsafe_code,
    reason = "private test names are constructed once per cell"
)]
// SAFETY: tests issue each name once and reuse it only after take returns it.
// Each test has one private storage and keeps its names in that test. Names
// never leave this module's sealed construction/return protocol.
unsafe impl UniqueQueueIndex<2> for Name {
    fn queue_index(&self) -> usize {
        self.0
    }
}

#[allow(
    unsafe_code,
    reason = "the test binding has one private store per invocation"
)]
// SAFETY: Domain is constructed only alongside that test's store; its two
// private names are never routed to another invocation's store. They contain
// no pointer, borrow or destructor that could access the retained values.
unsafe impl CustodyDomain<2> for Domain {
    type Name = Name;
}

struct Owner<'a>(&'a Cell<usize>, u8);
impl Drop for Owner<'_> {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

#[test]
fn whole_owners_return_out_of_order_and_reuse_their_exact_place() {
    let drops = Cell::new(0);
    let store = IndexedCustody::new(Domain);
    let first = store.insert(Name(0), Owner(&drops, 11));
    let mut second = store.insert(Name(1), Owner(&drops, 22));
    store.with_mut(&mut second, |owner| owner.1 += 1);
    let (name, owner) = store.take(second);
    assert_eq!(owner.1, 23);
    let second = store.insert(name, owner);
    let (_, owner) = store.take(first);
    assert_eq!(owner.1, 11);
    drop(owner);
    let (_, owner) = store.take(second);
    drop(owner);
    drop(store);
    assert_eq!(drops.get(), 2);
}

#[test]
#[allow(
    clippy::forget_non_drop,
    reason = "exercise name loss independently of destructor behavior"
)]
fn forgotten_names_leave_owners_for_store_destruction() {
    let drops = Cell::new(0);
    {
        let store = IndexedCustody::new(Domain);
        core::mem::forget(store.insert(Name(0), Owner(&drops, 1)));
        core::mem::forget(store.insert(Name(1), Owner(&drops, 2)));
        assert_eq!(drops.get(), 0);
    }
    assert_eq!(drops.get(), 2);
}

#[test]
fn a_name_crosses_threads_without_moving_or_sending_borrowed_context() {
    let context = Cell::new(7);
    let store = IndexedCustody::new(Domain);
    let name = store.insert(Name(0), &context);
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        assert_eq!(name.queue_index(), 0);
        tx.send(name).unwrap();
    })
    .join()
    .unwrap();
    let (_, returned) = store.take(rx.recv().unwrap());
    assert!(core::ptr::eq(returned, &context));
    returned.set(9);
    assert_eq!(context.get(), 9);
}
