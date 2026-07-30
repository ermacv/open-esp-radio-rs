//! Ownership boundary for the direct pre-COEX tail of `hal_init`.

use open_esp_radio_pac_esp32s31::ColdRadioRegisters;

/// OS-adapter slow-clock calibration reduced to the blob's 18-bit field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacSlowClockCalibration(u32);

impl MacSlowClockCalibration {
    const MASK: u32 = 0x0003_ffff;

    /// Preserve the exact truncation performed by `hal_timer_update_by_rtc`.
    pub const fn from_osi_value(value: u32) -> Self {
        Self(value & Self::MASK)
    }

    pub const fn value(self) -> u32 {
        self.0
    }
}

pub trait MacColdHalTailHardware {
    fn initialize_hal_tail(
        &mut self,
        event_mask: u32,
        slow_clock_calibration: MacSlowClockCalibration,
    );
}

impl MacColdHalTailHardware for ColdRadioRegisters {
    fn initialize_hal_tail(
        &mut self,
        event_mask: u32,
        slow_clock_calibration: MacSlowClockCalibration,
    ) {
        let programmed = self.initialize_mac_hal_tail(event_mask, slow_clock_calibration.value());
        debug_assert!(programmed);
    }
}
