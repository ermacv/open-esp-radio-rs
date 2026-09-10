//! Inclusive PHY operation timing; no raw register or vendor ABI values.
use serde::{Deserialize, Serialize};
mod rfpll;
pub use rfpll::{RfpllCorrectionEvidence, RfpllEvidence, STATION_RFPLL_SAMPLE_MAX_AGE_MICROS};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct PhyOperationTiming {
    pub started: u16,
    pub completed: u16,
    pub failed: u16,
    pub elapsed_micros: u32,
    pub maximum_micros: u32,
}

/// Time inside polls of a selected expensive child. Includes interrupts and
/// observation overhead; the remainder of its operation time is outside polls,
/// not necessarily requested timer delay or time with the CPU idle.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct PhyPollTiming {
    pub pending: u32,
    /// Pending return to next poll entry, not timer readiness or CPU idle.
    pub suspended_micros: u32,
    pub maximum_suspension_micros: u32,
    pub polls: u32,
    pub elapsed_micros: u32,
    pub maximum_micros: u32,
}

impl PhyPollTiming {
    fn fits(self, operation: PhyOperationTiming) -> bool {
        self.polls.checked_sub(self.pending)
            == Some(u32::from(operation.completed) + u32::from(operation.failed))
            && self
                .elapsed_micros
                .checked_add(self.suspended_micros)
                .is_some_and(|total| total <= operation.elapsed_micros)
            && self.maximum_suspension_micros <= self.suspended_micros
            && self.maximum_suspension_micros <= operation.maximum_micros
            && (self.pending != 0 || self.suspended_micros == 0)
            && self.maximum_micros <= self.elapsed_micros
            && self.maximum_micros <= operation.maximum_micros
            && (operation.started != 0 || self == Self::default())
    }
}

/// Parent intervals include their children; do not sum them as CPU/RF time.
/// Counts distinguish selected calls from work absent from the invocation.
/// Calibration branch flags remain the authority for completed calibration.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct PhyTimingEvidence {
    pub dcode_waits: PhyDcodeWaitEvidence,
    pub invalid: bool,
    pub rfpll: PhyOperationTiming,
    pub wifi_power: PhyOperationTiming,
    pub bluetooth_ieee802154_power: PhyOperationTiming,
    pub wifi_i2c: PhyOperationTiming,
    pub calibration: PhyOperationTiming,
    pub temperature: PhyOperationTiming,
    pub pbus_clear: PhyOperationTiming,
    pub dcode: PhyOperationTiming,
    pub rx_gain: PhyOperationTiming,
    pub channel_restore: PhyOperationTiming,
    pub force_tx_rx: PhyOperationTiming,
    pub tx_dc_pwdet: PhyOperationTiming,
    pub tx_gain_publication: PhyOperationTiming,
    pub frequency_settle: PhyOperationTiming,
    pub dcode_polls: PhyPollTiming,
    pub rx_gain_polls: PhyPollTiming,
    pub tx_dc_pwdet_polls: PhyPollTiming,
}

impl PhyTimingEvidence {
    /// All observed work terminated successfully and timing remained valid.
    pub fn is_complete(&self) -> bool {
        !self.invalid
            && self.dcode_waits.fits(self.dcode)
            && self.dcode_polls.fits(self.dcode)
            && self.rx_gain_polls.fits(self.rx_gain)
            && self.tx_dc_pwdet_polls.fits(self.tx_dc_pwdet)
            && [
                self.rfpll,
                self.wifi_power,
                self.bluetooth_ieee802154_power,
                self.wifi_i2c,
                self.calibration,
                self.temperature,
                self.pbus_clear,
                self.dcode,
                self.rx_gain,
                self.channel_restore,
                self.force_tx_rx,
                self.tx_dc_pwdet,
                self.tx_gain_publication,
                self.frequency_settle,
            ]
            .iter()
            .all(|entry| entry.failed == 0 && entry.started == entry.completed)
    }
}

#[cfg(test)]
mod tests;

/// Sequential timer intervals, observed against the timer's own deadline.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct PhyWaitTiming {
    pub count: u16,
    pub requested_micros: u32,
    pub elapsed_micros: u32,
    pub maximum_lateness_micros: u32,
}
impl PhyWaitTiming {
    fn is_valid(self) -> bool {
        self.requested_micros <= self.elapsed_micros
            && self.maximum_lateness_micros <= self.elapsed_micros - self.requested_micros
            && (self.count != 0 || self == Self::default())
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct PhyBusWaitEvidence {
    /// Subset of timing.count; other waits precede completion reads.
    pub bus_busy: u16,
    pub timing: PhyWaitTiming,
}
impl PhyBusWaitEvidence {
    fn is_valid(self) -> bool {
        self.bus_busy <= self.timing.count && self.timing.is_valid()
    }
}

/// Direct Dcode and nested RFPLL waits; PLL lock counts reuse existing analog
/// status reads, not extra register reads or timestamps of physical readiness.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct PhyDcodeWaitEvidence {
    pub i2c: PhyBusWaitEvidence,
    pub rfpll_i2c: PhyBusWaitEvidence,
    pub rfpll_settle: PhyWaitTiming,
    pub pll_locked: u16,
    pub pll_unlocked: u16,
}
impl PhyDcodeWaitEvidence {
    fn fits(self, operation: PhyOperationTiming) -> bool {
        self.i2c.is_valid()
            && self.rfpll_i2c.is_valid()
            && self.rfpll_settle.is_valid()
            && self
                .i2c
                .timing
                .elapsed_micros
                .checked_add(self.rfpll_i2c.timing.elapsed_micros)
                .and_then(|total| total.checked_add(self.rfpll_settle.elapsed_micros))
                .is_some_and(|total| total <= operation.elapsed_micros)
            && (operation.started != 0 || self == Self::default())
    }
}

/// Detailed TX DC/PWDET waits emitted before the correlated pause completion.
/// Sequential intervals are inclusive of timer dispatch and resumption latency.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct PhyTxWaitEvidence {
    pub pbus: PhyBusWaitEvidence,
    pub search: PhyWaitTiming,
    pub tone: PhyWaitTiming,
    pub sar: PhyWaitTiming,
    pub root: PhyWaitTiming,
    pub sar_ready: u16,
    pub sar_not_ready: u16,
}
impl PhyTxWaitEvidence {
    pub fn fits(self, operation: PhyOperationTiming) -> bool {
        self.pbus.is_valid()
            && [self.search, self.tone, self.sar, self.root]
                .iter()
                .all(|v| v.is_valid())
            && [
                self.pbus.timing,
                self.search,
                self.tone,
                self.sar,
                self.root,
            ]
            .iter()
            .try_fold(0u32, |sum, v| sum.checked_add(v.elapsed_micros))
            .is_some_and(|total| total <= operation.elapsed_micros)
            && (operation.started != 0 || self == Self::default())
    }
}

/// Duration of the bounded automatic-service measurement within UDP traffic.
pub const STATION_TRACKING_SERVICE_WINDOW_MICROS: u64 = 4_000_000;

/// Separate detail frame, correlated before StationPauseCompleted.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct StationTrackingServiceEvidence {
    /// Temperature, Wi-Fi power, analog-I2C, common calibration, Wi-Fi TX, RFPLL.
    pub operations: [u16; 6],
    pub deferred: u16,
    pub common_calibrated: u16,
    pub wifi_calibrated: u16,
    pub elapsed_micros: u64,
    pub pause_micros: u64,
    pub maximum_pause_micros: u64,
    pub failed: bool,
    pub suspended: bool,
    pub invalid: bool,
}
impl StationTrackingServiceEvidence {
    pub fn is_valid(self) -> bool {
        !self.failed
            && !self.suspended
            && !self.invalid
            && self.maximum_pause_micros <= self.pause_micros
            && self.pause_micros <= self.elapsed_micros
            && self.common_calibrated <= self.operations[3]
            && self.wifi_calibrated <= self.operations[4]
    }
}
