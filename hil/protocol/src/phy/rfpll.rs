//! Terminal RFPLL procedure values; no readback, RF lock or Celsius claim.
use serde::{Deserialize, Serialize};

/// Diagnostic freshness bound, not a thermally safe calibration interval.
pub const STATION_RFPLL_SAMPLE_MAX_AGE_MICROS: u64 = 1_000_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RfpllEvidence {
    /// Sensor value used by this invocation; freshness requires a dated acquisition.
    pub temperature: i16,
    /// Age from sensor acquisition start at RFPLL entry, including sensor waits.
    pub sample_age_micros: Option<u64>,
    pub reference_before: i16,
    pub reference_after: i16,
    pub threshold: u8,
    pub channel: u16,
    /// None means skipped by the temperature predicate, not a zero correction.
    pub correction: Option<RfpllCorrectionEvidence>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RfpllCorrectionEvidence {
    pub initial_cap: u16,
    /// Requested final helper value; not readback of the clamped capacitor.
    pub selected_cap: u16,
    pub accepted_samples: u8,
    pub entries_updated: u8,
    /// Present only after nonzero correction and channel-index restoration.
    pub restored_frequency_index: Option<u8>,
}

impl RfpllCorrectionEvidence {
    pub const fn delta(self) -> i16 {
        self.selected_cap.wrapping_sub(self.initial_cap) as i16
    }
}

impl RfpllEvidence {
    pub fn is_valid(self) -> bool {
        let due = (i32::from(self.temperature) - i32::from(self.reference_before)).unsigned_abs()
            >= u32::from(self.threshold);
        match self.correction {
            None => !due && self.reference_after == self.reference_before,
            Some(correction) => {
                due && self.reference_after == self.temperature
                    && correction.accepted_samples <= 20
                    && (correction.accepted_samples != 0 || correction.delta() == 0)
                    && if correction.delta() == 0 {
                        correction.entries_updated == 0
                            && correction.restored_frequency_index.is_none()
                    } else {
                        correction.entries_updated != 0
                            && correction.restored_frequency_index.is_some()
                    }
            }
        }
    }
}

#[cfg(test)]
mod tests;
