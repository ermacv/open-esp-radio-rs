//! Compiled vendor-comparison entry points over an isolated HAL owner.
//!
//! Each call claims the validation radio owner, borrows its coexistence
//! timer bank and runs the production core or timer sequence against it.

use core::cell::RefCell;

use oer_esp32s31_hal::{
    coex::{CoexPolicyTimer, CoexTimerBank},
    owner::RadioRuntimeOwner,
    types::{CoexTimerClientValue, CoexTimerPtiValue},
};

use crate::{
    CoexClient, CoexClientRequest, CoexClockHardware, CoexCore, CoexError, CoexEventId, CoexPti,
    CoexPtiTable, CoexTimerClock, CoexTimerHardware, CoexTimerIndex, hal::timer_clock,
};

const fn bank_timer(index: CoexTimerIndex) -> CoexPolicyTimer {
    match index {
        CoexTimerIndex::Timer0 => CoexPolicyTimer::Timer0,
        CoexTimerIndex::Timer1 => CoexPolicyTimer::Timer1,
        CoexTimerIndex::Timer2 => CoexPolicyTimer::Timer2,
        CoexTimerIndex::Timer3 => CoexPolicyTimer::Timer3,
        CoexTimerIndex::Timer4 => CoexPolicyTimer::Timer4,
    }
}

/// One timer bank shared by the timer and clock ports of a single call.
struct SharedBank<'registers> {
    bank: RefCell<CoexTimerBank<'registers>>,
    real_chip: bool,
}

struct TimerPort<'bank, 'registers> {
    shared: &'bank SharedBank<'registers>,
}

struct ClockPort<'bank, 'registers> {
    shared: &'bank SharedBank<'registers>,
}

impl CoexClockHardware for ClockPort<'_, '_> {
    fn sample(&mut self) -> Result<CoexTimerClock, CoexError> {
        let observation = self.shared.bank.borrow_mut().sample_low_power_clock();
        timer_clock(observation, self.shared.real_chip)
    }
}

impl CoexTimerHardware for TimerPort<'_, '_> {
    fn pti(&mut self, event: CoexEventId) -> CoexPti {
        // The isolated validation owner has no arbiter; it carries the
        // arbiter's cold table.
        CoexPtiTable::VENDOR.pti(event)
    }

    fn configure_request(
        &mut self,
        index: CoexTimerIndex,
        client: CoexClient,
        pti: CoexPti,
    ) -> Result<(), CoexError> {
        let client = CoexTimerClientValue::new(client as u32).ok_or(CoexError::Hardware)?;
        let pti = CoexTimerPtiValue::new(u32::from(pti.value())).ok_or(CoexError::Hardware)?;
        self.shared
            .bank
            .borrow_mut()
            .configure(bank_timer(index), client, pti);
        Ok(())
    }

    fn set_primary_target(
        &mut self,
        index: CoexTimerIndex,
        tick_image: u32,
    ) -> Result<(), CoexError> {
        self.shared
            .bank
            .borrow_mut()
            .set_primary_target(bank_timer(index), tick_image);
        Ok(())
    }

    fn set_secondary_target(
        &mut self,
        index: CoexTimerIndex,
        tick_image: u32,
    ) -> Result<(), CoexError> {
        self.shared
            .bank
            .borrow_mut()
            .set_secondary_target(bank_timer(index), tick_image);
        Ok(())
    }

    fn enable(&mut self, index: CoexTimerIndex) -> Result<(), CoexError> {
        self.shared.bank.borrow_mut().enable(bank_timer(index));
        Ok(())
    }

    fn disable(&mut self, index: CoexTimerIndex) -> Result<(), CoexError> {
        self.shared.bank.borrow_mut().disable(bank_timer(index));
        Ok(())
    }

    fn force(&mut self, index: CoexTimerIndex) -> Result<(), CoexError> {
        self.shared.bank.borrow_mut().force(bank_timer(index));
        Ok(())
    }

    fn unforce(&mut self, index: CoexTimerIndex) -> Result<(), CoexError> {
        self.shared.bank.borrow_mut().unforce(bank_timer(index));
        Ok(())
    }
}

fn with_timer(index: u32, operation: impl FnOnce(&mut CoexTimerBank<'_>, CoexPolicyTimer)) {
    let Some(timer) = CoexPolicyTimer::new(index as u8) else {
        return;
    };
    let mut owner = RadioRuntimeOwner::claim_for_validation();
    operation(&mut owner.coex_timer_bank(), timer);
}

pub fn enable_timer(index: u32) {
    with_timer(index, |bank, timer| bank.enable(timer));
}

pub fn disable_timer(index: u32) {
    with_timer(index, |bank, timer| bank.disable(timer));
}

pub fn force_timer(index: u32) {
    with_timer(index, |bank, timer| bank.force(timer));
}

pub fn unforce_timer(index: u32) {
    with_timer(index, |bank, timer| bank.unforce(timer));
}

/// Execute one complete timer program. The caller supplies only the chip
/// clock environment and typed values.
pub fn program_timer(
    real_chip: bool,
    index: CoexTimerIndex,
    client: CoexClient,
    pti: CoexPti,
    latency: u32,
    duration: u32,
) -> Result<(), CoexError> {
    let mut owner = RadioRuntimeOwner::claim_for_validation();
    let shared = SharedBank {
        bank: RefCell::new(owner.coex_timer_bank()),
        real_chip,
    };
    crate::program_timer(
        &mut TimerPort { shared: &shared },
        &mut ClockPort { shared: &shared },
        index,
        client,
        pti,
        latency,
        duration,
    )
}

/// Execute one enabled core request. The boolean selects Bluetooth (`false`)
/// or Wi-Fi (`true`).
pub fn core_request(
    real_chip: bool,
    wifi: bool,
    request: CoexClientRequest,
) -> Result<(), CoexError> {
    let mut core = CoexCore::new();
    core.enable();
    let mut owner = RadioRuntimeOwner::claim_for_validation();
    let shared = SharedBank {
        bank: RefCell::new(owner.coex_timer_bank()),
        real_chip,
    };
    let mut timer = TimerPort { shared: &shared };
    let mut clock = ClockPort { shared: &shared };
    if wifi {
        core.request_wifi(&mut timer, &mut clock, request)
            .map(|_| ())
    } else {
        core.request_bluetooth(&mut timer, &mut clock, request)
            .map(|_| ())
    }
}

/// Execute one core release.
pub fn core_release(event: CoexEventId) -> Result<(), CoexError> {
    let mut core = CoexCore::new();
    let mut owner = RadioRuntimeOwner::claim_for_validation();
    let shared = SharedBank {
        bank: RefCell::new(owner.coex_timer_bank()),
        real_chip: true,
    };
    core.release(&mut TimerPort { shared: &shared }, event)
        .map(|_| ())
}
