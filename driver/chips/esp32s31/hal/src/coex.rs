//! Closed HAL bridge for the radio-owned coexistence timer bank.
//!
//! The executor-neutral COEX core sees only [`CoexTimerHardware`]. Raw PAC
//! ownership stays inside this adapter and is never exposed through `Deref`.

#![cfg(feature = "validation-probes")]

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

struct CoexTimerHal<'registers> {
    registers: &'registers mut RadioRegisters,
}

impl<'registers> CoexTimerHal<'registers> {
    fn new(registers: &'registers mut RadioRegisters) -> Self {
        Self { registers }
    }
}

/// Publish the complete reviewed Bluetooth coexistence PTI image.
///
/// This is intentionally a parameter-free operation: the only two writable
/// halfword images proved by `libbtbb::coex_pti_v2` remain private to the
/// closed PAC. The HAL exposes no raw address, register image or integer
/// field through which an unreviewed value could be written.
fn configure_bluetooth_pti(registers: &mut RadioRegisters) {
    registers.configure_reviewed_bluetooth_pti();
}

/// Run the reviewed Bluetooth PTI transaction in an isolated validation
/// image without exposing the PAC owner to the probe crate.
#[cfg(feature = "validation-probes")]
#[doc(hidden)]
pub fn validation_configure_bluetooth_pti() {
    let mut registers = open_esp_radio_esp32s31_pac::validation::radio_registers();
    configure_bluetooth_pti(&mut registers);
}

#[doc(hidden)]
pub fn validation_enable_timer(index: u32) {
    let Some(timer) = CoexTimerRegister::new(index as u8) else {
        return;
    };
    let mut registers = open_esp_radio_esp32s31_pac::validation::radio_registers();
    registers.enable_coex_timer(timer);
}

#[doc(hidden)]
pub fn validation_disable_timer(index: u32) {
    let Some(timer) = CoexTimerRegister::new(index as u8) else {
        return;
    };
    let mut registers = open_esp_radio_esp32s31_pac::validation::radio_registers();
    registers.disable_coex_timer(timer);
}

#[doc(hidden)]
pub fn validation_force_timer(index: u32) {
    let Some(timer) = CoexTimerRegister::new(index as u8) else {
        return;
    };
    let mut registers = open_esp_radio_esp32s31_pac::validation::radio_registers();
    registers.force_coex_timer(timer);
}

#[doc(hidden)]
pub fn validation_unforce_timer(index: u32) {
    let Some(timer) = CoexTimerRegister::new(index as u8) else {
        return;
    };
    let mut registers = open_esp_radio_esp32s31_pac::validation::radio_registers();
    registers.unforce_coex_timer(timer);
}

/// Execute one complete COEX timer program against an isolated validation
/// owner. The caller supplies only the clock environment and typed values.
#[cfg(feature = "validation-probes")]
#[doc(hidden)]
pub fn validation_program_timer<C: open_esp_radio_esp32s31_coex::CoexClockHardware>(
    clock: &mut C,
    index: CoexTimerIndex,
    client: CoexClient,
    pti: CoexPti,
    latency: u32,
    duration: u32,
) -> Result<(), CoexError> {
    let mut registers = open_esp_radio_esp32s31_pac::validation::radio_registers();
    let mut timer = CoexTimerHal::new(&mut registers);
    open_esp_radio_esp32s31_coex::program_timer(
        &mut timer,
        &mut *clock,
        index,
        client,
        pti,
        latency,
        duration,
    )
}

/// Execute one enabled COEX-core request against an isolated validation
/// owner. The boolean selects Bluetooth (`false`) or Wi-Fi (`true`).
#[cfg(feature = "validation-probes")]
#[doc(hidden)]
pub fn validation_core_request<C: open_esp_radio_esp32s31_coex::CoexClockHardware>(
    clock: &mut C,
    wifi: bool,
    request: open_esp_radio_esp32s31_coex::CoexClientRequest,
) -> Result<(), CoexError> {
    use open_esp_radio_esp32s31_coex::{CoexCore, CoexPtiTable};

    let mut core = CoexCore::new(CoexPtiTable::reviewed_vendor());
    core.enable();
    let mut registers = open_esp_radio_esp32s31_pac::validation::radio_registers();
    let mut timer = CoexTimerHal::new(&mut registers);
    if wifi {
        core.request_wifi(&mut timer, clock, request).map(|_| ())
    } else {
        core.request_bluetooth(&mut timer, clock, request)
            .map(|_| ())
    }
}

/// Execute one COEX-core release against an isolated validation owner.
#[cfg(feature = "validation-probes")]
#[doc(hidden)]
pub fn validation_core_release(
    event: open_esp_radio_esp32s31_coex::CoexEventId,
) -> Result<(), CoexError> {
    use open_esp_radio_esp32s31_coex::{CoexCore, CoexPtiTable};

    let mut core = CoexCore::new(CoexPtiTable::reviewed_vendor());
    let mut registers = open_esp_radio_esp32s31_pac::validation::radio_registers();
    let mut timer = CoexTimerHal::new(&mut registers);
    core.release(&mut timer, event).map(|_| ())
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
