//! Real Embassy scheduling with retained wakers and a non-Send radio-owner stand-in.
use std::{
    cell::Cell,
    future::poll_fn,
    rc::Rc,
    sync::Mutex,
    task::{Poll, Waker},
};
#[path = "../src/supervisor/station/execution/exchange.rs"]
mod exchange;
use exchange::Exchange;
static SAVED: Mutex<Option<Waker>> = Mutex::new(None);
#[unsafe(no_mangle)]
fn __pender(_: *mut ()) {}

#[embassy_executor::task]
async fn worker(exchange: &'static Exchange<Rc<Cell<u32>>, Rc<Cell<u32>>>) {
    loop {
        let owner = exchange.next().await;
        poll_fn(|cx| {
            *SAVED.lock().unwrap() = Some(cx.waker().clone());
            Poll::Ready(())
        })
        .await;
        owner.set(owner.get() + 1);
        exchange.finish(owner);
    }
}

#[test]
fn late_epoch_wakes_cannot_prevent_restarting_the_permanent_worker() {
    let executor = Box::leak(Box::new(embassy_executor::raw::Executor::new(
        std::ptr::null_mut(),
    )));
    let exchange = Box::leak(Box::new(Exchange::new()));
    executor.spawner().spawn(worker(exchange).unwrap());
    let mut owner = Rc::new(Cell::new(0));
    for epoch in 1..=32 {
        exchange.submit(owner).unwrap();
        // Only this thread polls the raw executor; the pender never re-enters it.
        unsafe {
            executor.poll();
        }
        let mut completion = std::pin::pin!(exchange.wait_completed());
        assert!(
            std::future::Future::poll(
                completion.as_mut(),
                &mut std::task::Context::from_waker(Waker::noop())
            )
            .is_ready()
        );
        let rejected = Rc::new(Cell::new(99));
        assert!(Rc::ptr_eq(
            &exchange.submit(rejected.clone()).unwrap_err(),
            &rejected
        ));
        owner = exchange.take_return();
        assert_eq!(owner.get(), epoch);
        assert_eq!(Rc::strong_count(&owner), 1);
        // An old radio event arrives after completion and before the next submission.
        SAVED.lock().unwrap().as_ref().unwrap().wake_by_ref();
    }
    // Drain a final stale wake with no owner queued: the worker must stay asleep.
    unsafe {
        executor.poll();
    }
    assert_eq!(owner.get(), 32);
}
