//! ESP32-S31 MMIO decode windows touched by the open radio stack.
//!
//! These are chip-level address regions, not four parts of one monolithic
//! radio peripheral. The radio core depends on clock, reset, PMU and analog
//! services living in separate system regions.

/// One 1-MiB chip-level MMIO decode window used by the current SVD.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Esp32s31MmioWindow {
    /// Modem, PHY and Wi-Fi MAC register fabric.
    ModemRadioCore,
    /// High-performance system peripherals, including HP clock/reset.
    HighPerformanceSystem,
    /// Low-power system peripherals, including PMU and LP clock/reset.
    LowPowerSystem,
    /// Low-power analog/peripheral fabric, including temperature sensing.
    LowPowerPeripheral,
}

impl Esp32s31MmioWindow {
    pub const fn start(self) -> usize {
        match self {
            Self::ModemRadioCore => 0x2010_0000,
            Self::HighPerformanceSystem => 0x2050_0000,
            Self::LowPowerSystem => 0x2070_0000,
            Self::LowPowerPeripheral => 0x2080_0000,
        }
    }

    pub const fn end_exclusive(self) -> usize {
        self.start() + 0x0010_0000
    }

    pub const fn contains(self, address: usize) -> bool {
        address >= self.start() && address < self.end_exclusive()
    }
}

/// Classify an address into one of the independently decoded MMIO windows.
///
/// Sources:
///
/// - `esp-wifi-sys/c/include/esp32s31/soc/reg_base.h` for the HP, LP-system
///   and LP-peripheral windows;
/// - the pinned modem register structures, ROM MMIO instructions and
///   `IEEE802154_REG_BASE` in that header for the `0x201x_xxxx` modem/radio
///   window.
pub const fn classify_mmio_window(address: usize) -> Option<Esp32s31MmioWindow> {
    let windows = [
        Esp32s31MmioWindow::ModemRadioCore,
        Esp32s31MmioWindow::HighPerformanceSystem,
        Esp32s31MmioWindow::LowPowerSystem,
        Esp32s31MmioWindow::LowPowerPeripheral,
    ];
    let mut index = 0;
    while index < windows.len() {
        if windows[index].contains(address) {
            return Some(windows[index]);
        }
        index += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{classify_mmio_window, Esp32s31MmioWindow};

    #[test]
    fn classifies_each_independent_one_megabyte_window() {
        assert_eq!(
            classify_mmio_window(0x2010_0000),
            Some(Esp32s31MmioWindow::ModemRadioCore)
        );
        assert_eq!(
            classify_mmio_window(0x2058_7000),
            Some(Esp32s31MmioWindow::HighPerformanceSystem)
        );
        assert_eq!(
            classify_mmio_window(0x2070_4000),
            Some(Esp32s31MmioWindow::LowPowerSystem)
        );
        assert_eq!(
            classify_mmio_window(0x2081_8000),
            Some(Esp32s31MmioWindow::LowPowerPeripheral)
        );
    }

    #[test]
    fn rejects_holes_and_window_end_boundaries() {
        assert_eq!(classify_mmio_window(0x200f_ffff), None);
        assert_eq!(classify_mmio_window(0x2020_0000), None);
        assert_eq!(classify_mmio_window(0x2060_0000), None);
        assert_eq!(classify_mmio_window(0x2090_0000), None);
    }
}
