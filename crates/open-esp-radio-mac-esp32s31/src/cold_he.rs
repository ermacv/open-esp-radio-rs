//! Ownership boundary for bounded complete `hal_he_init` transactions.

use open_esp_radio_pac_esp32s31::{
    MacTxPowerPair, MacTxPowerTable, RadioRegisters, MAC_TX_POWER_RATE_COUNT,
};

pub trait MacColdHeHardware {
    fn initialize_he_prefix(&mut self);
    fn initialize_tx_power(&mut self, table: &MacTxPowerTable);
}

/// Calibrated two-byte target-power producer owned by the PHY/platform layer.
///
/// This replaces the direct MAC dependency on ROM `phy_get_max_pwr` and on the
/// vendor global `s_phy_get_max_pwr`. The current PHY implementation may still
/// use the pinned ROM routine behind this boundary; the MAC owns the resulting
/// fixed table and never borrows PHY calibration state.
pub trait MacTxPowerSource {
    fn mac_tx_power_pair(&mut self, rate: u8) -> MacTxPowerPair;
}

pub fn query_tx_power_table(source: &mut impl MacTxPowerSource) -> MacTxPowerTable {
    let mut entries = [MacTxPowerPair::ZERO; MAC_TX_POWER_RATE_COUNT];
    for (rate, entry) in entries.iter_mut().enumerate() {
        *entry = source.mac_tx_power_pair(rate as u8);
    }
    MacTxPowerTable::new(entries)
}

impl MacColdHeHardware for RadioRegisters {
    fn initialize_he_prefix(&mut self) {
        self.initialize_mac_he_prefix();
    }

    fn initialize_tx_power(&mut self, table: &MacTxPowerTable) {
        self.initialize_mac_tx_power(table);
    }
}
