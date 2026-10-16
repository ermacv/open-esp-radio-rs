use core::cell::Cell;

use oer_esp32s31_hal::types::{
    MacInterruptEvents, MacInterruptObservation, MacPowerInterruptObservation,
};

use crate::irq::{
    EVENT_COLLISION, EVENT_RX_SUCCESS, EVENT_TX_COMPLETE, EVENT_TX_TIMEOUT, IrqDisposition,
    IrqSink, MacInterrupt, MacInterruptRoute, MacPowerInterrupt, PowerIrqDisposition,
    handle_mac_irq, handle_power_irq,
};

use std::{
    rc::Rc,
    sync::atomic::{AtomicU32, Ordering},
};

use super::{IrqConsumer, Moderated, PowerIrqConsumer, Unmoderated};
use crate::irq::epoch::{
    InterruptEpoch, MacInterruptEpochActivateError, MacInterruptEpochQuiesceError,
    MacInterruptEpochStateError,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RouteError {
    Activation,
    Quiescence,
}

struct Route {
    active: bool,
}

impl MacInterruptRoute for Route {
    type Platform = Cell<u8>;
    type Setup = u8;
    type Error = RouteError;

    fn activate(
        &mut self,
        platform: &Self::Platform,
        setup: Self::Setup,
        _event_mask: oer_esp32s31_hal::types::MacInterruptMask,
    ) -> Result<(), (Self::Error, Self::Setup)> {
        if platform.get() == 10 {
            return Err((RouteError::Activation, setup));
        }
        self.active = true;
        platform.set(1);
        Ok(())
    }

    fn quiesce(&mut self, platform: &Self::Platform) -> Result<Self::Setup, Self::Error> {
        if platform.get() == 20 {
            return Err(RouteError::Quiescence);
        }
        assert!(self.active);
        self.active = false;
        platform.set(2);
        Ok(7)
    }
}

struct DropObservedRoute {
    dropped: Rc<Cell<bool>>,
}

impl Drop for DropObservedRoute {
    fn drop(&mut self) {
        self.dropped.set(true);
    }
}

impl MacInterruptRoute for DropObservedRoute {
    type Platform = Cell<u8>;
    type Setup = u8;
    type Error = RouteError;

    fn activate(
        &mut self,
        platform: &Self::Platform,
        _setup: Self::Setup,
        _event_mask: oer_esp32s31_hal::types::MacInterruptMask,
    ) -> Result<(), (Self::Error, Self::Setup)> {
        platform.set(1);
        Ok(())
    }

    fn quiesce(&mut self, platform: &Self::Platform) -> Result<Self::Setup, Self::Error> {
        platform.set(2);
        Ok(7)
    }
}

#[test]
fn maps_one_combined_snapshot_to_bounded_rx_and_tx_wakes() {
    let runtime = IrqConsumer::new(Unmoderated);

    runtime.publish(EVENT_TX_TIMEOUT | EVENT_COLLISION | EVENT_TX_COMPLETE | EVENT_RX_SUCCESS);

    assert_eq!(runtime.rx_post_count(), 1);
    assert!(runtime.rx_signaled());
    assert_eq!(
        runtime.try_take_tx(),
        Some(EVENT_TX_TIMEOUT | EVENT_COLLISION | EVENT_TX_COMPLETE)
    );
    // Three TX causes coalesce into one wake without losing their bits.
    assert_eq!(runtime.try_take_tx(), None);
}

#[test]
fn repeated_rx_images_coalesce_the_wake_without_losing_irq_evidence() {
    let runtime = IrqConsumer::new(Unmoderated);

    runtime.publish(EVENT_RX_SUCCESS);
    runtime.publish(EVENT_RX_SUCCESS);

    assert_eq!(runtime.rx_post_count(), 2);
    assert!(runtime.rx_signaled());
    ready(runtime.wait_rx());
    assert!(!runtime.rx_signaled());
}

#[test]
fn staging_capacity_wake_does_not_forge_interrupt_evidence() {
    let runtime = IrqConsumer::new(Unmoderated);

    runtime.notify_rx_capacity();
    ready(runtime.wait_rx_capacity());

    assert!(!runtime.rx_signaled());
    assert_eq!(runtime.rx_post_count(), 0);
}

#[test]
fn live_ring_handoff_probe_does_not_forge_interrupt_evidence() {
    let runtime = IrqConsumer::new(Unmoderated);

    runtime.notify_rx_handoff();
    assert!(runtime.rx_signaled());
    ready(runtime.wait_rx());

    assert!(!runtime.rx_signaled());
    assert_eq!(runtime.rx_post_count(), 0);
}

#[test]
fn quiesced_epoch_drain_removes_every_coalesced_wake() {
    let runtime = IrqConsumer::new(Unmoderated);

    runtime.publish(EVENT_RX_SUCCESS | EVENT_TX_COMPLETE | EVENT_TX_TIMEOUT);
    runtime.notify_rx_capacity();

    assert_eq!(
        runtime.drain_pending(),
        super::IrqDrain {
            rx: true,
            rx_capacity: true,
            tx_events: EVENT_TX_COMPLETE | EVENT_TX_TIMEOUT,
        }
    );
    assert_eq!(runtime.drain_pending(), Default::default());
    assert!(!runtime.rx_signaled());
    assert_eq!(runtime.try_take_tx(), None);
}

#[test]
fn irq_epoch_recovers_setup_before_draining_every_executor_wake() {
    let mac = IrqConsumer::new(Unmoderated);
    let power = PowerIrqConsumer::new();
    let platform = Cell::new(0);
    let mut epoch = InterruptEpoch::new(Route { active: false }, 7, &mac, &power);

    assert_eq!(epoch.setup(), Ok(&7));
    epoch
        .activate(
            &platform,
            oer_esp32s31_hal::types::MacInterruptMask::COLD_RX,
        )
        .unwrap();
    assert!(epoch.is_active());
    assert_eq!(epoch.setup(), Err(MacInterruptEpochStateError::Active));
    mac.publish(EVENT_RX_SUCCESS | EVENT_TX_COMPLETE);
    mac.notify_rx_capacity();
    let power_observation =
        MacPowerInterruptObservation::from_semantic_events(true, false, true, false, true, true);
    power.publish(power_observation);

    let drained = epoch.quiesce(&platform).unwrap();
    assert_eq!(platform.get(), 2);
    assert!(drained.mac.rx);
    assert!(drained.mac.rx_capacity);
    assert_eq!(drained.mac.tx_events, EVENT_TX_COMPLETE);
    assert_eq!(drained.power_events, power_observation);
    assert_eq!(epoch.setup(), Ok(&7));
    assert_eq!(mac.drain_pending(), Default::default());
    assert_eq!(
        power.drain_pending(),
        MacPowerInterruptObservation::default()
    );
}

#[test]
fn inactive_irq_epoch_returns_the_exact_route_setup_and_consumers() {
    let mac = IrqConsumer::new(Unmoderated);
    let power = PowerIrqConsumer::new();
    let epoch = InterruptEpoch::new(Route { active: false }, 9, &mac, &power);

    let crate::irq::epoch::InactiveIrqParts {
        route,
        setup,
        mac: returned_mac,
        power: returned_power,
    } = epoch
        .try_into_inactive_parts()
        .unwrap_or_else(|_| panic!("an inactive epoch must be decomposable"));

    assert!(!route.active);
    assert_eq!(setup, 9);
    assert!(core::ptr::eq(returned_mac, &mac));
    assert!(core::ptr::eq(returned_power, &power));
}

#[test]
fn active_irq_epoch_cannot_release_its_route_or_setup() {
    let mac = IrqConsumer::new(Unmoderated);
    let power = PowerIrqConsumer::new();
    let platform = Cell::new(0);
    let mut epoch = InterruptEpoch::new(Route { active: false }, 9, &mac, &power);
    epoch
        .activate(
            &platform,
            oer_esp32s31_hal::types::MacInterruptMask::COLD_RX,
        )
        .unwrap();

    let mut epoch = match epoch.try_into_inactive_parts() {
        Ok(_) => panic!("an active route must not be extractable"),
        Err(epoch) => epoch,
    };
    assert!(epoch.is_active());
    assert_eq!(platform.get(), 1);

    epoch.quiesce(&platform).unwrap();
    let crate::irq::epoch::InactiveIrqParts { route, setup, .. } = epoch
        .try_into_inactive_parts()
        .unwrap_or_else(|_| panic!("the quiesced epoch must become extractable"));
    assert!(!route.active);
    assert_eq!(setup, 7);
}

#[test]
fn irq_epoch_retains_the_exact_frontier_on_each_route_failure() {
    let mac = IrqConsumer::new(Unmoderated);
    let power = PowerIrqConsumer::new();
    let platform = Cell::new(10);
    let mut epoch = InterruptEpoch::new(Route { active: false }, 7, &mac, &power);

    assert_eq!(
        epoch.activate(
            &platform,
            oer_esp32s31_hal::types::MacInterruptMask::COLD_RX,
        ),
        Err(MacInterruptEpochActivateError::Route(
            RouteError::Activation
        ))
    );
    assert_eq!(epoch.setup(), Ok(&7));
    platform.set(0);
    epoch
        .activate(
            &platform,
            oer_esp32s31_hal::types::MacInterruptMask::COLD_RX,
        )
        .unwrap();
    platform.set(20);
    assert_eq!(
        epoch.quiesce(&platform),
        Err(MacInterruptEpochQuiesceError::Route(RouteError::Quiescence))
    );
    assert!(epoch.is_active());
    platform.set(0);
    epoch.quiesce(&platform).unwrap();
    assert_eq!(epoch.setup(), Ok(&7));
    assert_eq!(
        epoch.quiesce(&platform),
        Err(MacInterruptEpochQuiesceError::AlreadyQuiesced)
    );
}

#[test]
fn active_irq_epoch_drop_retains_the_installed_route_without_panicking() {
    let mac = IrqConsumer::new(Unmoderated);
    let power = PowerIrqConsumer::new();
    let platform = Cell::new(0);
    let dropped = Rc::new(Cell::new(false));
    let mut epoch = InterruptEpoch::new(
        DropObservedRoute {
            dropped: dropped.clone(),
        },
        7,
        &mac,
        &power,
    );
    epoch
        .activate(
            &platform,
            oer_esp32s31_hal::types::MacInterruptMask::COLD_RX,
        )
        .unwrap();

    drop(epoch);

    assert_eq!(platform.get(), 1);
    assert!(!dropped.get());
}

#[test]
fn retains_unhandled_evidence_through_the_irq_sink_contract() {
    let runtime = IrqConsumer::new(Unmoderated);
    IrqSink::record_unhandled_event(&runtime);
    assert!(runtime.observed_unhandled());
}

struct Interrupt {
    status: MacInterruptObservation,
    rx_masked: Cell<bool>,
    rx_was_masked_before_ack: Cell<bool>,
    acknowledged: Cell<bool>,
}

impl MacInterrupt for Interrupt {
    type Snapshot = MacInterruptObservation;

    fn status(&mut self) -> Self::Snapshot {
        self.status
    }

    fn mask_rx_delivery(&mut self) {
        self.rx_masked.set(true);
    }

    fn acknowledge(&mut self, snapshot: Self::Snapshot) {
        let _snapshot = snapshot;
        self.rx_was_masked_before_ack.set(self.rx_masked.get());
        self.acknowledged.set(true);
    }
}

#[test]
fn production_handler_acknowledges_before_publishing_driver_work() {
    let status = MacInterruptObservation::from_semantic_events(
        MacInterruptEvents::RX_SUCCESS.union(MacInterruptEvents::TX_COMPLETE),
        false,
        false,
    );
    let mut interrupt = Interrupt {
        status,
        rx_masked: Cell::new(false),
        rx_was_masked_before_ack: Cell::new(false),
        acknowledged: Cell::new(false),
    };
    let runtime = IrqConsumer::new(Unmoderated);

    let (disposition, snapshot) = handle_mac_irq(&mut interrupt, &runtime);

    assert_eq!(disposition, IrqDisposition::Posted);
    assert!(snapshot.had_status);
    assert_eq!(snapshot.posted_events, EVENT_RX_SUCCESS | EVENT_TX_COMPLETE);
    assert!(interrupt.acknowledged.get());
    assert!(!interrupt.rx_masked.get());
    assert!(runtime.rx_signaled());
    assert_eq!(runtime.try_take_tx(), Some(EVENT_TX_COMPLETE));
}

#[test]
fn spurious_status_neither_acknowledges_nor_wakes_the_driver() {
    let mut interrupt = Interrupt {
        status: MacInterruptObservation::default(),
        rx_masked: Cell::new(false),
        rx_was_masked_before_ack: Cell::new(false),
        acknowledged: Cell::new(false),
    };
    let runtime = IrqConsumer::new(Unmoderated);

    assert_eq!(
        handle_mac_irq(&mut interrupt, &runtime).0,
        IrqDisposition::Spurious
    );
    assert!(!interrupt.acknowledged.get());
    assert!(!runtime.rx_signaled());
    assert_eq!(runtime.try_take_tx(), None);
}

std::thread_local! {
    // A callback has no fixture argument. Each host test executes its callback
    // on its own thread, so parallel moderation fixtures share no counter.
    static RX_UNMASK_CALLS: Cell<u32> = const { Cell::new(0) };
}

fn record_rx_unmask() {
    RX_UNMASK_CALLS.set(RX_UNMASK_CALLS.get() + 1);
}

#[test]
fn moderated_rx_masks_before_ack_and_restores_only_on_bottom_half_drain() {
    RX_UNMASK_CALLS.set(0);
    let status = MacInterruptObservation::from_semantic_events(
        MacInterruptEvents::RX_SUCCESS.union(MacInterruptEvents::TX_COMPLETE),
        false,
        false,
    );
    let mut interrupt = Interrupt {
        status,
        rx_masked: Cell::new(false),
        rx_was_masked_before_ack: Cell::new(false),
        acknowledged: Cell::new(false),
    };
    let runtime = IrqConsumer::new(Moderated::new(record_rx_unmask));
    runtime.begin_rx_moderation();

    let (disposition, snapshot) = handle_mac_irq(&mut interrupt, &runtime);

    assert_eq!(disposition, IrqDisposition::Posted);
    assert!(snapshot.had_status);
    assert_eq!(snapshot.posted_events, EVENT_RX_SUCCESS | EVENT_TX_COMPLETE);
    assert!(interrupt.rx_masked.get());
    assert!(interrupt.rx_was_masked_before_ack.get());
    assert!(interrupt.acknowledged.get());
    assert_eq!(runtime.try_take_tx(), Some(EVENT_TX_COMPLETE));
    assert_eq!(RX_UNMASK_CALLS.get(), 0);

    assert!(runtime.unmask_rx_after_drain());
    assert_eq!(RX_UNMASK_CALLS.get(), 1);
    runtime.end_rx_moderation();
    assert!(!runtime.unmask_rx_after_drain());
    assert_eq!(RX_UNMASK_CALLS.get(), 1);
}

#[test]
fn moderated_irq_epoch_owns_activation_failure_and_quiesce_cleanup() {
    RX_UNMASK_CALLS.set(0);
    let mac = IrqConsumer::new(Moderated::new(record_rx_unmask));
    let power = PowerIrqConsumer::new();
    let platform = Cell::new(10);
    let mut epoch = InterruptEpoch::new(Route { active: false }, 7, &mac, &power);

    assert_eq!(
        epoch.activate_rx_moderated(
            &platform,
            oer_esp32s31_hal::types::MacInterruptMask::COLD_RX,
        ),
        Err(MacInterruptEpochActivateError::Route(
            RouteError::Activation
        ))
    );
    assert!(!epoch.is_active());
    assert!(!epoch.is_rx_moderated());
    assert!(!mac.is_rx_moderation_active());

    platform.set(0);
    epoch
        .activate_rx_moderated(
            &platform,
            oer_esp32s31_hal::types::MacInterruptMask::COLD_RX,
        )
        .unwrap();
    assert!(epoch.is_active());
    assert!(epoch.is_rx_moderated());
    assert!(mac.is_rx_moderation_active());

    epoch.quiesce(&platform).unwrap();
    assert!(!epoch.is_active());
    assert!(!epoch.is_rx_moderated());
    assert!(!mac.is_rx_moderation_active());
}

struct PowerInterrupt {
    status: MacPowerInterruptObservation,
    acknowledged: Cell<bool>,
}

impl MacPowerInterrupt for PowerInterrupt {
    type Snapshot = MacPowerInterruptObservation;

    fn status(&mut self) -> Self::Snapshot {
        self.status
    }

    fn acknowledge(&mut self, snapshot: Self::Snapshot) {
        let _snapshot = snapshot;
        self.acknowledged.set(true);
    }
}

#[test]
fn power_irq_retains_semantic_causes_without_register_images() {
    let status =
        MacPowerInterruptObservation::from_semantic_events(false, true, false, true, false, true);
    let mut interrupt = PowerInterrupt {
        status,
        acknowledged: Cell::new(false),
    };
    let runtime = PowerIrqConsumer::new();

    let (disposition, snapshot) = handle_power_irq(&mut interrupt, &runtime);

    assert_eq!(disposition, PowerIrqDisposition::Posted);
    assert_eq!(snapshot.observation, status);
    assert!(interrupt.acknowledged.get());
    assert_eq!(runtime.try_take(), Some(status));
    assert_eq!(runtime.try_take(), None);
}

#[test]
fn spurious_power_irq_neither_acknowledges_nor_wakes_the_driver() {
    let mut interrupt = PowerInterrupt {
        status: MacPowerInterruptObservation::default(),
        acknowledged: Cell::new(false),
    };
    let runtime = PowerIrqConsumer::new();

    assert_eq!(
        handle_power_irq(&mut interrupt, &runtime).0,
        PowerIrqDisposition::Spurious
    );
    assert!(!interrupt.acknowledged.get());
    assert_eq!(runtime.try_take(), None);
}

fn ready<F: core::future::Future>(future: F) -> F::Output {
    let mut future = core::pin::pin!(future);
    match future.as_mut().poll(&mut core::task::Context::from_waker(
        core::task::Waker::noop(),
    )) {
        core::task::Poll::Ready(value) => value,
        core::task::Poll::Pending => panic!("published IRQ facts must be ready"),
    }
}

#[test]
fn a_captured_batch_orders_rx_first_and_is_not_extended_by_new_interrupts() {
    let irq = IrqConsumer::default();
    irq.publish(EVENT_COLLISION | EVENT_TX_TIMEOUT | EVENT_RX_SUCCESS | EVENT_TX_COMPLETE);
    irq.notify_rx_capacity();
    let mut batch = irq.take_batch();
    assert!(batch.rx_capacity_returned());
    assert_eq!(batch.next(), Some(crate::irq::IrqWork::RxSuccess));
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                for _ in 0..512 {
                    irq.publish(EVENT_RX_SUCCESS);
                }
            })
            .join()
            .unwrap();
    });
    assert_eq!(
        batch.collect::<std::vec::Vec<_>>(),
        [
            crate::irq::IrqWork::TxComplete,
            crate::irq::IrqWork::TxTimeout,
            crate::irq::IrqWork::Collision,
        ]
    );
    assert_eq!(
        irq.take_batch().collect::<std::vec::Vec<_>>(),
        [crate::irq::IrqWork::RxSuccess]
    );
    assert!(irq.take_batch().is_empty());
    assert_eq!(irq.rx_post_count(), 513);
}

#[derive(Default)]
struct WakeCount(AtomicU32);
impl std::task::Wake for WakeCount {
    fn wake(self: std::sync::Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
    fn wake_by_ref(self: &std::sync::Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn publication_before_poll_or_from_another_thread_after_registration_makes_progress() {
    use core::{
        future::Future,
        task::{Context, Poll, Waker},
    };
    let irq = IrqConsumer::default();
    let wakes = std::sync::Arc::new(WakeCount::default());
    let waker = Waker::from(wakes.clone());
    let mut cx = Context::from_waker(&waker);
    irq.publish(EVENT_TX_COMPLETE);
    assert_eq!(
        ready(irq.wait_batch()).next(),
        Some(crate::irq::IrqWork::TxComplete)
    );

    for count in 1..=32 {
        let mut wait = core::pin::pin!(irq.wait_batch());
        assert!(wait.as_mut().poll(&mut cx).is_pending());
        std::thread::scope(|scope| {
            scope
                .spawn(|| irq.publish(EVENT_RX_SUCCESS | EVENT_TX_COMPLETE))
                .join()
                .unwrap();
        });
        assert!(wakes.0.load(Ordering::Relaxed) >= count);
        let Poll::Ready(batch) = wait.as_mut().poll(&mut cx) else {
            panic!("lost IRQ wake");
        };
        assert_eq!(
            batch.collect::<std::vec::Vec<_>>(),
            [
                crate::irq::IrqWork::RxSuccess,
                crate::irq::IrqWork::TxComplete
            ]
        );
    }
}

#[test]
fn cancelling_a_wait_does_not_consume_published_facts() {
    use core::{
        future::Future,
        task::{Context, Waker},
    };
    let irq = IrqConsumer::default();
    {
        let mut wait = core::pin::pin!(irq.wait_batch());
        assert!(
            wait.as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
        irq.publish(EVENT_RX_SUCCESS | EVENT_TX_TIMEOUT);
    }
    assert_eq!(
        ready(irq.wait_batch()).collect::<std::vec::Vec<_>>(),
        [
            crate::irq::IrqWork::RxSuccess,
            crate::irq::IrqWork::TxTimeout
        ]
    );
    assert!(irq.take_batch().is_empty());
}

#[test]
fn rx_publication_counts_wrap_and_exclude_capacity_and_handoff_probes() {
    let irq = IrqConsumer::default();
    irq.rx_posts.store(u32::MAX, Ordering::Relaxed);
    irq.notify_rx_capacity();
    irq.notify_rx_handoff();
    assert_eq!(irq.rx_post_count(), u32::MAX);
    irq.publish(EVENT_RX_SUCCESS);
    assert_eq!(irq.rx_post_count(), 0);
}

#[test]
fn power_facts_coalesce_into_one_complete_snapshot() {
    let power = PowerIrqConsumer::new();
    power.publish(MacPowerInterruptObservation::from_semantic_events(
        true, false, true, false, false, false,
    ));
    power.publish(MacPowerInterruptObservation::from_semantic_events(
        false, true, false, true, true, true,
    ));
    assert_eq!(
        ready(power.wait()),
        MacPowerInterruptObservation::from_semantic_events(true, true, true, true, true, true)
    );
    assert_eq!(power.try_take(), None);
}
