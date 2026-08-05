//! Ordinary TX ownership retained across ESP32-S31 station protocol epochs.
//!
//! A station starts with one control TX owner, lends it to the connected
//! runner, and must recover the same descriptor resources before scan or join
//! can run again. This owner retains the construction policy while the
//! descriptor itself is absent, so platform/HIL code never reconstructs a
//! driver object from duplicated constants. Construction of a concrete async
//! TX implementation belongs to its runtime adapter.

use crate::tx::ControlTxConfig;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31StaTxEpochError {
    OwnerUnavailable,
    OwnerAlreadyPresent,
}

/// One station-wide ordinary descriptor owner and its immutable policy.
pub struct Esp32s31StaTxEpoch<C> {
    control: Option<C>,
    config: ControlTxConfig,
}

impl<C> Esp32s31StaTxEpoch<C> {
    pub const fn from_control(control: C, config: ControlTxConfig) -> Self {
        Self {
            control: Some(control),
            config,
        }
    }

    pub const fn config(&self) -> ControlTxConfig {
        self.config
    }

    pub fn control(&self) -> Result<&C, Esp32s31StaTxEpochError> {
        self.control
            .as_ref()
            .ok_or(Esp32s31StaTxEpochError::OwnerUnavailable)
    }

    pub fn control_mut(&mut self) -> Result<&mut C, Esp32s31StaTxEpochError> {
        self.control
            .as_mut()
            .ok_or(Esp32s31StaTxEpochError::OwnerUnavailable)
    }

    pub fn take_control(&mut self) -> Result<C, Esp32s31StaTxEpochError> {
        self.control
            .take()
            .ok_or(Esp32s31StaTxEpochError::OwnerUnavailable)
    }

    /// Restore a returned protocol owner without overwriting a live phase.
    pub fn restore_control(&mut self, control: C) -> Result<(), (Esp32s31StaTxEpochError, C)> {
        if self.control.is_some() {
            return Err((Esp32s31StaTxEpochError::OwnerAlreadyPresent, control));
        }
        self.control = Some(control);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_never_overwrites_or_duplicates_the_phase_owner() {
        let config = ControlTxConfig {
            unicast_attempt_limit: 4,
            completion_timeout_us: 250_000,
            poll_interval_us: 1,
        };
        let mut epoch = Esp32s31StaTxEpoch::from_control(7_u8, config);
        assert_eq!(epoch.control(), Ok(&7));
        assert_eq!(epoch.take_control(), Ok(7));
        assert_eq!(
            epoch.take_control(),
            Err(Esp32s31StaTxEpochError::OwnerUnavailable)
        );
        assert_eq!(epoch.restore_control(9), Ok(()));
        assert_eq!(
            epoch.restore_control(11),
            Err((Esp32s31StaTxEpochError::OwnerAlreadyPresent, 11))
        );
        assert_eq!(epoch.config(), config);
    }
}
