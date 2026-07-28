//! Generated-PAC ownership for the complete cold MAC TX-power publication.

use super::RadioRegisters;

/// Number of two-byte rate entries produced by complete `hal_init_tx_pwr`.
pub const MAC_TX_POWER_RATE_COUNT: usize = 43;

/// Calibrated PHY gain-table indices for one MAC rate.
///
/// These values are signed indices after the ROM divides its quarter-unit
/// target-power values by four. They are not dBm.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacTxPowerPair {
    pub primary: i8,
    pub alternate: i8,
}

impl MacTxPowerPair {
    pub const ZERO: Self = Self {
        primary: 0,
        alternate: 0,
    };
}

/// Rust-owned replacement for vendor `s_phy_get_max_pwr[43][2]`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacTxPowerTable {
    entries: [MacTxPowerPair; MAC_TX_POWER_RATE_COUNT],
}

impl MacTxPowerTable {
    pub const fn new(entries: [MacTxPowerPair; MAC_TX_POWER_RATE_COUNT]) -> Self {
        Self { entries }
    }

    pub const fn pair(&self, rate: u8) -> Option<MacTxPowerPair> {
        let index = rate as usize;
        if index < MAC_TX_POWER_RATE_COUNT {
            Some(self.entries[index])
        } else {
            None
        }
    }

    const fn primary_byte(&self, rate: u8) -> u8 {
        // Every rate used by the three cold leaves is in 0..=25. Preserve the
        // complete hal_get_tx_pwr remap as part of the owned table API because
        // the same table later formats HE rates above 25.
        let table_rate = if rate > 25 {
            rate.wrapping_sub(10)
        } else {
            rate
        };
        self.entries[table_rate as usize].primary as u8
    }

    const fn primary_index(&self, rate: u8) -> u8 {
        self.primary_byte(rate) & 0x3f
    }
}

fn relative_index(base: u8, floor: u8) -> u8 {
    base.max(floor).wrapping_sub(floor) & 0x3f
}

impl RadioRegisters {
    /// Publish the complete TB, immediate-response and TB-RU power tables.
    ///
    /// SOURCE: complete pinned `_oracles/libpp.a` functions recorded as
    /// `BLOB_LIBPP_HAL_TX_POWER_INIT`. This method preserves all 56
    /// fresh-read RMW edges and their original child-call order.
    pub fn initialize_mac_tx_power(&mut self, table: &MacTxPowerTable) {
        let init = &self.peripherals.wifi_mac_tx_power_init;

        // Complete hal_init_tb_power: rates 16..=25, one six-bit field per RMW.
        let tb0 = init.tb_power(0);
        tb0.modify(|_, w| unsafe { w.power_0().bits(table.primary_index(16)) });
        tb0.modify(|_, w| unsafe { w.power_1().bits(table.primary_index(17)) });
        tb0.modify(|_, w| unsafe { w.power_2().bits(table.primary_index(18)) });
        tb0.modify(|_, w| unsafe { w.power_3().bits(table.primary_index(19)) });
        let tb1 = init.tb_power(1);
        tb1.modify(|_, w| unsafe { w.power_0().bits(table.primary_index(20)) });
        tb1.modify(|_, w| unsafe { w.power_1().bits(table.primary_index(21)) });
        tb1.modify(|_, w| unsafe { w.power_2().bits(table.primary_index(22)) });
        tb1.modify(|_, w| unsafe { w.power_3().bits(table.primary_index(23)) });
        let tb2 = init.tb_power(2);
        tb2.modify(|_, w| unsafe { w.power_0().bits(table.primary_index(24)) });
        tb2.modify(|_, w| unsafe { w.power_1().bits(table.primary_index(25)) });

        // Complete hal_init_imrsp_power. Each tuple is
        // (format, encoded rate, table lookup rate). The blob replaces these
        // three fields through three separate RMWs for every command word.
        const RESPONSE: [(u8, u8, u8); 10] = [
            (0, 0, 0),
            (0, 1, 0),
            (0, 5, 5),
            (0, 11, 11),
            (0, 10, 10),
            (0, 9, 9),
            (2, 16, 16),
            (2, 17, 17),
            (2, 18, 18),
            (2, 18, 18),
        ];
        for (word, (format, encoded_rate, lookup_rate)) in RESPONSE.into_iter().enumerate() {
            let response = init.immediate_response(word);
            response.modify(|_, w| unsafe { w.format_unknown().bits(format) });
            response.modify(|_, w| unsafe { w.rate_index().bits(encoded_rate) });
            response
                .modify(|_, w| unsafe { w.power_index().bits(table.primary_index(lookup_rate)) });
        }

        // Complete hal_init_tb_ru_power and the complete setter jump tables.
        // The complete leaf zero-extends the signed table byte before each
        // unsigned clamp. Do not truncate to the six-bit destination field
        // until after subtraction: negative i8 values therefore retain the
        // blob's modulo-256 then modulo-64 behavior.
        let base = table.primary_byte(16);
        let delta_10 = relative_index(base, 10);
        let delta_7 = relative_index(base, 7);
        let delta_4 = relative_index(base, 4);

        // Selectors 0..=8.
        let ru0 = init.tb_ru_power(0);
        ru0.modify(|_, w| unsafe { w.power_0().bits(delta_10) });
        ru0.modify(|_, w| unsafe { w.power_1().bits(delta_10) });
        ru0.modify(|_, w| unsafe { w.power_2().bits(delta_10) });
        ru0.modify(|_, w| unsafe { w.power_3().bits(delta_10) });
        ru0.modify(|_, w| unsafe { w.power_4().bits(delta_10) });
        let ru1 = init.tb_ru_power(1);
        ru1.modify(|_, w| unsafe { w.power_0().bits(delta_10) });
        ru1.modify(|_, w| unsafe { w.power_1().bits(delta_10) });
        ru1.modify(|_, w| unsafe { w.power_2().bits(delta_10) });
        ru1.modify(|_, w| unsafe { w.power_3().bits(delta_10) });

        // Selectors 37..=40.
        ru1.modify(|_, w| unsafe { w.power_4().bits(delta_7) });
        let ru2 = init.tb_ru_power(2);
        ru2.modify(|_, w| unsafe { w.power_0().bits(delta_7) });
        ru2.modify(|_, w| unsafe { w.power_1().bits(delta_7) });
        ru2.modify(|_, w| unsafe { w.power_2().bits(delta_7) });

        // Selectors 53, 54 and 61.
        ru2.modify(|_, w| unsafe { w.power_3().bits(delta_4) });
        ru2.modify(|_, w| unsafe { w.power_4().bits(delta_4) });
        init.tb_ru_power_tail()
            .modify(|_, w| unsafe { w.power_index().bits(base & 0x3f) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_retains_both_bytes_and_vendor_rate_remap() {
        let entries = core::array::from_fn(|rate| MacTxPowerPair {
            primary: rate as i8,
            alternate: -(rate as i8),
        });
        let table = MacTxPowerTable::new(entries);
        assert_eq!(
            table.pair(42),
            Some(MacTxPowerPair {
                primary: 42,
                alternate: -42
            })
        );
        assert_eq!(table.pair(43), None);
        assert_eq!(table.primary_index(25), 25);
        assert_eq!(table.primary_index(26), 16);
    }

    #[test]
    fn tb_ru_delta_matches_unsigned_blob_clamp() {
        assert_eq!(relative_index(3, 10), 0);
        assert_eq!(relative_index(10, 10), 0);
        assert_eq!(relative_index(21, 10), 11);
        assert_eq!(relative_index(0x80, 10), 54);
    }
}
