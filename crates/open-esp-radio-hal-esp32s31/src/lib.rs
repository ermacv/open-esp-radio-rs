#![no_std]

#[cfg(test)]
extern crate std;

use core::future::Future;
use core::marker::PhantomData;

pub use open_esp_radio_pac_esp32s31::{power as radio_registers, RadioRegisters, Register32};
pub mod analog_i2c;
pub mod pbus;
pub mod phy_agc;
pub mod phy_baseband;
pub mod phy_frequency;
pub mod phy_i2c;
pub mod phy_memory;
pub mod phy_power_detector;
pub mod power;
pub use power::{PowerCheckpoint, PowerError, PowerEvidence, PowerOperation, PowerSequence};

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

/// Failed power transition retaining the unique unpowered radio owner.
pub struct PowerUpFailure<P> {
    radio: Radio<P, state::Owned>,
    error: PowerError,
}

impl<P> PowerUpFailure<P> {
    /// Inspect the exact failed read-back checkpoint.
    pub const fn error(&self) -> PowerError {
        self.error
    }

    /// Recover the owner for diagnostics, reset, or a controlled retry.
    pub fn into_radio(self) -> Radio<P, state::Owned> {
        self.radio
    }

    /// Recover both the owner and the checkpoint error.
    pub fn into_parts(self) -> (Radio<P, state::Owned>, PowerError) {
        (self.radio, self.error)
    }
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

    /// Adopt radio clocks and resets established by an external PHY oracle.
    ///
    /// This is intentionally separate from [`Self::power_up`]: a comparison
    /// HIL may first run the vendor cold initializer and must not pulse the
    /// already calibrated radio reset merely to obtain the Rust type state.
    ///
    /// # Safety
    ///
    /// The caller must have completed the ESP32-S31 modem/PHY clock, power and
    /// reset prerequisites represented by [`state::Powered`]. No external
    /// driver may continue accessing the radio after this transition; the
    /// peripheral token and returned value must remain the unique owner.
    pub unsafe fn assume_powered_after_external_initialization(self) -> Radio<P, state::Powered> {
        Radio {
            peripheral: self.peripheral,
            registers: self.registers,
            state: PhantomData,
        }
    }

    /// Execute the finite modem/PHY clock and reset prerequisites.
    ///
    /// Register fields come from the pinned ESP32-S31 modem/PMU headers and
    /// SVD. The exact operation order and field values reproduce the pinned
    /// S31 `esp-hal` clock path at commit `6899213e`; the ROM-only frontend
    /// gates are a later owned PHY transition and are not folded into this
    /// type-state change.
    ///
    /// The method is target-only because host tests use a private fake
    /// register backend. A successful read-back is the only safe path into
    /// `Radio<P, Powered>`.
    #[cfg(target_arch = "riscv32")]
    pub fn power_up(mut self) -> Result<Radio<P, state::Powered>, PowerUpFailure<P>> {
        if let Err(error) = power::execute(&mut self.registers) {
            return Err(PowerUpFailure { radio: self, error });
        }
        Ok(Radio {
            peripheral: self.peripheral,
            registers: self.registers,
            state: PhantomData,
        })
    }

    #[cfg(test)]
    fn power_up_with(
        self,
        io: &mut impl power::RegisterIo,
    ) -> Result<Radio<P, state::Powered>, PowerUpFailure<P>> {
        if let Err(error) = power::execute(io) {
            return Err(PowerUpFailure { radio: self, error });
        }
        Ok(Radio {
            peripheral: self.peripheral,
            registers: self.registers,
            state: PhantomData,
        })
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
    use std::vec::Vec;

    use open_esp_radio_pac_esp32s31::power::modem_syscon;

    use super::{power::RegisterIo, state, Radio, Register32};

    #[derive(Debug, Eq, PartialEq)]
    struct TestPeripheral(u8);

    fn require_owned(_: &Radio<TestPeripheral, state::Owned>) {}
    fn require_powered(_: &Radio<TestPeripheral, state::Powered>) {}

    #[derive(Default)]
    struct FakeRegisters {
        values: Vec<(Register32, u32)>,
        writes: Vec<(Register32, u32)>,
    }

    impl RegisterIo for FakeRegisters {
        fn read(&mut self, register: Register32) -> u32 {
            self.values
                .iter()
                .find_map(|(candidate, value)| (*candidate == register).then_some(*value))
                .unwrap_or(0)
        }

        fn write(&mut self, register: Register32, value: u32) {
            if let Some(entry) = self
                .values
                .iter_mut()
                .find(|(candidate, _)| *candidate == register)
            {
                entry.1 = value;
            } else {
                self.values.push((register, value));
            }
            self.writes.push((register, value));
        }
    }

    #[test]
    fn peripheral_token_follows_the_type_state_owner() {
        // SAFETY: this test token represents no real hardware and no MMIO is
        // accessed.
        let owned = unsafe { Radio::claim(TestPeripheral(7)) };
        require_owned(&owned);

        let powered = owned
            .power_up_with(&mut FakeRegisters::default())
            .unwrap_or_else(|_| panic!("fake prerequisite sequence failed"));
        require_powered(&powered);
        assert_eq!(powered.peripheral(), &TestPeripheral(7));
    }

    #[test]
    fn external_initialization_bridge_preserves_the_unique_owner() {
        // SAFETY: this test token represents no real hardware and no MMIO is
        // accessed.
        let owned = unsafe { Radio::claim(TestPeripheral(8)) };
        require_owned(&owned);

        // SAFETY: this host-only type-state test models a completed external
        // initializer and performs no register access.
        let powered = unsafe { owned.assume_powered_after_external_initialization() };
        require_powered(&powered);
        assert_eq!(powered.peripheral(), &TestPeripheral(8));
    }

    #[test]
    fn unpowered_owner_can_release_the_original_token() {
        // SAFETY: this test token represents no real hardware.
        let owned = unsafe { Radio::claim(TestPeripheral(9)) };
        assert_eq!(owned.release(), TestPeripheral(9));
    }

    #[test]
    fn failed_power_transition_returns_the_unique_owned_radio() {
        struct StuckReset;

        impl RegisterIo for StuckReset {
            fn read(&mut self, register: Register32) -> u32 {
                if register == modem_syscon::MODEM_RST_CONF {
                    (1 << 8) | (1 << 9)
                } else {
                    0
                }
            }

            fn write(&mut self, _register: Register32, _value: u32) {}
        }

        // SAFETY: this test token represents no real hardware.
        let owned = unsafe { Radio::claim(TestPeripheral(11)) };
        let failure = match owned.power_up_with(&mut StuckReset) {
            Ok(_) => panic!("stuck reset unexpectedly powered the radio"),
            Err(failure) => failure,
        };
        let recovered = failure.into_radio();
        require_owned(&recovered);
        assert_eq!(recovered.release(), TestPeripheral(11));
    }
}
