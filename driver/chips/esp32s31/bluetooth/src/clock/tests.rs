use std::{cell::RefCell, rc::Rc, vec::Vec};

use open_esp_radio_esp32s31_pac::{
    BluetoothLowPowerClockObservation, PlatformClockPowerObservation, RadioHardware,
    SharedModemClockObservation,
};

use super::{
    BluetoothClockCheckpoint, BluetoothClockedResources, BluetoothColdOwner,
    BluetoothModemClockState, BluetoothSharedClockControl, disable_owned, enable_owned,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    PrepareSharedMap,
    PrepareModemMap,
    EnablePllSource,
    RetainCoexistence,
    EnableControllerDependents,
    EnableApb,
    ResetDomains,
    SelectMainXtal,
    ObservePlatform,
    ObserveShared,
    ReleaseLowPower,
    DisableApb,
    DisablePllSource,
    ReleaseCoexistence,
    DisableControllerDependents,
    Cleanup,
}

struct FakePlatform {
    operations: Rc<RefCell<Vec<Operation>>>,
}

struct FakeShared {
    operations: Rc<RefCell<Vec<Operation>>>,
    platform: PlatformClockPowerObservation,
    shared: SharedModemClockObservation,
    low_power: BluetoothLowPowerClockObservation,
}

impl FakeShared {
    fn ready(operations: Rc<RefCell<Vec<Operation>>>) -> Self {
        Self {
            operations,
            platform: PlatformClockPowerObservation {
                hp_active_icg_selected: false,
                modem_register_bus_clock_enabled: false,
                ref_160m_clock_enabled: true,
                modem_source_clocks_configured: true,
            },
            shared: SharedModemClockObservation {
                power_state_map_configured: true,
                coexistence_clock_enabled: true,
                phy_i2c_master_clock_enabled: false,
                low_power_timer_clock_enabled: true,
            },
            low_power: BluetoothLowPowerClockObservation {
                exclusive_main_xtal_selected: true,
                bluetooth_divider_configured: true,
                timer_enabled: true,
            },
        }
    }
}

impl BluetoothSharedClockControl for FakeShared {
    fn retain_platform_pll_source(&mut self) {
        self.operations
            .borrow_mut()
            .push(Operation::EnablePllSource);
    }

    fn release_platform_pll_source(&mut self) {
        self.operations
            .borrow_mut()
            .push(Operation::DisablePllSource);
    }

    fn platform_clock_power_observation(&self) -> PlatformClockPowerObservation {
        self.operations
            .borrow_mut()
            .push(Operation::ObservePlatform);
        self.platform
    }

    fn prepare_modem_syscon_clock_map(&mut self) {
        self.operations
            .borrow_mut()
            .push(Operation::PrepareModemMap);
    }

    fn enable_bluetooth_controller_dependents(&mut self) {
        self.operations
            .borrow_mut()
            .push(Operation::EnableControllerDependents);
    }

    fn enable_bluetooth_apb_clocks(&mut self) {
        self.operations.borrow_mut().push(Operation::EnableApb);
    }

    fn reset_bluetooth_controller_domains(&mut self) {
        self.operations.borrow_mut().push(Operation::ResetDomains);
    }

    fn modem_syscon_clock_state(&self) -> BluetoothModemClockState {
        BluetoothModemClockState {
            controller_clocks_enabled: true,
            apb_clocks_enabled: true,
            controller_resets_released: true,
        }
    }

    fn disable_bluetooth_apb_clocks(&mut self) {
        self.operations.borrow_mut().push(Operation::DisableApb);
    }

    fn disable_bluetooth_controller_dependents(&mut self) {
        self.operations
            .borrow_mut()
            .push(Operation::DisableControllerDependents);
    }

    fn prepare_shared_modem_clock_map(&mut self) {
        self.operations
            .borrow_mut()
            .push(Operation::PrepareSharedMap);
    }

    fn retain_coexistence_clock(&mut self) {
        self.operations
            .borrow_mut()
            .push(Operation::RetainCoexistence);
    }

    fn release_coexistence_clock(&mut self) {
        self.operations
            .borrow_mut()
            .push(Operation::ReleaseCoexistence);
    }

    fn retain_main_xtal_low_power_clock(&mut self) {
        self.operations.borrow_mut().push(Operation::SelectMainXtal);
    }

    fn release_bluetooth_low_power_timer(&mut self) {
        self.operations
            .borrow_mut()
            .push(Operation::ReleaseLowPower);
    }

    fn bluetooth_shared_clock_observation(
        &self,
    ) -> (
        SharedModemClockObservation,
        BluetoothLowPowerClockObservation,
    ) {
        self.operations.borrow_mut().push(Operation::ObserveShared);
        (self.shared, self.low_power)
    }
}

impl FakePlatform {
    fn ready(operations: Rc<RefCell<Vec<Operation>>>) -> Self {
        Self { operations }
    }
}

fn record_cleanup(_: &mut BluetoothColdOwner, platform: &mut FakePlatform) {
    platform.operations.borrow_mut().push(Operation::Cleanup);
}

fn validation_clocked(
    operations: Rc<RefCell<Vec<Operation>>>,
) -> BluetoothClockedResources<FakePlatform> {
    BluetoothClockedResources {
        registers: Some(BluetoothColdOwner::from_radio_hardware(
            RadioHardware::for_validation(),
        )),
        platform: Some(FakePlatform::ready(operations)),
        cleanup: record_cleanup,
        cleanup_armed: true,
    }
}

#[test]
fn dropping_clocked_resources_runs_reversible_cleanup_once() {
    let operations = Rc::new(RefCell::new(Vec::new()));
    drop(validation_clocked(operations.clone()));
    assert_eq!(*operations.borrow(), [Operation::Cleanup]);
}

#[test]
fn consuming_clocked_resources_disarms_drop_cleanup() {
    let operations = Rc::new(RefCell::new(Vec::new()));
    let resources = validation_clocked(operations.clone());
    let (_registers, _platform) = resources.into_parts();
    assert!(operations.borrow().is_empty());
}

#[test]
fn exact_clock_order_joins_route_owned_observations() {
    let operations = Rc::new(RefCell::new(Vec::new()));
    let mut shared = FakeShared::ready(operations.clone());
    assert_eq!(enable_owned(&mut shared), Ok(()));
    disable_owned(&mut shared);

    assert_eq!(
        *operations.borrow(),
        [
            Operation::PrepareSharedMap,
            Operation::PrepareModemMap,
            Operation::EnablePllSource,
            Operation::RetainCoexistence,
            Operation::EnableControllerDependents,
            Operation::EnableApb,
            Operation::ResetDomains,
            Operation::SelectMainXtal,
            Operation::ObservePlatform,
            Operation::ObserveShared,
            Operation::ReleaseLowPower,
            Operation::DisableApb,
            Operation::DisablePllSource,
            Operation::ReleaseCoexistence,
            Operation::DisableControllerDependents,
        ]
    );
}

#[test]
fn failed_readback_rolls_back_before_returning_owners() {
    let operations = Rc::new(RefCell::new(Vec::new()));
    let mut shared = FakeShared::ready(operations.clone());
    shared.low_power.timer_enabled = false;

    let failure = enable_owned(&mut shared).unwrap_err();
    assert_eq!(
        failure.checkpoint,
        BluetoothClockCheckpoint::LowPowerTimerClock
    );
    assert_eq!(
        &operations.borrow()[10..],
        [
            Operation::ReleaseLowPower,
            Operation::DisableApb,
            Operation::DisablePllSource,
            Operation::ReleaseCoexistence,
            Operation::DisableControllerDependents,
        ]
    );
}
