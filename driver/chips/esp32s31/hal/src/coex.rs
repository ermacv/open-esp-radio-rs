//! Closed HAL bridge for the radio-owned coexistence timer bank.
//!
//! The executor-neutral COEX core sees only [`CoexTimerHardware`]. Raw PAC
//! ownership stays inside this adapter and is never exposed through `Deref`.

use open_esp_radio_esp32s31_coex::{
    CoexClient, CoexError, CoexPti, CoexTimerHardware, CoexTimerIndex,
};
use open_esp_radio_esp32s31_pac::{CoexTimerRegister, RadioRegisters};

const fn pac_timer(index: CoexTimerIndex) -> CoexTimerRegister {
    match index {
        CoexTimerIndex::Timer0 => CoexTimerRegister::Timer0,
        CoexTimerIndex::Timer1 => CoexTimerRegister::Timer1,
        CoexTimerIndex::Timer2 => CoexTimerRegister::Timer2,
        CoexTimerIndex::Timer3 => CoexTimerRegister::Timer3,
        CoexTimerIndex::Timer4 => CoexTimerRegister::Timer4,
    }
}

pub struct CoexTimerHal<'registers> {
    registers: &'registers mut RadioRegisters,
}

impl<'registers> CoexTimerHal<'registers> {
    pub fn new(registers: &'registers mut RadioRegisters) -> Self {
        Self { registers }
    }
}

impl CoexTimerHardware for CoexTimerHal<'_> {
    fn configure_request(
        &mut self,
        index: CoexTimerIndex,
        client: CoexClient,
        pti: CoexPti,
    ) -> Result<(), CoexError> {
        self.registers
            .configure_coex_timer(pac_timer(index), client as u8, pti.value());
        Ok(())
    }

    fn set_primary_target(
        &mut self,
        index: CoexTimerIndex,
        tick_image: u32,
    ) -> Result<(), CoexError> {
        self.registers
            .set_coex_timer_primary_target(pac_timer(index), tick_image);
        Ok(())
    }

    fn set_secondary_target(
        &mut self,
        index: CoexTimerIndex,
        tick_image: u32,
    ) -> Result<(), CoexError> {
        self.registers
            .set_coex_timer_secondary_target(pac_timer(index), tick_image);
        Ok(())
    }

    fn enable(&mut self, index: CoexTimerIndex) -> Result<(), CoexError> {
        self.registers.enable_coex_timer(pac_timer(index));
        Ok(())
    }

    fn disable(&mut self, index: CoexTimerIndex) -> Result<(), CoexError> {
        self.registers.disable_coex_timer(pac_timer(index));
        Ok(())
    }

    fn force(&mut self, index: CoexTimerIndex) -> Result<(), CoexError> {
        self.registers.force_coex_timer(pac_timer(index));
        Ok(())
    }

    fn unforce(&mut self, index: CoexTimerIndex) -> Result<(), CoexError> {
        self.registers.unforce_coex_timer(pac_timer(index));
        Ok(())
    }
}
