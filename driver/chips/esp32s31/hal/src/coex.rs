//! Closed HAL bridge for the radio-owned coexistence timer bank.
//!
//! The executor-neutral COEX core sees only [`CoexTimerHardware`]. Raw PAC
//! ownership stays inside this adapter and is never exposed through `Deref`.

use open_esp_radio_esp32s31_coex::{
    CoexClient, CoexError, CoexPti, CoexTimerHardware, CoexTimerIndex,
};
use open_esp_radio_esp32s31_pac::{
    CoexTimerClientValue, CoexTimerPtiValue, CoexTimerRegister, CoexTimerTickImage, RadioRegisters,
};

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

/// Publish the complete reviewed Bluetooth coexistence PTI image.
///
/// This is intentionally a parameter-free operation: the only two writable
/// halfword images proved by `libbtbb::coex_pti_v2` remain private to the
/// closed PAC. The HAL exposes no raw address, register image or integer
/// field through which an unreviewed value could be written.
pub fn configure_bluetooth_pti(registers: &mut RadioRegisters) {
    registers.configure_reviewed_bluetooth_pti();
}

impl CoexTimerHardware for CoexTimerHal<'_> {
    fn configure_request(
        &mut self,
        index: CoexTimerIndex,
        client: CoexClient,
        pti: CoexPti,
    ) -> Result<(), CoexError> {
        let client = CoexTimerClientValue::new(client as u32).ok_or(CoexError::Hardware)?;
        let pti = CoexTimerPtiValue::new(u32::from(pti.value())).ok_or(CoexError::Hardware)?;
        self.registers
            .configure_coex_timer(pac_timer(index), client, pti);
        Ok(())
    }

    fn set_primary_target(
        &mut self,
        index: CoexTimerIndex,
        tick_image: u32,
    ) -> Result<(), CoexError> {
        // Complete `coex_hw_timer_set` replaces only the low 24 bits of the
        // converted image. Normalize at this reviewed HAL boundary so the
        // closed PAC still receives a representable register-field value.
        let tick_image = CoexTimerTickImage::new(tick_image & CoexTimerTickImage::MAX)
            .ok_or(CoexError::Hardware)?;
        self.registers
            .set_coex_timer_primary_target(pac_timer(index), tick_image);
        Ok(())
    }

    fn set_secondary_target(
        &mut self,
        index: CoexTimerIndex,
        tick_image: u32,
    ) -> Result<(), CoexError> {
        let tick_image = CoexTimerTickImage::new(tick_image & CoexTimerTickImage::MAX)
            .ok_or(CoexError::Hardware)?;
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
