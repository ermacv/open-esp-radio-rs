#![no_std]

use core::future::Future;
use core::marker::PhantomData;

pub use open_esp_radio_pac_esp32s31::RadioRegisters;

/// Type states for the coarse radio power lifecycle.
pub mod state {
    /// The application uniquely owns the peripheral, but the open driver has
    /// not yet established its clock/reset prerequisites.
    pub struct Owned {
        _private: (),
    }

    /// The radio clock/reset prerequisites have been established and finite
    /// PHY register operations may access MMIO.
    pub struct Powered {
        _private: (),
    }
}

/// Unique application-visible owner of an ESP32-S31 radio peripheral.
///
/// `P` is the integration layer's singleton token (for example,
/// `esp_hal::peripherals::WIFI`). Keeping it inside this value ties the open
/// driver's register capability to the safe peripheral owner.
pub struct Radio<P, State = state::Owned> {
    peripheral: P,
    registers: RadioRegisters,
    state: PhantomData<State>,
}

impl<P> Radio<P, state::Owned> {
    /// Bind the integration layer's unique peripheral token to the open
    /// driver's register capability.
    ///
    /// # Safety
    ///
    /// The token must uniquely represent the ESP32-S31 Wi-Fi/radio peripheral,
    /// and ROM/vendor code must not own or mutate that peripheral until this
    /// value is released.
    pub const unsafe fn claim(peripheral: P) -> Self {
        Self {
            peripheral,
            registers: unsafe { RadioRegisters::steal() },
            state: PhantomData,
        }
    }

    /// Release a radio that has not crossed into the powered state.
    pub fn release(self) -> P {
        self.peripheral
    }

    /// Mark externally established clock/reset prerequisites as complete.
    ///
    /// # Safety
    ///
    /// The caller must have enabled every clock and power domain required by
    /// the PHY register graph, released the documented resets, selected the
    /// correct 40 MHz source, and excluded concurrent ROM/vendor access.
    ///
    /// This temporary integration seam will become safe once those
    /// prerequisites are implemented by the source-owned ESP32-S31 HAL.
    pub unsafe fn assume_powered(self) -> Radio<P, state::Powered> {
        Radio {
            peripheral: self.peripheral,
            registers: self.registers,
            state: PhantomData,
        }
    }
}

impl<P> Radio<P, state::Powered> {
    /// Borrow the integration token without releasing register ownership.
    pub const fn peripheral(&self) -> &P {
        &self.peripheral
    }

    /// Internal register capability used by source-owned target bindings.
    ///
    /// The returned borrow cannot outlive the unique powered radio owner.
    #[doc(hidden)]
    pub fn registers_mut(&mut self) -> &mut RadioRegisters {
        &mut self.registers
    }
}

/// Executor-neutral source of asynchronous deadlines.
pub trait AsyncDelay {
    type Error;

    fn delay_micros(&mut self, micros: u32) -> impl Future<Output = Result<(), Self::Error>> + '_;
}

/// Executor-neutral interrupt/event edge.
pub trait AsyncEvent {
    type Event;
    type Error;

    fn wait(&mut self) -> impl Future<Output = Result<Self::Event, Self::Error>> + '_;
}

#[cfg(test)]
mod tests {
    use super::{state, Radio};

    #[derive(Debug, Eq, PartialEq)]
    struct TestPeripheral(u8);

    fn require_owned(_: &Radio<TestPeripheral, state::Owned>) {}
    fn require_powered(_: &Radio<TestPeripheral, state::Powered>) {}

    #[test]
    fn peripheral_token_follows_the_type_state_owner() {
        // SAFETY: this test token represents no real hardware and no MMIO is
        // accessed.
        let owned = unsafe { Radio::claim(TestPeripheral(7)) };
        require_owned(&owned);

        // SAFETY: no target operation is executed in this host-only test.
        let powered = unsafe { owned.assume_powered() };
        require_powered(&powered);
        assert_eq!(powered.peripheral(), &TestPeripheral(7));
    }

    #[test]
    fn unpowered_owner_can_release_the_original_token() {
        // SAFETY: this test token represents no real hardware.
        let owned = unsafe { Radio::claim(TestPeripheral(9)) };
        assert_eq!(owned.release(), TestPeripheral(9));
    }
}
