use core::{
    future::Future,
    pin::pin,
    task::{Context, Poll, Waker},
};

use embassy_sync::blocking_mutex::raw::NoopRawMutex;

use super::DurableFirstFault;

#[test]
fn first_fault_is_durable_and_later_faults_cannot_replace_it() {
    let fault = DurableFirstFault::<NoopRawMutex, u8>::new();

    fault.publish(3);
    fault.publish(7);

    assert_eq!(fault.get(), Some(3));
}

#[test]
fn wait_rechecks_durable_state_after_registration() {
    let fault = DurableFirstFault::<NoopRawMutex, u8>::new();
    let mut wait = pin!(fault.wait());
    let waker = Waker::noop();
    let mut context = Context::from_waker(waker);

    assert_eq!(wait.as_mut().poll(&mut context), Poll::Pending);
    fault.publish(11);
    assert_eq!(wait.as_mut().poll(&mut context), Poll::Ready(11));
    assert_eq!(fault.get(), Some(11));
}

#[test]
fn cancelling_a_wait_does_not_lose_a_later_fault() {
    let fault = DurableFirstFault::<NoopRawMutex, u8>::new();
    let waker = Waker::noop();
    let mut context = Context::from_waker(waker);

    {
        let mut cancelled = pin!(fault.wait());
        assert_eq!(cancelled.as_mut().poll(&mut context), Poll::Pending);
    }

    fault.publish(19);
    let mut replacement = pin!(fault.wait());
    assert_eq!(replacement.as_mut().poll(&mut context), Poll::Ready(19));
    assert_eq!(fault.get(), Some(19));
}
