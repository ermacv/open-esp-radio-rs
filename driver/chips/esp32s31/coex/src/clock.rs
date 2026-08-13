use crate::CoexError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoexTimerClock {
    pub selector: CoexClockSelector,
    /// Raw twelve-bit divider field sampled from `COEX_LP_CLK_CONF`.
    pub divider_field: u16,
    pub xtal_mhz: u32,
    pub real_chip: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum CoexClockSelector {
    Selector1 = 1,
    Selector2 = 2,
    Selector4 = 4,
    Selector8 = 8,
}

impl CoexClockSelector {
    pub const fn from_bits(value: u8) -> Result<Self, CoexError> {
        match value {
            1 => Ok(Self::Selector1),
            2 => Ok(Self::Selector2),
            4 => Ok(Self::Selector4),
            8 => Ok(Self::Selector8),
            _ => Err(CoexError::UnsupportedClock),
        }
    }

    /// Validate the public divisor used by `coex_hw_timer_freq_set`.
    /// The register stores this value minus one.
    pub const fn accepts_divisor(self, divisor: u16) -> bool {
        divisor <= 4096
            && match self {
                Self::Selector8 => divisor == 1,
                Self::Selector4 => divisor >= 50,
                Self::Selector2 => divisor >= 40,
                Self::Selector1 => divisor >= 3,
            }
    }
}

impl CoexTimerClock {
    /// Decode the two fresh `COEX_LP_CLK_CONF` images sampled by the vendor
    /// helper. Keeping this interpretation in the executor-neutral core lets
    /// platform adapters retain MMIO ownership without duplicating bit logic.
    pub fn from_register_images(
        selector_image: u32,
        divider_image: u32,
        xtal_mhz: u32,
        real_chip: bool,
    ) -> Result<Self, CoexError> {
        Ok(Self {
            selector: CoexClockSelector::from_bits((selector_image & 0x0f) as u8)?,
            divider_field: ((divider_image >> 4) & 0x0fff) as u16,
            xtal_mhz,
            real_chip,
        })
    }

    /// Reproduce `coex_hw_timer_tick_get`, including its two-stage integer
    /// division for clock sources two and four.
    pub fn tick_image(self, value: u32) -> Result<u32, CoexError> {
        let numerator = u64::from(value) << 19;
        let divider = u64::from(self.divider_field & 0x0fff) + 1;
        let scale = match self.selector {
            CoexClockSelector::Selector8 => {
                if self.real_chip {
                    16_000_000_u64
                } else {
                    16_384_000_u64
                }
            }
            CoexClockSelector::Selector4 => {
                if self.xtal_mhz == 0 {
                    return Err(CoexError::UnsupportedClock);
                }
                (524_288_u64 * 1_000_000 * divider) / (u64::from(self.xtal_mhz) * 1_000_000)
            }
            CoexClockSelector::Selector2 => (524_288_u64 * 1_000_000 * divider) / 20_000_000,
            CoexClockSelector::Selector1 => 1,
        };
        if scale == 0 {
            return Err(CoexError::UnsupportedClock);
        }
        Ok((numerator / scale) as u32)
    }
}

/// Platform-owned access to the shared low-power clock configuration.
///
/// Sampling is deliberately mutable: each call represents the two fresh MMIO
/// reads performed by one vendor `coex_hw_timer_tick_get` invocation.
pub trait CoexClockHardware {
    fn sample(&mut self) -> Result<CoexTimerClock, CoexError>;
}
