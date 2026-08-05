//! Embassy TX binding for the executor-independent ESP32-S31 STA peer port.
//!
//! Peer policy and hardware programming live in the chip station crate. This
//! module only teaches the Embassy-composed control TX owner how to provide
//! that port's narrow transmit capability and preserves the former public
//! module path during the extraction.

use open_esp_radio_esp32s31_wifi_lmac::{edca::EdcaParametersError, tx::HtPeerAmpduParameters};
use open_esp_radio_esp32s31_wifi_sta::peer::Esp32s31StaPeerTransmit;
use open_esp_radio_ieee80211::wmm::WmmParameterSet;

use crate::{
    control_tx::Esp32s31ControlTx,
    ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile, WifiTxTimer},
};

impl<P, E, T, const BUFFER_SIZE: usize> Esp32s31StaPeerTransmit
    for Esp32s31ControlTx<'_, P, E, T, BUFFER_SIZE>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
{
    fn install_ht_ampdu_policy(&mut self, parameters: HtPeerAmpduParameters) {
        Esp32s31ControlTx::install_ht_ampdu_policy(self, parameters);
    }

    fn install_he_bss_color(&mut self, bss_color: u8) {
        Esp32s31ControlTx::install_he_bss_color(self, bss_color);
    }

    fn install_wmm_edca(&mut self, parameters: WmmParameterSet) -> Result<(), EdcaParametersError> {
        Esp32s31ControlTx::install_wmm_edca(self, parameters)
    }
}
