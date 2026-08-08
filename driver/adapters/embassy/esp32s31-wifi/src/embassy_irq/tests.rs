use core::cell::Cell;

use open_esp_radio_embassy_net::NoopRawMutex;
use open_esp_radio_esp32s31_wifi_mac::irq::{
    IrqDisposition, IrqSink, MAC_INT_COLLISION, MAC_INT_RX_SUCCESS, MAC_INT_TX_COMPLETE,
    MAC_INT_TX_TIMEOUT, MacInterrupt, MacInterruptRoute, MacPowerInterrupt, PowerIrqDisposition,
    handle_mac_irq, handle_power_irq,
};

use super::{
    EmbassyMacIrqRuntime, EmbassyPowerIrqRuntime, Esp32s31MacInterruptEpoch,
    Esp32s31MacInterruptEpochActivateError, Esp32s31MacInterruptEpochQuiesceError,
    Esp32s31MacInterruptEpochStateError,
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
        _event_mask: u32,
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

#[test]
fn maps_one_combined_snapshot_to_bounded_rx_and_tx_wakes() {
    let runtime = EmbassyMacIrqRuntime::<NoopRawMutex>::new();

    runtime
        .publish(MAC_INT_TX_TIMEOUT | MAC_INT_COLLISION | MAC_INT_TX_COMPLETE | MAC_INT_RX_SUCCESS);

    assert_eq!(runtime.rx_post_count(), 1);
    assert!(runtime.rx_signaled());
    assert_eq!(
        runtime.try_take_tx(),
        Some(MAC_INT_TX_TIMEOUT | MAC_INT_COLLISION | MAC_INT_TX_COMPLETE)
    );
    // Three TX causes coalesce into one wake without losing their bits.
    assert_eq!(runtime.try_take_tx(), None);
}

#[test]
fn staging_capacity_wake_does_not_forge_interrupt_evidence() {
    let runtime = EmbassyMacIrqRuntime::<NoopRawMutex>::new();

    runtime.notify_rx_capacity();
    embassy_futures::block_on(runtime.wait_rx_capacity());

    assert!(!runtime.rx_signaled());
    assert_eq!(runtime.rx_post_count(), 0);
}

#[test]
fn live_ring_handoff_probe_does_not_forge_interrupt_evidence() {
    let runtime = EmbassyMacIrqRuntime::<NoopRawMutex>::new();

    runtime.notify_rx_handoff();
    assert!(runtime.rx_signaled());
    embassy_futures::block_on(runtime.wait_rx());

    assert!(!runtime.rx_signaled());
    assert_eq!(runtime.rx_post_count(), 0);
}

#[test]
fn quiesced_epoch_drain_removes_every_coalesced_wake() {
    let runtime = EmbassyMacIrqRuntime::<NoopRawMutex>::new();

    runtime.publish(MAC_INT_RX_SUCCESS | MAC_INT_TX_COMPLETE | MAC_INT_TX_TIMEOUT);
    runtime.notify_rx_capacity();

    assert_eq!(
        runtime.drain_pending(),
        super::EmbassyMacIrqDrain {
            rx: true,
            rx_capacity: true,
            tx_events: MAC_INT_TX_COMPLETE | MAC_INT_TX_TIMEOUT,
        }
    );
    assert_eq!(runtime.drain_pending(), Default::default());
    assert!(!runtime.rx_signaled());
    assert_eq!(runtime.try_take_tx(), None);
}

#[test]
fn irq_epoch_recovers_setup_before_draining_every_executor_wake() {
    let mac = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
    let power = EmbassyPowerIrqRuntime::<NoopRawMutex>::new();
    let platform = Cell::new(0);
    let mut epoch = Esp32s31MacInterruptEpoch::new(Route { active: false }, 7, &mac, &power);

    assert_eq!(epoch.setup(), Ok(&7));
    epoch.activate(&platform, 0x1234).unwrap();
    assert!(epoch.is_active());
    assert_eq!(
        epoch.setup(),
        Err(Esp32s31MacInterruptEpochStateError::Active)
    );
    mac.publish(MAC_INT_RX_SUCCESS | MAC_INT_TX_COMPLETE);
    mac.notify_rx_capacity();
    power.publish(0x55);

    let drained = epoch.quiesce(&platform).unwrap();
    assert_eq!(platform.get(), 2);
    assert!(drained.mac.rx);
    assert!(drained.mac.rx_capacity);
    assert_eq!(drained.mac.tx_events, MAC_INT_TX_COMPLETE);
    assert_eq!(drained.power_events, 0x55);
    assert_eq!(epoch.setup(), Ok(&7));
    assert_eq!(mac.drain_pending(), Default::default());
    assert_eq!(power.drain_pending(), 0);
}

#[test]
fn irq_epoch_retains_the_exact_frontier_on_each_route_failure() {
    let mac = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
    let power = EmbassyPowerIrqRuntime::<NoopRawMutex>::new();
    let platform = Cell::new(10);
    let mut epoch = Esp32s31MacInterruptEpoch::new(Route { active: false }, 7, &mac, &power);

    assert_eq!(
        epoch.activate(&platform, 0x1234),
        Err(Esp32s31MacInterruptEpochActivateError::Route(
            RouteError::Activation
        ))
    );
    assert_eq!(epoch.setup(), Ok(&7));
    platform.set(0);
    epoch.activate(&platform, 0x1234).unwrap();
    platform.set(20);
    assert_eq!(
        epoch.quiesce(&platform),
        Err(Esp32s31MacInterruptEpochQuiesceError::Route(
            RouteError::Quiescence
        ))
    );
    assert!(epoch.is_active());
    platform.set(0);
    epoch.quiesce(&platform).unwrap();
    assert_eq!(epoch.setup(), Ok(&7));
    assert_eq!(
        epoch.quiesce(&platform),
        Err(Esp32s31MacInterruptEpochQuiesceError::AlreadyQuiesced)
    );
}

#[test]
fn active_irq_epoch_cannot_be_silently_destroyed() {
    let mac = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
    let power = EmbassyPowerIrqRuntime::<NoopRawMutex>::new();
    let platform = Cell::new(0);
    let mut epoch = Esp32s31MacInterruptEpoch::new(Route { active: false }, 7, &mac, &power);
    epoch.activate(&platform, 0x1234).unwrap();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(epoch)));

    assert!(result.is_err());
}

#[test]
fn retains_unhandled_evidence_through_the_irq_sink_contract() {
    let runtime = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
    IrqSink::record_unhandled(&runtime, 0x8000_0000);
    assert_eq!(runtime.observed_unhandled(), 0x8000_0000);
}

struct Interrupt {
    status: u32,
    acknowledged: Cell<Option<u32>>,
}

impl MacInterrupt for Interrupt {
    type Snapshot = u32;

    fn status(&mut self) -> Self::Snapshot {
        self.status
    }

    fn acknowledge(&mut self, snapshot: Self::Snapshot) {
        self.acknowledged.set(Some(snapshot));
    }
}

#[test]
fn production_handler_acknowledges_before_publishing_embassy_work() {
    let status = MAC_INT_RX_SUCCESS | MAC_INT_TX_COMPLETE;
    let mut interrupt = Interrupt {
        status,
        acknowledged: Cell::new(None),
    };
    let runtime = EmbassyMacIrqRuntime::<NoopRawMutex>::new();

    let (disposition, snapshot) = handle_mac_irq(&mut interrupt, &runtime);

    assert_eq!(disposition, IrqDisposition::Posted);
    assert_eq!(snapshot.status, status);
    assert_eq!(interrupt.acknowledged.get(), Some(status));
    assert!(runtime.rx_signaled());
    assert_eq!(runtime.try_take_tx(), Some(MAC_INT_TX_COMPLETE));
}

#[test]
fn spurious_status_neither_acknowledges_nor_wakes_embassy() {
    let mut interrupt = Interrupt {
        status: 0,
        acknowledged: Cell::new(None),
    };
    let runtime = EmbassyMacIrqRuntime::<NoopRawMutex>::new();

    assert_eq!(
        handle_mac_irq(&mut interrupt, &runtime).0,
        IrqDisposition::Spurious
    );
    assert_eq!(interrupt.acknowledged.get(), None);
    assert!(!runtime.rx_signaled());
    assert_eq!(runtime.try_take_tx(), None);
}

struct PowerInterrupt {
    status: u32,
    acknowledged: Cell<Option<u32>>,
}

impl MacPowerInterrupt for PowerInterrupt {
    type Snapshot = u32;

    fn status(&mut self) -> Self::Snapshot {
        self.status
    }

    fn acknowledge(&mut self, snapshot: Self::Snapshot) {
        self.acknowledged.set(Some(snapshot));
    }
}

#[test]
fn power_irq_retains_the_complete_acknowledged_image_without_decoding_it() {
    let status = 0x8040_0010;
    let mut interrupt = PowerInterrupt {
        status,
        acknowledged: Cell::new(None),
    };
    let runtime = EmbassyPowerIrqRuntime::<NoopRawMutex>::new();

    let (disposition, snapshot) = handle_power_irq(&mut interrupt, &runtime);

    assert_eq!(disposition, PowerIrqDisposition::Posted);
    assert_eq!(snapshot.status, status);
    assert_eq!(interrupt.acknowledged.get(), Some(status));
    assert_eq!(runtime.try_take(), Some(status));
    assert_eq!(runtime.try_take(), None);
}

#[test]
fn spurious_power_irq_neither_acknowledges_nor_wakes_embassy() {
    let mut interrupt = PowerInterrupt {
        status: 0,
        acknowledged: Cell::new(None),
    };
    let runtime = EmbassyPowerIrqRuntime::<NoopRawMutex>::new();

    assert_eq!(
        handle_power_irq(&mut interrupt, &runtime).0,
        PowerIrqDisposition::Spurious
    );
    assert_eq!(interrupt.acknowledged.get(), None);
    assert_eq!(runtime.try_take(), None);
}
