//! Closed HAL bridge for the radio-owned coexistence timer bank.
//!
//! The executor-neutral COEX core sees only [`CoexTimerHardware`]. Raw PAC
//! ownership stays inside this adapter and is never exposed through `Deref`.

#![cfg(feature = "validation-probes")]

use core::cell::RefCell;

use open_esp_radio_esp32s31_coex::{
    CoexClient, CoexClockHardware, CoexError, CoexPti, CoexTimerClock, CoexTimerHardware,
    CoexTimerIndex,
};
use open_esp_radio_esp32s31_pac::{
    CoexTimerClientValue, CoexTimerPtiValue, CoexTimerRegister, WifiRadioRegisters,
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

struct ValidationCoexRegisters<'registers> {
    registers: RefCell<&'registers mut WifiRadioRegisters>,
    real_chip: bool,
}

impl<'registers> ValidationCoexRegisters<'registers> {
    fn new(registers: &'registers mut WifiRadioRegisters, real_chip: bool) -> Self {
        Self {
            registers: RefCell::new(registers),
            real_chip,
        }
    }
}

struct CoexTimerHal<'registers, 'owner> {
    owner: &'owner ValidationCoexRegisters<'registers>,
}

struct CoexClockHal<'registers, 'owner> {
    owner: &'owner ValidationCoexRegisters<'registers>,
}

impl CoexClockHardware for CoexClockHal<'_, '_> {
    fn sample(&mut self) -> Result<CoexTimerClock, CoexError> {
        crate::wifi::mac::coex_timer_clock_for_chip(
            self.owner
                .registers
                .borrow()
                .sample_coexistence_low_power_clock(),
            self.owner.real_chip,
        )
    }
}

#[doc(hidden)]
pub fn validation_enable_timer(index: u32) {
    let Some(timer) = CoexTimerRegister::new(index as u8) else {
        return;
    };
    crate::RadioRuntimeOwner::claim_for_validation()
        .pac_mut()
        .enable_coex_timer(timer);
}

#[doc(hidden)]
pub fn validation_disable_timer(index: u32) {
    let Some(timer) = CoexTimerRegister::new(index as u8) else {
        return;
    };
    crate::RadioRuntimeOwner::claim_for_validation()
        .pac_mut()
        .disable_coex_timer(timer);
}

#[doc(hidden)]
pub fn validation_force_timer(index: u32) {
    let Some(timer) = CoexTimerRegister::new(index as u8) else {
        return;
    };
    crate::RadioRuntimeOwner::claim_for_validation()
        .pac_mut()
        .force_coex_timer(timer);
}

#[doc(hidden)]
pub fn validation_unforce_timer(index: u32) {
    let Some(timer) = CoexTimerRegister::new(index as u8) else {
        return;
    };
    crate::RadioRuntimeOwner::claim_for_validation()
        .pac_mut()
        .unforce_coex_timer(timer);
}

/// Execute one complete COEX timer program against an isolated validation
/// owner. The caller supplies only the chip clock environment and typed values.
#[cfg(feature = "validation-probes")]
#[doc(hidden)]
pub fn validation_program_timer(
    real_chip: bool,
    index: CoexTimerIndex,
    client: CoexClient,
    pti: CoexPti,
    latency: u32,
    duration: u32,
) -> Result<(), CoexError> {
    let mut owner = crate::RadioRuntimeOwner::claim_for_validation();
    let registers = ValidationCoexRegisters::new(owner.pac_mut(), real_chip);
    let mut timer = CoexTimerHal { owner: &registers };
    let mut clock = CoexClockHal { owner: &registers };
    open_esp_radio_esp32s31_coex::program_timer(
        &mut timer, &mut clock, index, client, pti, latency, duration,
    )
}

/// Execute one enabled COEX-core request against an isolated validation
/// owner. The boolean selects Bluetooth (`false`) or Wi-Fi (`true`).
#[cfg(feature = "validation-probes")]
#[doc(hidden)]
pub fn validation_core_request(
    real_chip: bool,
    wifi: bool,
    request: open_esp_radio_esp32s31_coex::CoexClientRequest,
) -> Result<(), CoexError> {
    use open_esp_radio_esp32s31_coex::{CoexCore, CoexPtiTable};

    let mut core = CoexCore::new(CoexPtiTable::reviewed_vendor());
    core.enable();
    let mut owner = crate::RadioRuntimeOwner::claim_for_validation();
    let registers = ValidationCoexRegisters::new(owner.pac_mut(), real_chip);
    let mut timer = CoexTimerHal { owner: &registers };
    let mut clock = CoexClockHal { owner: &registers };
    if wifi {
        core.request_wifi(&mut timer, &mut clock, request)
            .map(|_| ())
    } else {
        core.request_bluetooth(&mut timer, &mut clock, request)
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
    let mut owner = crate::RadioRuntimeOwner::claim_for_validation();
    let registers = ValidationCoexRegisters::new(owner.pac_mut(), true);
    let mut timer = CoexTimerHal { owner: &registers };
    core.release(&mut timer, event).map(|_| ())
}

impl CoexTimerHardware for CoexTimerHal<'_, '_> {
    fn configure_request(
        &mut self,
        index: CoexTimerIndex,
        client: CoexClient,
        pti: CoexPti,
    ) -> Result<(), CoexError> {
        let client = CoexTimerClientValue::new(client as u32).ok_or(CoexError::Hardware)?;
        let pti = CoexTimerPtiValue::new(u32::from(pti.value())).ok_or(CoexError::Hardware)?;
        self.owner
            .registers
            .borrow_mut()
            .configure_coex_timer(pac_timer(index), client, pti);
        Ok(())
    }

    fn set_primary_target(
        &mut self,
        index: CoexTimerIndex,
        tick_image: u32,
    ) -> Result<(), CoexError> {
        self.owner
            .registers
            .borrow_mut()
            .set_coex_timer_primary_target(pac_timer(index), tick_image);
        Ok(())
    }

    fn set_secondary_target(
        &mut self,
        index: CoexTimerIndex,
        tick_image: u32,
    ) -> Result<(), CoexError> {
        self.owner
            .registers
            .borrow_mut()
            .set_coex_timer_secondary_target(pac_timer(index), tick_image);
        Ok(())
    }

    fn enable(&mut self, index: CoexTimerIndex) -> Result<(), CoexError> {
        self.owner
            .registers
            .borrow_mut()
            .enable_coex_timer(pac_timer(index));
        Ok(())
    }

    fn disable(&mut self, index: CoexTimerIndex) -> Result<(), CoexError> {
        self.owner
            .registers
            .borrow_mut()
            .disable_coex_timer(pac_timer(index));
        Ok(())
    }

    fn force(&mut self, index: CoexTimerIndex) -> Result<(), CoexError> {
        self.owner
            .registers
            .borrow_mut()
            .force_coex_timer(pac_timer(index));
        Ok(())
    }

    fn unforce(&mut self, index: CoexTimerIndex) -> Result<(), CoexError> {
        self.owner
            .registers
            .borrow_mut()
            .unforce_coex_timer(pac_timer(index));
        Ok(())
    }
}
