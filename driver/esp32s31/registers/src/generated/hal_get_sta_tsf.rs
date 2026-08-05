//! Instruction-exact station TSF snapshot transaction.

use crate::svd;

/// Snapshot either or both station TSF words using the complete ROM leaf's
/// conditional-output semantics.
#[inline(always)]
pub(crate) fn generated_hal_get_sta_tsf(
    registers: &svd::WifiMacStaTsfLoad,
    low: Option<&mut u32>,
    high: Option<&mut u32>,
) {
    registers
        .control()
        .modify(|_, w| w.snapshot_station_tsf().set_bit());
    if let Some(low) = low {
        *low = registers.snapshot_low().read().value().bits();
    }
    if let Some(high) = high {
        *high = registers.snapshot_high().read().value().bits();
    }
    registers
        .control()
        .modify(|_, w| w.snapshot_station_tsf().clear_bit());
}
