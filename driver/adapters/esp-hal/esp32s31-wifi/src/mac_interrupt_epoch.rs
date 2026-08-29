//! ESP-HAL routing and stable ISR storage for finite MAC interrupt epochs.

#![forbid(unsafe_code)]

use core::cell::RefCell;

use crate::EspHalRadioPeripheral;
use critical_section::Mutex;
use esp_hal::interrupt::InterruptHandler;
use open_esp_radio_esp32s31_hal::{
    ConnectedStaInterruptPrepared, MacInterruptMask, MacInterruptRegisters, MacInterruptSetup,
    MacPowerInterruptObservation, MacPowerInterruptRegisters, MacPowerWakeCause, RadioRuntimeOwner,
    radio_arena::{Esp32s31RadioAccess, Esp32s31RadioOwnerArenaError},
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

/// Failure to apply role policy through the MAC register capability currently
/// installed in the hard ISR slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspHalActiveMacInterruptError {
    Inactive,
    RadioOwner(Esp32s31RadioOwnerArenaError),
}

/// Summary of one bounded hard-MAC handler run.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EspHalMacInterruptServiceReport {
    pub had_status: bool,
    pub posted_events: u32,
    pub had_auxiliary_event: bool,
    pub had_unhandled_event: bool,
}

/// Summary of one bounded hard-power handler run.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EspHalPowerInterruptServiceReport {
    pub observation: MacPowerInterruptObservation,
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

/// Prepare a connected STA role without tearing down the physical IRQ route.
///
/// The critical section lends the unique ISR-owned register capability only
/// for this synchronous transaction; it cannot cross a scheduling boundary.
pub fn prepare_active_connected_sta_without_power_save(
    radio: &mut RadioRuntimeOwner,
) -> Result<ConnectedStaInterruptPrepared, EspHalActiveMacInterruptError> {
    critical_section::with(|critical_section| {
        let mut interrupt = MAC_INTERRUPT_REGISTERS.borrow_ref_mut(critical_section);
        let Some(interrupt) = interrupt.as_mut() else {
            return Err(EspHalActiveMacInterruptError::Inactive);
        };
        Ok(interrupt.prepare_connected_sta_without_power_save(radio))
    })
}

/// Arena-backed form of
/// [`prepare_active_connected_sta_without_power_save`].
pub fn prepare_active_connected_sta_without_power_save_with_access(
    access: &Esp32s31RadioAccess<'_>,
) -> Result<ConnectedStaInterruptPrepared, EspHalActiveMacInterruptError> {
    critical_section::with(|critical_section| {
        let mut interrupt = MAC_INTERRUPT_REGISTERS.borrow_ref_mut(critical_section);
        let Some(interrupt) = interrupt.as_mut() else {
            return Err(EspHalActiveMacInterruptError::Inactive);
        };
        access
            .try_prepare_active_connected_sta_without_power_save(interrupt)
            .map_err(EspHalActiveMacInterruptError::RadioOwner)
    })
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

/// Service one complete MAC snapshot in recovered vendor priority.
///
/// ESP-HAL configures the CPU route as a level interrupt. The complete snapshot
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
            had_status: snapshot.had_status,
            posted_events: snapshot.posted_events,
            had_auxiliary_event: snapshot.had_auxiliary_event,
            had_unhandled_event: snapshot.had_unhandled_event,
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
            observation: snapshot.observation,
        }
    })
}
