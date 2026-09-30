//! Portable EDCA contention: the CSMA/CA backoff draw above the lower-MAC
//! port.
//!
//! A backend that counts down a chosen backoff but does not draw it (no
//! [`HardwareServices::BACKOFF_DRAW`](oer_ieee80211_lower_mac::HardwareServices::BACKOFF_DRAW))
//! receives the slot count with each attempt as
//! [`Backoff::Slots`]. [`EdcaContention`] keeps the contention window of one
//! access category (IEEE 802.11-2020 10.23.2): it starts at `CWmin`, doubles
//! after each failed attempt of a frame up to `CWmax` and returns to
//! `CWmin` when the frame ends. The random source is injected, so a seeded
//! source makes the draws deterministic.

use oer_ieee80211_lower_mac::Backoff;
use oer_ieee80211_mac::extensions::wmm::WmmAcParameters;

/// The largest contention-window exponent: the four-bit ECW fields of the
/// EDCA Parameter Set.
pub const MAX_CW_EXPONENT: u8 = 15;

/// A source of uniformly distributed random words for backoff draws.
pub trait BackoffEntropy {
    fn next_u32(&mut self) -> u32;
}

impl<F: FnMut() -> u32> BackoffEntropy for F {
    fn next_u32(&mut self) -> u32 {
        self()
    }
}

/// The contention window of one access category and the retry count of its
/// current frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EdcaContention {
    cw_min_exponent: u8,
    cw_max_exponent: u8,
    retries: u8,
}

impl EdcaContention {
    /// The window of `ECWmin` and `ECWmax`, each clamped to
    /// [`MAX_CW_EXPONENT`]; a maximum below the minimum is raised to it.
    pub const fn new(cw_min_exponent: u8, cw_max_exponent: u8) -> Self {
        let cw_min_exponent = if cw_min_exponent > MAX_CW_EXPONENT {
            MAX_CW_EXPONENT
        } else {
            cw_min_exponent
        };
        let cw_max_exponent = if cw_max_exponent > MAX_CW_EXPONENT {
            MAX_CW_EXPONENT
        } else if cw_max_exponent < cw_min_exponent {
            cw_min_exponent
        } else {
            cw_max_exponent
        };
        Self {
            cw_min_exponent,
            cw_max_exponent,
            retries: 0,
        }
    }

    /// The window an AC Parameter Record advertises.
    pub const fn from_wmm(parameters: WmmAcParameters) -> Self {
        Self::new(parameters.ecw_min, parameters.ecw_max)
    }

    /// The exponent `n` of the current window `CW = 2^n - 1`.
    pub const fn cw_exponent(self) -> u8 {
        let raised = self.cw_min_exponent.saturating_add(self.retries);
        if raised > self.cw_max_exponent {
            self.cw_max_exponent
        } else {
            raised
        }
    }

    /// The current window `CW` in slots.
    pub const fn cw(self) -> u16 {
        ((1_u32 << self.cw_exponent()) - 1) as u16
    }

    /// Failed attempts of the current frame.
    pub const fn retries(self) -> u8 {
        self.retries
    }

    /// Draw the backoff of the next attempt uniformly from `0..=CW`.
    pub fn draw(self, entropy: &mut impl BackoffEntropy) -> Backoff {
        Backoff::Slots((entropy.next_u32() & u32::from(self.cw())) as u16)
    }

    /// Double the window after a failed attempt of the frame (an ACK or CTS
    /// timeout or a collision), until it reaches `CWmax`.
    pub fn record_failure(&mut self) {
        if self.cw_exponent() < self.cw_max_exponent {
            self.retries += 1;
        }
    }

    /// Return to `CWmin` when the frame ends: after its success, or after
    /// the caller's retry limit discarded it.
    pub fn reset(&mut self) {
        self.retries = 0;
    }
}

/// The backoff of attempt `retries + 1` of a frame under `parameters`,
/// without keeping state: the window is `CWmin` doubled `retries` times,
/// bounded by `CWmax`.
pub fn draw_backoff(
    parameters: WmmAcParameters,
    retries: u8,
    entropy: &mut impl BackoffEntropy,
) -> Backoff {
    let mut contention = EdcaContention::from_wmm(parameters);
    for _ in 0..retries {
        contention.record_failure();
    }
    contention.draw(entropy)
}

#[cfg(test)]
mod tests;
