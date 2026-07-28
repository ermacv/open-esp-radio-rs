//! Owned ESP32-S31 cold-PHY prelude register leaves.
//!
//! This module contains only semantic operations used before and around the
//! RF initializer. Numeric MMIO identities stay in the generated PAC.

#[cfg(target_arch = "riscv32")]
use open_esp_radio_pac_esp32s31::RadioRegisters;

/// Platform operation required by the fixed-crystal PHY prelude.
///
/// `MODEM_LPCON` is a documented system peripheral, so its ownership and
/// field decoding remain in the integration layer's official PAC.
pub trait PhyPreludePlatformControl {
    fn configure_fixed_xtal_40mhz_tick(&mut self);
}

/// Configure the fixed ESP32-S31 40 MHz crystal-derived tick value.
///
/// Complete pinned `libphy.a[phy_init.o]::phy_get_xtal_freq`, size `0x40`,
/// replaces the six-bit target with `frequency_mhz - 1`. ESP32-S31's public
/// chip contract fixes the crystal at 40 MHz, so this finite method performs
/// one official-PAC read/modify/write with no hidden RTC state.
pub fn configure_fixed_xtal_40mhz(platform: &mut impl PhyPreludePlatformControl) {
    platform.configure_fixed_xtal_40mhz_tick();
}

/// Sample the full-width counter used by the SDM-stability deadline.
///
/// Complete rev0 ROM `phy_wait_i2c_sdm_stable` at `0x2f823e76`, size
/// `0x4a`, samples this word before and after each PHY-I2C read and compares
/// their wrapping unsigned difference with 9,999. This method performs one
/// read; deadline arithmetic and retry ownership stay in the transition.
#[cfg(target_arch = "riscv32")]
pub fn sample_sdm_deadline_counter(registers: &mut RadioRegisters) -> u32 {
    registers.sample_sdm_deadline_counter()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakePlatform {
        calls: usize,
    }

    impl PhyPreludePlatformControl for FakePlatform {
        fn configure_fixed_xtal_40mhz_tick(&mut self) {
            self.calls += 1;
        }
    }

    #[test]
    fn fixed_xtal_delegates_one_semantic_platform_operation() {
        let mut platform = FakePlatform::default();
        configure_fixed_xtal_40mhz(&mut platform);
        assert_eq!(platform.calls, 1);
    }
}
