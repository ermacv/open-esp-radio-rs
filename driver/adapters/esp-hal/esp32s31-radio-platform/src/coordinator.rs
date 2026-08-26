//! Platform-owned reference accounting for the upstream 160 MHz source.
//!
//! MODEM_SYSCON dependencies live in the affine custom-PAC route. This
//! coordinator retains only the remaining HP_SYS_CLKRST source and the
//! exclusive platform lease.

use core::cell::RefCell;

use critical_section::Mutex;
use open_esp_radio_esp32s31_bluetooth::{BluetoothClockControl, BluetoothPlatformClockState};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ClockDevice {
    Pll160mSource,
}

pub(crate) trait ClockIo {
    fn clock_is_enabled(&self, device: ClockDevice) -> bool;
    fn set_clock_enabled(&mut self, device: ClockDevice, enabled: bool);
}

struct CoordinatorState {
    pll_count: u16,
    pll_restore_enabled: bool,
    bluetooth_reserved: bool,
}

impl CoordinatorState {
    const fn new() -> Self {
        Self {
            pll_count: 0,
            pll_restore_enabled: false,
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
                pll_acquired: false,
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

    fn acquire_pll(&self) {
        self.with_inner(|inner| {
            if inner.state.pll_count == 0 {
                let observed = inner.io.clock_is_enabled(ClockDevice::Pll160mSource);
                inner.state.pll_restore_enabled = observed;
                if !observed {
                    inner.io.set_clock_enabled(ClockDevice::Pll160mSource, true);
                }
            }
            inner.state.pll_count = inner
                .state
                .pll_count
                .checked_add(1)
                .expect("ESP32-S31 PLL reference count cannot overflow");
        });
    }

    fn release_pll(&self) {
        self.with_inner(|inner| {
            assert!(
                inner.state.pll_count != 0,
                "unbalanced ESP32-S31 PLL release"
            );
            inner.state.pll_count -= 1;
            if inner.state.pll_count == 0 {
                if !inner.state.pll_restore_enabled {
                    inner
                        .io
                        .set_clock_enabled(ClockDevice::Pll160mSource, false);
                }
                inner.state.pll_restore_enabled = false;
            }
        });
    }

    fn pll_enabled(&self) -> bool {
        self.with_inner(|inner| inner.io.clock_is_enabled(ClockDevice::Pll160mSource))
    }

    fn release_bluetooth_reservation(&self) {
        self.with_inner(|inner| {
            assert!(
                inner.state.bluetooth_reserved,
                "unbalanced Bluetooth platform release"
            );
            inner.state.bluetooth_reserved = false;
        });
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothPlatformBusy;

pub(crate) struct BluetoothPlatformLease<'a, I: ClockIo> {
    coordinator: &'a ClockCoordinator<I>,
    pll_acquired: bool,
}

impl<I: ClockIo> BluetoothClockControl for BluetoothPlatformLease<'_, I> {
    fn enable_bluetooth_controller_pll_source(&mut self) {
        if !self.pll_acquired {
            self.coordinator.acquire_pll();
            self.pll_acquired = true;
        }
    }

    fn bluetooth_platform_clock_state(&mut self) -> BluetoothPlatformClockState {
        let enabled = self.coordinator.pll_enabled();
        BluetoothPlatformClockState {
            pll_160mhz_source_enabled: enabled,
        }
    }

    fn disable_bluetooth_controller_pll_source(&mut self) {
        if self.pll_acquired {
            self.coordinator.release_pll();
            self.pll_acquired = false;
        }
    }
}

impl<I: ClockIo> Drop for BluetoothPlatformLease<'_, I> {
    fn drop(&mut self) {
        self.disable_bluetooth_controller_pll_source();
        self.coordinator.release_bluetooth_reservation();
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc, vec::Vec};

    use open_esp_radio_esp32s31_bluetooth::BluetoothClockControl;

    use super::{ClockCoordinator, ClockDevice, ClockIo};

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct Operation(bool);

    struct FakeIo {
        enabled: bool,
        operations: Rc<RefCell<Vec<Operation>>>,
    }

    impl ClockIo for FakeIo {
        fn clock_is_enabled(&self, _device: ClockDevice) -> bool {
            self.enabled
        }
        fn set_clock_enabled(&mut self, _device: ClockDevice, enabled: bool) {
            self.enabled = enabled;
            self.operations.borrow_mut().push(Operation(enabled));
        }
    }

    #[test]
    fn pll_is_physically_gated_once_and_restored_on_drop() {
        let operations = Rc::new(RefCell::new(Vec::new()));
        let coordinator = ClockCoordinator::new(FakeIo {
            enabled: false,
            operations: operations.clone(),
        });
        {
            let mut lease = coordinator.try_bluetooth().unwrap();
            lease.enable_bluetooth_controller_pll_source();
            lease.enable_bluetooth_controller_pll_source();
        }
        assert_eq!(&*operations.borrow(), &[Operation(true), Operation(false)]);
    }

    #[test]
    fn preexisting_pll_state_is_not_disabled() {
        let operations = Rc::new(RefCell::new(Vec::new()));
        let coordinator = ClockCoordinator::new(FakeIo {
            enabled: true,
            operations: operations.clone(),
        });
        {
            let mut lease = coordinator.try_bluetooth().unwrap();
            lease.enable_bluetooth_controller_pll_source();
        }
        assert!(operations.borrow().is_empty());
    }

    #[test]
    fn bluetooth_platform_lease_is_exclusive_and_drop_releases_it() {
        let coordinator = ClockCoordinator::new(FakeIo {
            enabled: false,
            operations: Rc::new(RefCell::new(Vec::new())),
        });
        let first = coordinator.try_bluetooth().unwrap();
        assert!(coordinator.try_bluetooth().is_err());
        drop(first);
        assert!(coordinator.try_bluetooth().is_ok());
    }
}
