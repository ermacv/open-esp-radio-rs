//! Transactional Wi-Fi PHY-I2C temperature tracking for ESP32-S31.
//!
//! The complete pinned `phy_tx_i2c_track` body partitions the signed current
//! temperature into four ranges. A range change performs two low-nibble
//! masked writes and only then commits the retained range. The linked vendor
//! body is 330 bytes at `0x1000_0140`; no vendor table or ABI image is needed
//! by this source-owned replacement.

use crate::phy_i2c::{
    MaskedI2cWriteAction, MaskedI2cWriteBinding, MaskedI2cWriteBindingError,
    MaskedI2cWriteCompletion, MaskedI2cWriteTransition, MaskedI2cWriteTransitionError,
    analog_registers,
};

/// Semantic replacement for the vendor range byte at `phy_param + 0x4d`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyWifiI2cTrackingBand {
    Nominal,
    Cold,
    Elevated,
    Hot,
}

impl PhyWifiI2cTrackingBand {
    /// Exact signed-temperature partition recovered from the complete body.
    pub const fn for_temperature(temperature: i16) -> Self {
        if temperature < -19 {
            Self::Cold
        } else if temperature <= 54 {
            Self::Nominal
        } else if temperature <= 94 {
            Self::Elevated
        } else {
            Self::Hot
        }
    }

    const fn values(self) -> (u8, u8) {
        match self {
            Self::Nominal => (10, 13),
            Self::Cold => (10, 15),
            Self::Elevated => (8, 15),
            Self::Hot => (6, 15),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyWifiI2cTrackingParameters {
    pub current_temperature: i16,
    pub previous_band: PhyWifiI2cTrackingBand,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyWifiI2cTrackingOutcome {
    pub band: PhyWifiI2cTrackingBand,
    pub changed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyWifiI2cTrackingAction {
    MaskedWrite(MaskedI2cWriteAction),
    Complete(PhyWifiI2cTrackingOutcome),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyWifiI2cTrackingCompletion {
    MaskedWrite(MaskedI2cWriteCompletion),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyWifiI2cTrackingTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

/// Unique finite owner of both masked writes for one selected range.
#[must_use = "the Wi-Fi tracking transition owns an in-flight PHY-I2C update"]
#[derive(Debug, Eq, PartialEq)]
pub struct PhyWifiI2cTrackingTransition {
    target_band: PhyWifiI2cTrackingBand,
    changed: bool,
    write_index: u8,
    write: Option<MaskedI2cWriteTransition>,
}

impl PhyWifiI2cTrackingTransition {
    pub fn new(parameters: PhyWifiI2cTrackingParameters) -> Self {
        let target_band = PhyWifiI2cTrackingBand::for_temperature(parameters.current_temperature);
        let changed = target_band != parameters.previous_band;
        Self {
            target_band,
            changed,
            write_index: 0,
            write: changed.then(|| Self::write(target_band, 0)),
        }
    }

    fn write(band: PhyWifiI2cTrackingBand, index: u8) -> MaskedI2cWriteTransition {
        let (first, second) = band.values();
        let (field, value) = if index == 0 {
            (analog_registers::WIFI_TX_TEMPERATURE_TRACKING_0, first)
        } else {
            (analog_registers::WIFI_TX_TEMPERATURE_TRACKING_1, second)
        };
        MaskedI2cWriteTransition::new(field, value)
    }

    pub const fn action(&self) -> PhyWifiI2cTrackingAction {
        match self.write {
            Some(write) => PhyWifiI2cTrackingAction::MaskedWrite(write.action()),
            None => PhyWifiI2cTrackingAction::Complete(PhyWifiI2cTrackingOutcome {
                band: self.target_band,
                changed: self.changed,
            }),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyWifiI2cTrackingCompletion,
    ) -> Result<(), PhyWifiI2cTrackingTransitionError> {
        let Some(mut write) = self.write else {
            return Err(PhyWifiI2cTrackingTransitionError::AlreadyComplete);
        };
        let PhyWifiI2cTrackingCompletion::MaskedWrite(completion) = completion;
        write.advance(completion).map_err(|error| match error {
            MaskedI2cWriteTransitionError::WrongCompletion => {
                PhyWifiI2cTrackingTransitionError::WrongCompletion
            }
            MaskedI2cWriteTransitionError::AlreadyComplete => {
                PhyWifiI2cTrackingTransitionError::AlreadyComplete
            }
        })?;

        if write.action() == MaskedI2cWriteAction::Complete {
            self.write_index += 1;
            self.write = (self.write_index < 2).then(|| Self::write(self.target_band, 1));
        } else {
            self.write = Some(write);
        }
        Ok(())
    }

    /// Lower one explicit masked read/write edge to the existing target
    /// PHY-I2C transaction. Terminal outcomes cannot be lowered.
    pub fn lower_external(&self) -> Result<MaskedI2cWriteBinding, MaskedI2cWriteBindingError> {
        match self.action() {
            PhyWifiI2cTrackingAction::MaskedWrite(action) => MaskedI2cWriteBinding::new(action),
            PhyWifiI2cTrackingAction::Complete(_) => {
                Err(MaskedI2cWriteBindingError::UnsupportedAction)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phy_i2c::PhyI2cAddress;

    #[test]
    fn temperature_partition_matches_all_reviewed_boundaries() {
        assert_eq!(
            PhyWifiI2cTrackingBand::for_temperature(-20),
            PhyWifiI2cTrackingBand::Cold
        );
        assert_eq!(
            PhyWifiI2cTrackingBand::for_temperature(-19),
            PhyWifiI2cTrackingBand::Nominal
        );
        assert_eq!(
            PhyWifiI2cTrackingBand::for_temperature(54),
            PhyWifiI2cTrackingBand::Nominal
        );
        assert_eq!(
            PhyWifiI2cTrackingBand::for_temperature(55),
            PhyWifiI2cTrackingBand::Elevated
        );
        assert_eq!(
            PhyWifiI2cTrackingBand::for_temperature(94),
            PhyWifiI2cTrackingBand::Elevated
        );
        assert_eq!(
            PhyWifiI2cTrackingBand::for_temperature(95),
            PhyWifiI2cTrackingBand::Hot
        );
    }

    #[test]
    fn unchanged_band_completes_without_touching_i2c() {
        let transition = PhyWifiI2cTrackingTransition::new(PhyWifiI2cTrackingParameters {
            current_temperature: 25,
            previous_band: PhyWifiI2cTrackingBand::Nominal,
        });
        assert_eq!(
            transition.action(),
            PhyWifiI2cTrackingAction::Complete(PhyWifiI2cTrackingOutcome {
                band: PhyWifiI2cTrackingBand::Nominal,
                changed: false,
            })
        );
    }

    #[test]
    fn hot_transition_owns_both_masked_writes_before_commit() {
        let mut transition = PhyWifiI2cTrackingTransition::new(PhyWifiI2cTrackingParameters {
            current_temperature: 95,
            previous_band: PhyWifiI2cTrackingBand::Nominal,
        });
        complete_write(
            &mut transition,
            PhyI2cAddress::new(0x6b, 3).unwrap(),
            0xa0,
            0xa6,
        );
        complete_write(
            &mut transition,
            PhyI2cAddress::new(0x6b, 7).unwrap(),
            0x40,
            0x4f,
        );
        assert_eq!(
            transition.action(),
            PhyWifiI2cTrackingAction::Complete(PhyWifiI2cTrackingOutcome {
                band: PhyWifiI2cTrackingBand::Hot,
                changed: true,
            })
        );
    }

    fn complete_write(
        transition: &mut PhyWifiI2cTrackingTransition,
        address: PhyI2cAddress,
        read_value: u8,
        written_value: u8,
    ) {
        assert_eq!(
            transition.action(),
            PhyWifiI2cTrackingAction::MaskedWrite(MaskedI2cWriteAction::ReadByte { address })
        );
        transition
            .advance(PhyWifiI2cTrackingCompletion::MaskedWrite(
                MaskedI2cWriteCompletion::I2cReadCompleted {
                    address,
                    value: read_value,
                },
            ))
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyWifiI2cTrackingAction::MaskedWrite(MaskedI2cWriteAction::WriteByte {
                address,
                value: written_value,
            })
        );
        transition
            .advance(PhyWifiI2cTrackingCompletion::MaskedWrite(
                MaskedI2cWriteCompletion::I2cWriteCompleted { address },
            ))
            .unwrap();
    }
}
