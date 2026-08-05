//! Executor- and chip-independent connected-station beacon-loss policy.
//!
//! This owner performs no MMIO and does not put the modem to sleep. RX supplies
//! typed beacon observations, while the runtime supplies monotonic time and
//! owns the resulting disconnect edge. Power-save policy can consume the same
//! TIM observation without importing vendor PM contexts or RTOS timers.

use open_esp_radio_ieee80211::station_beacon::{StaBeaconObservation, StaTimObservation};

const TU_MICROS: u64 = 1_024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaBeaconLossConfigError {
    ZeroInterval,
    ZeroMissLimit,
    DeadlineOverflow,
}

/// Association-derived limit for consecutive missing infrastructure beacons.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaBeaconLossConfig {
    interval_tu: u16,
    miss_limit: u8,
    window_micros: u64,
}

impl StaBeaconLossConfig {
    pub const fn new(interval_tu: u16, miss_limit: u8) -> Result<Self, StaBeaconLossConfigError> {
        if interval_tu == 0 {
            return Err(StaBeaconLossConfigError::ZeroInterval);
        }
        if miss_limit == 0 {
            return Err(StaBeaconLossConfigError::ZeroMissLimit);
        }
        // u16 * u8 * 1024 is bounded far below u64::MAX.
        let window_micros = interval_tu as u64 * miss_limit as u64 * TU_MICROS;
        Ok(Self {
            interval_tu,
            miss_limit,
            window_micros,
        })
    }

    pub const fn interval_tu(self) -> u16 {
        self.interval_tu
    }

    pub const fn miss_limit(self) -> u8 {
        self.miss_limit
    }

    pub const fn window_micros(self) -> u64 {
        self.window_micros
    }
}

/// Finite beacon/TIM state owned by the connected executor task.
pub struct StaBeaconMonitor {
    config: StaBeaconLossConfig,
    deadline_micros: Option<u64>,
    last_observation: Option<StaBeaconObservation>,
    observed: u32,
}

impl StaBeaconMonitor {
    pub const fn new(config: StaBeaconLossConfig) -> Self {
        Self {
            config,
            deadline_micros: None,
            last_observation: None,
            observed: 0,
        }
    }

    pub const fn config(&self) -> StaBeaconLossConfig {
        self.config
    }

    pub const fn deadline_micros(&self) -> Option<u64> {
        self.deadline_micros
    }

    pub const fn last_observation(&self) -> Option<StaBeaconObservation> {
        self.last_observation
    }

    pub const fn last_tim(&self) -> Option<StaTimObservation> {
        match self.last_observation {
            Some(observation) => observation.tim,
            None => None,
        }
    }

    pub const fn observed(&self) -> u32 {
        self.observed
    }

    /// Arm from the association-complete edge before the first beacon arrives.
    pub fn arm(&mut self, now_micros: u64) -> Result<(), StaBeaconLossConfigError> {
        if self.deadline_micros.is_none() {
            self.deadline_micros = Some(
                now_micros
                    .checked_add(self.config.window_micros)
                    .ok_or(StaBeaconLossConfigError::DeadlineOverflow)?,
            );
        }
        Ok(())
    }

    /// Refresh the absolute deadline from a beacon already authenticated by
    /// BSSID/address classification. The association's interval remains the
    /// policy source; an unprotected beacon cannot silently stretch it.
    pub fn observe(
        &mut self,
        now_micros: u64,
        observation: StaBeaconObservation,
    ) -> Result<(), StaBeaconLossConfigError> {
        self.deadline_micros = Some(
            now_micros
                .checked_add(self.config.window_micros)
                .ok_or(StaBeaconLossConfigError::DeadlineOverflow)?,
        );
        self.last_observation = Some(observation);
        self.observed = self.observed.saturating_add(1);
        Ok(())
    }

    pub const fn expired(&self, now_micros: u64) -> bool {
        matches!(self.deadline_micros, Some(deadline) if now_micros >= deadline)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BEACON: StaBeaconObservation = StaBeaconObservation {
        timestamp_tsf: 10,
        interval_tu: 100,
        capability_information: 0,
        tim: None,
    };

    #[test]
    fn exact_deadline_is_lost_but_an_observation_refreshes_the_window() {
        let config = StaBeaconLossConfig::new(100, 3).unwrap();
        let mut monitor = StaBeaconMonitor::new(config);
        monitor.arm(1_000).unwrap();
        assert_eq!(monitor.deadline_micros(), Some(308_200));
        assert!(!monitor.expired(308_199));

        monitor.observe(308_200, BEACON).unwrap();
        assert!(!monitor.expired(308_200));
        assert_eq!(monitor.deadline_micros(), Some(615_400));
        assert_eq!(monitor.observed(), 1);
        assert_eq!(monitor.last_observation(), Some(BEACON));
        assert!(monitor.expired(615_400));
    }

    #[test]
    fn construction_rejects_unbounded_or_vacuous_policy() {
        assert_eq!(
            StaBeaconLossConfig::new(0, 3),
            Err(StaBeaconLossConfigError::ZeroInterval)
        );
        assert_eq!(
            StaBeaconLossConfig::new(100, 0),
            Err(StaBeaconLossConfigError::ZeroMissLimit)
        );
    }
}
