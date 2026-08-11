//! Exact generated-PAC transactions for the internal coexistence timer bank.

#![forbid(unsafe_code)]

use super::RadioRegisters;

pub const COEX_TIMER_COUNT: u8 = 5;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoexTimerRegisterError {
    Index,
}

impl RadioRegisters {
    fn checked_coex_timer_index(index: u8) -> Result<usize, CoexTimerRegisterError> {
        if index < COEX_TIMER_COUNT {
            Ok(usize::from(index))
        } else {
            Err(CoexTimerRegisterError::Index)
        }
    }

    /// Enable one timer in the exact disable-clear/enable-set order.
    pub fn enable_coex_timer(&mut self, index: u8) -> Result<(), CoexTimerRegisterError> {
        let index = Self::checked_coex_timer_index(index)?;
        let timers = &self.peripherals.coex_hw_timer;
        timers
            .disable_control(index)
            .modify(|_, writer| writer.disable().clear_bit());
        timers
            .enable_control(index)
            .modify(|_, writer| writer.enable().set_bit());
        Ok(())
    }

    /// Disable one timer in the exact enable-clear/disable-set order.
    pub fn disable_coex_timer(&mut self, index: u8) -> Result<(), CoexTimerRegisterError> {
        let index = Self::checked_coex_timer_index(index)?;
        let timers = &self.peripherals.coex_hw_timer;
        timers
            .enable_control(index)
            .modify(|_, writer| writer.enable().clear_bit());
        timers
            .disable_control(index)
            .modify(|_, writer| writer.disable().set_bit());
        Ok(())
    }

    /// Force one timer by clearing only its low 24-bit tick image.
    pub fn force_coex_timer(&mut self, index: u8) -> Result<(), CoexTimerRegisterError> {
        let index = Self::checked_coex_timer_index(index)?;
        self.peripherals
            .coex_hw_timer
            .configuration(index)
            .modify(|_, writer| writer.primary_tick_image().set(0));
        Ok(())
    }

    /// Remove the force condition using the vendor's exact value of 1000.
    pub fn unforce_coex_timer(&mut self, index: u8) -> Result<(), CoexTimerRegisterError> {
        let index = Self::checked_coex_timer_index(index)?;
        self.peripherals
            .coex_hw_timer
            .configuration(index)
            .modify(|_, writer| writer.primary_tick_image().set(1_000));
        Ok(())
    }

    /// Program the first two fresh-read RMW edges of `coex_hw_timer_set`.
    ///
    /// Tick conversion deliberately does not happen in this method. The
    /// vendor samples the platform clock after these two writes and again
    /// between the primary and secondary target writes.
    pub fn configure_coex_timer(
        &mut self,
        index: u8,
        parameter_1: u8,
        parameter_2: u8,
    ) -> Result<(), CoexTimerRegisterError> {
        let index = Self::checked_coex_timer_index(index)?;
        let timers = &self.peripherals.coex_hw_timer;
        let configuration = timers.configuration(index);
        configuration.modify(|_, writer| writer.parameter_1().set(parameter_1 & 0x03));
        configuration.modify(|_, writer| writer.parameter_2().set(parameter_2 & 0x0f));
        Ok(())
    }

    /// Publish the converted primary target in the third fresh-read RMW edge
    /// of `coex_hw_timer_set`.
    pub fn set_coex_timer_primary_target(
        &mut self,
        index: u8,
        primary_tick_image: u32,
    ) -> Result<(), CoexTimerRegisterError> {
        let index = Self::checked_coex_timer_index(index)?;
        self.peripherals
            .coex_hw_timer
            .configuration(index)
            .modify(|_, writer| {
                writer
                    .primary_tick_image()
                    .set(primary_tick_image & 0x00ff_ffff)
            });
        Ok(())
    }

    /// Publish the converted secondary target in the final fresh-read RMW
    /// edge of `coex_hw_timer_set`.
    pub fn set_coex_timer_secondary_target(
        &mut self,
        index: u8,
        secondary_tick_image: u32,
    ) -> Result<(), CoexTimerRegisterError> {
        let index = Self::checked_coex_timer_index(index)?;
        self.peripherals
            .coex_hw_timer
            .secondary_target(index)
            .modify(|_, writer| {
                writer
                    .secondary_tick_image()
                    .set(secondary_tick_image & 0x00ff_ffff)
            });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timer_index_domain_includes_the_fifth_timer() {
        for index in 0..COEX_TIMER_COUNT {
            assert_eq!(
                RadioRegisters::checked_coex_timer_index(index),
                Ok(usize::from(index))
            );
        }
        assert_eq!(
            RadioRegisters::checked_coex_timer_index(COEX_TIMER_COUNT),
            Err(CoexTimerRegisterError::Index)
        );
    }
}
