//! Read-only thermal demand, not permission to access RF hardware.
//!
//! A decision describes the supplied sample only. Channel restoration can
//! replace that sample, so the executor reevaluates TX demand after common
//! calibration. Temperatures and thresholds use PHY sensor units, not degrees C.

use super::{PhyCalibrationTrackingParameters, PhyCalibrationTrackingRequest};
use crate::tracking::parameters::PhyCalibrationTrackClass;

const DEFAULT_THRESHOLD: u8 = 30;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ThermalDemand {
    pub current: i16,
    pub reference: i16,
    pub threshold: u8,
}

impl ThermalDemand {
    pub const fn delta(self) -> u32 {
        (self.current as i32 - self.reference as i32).unsigned_abs()
    }

    /// Equality is due; a zero diagnostic threshold forces a branch even
    /// when the retained temperature has not changed.
    pub const fn is_due(self) -> bool {
        self.delta() >= self.threshold as u32
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Decision {
    pub class: PhyCalibrationTrackClass,
    pub common: ThermalDemand,
    pub transmit: ThermalDemand,
}

impl PhyCalibrationTrackingParameters {
    /// Observe the common and selected TX reference without committing either.
    /// This value carries neither sample freshness nor an RF grant.
    pub const fn decision(self, request: PhyCalibrationTrackingRequest) -> Decision {
        let threshold = match self.threshold_override {
            Some(value) => value,
            None => DEFAULT_THRESHOLD,
        };
        Decision {
            class: request.class,
            common: ThermalDemand {
                current: self.current_temperature,
                reference: self.common_reference_temperature,
                threshold,
            },
            transmit: ThermalDemand {
                current: self.current_temperature,
                reference: match request.class {
                    PhyCalibrationTrackClass::Wifi => self.wifi_reference_temperature,
                    PhyCalibrationTrackClass::BluetoothIeee802154 => {
                        self.bluetooth_ieee802154_reference_temperature
                    }
                },
                threshold,
            },
        }
    }
}

#[cfg(test)]
mod tests;
