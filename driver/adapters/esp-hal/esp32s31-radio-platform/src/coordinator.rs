//! Role-neutral reference accounting for shared modem clock dependencies.

use core::cell::RefCell;

use critical_section::Mutex;
use open_esp_radio_esp32s31_bluetooth::{BluetoothClockControl, BluetoothPlatformClockState};

const DEVICE_COUNT: usize = ClockDevice::Count as usize;

const BLUETOOTH_CONTROLLER_PLL_SOURCE: [ClockDevice; 1] = [ClockDevice::Pll160mSource];

const BLUETOOTH_CONTROLLER_DEPENDENTS: [ClockDevice; 6] = [
    ClockDevice::WifiBaseband80x1,
    ClockDevice::Etm,
    ClockDevice::BluetoothMac,
    ClockDevice::BluetoothPeripheral,
    ClockDevice::BluetoothApb,
    ClockDevice::BluetoothBaseband,
];

const BLUETOOTH_APB_DEPENDENCIES: [ClockDevice; 4] = [
    ClockDevice::Pll160mSource,
    ClockDevice::Etm,
    ClockDevice::BluetoothMac,
    ClockDevice::BluetoothApb,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ClockDevice {
    Pll160mSource,
    WifiBaseband80x1,
    Etm,
    BluetoothMac,
    BluetoothPeripheral,
    BluetoothApb,
    BluetoothBaseband,
    Count,
}

impl ClockDevice {
    const fn index(self) -> usize {
        self as usize
    }
}

pub(crate) trait ClockIo {
    fn prepare_icg_maps(&mut self);
    fn clock_is_enabled(&self, device: ClockDevice) -> bool;
    fn set_clock_enabled(&mut self, device: ClockDevice, enabled: bool);
    fn reset_bluetooth_controller_domains(&mut self);
    fn controller_resets_released(&self) -> bool;
}

#[derive(Clone, Copy)]
struct DeviceReference {
    count: u16,
    restore_enabled: bool,
}

impl DeviceReference {
    const EMPTY: Self = Self {
        count: 0,
        restore_enabled: false,
    };
}

struct CoordinatorState {
    devices: [DeviceReference; DEVICE_COUNT],
    bluetooth_reserved: bool,
}

impl CoordinatorState {
    const fn new() -> Self {
        Self {
            devices: [DeviceReference::EMPTY; DEVICE_COUNT],
            bluetooth_reserved: false,
        }
    }
}

struct CoordinatorInner<I> {
    io: I,
    state: CoordinatorState,
}

pub(crate) struct ClockCoordinator<I> {
    inner: Mutex<RefCell<CoordinatorInner<I>>>,
}

impl<I: ClockIo> ClockCoordinator<I> {
    pub(crate) const fn new(io: I) -> Self {
        Self {
            inner: Mutex::new(RefCell::new(CoordinatorInner {
                io,
                state: CoordinatorState::new(),
            })),
        }
    }

    pub(crate) fn try_bluetooth(
        &self,
    ) -> Result<BluetoothPlatformLease<'_, I>, BluetoothPlatformBusy> {
        let acquired = self.with_inner(|inner| {
            if inner.state.bluetooth_reserved {
                false
            } else {
                inner.state.bluetooth_reserved = true;
                true
            }
        });

        if acquired {
            Ok(BluetoothPlatformLease {
                coordinator: self,
                controller_pll_source_acquired: false,
                controller_dependents_acquired: false,
                apb_clocks_acquired: false,
            })
        } else {
            Err(BluetoothPlatformBusy)
        }
    }

    fn with_inner<R>(&self, operation: impl FnOnce(&mut CoordinatorInner<I>) -> R) -> R {
        critical_section::with(|critical_section| {
            operation(&mut self.inner.borrow(critical_section).borrow_mut())
        })
    }

    fn acquire(&self, dependencies: &[ClockDevice]) {
        self.with_inner(|inner| {
            inner.io.prepare_icg_maps();
            for &device in dependencies {
                let index = device.index();
                if inner.state.devices[index].count == 0 {
                    let restore_enabled = inner.io.clock_is_enabled(device);
                    inner.state.devices[index].restore_enabled = restore_enabled;
                    if !restore_enabled {
                        inner.io.set_clock_enabled(device, true);
                    }
                }
                inner.state.devices[index].count = inner.state.devices[index]
                    .count
                    .checked_add(1)
                    .expect("ESP32-S31 modem clock reference count overflow");
            }
        });
    }

    fn release(&self, dependencies: &[ClockDevice]) {
        self.with_inner(|inner| {
            for &device in dependencies {
                let index = device.index();
                let reference = &mut inner.state.devices[index];
                assert!(
                    reference.count != 0,
                    "unbalanced ESP32-S31 modem clock release"
                );
                reference.count -= 1;
                if reference.count == 0 {
                    if !reference.restore_enabled {
                        inner.io.set_clock_enabled(device, false);
                    }
                    reference.restore_enabled = false;
                }
            }
        });
    }

    fn reset_bluetooth_controller_domains(&self) {
        self.with_inner(|inner| inner.io.reset_bluetooth_controller_domains());
    }

    fn bluetooth_clock_state(&self) -> BluetoothPlatformClockState {
        self.with_inner(|inner| BluetoothPlatformClockState {
            controller_clocks_enabled: BLUETOOTH_CONTROLLER_PLL_SOURCE
                .iter()
                .chain(BLUETOOTH_CONTROLLER_DEPENDENTS.iter())
                .all(|&device| inner.io.clock_is_enabled(device)),
            apb_clocks_enabled: BLUETOOTH_APB_DEPENDENCIES
                .iter()
                .all(|&device| inner.io.clock_is_enabled(device)),
            controller_resets_released: inner.io.controller_resets_released(),
        })
    }

    fn release_bluetooth_reservation(&self) {
        self.with_inner(|inner| {
            assert!(
                inner.state.bluetooth_reserved,
                "unbalanced ESP32-S31 Bluetooth platform release"
            );
            inner.state.bluetooth_reserved = false;
        });
    }
}

/// Another Bluetooth lifecycle already owns the coordinator's Bluetooth slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothPlatformBusy;

pub(crate) struct BluetoothPlatformLease<'a, I: ClockIo> {
    coordinator: &'a ClockCoordinator<I>,
    controller_pll_source_acquired: bool,
    controller_dependents_acquired: bool,
    apb_clocks_acquired: bool,
}

impl<I: ClockIo> BluetoothClockControl for BluetoothPlatformLease<'_, I> {
    fn enable_bluetooth_controller_pll_source(&mut self) {
        if !self.controller_pll_source_acquired {
            self.coordinator.acquire(&BLUETOOTH_CONTROLLER_PLL_SOURCE);
            self.controller_pll_source_acquired = true;
        }
    }

    fn enable_bluetooth_controller_dependents(&mut self) {
        if !self.controller_dependents_acquired {
            self.coordinator.acquire(&BLUETOOTH_CONTROLLER_DEPENDENTS);
            self.controller_dependents_acquired = true;
        }
    }

    fn enable_bluetooth_apb_clocks(&mut self) {
        if !self.apb_clocks_acquired {
            self.coordinator.acquire(&BLUETOOTH_APB_DEPENDENCIES);
            self.apb_clocks_acquired = true;
        }
    }

    fn reset_bluetooth_controller_domains(&mut self) {
        self.coordinator.reset_bluetooth_controller_domains();
    }

    fn bluetooth_platform_clock_state(&mut self) -> BluetoothPlatformClockState {
        self.coordinator.bluetooth_clock_state()
    }

    fn disable_bluetooth_apb_clocks(&mut self) {
        if self.apb_clocks_acquired {
            self.coordinator.release(&BLUETOOTH_APB_DEPENDENCIES);
            self.apb_clocks_acquired = false;
        }
    }

    fn disable_bluetooth_controller_pll_source(&mut self) {
        if self.controller_pll_source_acquired {
            self.coordinator.release(&BLUETOOTH_CONTROLLER_PLL_SOURCE);
            self.controller_pll_source_acquired = false;
        }
    }

    fn disable_bluetooth_controller_dependents(&mut self) {
        if self.controller_dependents_acquired {
            self.coordinator.release(&BLUETOOTH_CONTROLLER_DEPENDENTS);
            self.controller_dependents_acquired = false;
        }
    }
}

impl<I: ClockIo> Drop for BluetoothPlatformLease<'_, I> {
    fn drop(&mut self) {
        self.disable_bluetooth_apb_clocks();
        self.disable_bluetooth_controller_pll_source();
        self.disable_bluetooth_controller_dependents();
        self.coordinator.release_bluetooth_reservation();
    }
}

#[cfg(test)]
mod tests {
    use std::vec::Vec;

    use super::{
        BLUETOOTH_APB_DEPENDENCIES, BLUETOOTH_CONTROLLER_DEPENDENTS,
        BLUETOOTH_CONTROLLER_PLL_SOURCE, BluetoothClockControl, ClockCoordinator, ClockDevice,
        ClockIo, DEVICE_COUNT,
    };

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Operation {
        PrepareIcg,
        SetClock(ClockDevice, bool),
        ResetDomains,
    }

    struct FakeIo {
        enabled: [bool; DEVICE_COUNT],
        resets_released: bool,
        operations: Vec<Operation>,
    }

    impl FakeIo {
        fn new() -> Self {
            Self {
                enabled: [false; DEVICE_COUNT],
                resets_released: false,
                operations: Vec::new(),
            }
        }
    }

    impl ClockIo for FakeIo {
        fn prepare_icg_maps(&mut self) {
            self.operations.push(Operation::PrepareIcg);
        }

        fn clock_is_enabled(&self, device: ClockDevice) -> bool {
            self.enabled[device.index()]
        }

        fn set_clock_enabled(&mut self, device: ClockDevice, enabled: bool) {
            self.enabled[device.index()] = enabled;
            self.operations.push(Operation::SetClock(device, enabled));
        }

        fn reset_bluetooth_controller_domains(&mut self) {
            self.resets_released = true;
            self.operations.push(Operation::ResetDomains);
        }

        fn controller_resets_released(&self) -> bool {
            self.resets_released
        }
    }

    #[test]
    fn overlapping_module_dependencies_are_physically_gated_once() {
        let coordinator = ClockCoordinator::new(FakeIo::new());
        let mut bluetooth = coordinator.try_bluetooth().unwrap();

        bluetooth.enable_bluetooth_controller_pll_source();
        bluetooth.enable_bluetooth_controller_dependents();
        bluetooth.enable_bluetooth_apb_clocks();
        bluetooth.reset_bluetooth_controller_domains();
        assert_eq!(
            bluetooth.bluetooth_platform_clock_state(),
            open_esp_radio_esp32s31_bluetooth::BluetoothPlatformClockState {
                controller_clocks_enabled: true,
                apb_clocks_enabled: true,
                controller_resets_released: true,
            }
        );

        bluetooth.disable_bluetooth_apb_clocks();
        bluetooth.disable_bluetooth_controller_pll_source();
        bluetooth.disable_bluetooth_controller_dependents();
        drop(bluetooth);

        coordinator.with_inner(|inner| {
            let enabled = inner
                .io
                .operations
                .iter()
                .filter(|operation| matches!(operation, Operation::SetClock(_, true)))
                .count();
            let disabled = inner
                .io
                .operations
                .iter()
                .filter(|operation| matches!(operation, Operation::SetClock(_, false)))
                .count();
            assert_eq!(
                enabled,
                BLUETOOTH_CONTROLLER_PLL_SOURCE.len() + BLUETOOTH_CONTROLLER_DEPENDENTS.len()
            );
            assert_eq!(
                disabled,
                BLUETOOTH_CONTROLLER_PLL_SOURCE.len() + BLUETOOTH_CONTROLLER_DEPENDENTS.len()
            );
            assert_eq!(
                inner
                    .io
                    .operations
                    .iter()
                    .filter(|operation| matches!(operation, Operation::PrepareIcg))
                    .count(),
                3
            );
            assert_eq!(
                inner
                    .io
                    .operations
                    .iter()
                    .filter(|operation| matches!(operation, Operation::ResetDomains))
                    .count(),
                1
            );
        });
    }

    #[test]
    fn preexisting_shared_clock_state_is_restored_not_disabled() {
        let mut io = FakeIo::new();
        io.enabled[ClockDevice::Pll160mSource.index()] = true;
        let coordinator = ClockCoordinator::new(io);

        let mut bluetooth = coordinator.try_bluetooth().unwrap();
        bluetooth.enable_bluetooth_controller_pll_source();
        bluetooth.enable_bluetooth_controller_dependents();
        bluetooth.enable_bluetooth_apb_clocks();
        bluetooth.disable_bluetooth_apb_clocks();
        bluetooth.disable_bluetooth_controller_pll_source();
        bluetooth.disable_bluetooth_controller_dependents();
        drop(bluetooth);

        coordinator.with_inner(|inner| {
            assert!(inner.io.enabled[ClockDevice::Pll160mSource.index()]);
            assert!(!inner.io.operations.iter().any(|operation| {
                matches!(
                    operation,
                    Operation::SetClock(ClockDevice::Pll160mSource, _)
                )
            }));
        });
    }

    #[test]
    fn dropping_an_active_bluetooth_lease_unwinds_owned_platform_state() {
        let coordinator = ClockCoordinator::new(FakeIo::new());

        {
            let mut bluetooth = coordinator.try_bluetooth().unwrap();
            bluetooth.enable_bluetooth_controller_pll_source();
            bluetooth.enable_bluetooth_controller_dependents();
            bluetooth.enable_bluetooth_apb_clocks();
        }

        coordinator.with_inner(|inner| {
            assert!(!inner.state.bluetooth_reserved);
            assert!(inner.state.devices.iter().all(|device| device.count == 0));
            assert!(inner.io.enabled.iter().all(|enabled| !enabled));
        });

        assert!(coordinator.try_bluetooth().is_ok());
    }

    #[test]
    fn bluetooth_lifecycle_slot_is_exclusive_and_recoverable() {
        let coordinator = ClockCoordinator::new(FakeIo::new());
        let first = coordinator.try_bluetooth().unwrap();
        assert!(coordinator.try_bluetooth().is_err());
        drop(first);
        assert!(coordinator.try_bluetooth().is_ok());
    }

    #[test]
    fn apb_dependency_set_is_a_subset_of_controller_set() {
        assert!(BLUETOOTH_APB_DEPENDENCIES.iter().all(|dependency| {
            BLUETOOTH_CONTROLLER_PLL_SOURCE.contains(dependency)
                || BLUETOOTH_CONTROLLER_DEPENDENTS.contains(dependency)
        }));
    }
}
