//! Safe ownership-bound HCCFR and ICCFR register transactions.
//!
//! Legal scalar values and discrete states come from the generated PAC. The
//! separately ordered read-modify-write edges remain identical to the
//! recovered vendor leaves.

#![forbid(unsafe_code)]

use super::RadioRegisters;

/// One HCCFR/ICCFR value accepted by the recovered twelve-bit MMIO fields.
///
/// Construction is explicit so register operations cannot accidentally pass
/// a wider integer to the PAC.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CfrValue(u16);

impl CfrValue {
    /// Largest value representable by either recovered CFR field.
    pub const MAX: u16 = 0x0fff;

    /// Construct a value already known to fit the recovered field width.
    pub const fn new(value: u16) -> Option<Self> {
        if value <= Self::MAX {
            Some(Self(value))
        } else {
            None
        }
    }

    /// Return the value accepted by the generated PAC field writer.
    pub const fn get(self) -> u16 {
        self.0
    }
}

impl RadioRegisters {
    /// Publish both fields of complete pinned `phy_config_hccfr` in order.
    pub fn configure_hccfr(&mut self, enabled: bool, value: CfrValue) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        bb.hccfr_control().modify(|_, w| {
            if enabled {
                w.enable().enabled()
            } else {
                w.enable().disabled()
            }
        });
        bb.hccfr_value().modify(|_, w| w.value().set(value.get()));
    }

    /// Apply either complete branch of pinned `phy_iccfr_en`.
    pub fn configure_iccfr_gate(&mut self, enabled: bool) {
        self.peripherals
            .phy_baseband_config_oracle
            .iccfr_enable_control()
            .modify(|_, w| {
                if enabled {
                    w.gate().enabled()
                } else {
                    w.gate().disabled()
                }
            });
    }

    /// Publish all five fields and the tail gate of pinned `phy_force_iccfr`.
    pub fn configure_forced_iccfr(&mut self, mode: bool, enabled: bool, value: CfrValue) {
        let control = self
            .peripherals
            .phy_baseband_config_oracle
            .iccfr_force_control();
        control.modify(|_, w| w.force_mode_high().bit(mode));
        control.modify(|_, w| {
            if enabled {
                w.force_enable().enabled()
            } else {
                w.force_enable().disabled()
            }
        });
        control.modify(|_, w| w.force_trigger().set_bit());
        control.modify(|_, w| w.force_mode_low().bit(mode));
        control.modify(|_, w| w.force_value().set(value.get()));
        self.configure_iccfr_gate(enabled);
    }
}

#[cfg(test)]
mod tests {
    use super::CfrValue;

    #[test]
    fn value_rejects_wide_typed_values() {
        assert_eq!(CfrValue::new(0), Some(CfrValue(0)));
        assert_eq!(CfrValue::new(CfrValue::MAX), Some(CfrValue(0x0fff)));
        assert_eq!(CfrValue::new(0x1000), None);
    }
}
