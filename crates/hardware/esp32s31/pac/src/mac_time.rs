//! The free-running MAC microsecond counter.
//!
//! Complete rev0 ROM `phy_wait_i2c_sdm_stable` samples this full word for the
//! PHY's SDM-stability deadline; the pinned `libpp.a[pm_coex.o]` Wi-Fi power
//! management reads the same word to place Wi-Fi in the coexistence cycle.
//! The word is read-only and a read has no effect, so the PAC splits its one
//! partition into a reader per owner and neither owner serializes the other.

use crate::svd;

/// One owner's reader of the MAC microsecond counter.
#[must_use = "dropping the counter reader loses its register authority"]
pub struct MacTimeCounter {
    registers: svd::PhyColdDeadlineOracle,
}

impl MacTimeCounter {
    /// Sample the full wrapping counter word once.
    pub fn sample(&self) -> u32 {
        svd::field_read::sample_phy_sdm_deadline_counter(&self.registers)
    }
}

/// Split the counter partition into the PHY's and Wi-Fi's readers.
///
/// Invariant: the partition is consumed here, and the counter word is
/// read-only without read side effects, so two readers of the same word
/// cannot change what the other observes.
#[allow(
    unsafe_code,
    reason = "consuming the read-only partition creates one reader per owner"
)]
pub(crate) fn split(
    peripherals: svd::peripheral_ownership::MacTimeCounterPeripherals,
) -> (MacTimeCounter, MacTimeCounter) {
    let svd::peripheral_ownership::MacTimeCounterPeripherals {
        phy_cold_deadline_oracle,
    } = peripherals;
    // SAFETY: the partition was consumed above; see the invariant.
    let wifi = unsafe { svd::PhyColdDeadlineOracle::steal() };
    (
        MacTimeCounter {
            registers: phy_cold_deadline_oracle,
        },
        MacTimeCounter { registers: wifi },
    )
}
