//! ESP-HAL routing and stable ISR storage for finite MAC interrupt epochs.

#![forbid(unsafe_code)]

use core::cell::RefCell;

use crate::EspHalRadioPeripheral;
use critical_section::Mutex;
use esp_hal::interrupt::InterruptHandler;
use open_esp_radio_esp32s31_registers::{
    MacInterruptRegisters, MacInterruptSetup, MacPowerInterruptRegisters,
};
use open_esp_radio_esp32s31_wifi_lmac::irq::{
    IrqSink, MacInterruptRoute, PowerIrqSink, handle_mac_irq, handle_power_irq,
};

const MAX_ISR_SNAPSHOTS: u8 = 32;

static MAC_INTERRUPT_REGISTERS: Mutex<RefCell<Option<MacInterruptRegisters>>> =
    Mutex::new(RefCell::new(None));
static POWER_INTERRUPT_REGISTERS: Mutex<RefCell<Option<MacPowerInterruptRegisters>>> =
    Mutex::new(RefCell::new(None));

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspHalMacInterruptRouteError {
    AlreadyActive,
    AlreadyQuiesced,
    StorageInvariant,
}

/// Summary of one bounded hard-MAC handler run.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EspHalMacInterruptServiceReport {
    pub first_status: u32,
    pub observed_status: u32,
    pub nonzero_snapshots: u8,
}

/// Summary of one bounded hard-power handler run.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EspHalPowerInterruptServiceReport {
    pub observed_status: u32,
    pub nonzero_snapshots: u8,
}

/// Concrete ESP-HAL CPU route for the unique S31 Wi-Fi interrupt owner.
///
/// Handler addresses are fixture/application composition. Register storage,
/// publication, CPU routing and recovery are platform-adapter mechanics.
pub struct EspHalMacInterruptRoute {
    mac_handler: InterruptHandler,
    power_handler: InterruptHandler,
    active: bool,
}

impl EspHalMacInterruptRoute {
    pub const fn new(mac_handler: InterruptHandler, power_handler: InterruptHandler) -> Self {
        Self {
            mac_handler,
            power_handler,
            active: false,
        }
    }

    fn storage_is_empty(&self) -> bool {
        critical_section::with(|critical_section| {
            MAC_INTERRUPT_REGISTERS
                .borrow_ref(critical_section)
                .is_none()
                && POWER_INTERRUPT_REGISTERS
                    .borrow_ref(critical_section)
                    .is_none()
        })
    }
}

impl MacInterruptRoute for EspHalMacInterruptRoute {
    type Platform = EspHalRadioPeripheral;
    type Setup = MacInterruptSetup;
    type Error = EspHalMacInterruptRouteError;

    fn activate(
        &mut self,
        platform: &Self::Platform,
        setup: Self::Setup,
        event_mask: u32,
    ) -> Result<(), (Self::Error, Self::Setup)> {
        if self.active {
            return Err((EspHalMacInterruptRouteError::AlreadyActive, setup));
        }
        if !self.storage_is_empty() {
            return Err((EspHalMacInterruptRouteError::StorageInvariant, setup));
        }
        let (mac, power) = setup.activate(event_mask);
        critical_section::with(|critical_section| {
            *MAC_INTERRUPT_REGISTERS.borrow_ref_mut(critical_section) = Some(mac);
            *POWER_INTERRUPT_REGISTERS.borrow_ref_mut(critical_section) = Some(power);
        });
        platform.bind_interrupts(self.mac_handler, self.power_handler);
        self.active = true;
        Ok(())
    }

    fn quiesce(&mut self, platform: &Self::Platform) -> Result<Self::Setup, Self::Error> {
        if !self.active {
            return Err(EspHalMacInterruptRouteError::AlreadyQuiesced);
        }
        // The handlers are bound on this task's core. Once both CPU routes are
        // disabled, an earlier same-core handler has returned and no new one
        // can begin while the PAC values are recovered.
        platform.disable_interrupts();
        let (mac, power) = critical_section::with(|critical_section| {
            let mut mac = MAC_INTERRUPT_REGISTERS.borrow_ref_mut(critical_section);
            let mut power = POWER_INTERRUPT_REGISTERS.borrow_ref_mut(critical_section);
            match (mac.take(), power.take()) {
                (Some(mac), Some(power)) => Ok((mac, power)),
                (mac_value, power_value) => {
                    *mac = mac_value;
                    *power = power_value;
                    Err(EspHalMacInterruptRouteError::StorageInvariant)
                }
            }
        })?;
        let setup = mac.deactivate(power);
        self.active = false;
        Ok(setup)
    }
}

/// Service the active MAC register capability in recovered vendor priority
/// until the status bank is empty or the finite ISR budget is exhausted.
#[inline]
pub fn service_mac_interrupt<S: IrqSink>(sink: &S) -> EspHalMacInterruptServiceReport {
    critical_section::with(|critical_section| {
        let mut registers = MAC_INTERRUPT_REGISTERS.borrow_ref_mut(critical_section);
        let Some(interrupt) = registers.as_mut() else {
            return EspHalMacInterruptServiceReport::default();
        };
        let mut report = EspHalMacInterruptServiceReport::default();
        for _ in 0..MAX_ISR_SNAPSHOTS {
            let (_, snapshot) = handle_mac_irq(interrupt, sink);
            if snapshot.status == 0 {
                break;
            }
            if report.nonzero_snapshots == 0 {
                report.first_status = snapshot.status;
            }
            report.observed_status |= snapshot.status;
            report.nonzero_snapshots += 1;
        }
        report
    })
}

/// Service the active power-interrupt bank with the same finite ISR budget.
#[inline]
pub fn service_power_interrupt<S: PowerIrqSink>(sink: &S) -> EspHalPowerInterruptServiceReport {
    critical_section::with(|critical_section| {
        let mut registers = POWER_INTERRUPT_REGISTERS.borrow_ref_mut(critical_section);
        let Some(interrupt) = registers.as_mut() else {
            return EspHalPowerInterruptServiceReport::default();
        };
        let mut report = EspHalPowerInterruptServiceReport::default();
        for _ in 0..MAX_ISR_SNAPSHOTS {
            let (_, snapshot) = handle_power_irq(interrupt, sink);
            if snapshot.status == 0 {
                break;
            }
            report.observed_status |= snapshot.status;
            report.nonzero_snapshots += 1;
        }
        report
    })
}
