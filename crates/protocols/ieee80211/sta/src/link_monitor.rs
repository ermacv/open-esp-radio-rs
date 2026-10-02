//! Executor- and chip-independent connected-station beacon-loss policy.
//!
//! This owner performs no MMIO and does not put the modem to sleep. RX supplies
//! typed beacon observations, while the runtime supplies monotonic time and
//! owns the resulting disconnect edge. Power-save policy can consume the same
//! TIM observation without importing vendor PM contexts or RTOS timers.

use oer_ieee80211_mac::station_beacon::{StaBeaconObservation, StaTimObservation};
use oer_time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaBeaconLossConfigError {
    ZeroInterval,
    ZeroMissLimit,
    ZeroTimeout,
    DeadlineOverflow,
}

/// Association-derived beacon-loss policy.
///
/// The connected station declares the link unreachable, and starts probing
/// the access point, when no beacon has arrived for `timeout`, as the
/// vendor's station beacon timeout does. `miss_limit` is the separate
/// consecutive-miss count the MAC's hardware beacon monitor uses while the
/// modem sleeps.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaBeaconLossConfig {
    interval_tu: u16,
    miss_limit: u8,
    window: Duration,
}

impl StaBeaconLossConfig {
    pub const fn new(
        interval_tu: u16,
        miss_limit: u8,
        timeout: Duration,
    ) -> Result<Self, StaBeaconLossConfigError> {
        if interval_tu == 0 {
            return Err(StaBeaconLossConfigError::ZeroInterval);
        }
        if miss_limit == 0 {
            return Err(StaBeaconLossConfigError::ZeroMissLimit);
        }
        if timeout.as_micros() == 0 {
            return Err(StaBeaconLossConfigError::ZeroTimeout);
        }
        Ok(Self {
            interval_tu,
            miss_limit,
            window: timeout,
        })
    }

    pub const fn interval_tu(self) -> u16 {
        self.interval_tu
    }

    pub const fn miss_limit(self) -> u8 {
        self.miss_limit
    }

    /// How long the link may go without a beacon before it is probed.
    pub const fn window(self) -> Duration {
        self.window
    }
}

/// Finite beacon/TIM state owned by the connected executor task.
// CAPABILITY: wifi-bounded-wait-owners
pub struct StaBeaconMonitor {
    config: StaBeaconLossConfig,
    deadline: Option<Instant>,
    last_observation: Option<StaBeaconObservation>,
    observed: u32,
}

impl StaBeaconMonitor {
    pub const fn new(config: StaBeaconLossConfig) -> Self {
        Self {
            config,
            deadline: None,
            last_observation: None,
            observed: 0,
        }
    }

    pub const fn config(&self) -> StaBeaconLossConfig {
        self.config
    }

    pub const fn deadline(&self) -> Option<Instant> {
        self.deadline
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
    pub fn arm(&mut self, now: Instant) -> Result<(), StaBeaconLossConfigError> {
        if self.deadline.is_none() {
            self.deadline = Some(
                now.checked_add(self.config.window)
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
        now: Instant,
        observation: StaBeaconObservation,
    ) -> Result<(), StaBeaconLossConfigError> {
        self.deadline = Some(
            now.checked_add(self.config.window)
                .ok_or(StaBeaconLossConfigError::DeadlineOverflow)?,
        );
        self.last_observation = Some(observation);
        self.observed = self.observed.saturating_add(1);
        Ok(())
    }

    /// Refresh link reachability from a response already validated as coming
    /// from the associated BSSID.
    ///
    /// This deliberately does not fabricate a beacon observation or TIM:
    /// active-probe reachability and passive beacon state remain distinct.
    pub fn observe_reachability(&mut self, now: Instant) -> Result<(), StaBeaconLossConfigError> {
        self.deadline = Some(
            now.checked_add(self.config.window)
                .ok_or(StaBeaconLossConfigError::DeadlineOverflow)?,
        );
        Ok(())
    }

    /// Replace the passive beacon deadline with one bounded active-probe
    /// response deadline selected by the chip control policy.
    pub fn wait_for_reachability(
        &mut self,
        now: Instant,
        timeout: Duration,
    ) -> Result<(), StaBeaconLossConfigError> {
        self.deadline = Some(
            now.checked_add(timeout)
                .ok_or(StaBeaconLossConfigError::DeadlineOverflow)?,
        );
        Ok(())
    }

    pub const fn expired(&self, now: Instant) -> bool {
        matches!(self.deadline, Some(deadline) if now.as_micros() >= deadline.as_micros())
    }
}

#[cfg(test)]
mod tests;
