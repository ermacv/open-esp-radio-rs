//! Wi-Fi maintenance demand from dated observations and existing predicates.
//!
//! This policy selects one operation, never grants RF access. After completing
//! and restoring that operation the composition inspects again. The sample
//! cadence is not a guaranteed thermally safe deferral interval.

use super::{
    inspection::Inspection,
    maintenance::Operation,
    temperature::{Acquisition, Freshness},
};
use core::num::NonZeroU64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Config {
    sample_period_micros: NonZeroU64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Suspension {
    Inactive,
    Inhibited,
    SharedRadio,
    Clock,
    SampleOverrun,
    RfpllUnsupported,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Demand {
    Suspended(Suspension),
    At(u64),
    Run(Operation),
}

impl Config {
    pub const fn new(sample_period_micros: NonZeroU64) -> Self {
        Self {
            sample_period_micros,
        }
    }
    pub const fn sample_period_micros(self) -> u64 {
        self.sample_period_micros.get()
    }

    pub fn inspect(
        self,
        snapshot: Inspection,
        now_micros: u64,
        external_observation: bool,
    ) -> Demand {
        if snapshot.inhibited {
            return Demand::Suspended(Suspension::Inhibited);
        }
        if snapshot.bluetooth_ieee802154.is_some() {
            return Demand::Suspended(Suspension::SharedRadio);
        }
        let Some(wifi) = snapshot.wifi else {
            return Demand::Suspended(Suspension::Inactive);
        };
        let period = self.sample_period_micros.get();
        if matches!(snapshot.temperature.acquisition, Acquisition::Overlong) {
            return Demand::Suspended(Suspension::SampleOverrun);
        }
        if let Acquisition::Window { started, completed } = snapshot.temperature.acquisition {
            if completed < started || now_micros < completed {
                return Demand::Suspended(Suspension::Clock);
            }
            // An impossible requested cadence must not create an immediate
            // observe/pause/observe loop that permanently withholds traffic.
            if completed - started >= period {
                return Demand::Suspended(Suspension::SampleOverrun);
            }
        }
        match snapshot.temperature.freshness(now_micros, period) {
            Freshness::ClockReversed => return Demand::Suspended(Suspension::Clock),
            Freshness::Unknown | Freshness::Stale { .. } => {
                return Demand::Run(Operation::Temperature);
            }
            Freshness::Fresh { .. } => {}
        }
        if external_observation {
            return Demand::Run(Operation::Temperature);
        }
        if snapshot.rfpll.is_some_and(|parameters| parameters.is_due()) {
            return Demand::Suspended(Suspension::RfpllUnsupported);
        }
        if snapshot
            .wifi_i2c
            .is_some_and(|parameters| parameters.update_required())
        {
            return Demand::Run(Operation::WifiI2c);
        }
        if wifi.power.update_required {
            return Demand::Run(Operation::WifiPower);
        }
        if let Some(calibration) = wifi.calibration {
            if calibration.common.is_due() {
                return Demand::Run(Operation::CommonCalibration);
            }
            if calibration.transmit.is_due() {
                return Demand::Run(Operation::WifiTxCalibration);
            }
        }
        let Acquisition::Window { started, .. } = snapshot.temperature.acquisition else {
            unreachable!("fresh observation has acquisition time")
        };
        match started
            .checked_add(period)
            .and_then(|deadline| deadline.checked_add(1))
        {
            Some(deadline) => Demand::At(deadline),
            None => Demand::Suspended(Suspension::Clock),
        }
    }
}

#[cfg(test)]
mod tests;
