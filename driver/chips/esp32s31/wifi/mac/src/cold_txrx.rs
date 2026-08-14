//! Ownership boundary for the complete cold `mac_txrx_init` transaction.

use open_esp_radio_esp32s31_hal::wifi_mac::WifiMacColdHal;

/// Vendor MAC-delay jitter reduced to the only representable hardware range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacDelaySlot(u8);

impl MacDelaySlot {
    const SLOT_COUNT: u32 = 11;

    pub const fn from_random(random: u32) -> Self {
        Self((random % Self::SLOT_COUNT) as u8)
    }

    pub const fn value(self) -> u8 {
        self.0
    }
}

pub trait MacColdTxRxHardware {
    fn initialize_txrx_prefix(&mut self);
    fn initialize_txrx_callbacks(&mut self, delay_slot: MacDelaySlot);
    fn initialize_txrx_suffix(&mut self);
}

impl MacColdTxRxHardware for WifiMacColdHal<'_> {
    fn initialize_txrx_prefix(&mut self) {
        WifiMacColdHal::initialize_txrx_prefix(self);
    }

    fn initialize_txrx_callbacks(&mut self, delay_slot: MacDelaySlot) {
        let programmed = self.initialize_txrx_callbacks(delay_slot.value());
        debug_assert!(programmed);
    }

    fn initialize_txrx_suffix(&mut self) {
        WifiMacColdHal::initialize_txrx_suffix(self);
    }
}
