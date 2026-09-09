use super::*;
use core::{
    future::Future,
    task::{Context, Poll, Waker},
};

fn paused<'a>(
    terminal: &Rc<Cell<u32>>,
    mac: &'a EmbassyMacIrqRuntime<NoopRawMutex>,
    power: &'a EmbassyPowerIrqRuntime<NoopRawMutex>,
    platform: &Cell<u8>,
) -> super::super::super::PausedInterruptEpoch<'a, DistinctRoute, NoopRawMutex> {
    let mut epoch = InterruptEpoch::new(
        DistinctRoute {
            active: false,
            terminal: terminal.clone(),
        },
        (),
        mac,
        power,
    );
    epoch.activate(platform, MacInterruptMask::COLD_RX).unwrap();
    mac.publish(EVENT_TX_COMPLETE);
    epoch
        .try_pause(platform)
        .unwrap_or_else(|_| panic!("pause"))
}

#[test]
fn owned_operation_preserves_route_and_work_across_an_actual_suspension() {
    let terminal = Rc::new(Cell::new(0));
    let mac = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
    let power = EmbassyPowerIrqRuntime::<NoopRawMutex>::new();
    let platform = Cell::new(0);
    let epoch = paused(&terminal, &mac, &power, &platform);
    let completed = Cell::new(false);
    let mut operation = std::boxed::Box::pin(epoch.try_with_authority(async |authority| {
        core::future::poll_fn(|_| {
            if completed.get() {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
        Ok::<_, Retained>((authority, 42))
    }));
    let mut cx = Context::from_waker(Waker::noop());
    assert!(operation.as_mut().poll(&mut cx).is_pending());
    assert_eq!(Rc::strong_count(&terminal), 3);
    assert_eq!(
        terminal.get(),
        0,
        "operation must not perform cold teardown"
    );
    assert_eq!(mac.drain_pending(), Default::default());
    mac.publish(EVENT_TX_TIMEOUT);
    completed.set(true);
    let Poll::Ready(Ok((epoch, result))) = operation.as_mut().poll(&mut cx) else {
        panic!("completed operation must return the original paused epoch");
    };
    assert_eq!(result, 42);
    // Completion itself must not replay the old work or install the route.
    assert_eq!(mac.try_take_tx(), Some(EVENT_TX_TIMEOUT));
    assert_eq!(mac.try_take_tx(), None);
    mac.publish(EVENT_TX_TIMEOUT);
    let epoch = epoch
        .try_resume(&platform)
        .unwrap_or_else(|_| panic!("resume"));
    assert_eq!(
        mac.try_take_tx(),
        Some(EVENT_TX_COMPLETE | EVENT_TX_TIMEOUT)
    );
    assert_eq!(mac.try_take_tx(), None);
    let (epoch, _) = epoch
        .try_pause(&platform)
        .unwrap_or_else(|_| panic!("pause"))
        .into_stopped();
    drop(epoch);
    drop(operation);
    assert_eq!(terminal.get(), 1);
    assert_eq!(Rc::strong_count(&terminal), 1);
}

#[test]
fn failed_operation_retains_noncopy_authority_and_never_replays_pending_work() {
    let terminal = Rc::new(Cell::new(0));
    let mac = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
    let power = EmbassyPowerIrqRuntime::<NoopRawMutex>::new();
    let platform = Cell::new(0);
    let epoch = paused(&terminal, &mac, &power, &platform);
    let mut operation = std::boxed::Box::pin(
        epoch.try_with_authority(async |authority| Err::<(Retained, ()), _>(authority)),
    );
    let Poll::Ready(Err(fault)) = operation
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    else {
        panic!("operation failure must remain a terminal owner");
    };
    assert!(Rc::ptr_eq(&fault.error().0, &terminal));
    assert_eq!(Rc::strong_count(&terminal), 3);
    assert_eq!(mac.drain_pending(), Default::default());
    assert_eq!(terminal.get(), 0);
    drop(operation);
    assert_eq!(Rc::strong_count(&terminal), 3);
    drop(fault);
    assert_eq!(Rc::strong_count(&terminal), 1);
    assert_eq!(
        terminal.get(),
        0,
        "dropping a fault cannot certify a clean stop"
    );
}

#[test]
fn cancelled_operation_does_not_resume_or_convert_to_cold_setup() {
    let terminal = Rc::new(Cell::new(0));
    let mac = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
    let power = EmbassyPowerIrqRuntime::<NoopRawMutex>::new();
    let platform = Cell::new(0);
    let epoch = paused(&terminal, &mac, &power, &platform);
    let mut operation = std::boxed::Box::pin(epoch.try_with_authority(async |authority| {
        core::future::pending::<()>().await;
        Ok::<_, Retained>((authority, ()))
    }));
    assert!(
        operation
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending()
    );
    assert_eq!(Rc::strong_count(&terminal), 3);
    drop(operation);
    assert_eq!(Rc::strong_count(&terminal), 1);
    assert_eq!(terminal.get(), 0);
    assert_eq!(mac.drain_pending(), Default::default());
    assert!(!mac.is_rx_moderation_active());
}
