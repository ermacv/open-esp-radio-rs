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
pub mod phy_iq_estimator;
pub mod phy_memory;
pub mod phy_power_detector;
pub mod phy_prelude;
pub mod phy_rx_dco;
pub mod phy_temperature;
pub mod power;
pub use power::{PowerCheckpoint, PowerClockControl, PowerClockImages, PowerError};

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
    pub unsafe fn claim(peripheral: P) -> Self {
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
}

impl<P: PowerClockControl> Radio<P, state::Owned> {
    /// Execute the finite modem/PHY clock and reset prerequisites.
    ///
    /// Register fields come from the official ESP32-S31 PAC. The exact
    /// operation order and field values reproduce the pinned S31 `esp-hal`
    /// clock path at commit `6899213e`; the ROM-only frontend gates are a
    /// later owned PHY transition and are not folded into this type-state
    /// change.
    ///
    /// `P` owns the official platform capability. A successful read-back is
    /// the only safe path into `Radio<P, Powered>`.
    pub fn power_up(mut self) -> Result<Radio<P, state::Powered>, PowerUpFailure<P>> {
        if let Err(error) = power::execute_owned(&mut self.peripheral) {
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

    /// Borrow the platform and recovered-radio capabilities independently.
    ///
    /// The two mutable borrows are tied to this unique powered owner and refer
    /// to disjoint fields, allowing a lifecycle function to coordinate an
    /// official system PAC operation with internal Wi-Fi MMIO.
    #[doc(hidden)]
    pub fn parts_mut(&mut self) -> (&mut P, &mut RadioRegisters) {
        (&mut self.peripheral, &mut self.registers)
    }

    /// Enable the Wi-Fi RX/baseband path after the PHY transition completes.
    ///
    /// Espressif's `enable_phy_with_wifi_rx` lifecycle wrapper performs this
    /// operation after `register_chipv7_phy` or `phy_wakeup_init`.  Keeping it
    /// on the powered owner makes that final lifecycle edge explicit and
    /// prevents application code from writing `WIFI_BB_CFG` without owning the
    /// radio peripheral.
    #[cfg(target_arch = "riscv32")]
    pub fn enable_wifi_rx(&mut self) {
        phy_frequency::set_wifi_enabled(&mut self.registers, true);
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
    use super::{state, PowerClockControl, PowerClockImages, Radio};

    #[derive(Debug, Eq, PartialEq)]
    struct TestPeripheral {
        id: u8,
        ready: bool,
    }

    fn require_owned(_: &Radio<TestPeripheral, state::Owned>) {}
    fn require_powered(_: &Radio<TestPeripheral, state::Powered>) {}

    impl PowerClockControl for TestPeripheral {
        fn set_wifi_baseband_and_mac_reset(&mut self, _asserted: bool) {}
        fn select_hp_active_modem_icg(&mut self) {}
        fn apply_modem_icg_selection(&mut self) {}
        fn apply_sleep_icg_selection(&mut self) {}
        fn enable_modem_register_bus_clock(&mut self) {}
        fn configure_hp_active_modem_clock_map(&mut self) {}
        fn configure_shared_modem_clock_map(&mut self) {}
        fn configure_modem_source_clocks(&mut self) {}
        fn set_wifi_baseband_reset(&mut self, _asserted: bool) {}
        fn enable_phy_calibration_clocks(&mut self) {}
        fn select_phy_i2c_160mhz_source(&mut self) {}
        fn enable_phy_i2c_master_clock(&mut self) {}

        fn power_clock_images(&self) -> PowerClockImages {
            PowerClockImages {
                reset_released: self.ready,
                hp_active_icg_selected: self.ready,
                modem_bus_clock_enabled: self.ready,
                hp_active_clock_map_configured: self.ready,
                shared_clock_map_configured: self.ready,
                modem_source_clocks_configured: self.ready,
                phy_calibration_clocks_enabled: self.ready,
                phy_i2c_160mhz_selected: self.ready,
                phy_i2c_master_clock_enabled: self.ready,
            }
        }
    }

    #[test]
    fn peripheral_token_follows_the_type_state_owner() {
        // SAFETY: this test token represents no real hardware and no MMIO is
        // accessed.
        let owned = unsafe { Radio::claim(TestPeripheral { id: 7, ready: true }) };
        require_owned(&owned);

        let powered = owned
            .power_up()
            .unwrap_or_else(|_| panic!("fake prerequisite sequence failed"));
        require_powered(&powered);
        assert_eq!(powered.peripheral(), &TestPeripheral { id: 7, ready: true });
    }

    #[test]
    fn external_initialization_bridge_preserves_the_unique_owner() {
        // SAFETY: this test token represents no real hardware and no MMIO is
        // accessed.
        let owned = unsafe { Radio::claim(TestPeripheral { id: 8, ready: true }) };
        require_owned(&owned);

        // SAFETY: this host-only type-state test models a completed external
        // initializer and performs no register access.
        let powered = unsafe { owned.assume_powered_after_external_initialization() };
        require_powered(&powered);
        assert_eq!(powered.peripheral(), &TestPeripheral { id: 8, ready: true });
    }

    #[test]
    fn unpowered_owner_can_release_the_original_token() {
        // SAFETY: this test token represents no real hardware.
        let owned = unsafe { Radio::claim(TestPeripheral { id: 9, ready: true }) };
        assert_eq!(owned.release(), TestPeripheral { id: 9, ready: true });
    }

    #[test]
    fn failed_power_transition_returns_the_unique_owned_radio() {
        // SAFETY: this test token represents no real hardware.
        let owned = unsafe {
            Radio::claim(TestPeripheral {
                id: 11,
                ready: false,
            })
        };
        let failure = match owned.power_up() {
            Ok(_) => panic!("stuck reset unexpectedly powered the radio"),
            Err(failure) => failure,
        };
        let recovered = failure.into_radio();
        require_owned(&recovered);
        assert_eq!(
            recovered.release(),
            TestPeripheral {
                id: 11,
                ready: false
            }
        );
    }
}
