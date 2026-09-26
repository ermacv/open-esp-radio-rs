//! IEEE 802.15.4 transmit-on timing override on the shared baseband.
//!
//! The pinned public ESP32-S31 path runs the common `bt_bb_v2_init_cmplx(1)`
//! body through the shared BTBB acquisition, and `ieee802154_mac_init` later
//! overrides the shared auxiliary transmit-on delay with argument 50. The MAC
//! receive-on delay written by the same vendor step is part of the MAC
//! foundation.

#![deny(unsafe_code)]

use crate::generated;

impl crate::SharedRadioRegisters {
    /// Apply the IEEE 802.15.4 shared TX-on delay override alone.
    ///
    /// SOURCE: the complete shared setter reached by `ieee802154_txon_delay_set`
    /// from `ieee802154_mac_init`, which ESP-IDF runs after
    /// `esp_btbb_enable`: `AUXILIARY_TX_ON_DELAY=((50-10)<<3)`. The BTBB
    /// initialization writes the Bluetooth value to the same field only on its
    /// first enable, so the last IEEE 802.15.4 MAC initialization wins and no
    /// release restores the previous value. The caller orders this after the
    /// shared BTBB initialization; the device fence is the caller's.
    #[doc(hidden)]
    pub fn override_ieee802154_shared_tx_on_delay(&mut self) {
        generated::override_ieee802154_shared_tx_on_delay(
            &self.shared_radio.shared_baseband_tx_timing,
            generated::Ieee802154SharedTxOnDelayOverride::Delay50,
        );
    }
}
