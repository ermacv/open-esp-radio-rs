//! Exclusive reservation for the standalone Bluetooth platform witnesses.
//!
//! Clock/reset ownership and reference accounting live in the affine custom
//! PAC route. This coordinator only prevents two Bluetooth lifecycles from
//! borrowing the same retained ESP-HAL singleton witnesses simultaneously.

use core::cell::RefCell;

use critical_section::Mutex;

struct CoordinatorState {
    bluetooth_reserved: bool,
}

impl CoordinatorState {
    const fn new() -> Self {
        Self {
            bluetooth_reserved: false,
        }
    }
}

pub(crate) struct ClockCoordinator {
    state: Mutex<RefCell<CoordinatorState>>,
}

impl ClockCoordinator {
    pub(crate) const fn new() -> Self {
        Self {
            state: Mutex::new(RefCell::new(CoordinatorState::new())),
        }
    }

    pub(crate) fn try_bluetooth(
        &self,
    ) -> Result<BluetoothPlatformLease<'_>, BluetoothPlatformBusy> {
        let acquired = self.with_state(|state| {
            if state.bluetooth_reserved {
                false
            } else {
                state.bluetooth_reserved = true;
                true
            }
        });
        if acquired {
            Ok(BluetoothPlatformLease { coordinator: self })
        } else {
            Err(BluetoothPlatformBusy)
        }
    }

    fn with_state<R>(&self, operation: impl FnOnce(&mut CoordinatorState) -> R) -> R {
        critical_section::with(|critical_section| {
            operation(&mut self.state.borrow(critical_section).borrow_mut())
        })
    }

    fn release_bluetooth_reservation(&self) {
        self.with_state(|state| {
            assert!(
                state.bluetooth_reserved,
                "unbalanced Bluetooth platform release"
            );
            state.bluetooth_reserved = false;
        });
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothPlatformBusy;

pub(crate) struct BluetoothPlatformLease<'a> {
    coordinator: &'a ClockCoordinator,
}

impl Drop for BluetoothPlatformLease<'_> {
    fn drop(&mut self) {
        self.coordinator.release_bluetooth_reservation();
    }
}

#[cfg(test)]
mod tests {
    use super::ClockCoordinator;

    #[test]
    fn bluetooth_platform_lease_is_exclusive_and_drop_releases_it() {
        let coordinator = ClockCoordinator::new();
        let first = coordinator.try_bluetooth().unwrap();
        assert!(coordinator.try_bluetooth().is_err());
        drop(first);
        assert!(coordinator.try_bluetooth().is_ok());
    }
}
