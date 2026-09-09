use super::*;
use oer_esp32s31_hal::types::MacInterruptMask;

mod operation;

// This model has no hardware status banks or power masks to destroy. Real
// routes must supply their distinct preservation transactions explicitly.
impl oer_esp32s31_wifi_mac::irq::MacInterruptPauseRoute for Route {
    type Paused = u8;

    fn pause(&mut self, platform: &Self::Platform) -> Result<Self::Paused, Self::Error> {
        self.quiesce(platform)
    }

    fn resume(
        &mut self,
        platform: &Self::Platform,
        paused: Self::Paused,
    ) -> Result<(), (Self::Error, Self::Paused)> {
        self.activate(platform, paused, MacInterruptMask::COLD_RX)
    }

    fn finish_pause(&mut self, paused: Self::Paused) -> Self::Setup {
        paused
    }
}

#[test]
fn pause_restores_acknowledged_work_without_fabricating_hardware_irqs() {
    let mac = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
    let power = EmbassyPowerIrqRuntime::<NoopRawMutex>::new();
    let platform = Cell::new(0);
    let mut epoch = InterruptEpoch::new(Route { active: false }, 7, &mac, &power);
    epoch
        .activate(&platform, MacInterruptMask::COLD_RX)
        .unwrap();
    mac.publish(EVENT_RX_SUCCESS | EVENT_TX_COMPLETE);
    mac.notify_rx_capacity();
    let old_power =
        MacPowerInterruptObservation::from_semantic_events(true, false, false, false, false);
    power.publish(old_power);
    let paused = epoch
        .try_pause(&platform)
        .unwrap_or_else(|_| panic!("pause"));
    assert_eq!(mac.drain_pending(), Default::default());
    assert!(power.try_take().is_none());
    // A returned buffer or newly published event must not be erased by replay.
    mac.publish(EVENT_TX_TIMEOUT);
    power.publish(MacPowerInterruptObservation::from_semantic_events(
        false, true, false, false, false,
    ));
    let mut epoch = paused
        .try_resume(&platform)
        .unwrap_or_else(|_| panic!("resume"));
    assert_eq!(mac.rx_post_count(), 1);
    assert_eq!(
        mac.drain_pending(),
        super::super::EmbassyMacIrqDrain {
            rx: true,
            rx_capacity: true,
            tx_events: EVENT_TX_COMPLETE | EVENT_TX_TIMEOUT,
        }
    );
    assert_eq!(
        power.try_take(),
        Some(MacPowerInterruptObservation::from_semantic_events(
            true, true, false, false, false
        ))
    );
    assert_eq!(mac.drain_pending(), Default::default());
    assert!(power.try_take().is_none());
    epoch.quiesce(&platform).unwrap();
}

#[test]
fn failed_activation_retains_work_and_original_moderation_until_retry() {
    fn unmask() {}
    let mac = EmbassyMacIrqRuntime::<NoopRawMutex>::new_with_rx_moderation(unmask);
    let power = EmbassyPowerIrqRuntime::<NoopRawMutex>::new();
    let platform = Cell::new(0);
    let mut epoch = InterruptEpoch::new(Route { active: false }, 7, &mac, &power);
    epoch
        .activate_rx_moderated(&platform, MacInterruptMask::COLD_RX)
        .unwrap();
    mac.publish(EVENT_TX_COMPLETE);
    let paused = epoch
        .try_pause(&platform)
        .unwrap_or_else(|_| panic!("pause"));
    assert!(!mac.is_rx_moderation_active());
    platform.set(10);
    let (paused, error) = paused.try_resume(&platform).err().expect("failed route");
    assert_eq!(
        error,
        MacInterruptEpochActivateError::Route(RouteError::Activation)
    );
    assert!(!mac.is_rx_moderation_active());
    assert_eq!(mac.drain_pending(), Default::default());
    platform.set(0);
    let mut epoch = paused
        .try_resume(&platform)
        .unwrap_or_else(|_| panic!("retry"));
    assert!(epoch.is_rx_moderated());
    assert!(mac.is_rx_moderation_active());
    assert!(
        mac.rx_signaled(),
        "resume must probe descriptors even without an old RX edge"
    );
    assert_eq!(mac.rx_post_count(), 0);
    assert_eq!(mac.try_take_tx(), Some(EVENT_TX_COMPLETE));
    assert_eq!(mac.try_take_tx(), None);
    epoch.quiesce(&platform).unwrap();
}

#[test]
fn failed_pause_does_not_drain_and_terminal_stop_does_not_replay() {
    let mac = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
    let power = EmbassyPowerIrqRuntime::<NoopRawMutex>::new();
    let platform = Cell::new(0);
    let mut epoch = InterruptEpoch::new(Route { active: false }, 7, &mac, &power);
    epoch
        .activate(&platform, MacInterruptMask::COLD_RX)
        .unwrap();
    mac.publish(EVENT_RX_SUCCESS);
    platform.set(20);
    let (epoch, error) = epoch.try_pause(&platform).err().expect("quiesce failure");
    assert_eq!(
        error,
        MacInterruptEpochQuiesceError::Route(RouteError::Quiescence)
    );
    assert!(mac.rx_signaled());
    platform.set(0);
    let paused = epoch
        .try_pause(&platform)
        .unwrap_or_else(|_| panic!("retry"));
    let (epoch, pending) = paused.into_stopped();
    assert!(!epoch.is_active());
    assert!(pending.mac.rx);
    assert!(!mac.rx_signaled());
    assert!(epoch.try_into_inactive_parts().is_ok());
}

// Deliberately different, non-Copy tokens: a pause must never become cold
// setup through the runtime without the route's terminal transaction.
struct DistinctRoute {
    active: bool,
    terminal: Rc<Cell<u32>>,
}
struct Retained(Rc<Cell<u32>>);

impl MacInterruptRoute for DistinctRoute {
    type Platform = Cell<u8>;
    type Setup = ();
    type Error = RouteError;

    fn activate(
        &mut self,
        _: &Self::Platform,
        _: (),
        _: MacInterruptMask,
    ) -> Result<(), (RouteError, ())> {
        self.active = true;
        Ok(())
    }

    fn quiesce(&mut self, _: &Self::Platform) -> Result<(), RouteError> {
        panic!("resumable route must not call terminal quiesce")
    }
}

impl oer_esp32s31_wifi_mac::irq::MacInterruptPauseRoute for DistinctRoute {
    type Paused = Retained;

    fn pause(&mut self, _: &Self::Platform) -> Result<Retained, RouteError> {
        assert!(self.active);
        self.active = false;
        Ok(Retained(self.terminal.clone()))
    }

    fn resume(
        &mut self,
        platform: &Self::Platform,
        paused: Retained,
    ) -> Result<(), (RouteError, Retained)> {
        assert!(!self.active);
        if platform.get() == 10 {
            return Err((RouteError::Activation, paused));
        }
        assert!(Rc::ptr_eq(&paused.0, &self.terminal));
        self.active = true;
        Ok(())
    }

    fn finish_pause(&mut self, paused: Retained) {
        assert!(!self.active);
        assert!(Rc::ptr_eq(&paused.0, &self.terminal));
        self.terminal.set(self.terminal.get() + 1);
    }
}

#[test]
fn distinct_pause_authority_survives_failure_until_explicit_terminal_cleanup() {
    let terminal = Rc::new(Cell::new(0));
    let mac = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
    let power = EmbassyPowerIrqRuntime::<NoopRawMutex>::new();
    let platform = Cell::new(0);
    let mut epoch = InterruptEpoch::new(
        DistinctRoute {
            active: false,
            terminal: terminal.clone(),
        },
        (),
        &mac,
        &power,
    );
    epoch
        .activate(&platform, MacInterruptMask::COLD_RX)
        .unwrap();
    for _ in 0..3 {
        let paused = epoch
            .try_pause(&platform)
            .unwrap_or_else(|_| panic!("pause"));
        assert_eq!(Rc::strong_count(&terminal), 3);
        platform.set(10);
        let (paused, _) = paused
            .try_resume(&platform)
            .err()
            .expect("retained failure");
        assert_eq!(Rc::strong_count(&terminal), 3);
        platform.set(0);
        epoch = paused
            .try_resume(&platform)
            .unwrap_or_else(|_| panic!("resume"));
        assert_eq!(Rc::strong_count(&terminal), 2);
        assert_eq!(terminal.get(), 0);
    }
    let paused = epoch
        .try_pause(&platform)
        .unwrap_or_else(|_| panic!("pause"));
    let (epoch, _) = paused.into_stopped();
    assert_eq!(terminal.get(), 1);
    assert!(!epoch.is_active());
    drop(epoch);
    assert_eq!(Rc::strong_count(&terminal), 1);
}
