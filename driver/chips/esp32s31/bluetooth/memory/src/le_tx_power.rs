//! Private conversion from requested LE transmit power to the S31 descriptor image.

#![forbid(unsafe_code)]

/// Reproduce the complete signed-byte conversion used by the current S31
/// controller. The returned five-bit value never crosses the memory-codec
/// boundary.
pub(super) const fn rounded_tx_power(default_tx_power_dbm: i8) -> u8 {
    match default_tx_power_dbm {
        i8::MIN..=-16 => 0,
        -15..=-13 => 3,
        -12..=-10 => 4,
        -9..=-7 => 5,
        -6..=-4 => 6,
        -3..=-1 => 7,
        0..=2 => 8,
        3..=5 => 9,
        6..=8 => 10,
        9..=11 => 11,
        12..=14 => 12,
        15..=17 => 13,
        18..=19 => 14,
        20..=i8::MAX => 15,
    }
}

#[cfg(test)]
mod tests;
