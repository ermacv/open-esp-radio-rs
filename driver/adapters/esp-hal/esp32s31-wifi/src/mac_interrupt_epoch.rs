//! ESP-HAL routing and stable ISR storage for finite MAC interrupt epochs.

#![forbid(unsafe_code)]

use core::cell::RefCell;

use crate::EspHalRadioPeripheral;
use critical_section::Mutex;
use esp_hal::interrupt::InterruptHandler;
use open_esp_radio_esp32s31_hal::{
    MacInterruptMask, MacInterruptRegisters, MacInterruptSetup, MacPowerInterruptRegisters,
    MacPowerWakeCause,
};
use open_esp_radio_esp32s31_wifi_mac::irq::{
    IrqSink, MacInterruptRoute, PowerIrqSink, handle_mac_irq, handle_power_irq,
};

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

/// Task-side result when the active ISR slot cannot lend its unique WDEVPWR
/// capability for one critical-section-bounded transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspHalActivePowerInterruptError {
    Inactive,
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

/// Restore the RX delivery source group for the installed MAC epoch.
///
/// The static register slot is the same unique capability borrowed by the
/// hard handler. Entering a critical section prevents a same-core ISR from
/// racing the enable-register RMW; no raw register authority escapes to the
/// Embassy task.
pub fn unmask_active_mac_rx_delivery_interrupts() {
    critical_section::with(|critical_section| {
        if let Some(interrupt) = MAC_INTERRUPT_REGISTERS
            .borrow_ref_mut(critical_section)
            .as_mut()
        {
            interrupt.unmask_rx_delivery_interrupts();
        }
    });
}

/// Mask WDEVPWR and acknowledge one reviewed TSF-timer cause while the same
/// critical section excludes the hard ISR.
pub fn mask_and_acknowledge_active_mac_power_wake_cause(
    cause: MacPowerWakeCause,
) -> Result<(), EspHalActivePowerInterruptError> {
    critical_section::with(|critical_section| {
        let mut power = POWER_INTERRUPT_REGISTERS.borrow_ref_mut(critical_section);
        let Some(power) = power.as_mut() else {
            return Err(EspHalActivePowerInterruptError::Inactive);
        };
        power.mask_and_acknowledge_wake_cause(cause);
        Ok(())
    })
}

impl MacInterruptRoute for EspHalMacInterruptRoute {
    type Platform = EspHalRadioPeripheral;
    type Setup = MacInterruptSetup;
    type Error = EspHalMacInterruptRouteError;

    fn activate(
        &mut self,
        platform: &Self::Platform,
        setup: Self::Setup,
        event_mask: MacInterruptMask,
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

/// Service one complete MAC status image in recovered vendor priority.
///
/// ESP-HAL configures the CPU route as a level interrupt. The complete image
/// is acknowledged before return; status which remains or arrives afterwards
/// therefore retriggers the route. Reading until an empty image added one
/// redundant MMIO read to every RX interrupt, while HIL observed no second
/// non-zero snapshot in more than 100,000 entries per saturation run.
#[inline]
pub fn service_mac_interrupt<S: IrqSink>(sink: &S) -> EspHalMacInterruptServiceReport {
    critical_section::with(|critical_section| {
        let mut registers = MAC_INTERRUPT_REGISTERS.borrow_ref_mut(critical_section);
        let Some(interrupt) = registers.as_mut() else {
            return EspHalMacInterruptServiceReport::default();
        };
        let (_, snapshot) = handle_mac_irq(interrupt, sink);
        EspHalMacInterruptServiceReport {
            first_status: snapshot.status,
            observed_status: snapshot.status,
            nonzero_snapshots: u8::from(snapshot.status != 0),
        }
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
        let (_, snapshot) = handle_power_irq(interrupt, sink);
        EspHalPowerInterruptServiceReport {
            observed_status: snapshot.status,
            nonzero_snapshots: u8::from(snapshot.status != 0),
        }
    })
}
