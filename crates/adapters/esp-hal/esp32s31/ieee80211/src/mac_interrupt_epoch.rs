//! ESP-HAL routing and stable ISR storage for finite MAC interrupt epochs.

#![forbid(unsafe_code)]

use core::{cell::RefCell, marker::PhantomData};

mod storage;

use crate::EspHalRadioPeripheral;

use critical_section::Mutex;

use esp_hal::{interrupt::InterruptHandler, system::Cpu};

use oer_esp32s31_hal::{
    ieee80211::arena::{RadioAccess, RadioOwnerArenaError},
    owner::{
        ConnectedStaInterruptPrepared, MacInterruptCheckpoint, MacInterruptRegisters,
        MacInterruptSetup, MacPowerInterruptRegisters, RadioRuntimeOwner,
    },
    types::{MacInterruptMask, MacPowerInterruptObservation, MacPowerWakeCause},
};

use oer_esp32s31_wifi_mac::irq::{
    IrqSink, MacInterruptPauseRoute, MacInterruptRoute, PowerIrqSink, handle_mac_irq,
    handle_power_irq,
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
    Paused,
    NotPaused,
    WrongCore,
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
    RadioOwner(RadioOwnerArenaError),
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
    phase: Phase,
    // Bind, detach and resume belong to the same executor/core. Runtime core
    // checks also reject misuse through a separately obtained platform token.
    _same_core: PhantomData<*mut ()>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Inactive,
    Active(Cpu),
    Paused(Cpu),
}

impl EspHalMacInterruptRoute {
    pub const fn new(mac_handler: InterruptHandler, power_handler: InterruptHandler) -> Self {
        Self {
            mac_handler,
            power_handler,
            phase: Phase::Inactive,
            _same_core: PhantomData,
        }
    }

    fn detach(
        &mut self,
        platform: &EspHalRadioPeripheral,
    ) -> Result<(MacInterruptRegisters, MacPowerInterruptRegisters), EspHalMacInterruptRouteError>
    {
        match self.phase {
            Phase::Active(core) if core != Cpu::current() => {
                return Err(EspHalMacInterruptRouteError::WrongCore);
            }
            Phase::Active(_) => {}
            Phase::Paused(_) => return Err(EspHalMacInterruptRouteError::Paused),
            Phase::Inactive => return Err(EspHalMacInterruptRouteError::AlreadyQuiesced),
        }
        critical_section::with(|cs| {
            let mut mac = MAC_INTERRUPT_REGISTERS.borrow_ref_mut(cs);
            let mut power = POWER_INTERRUPT_REGISTERS.borrow_ref_mut(cs);
            // Validate storage before changing routing. Once disabled on the
            // binding core, no handler can retain a borrow of either owner.
            storage::detach_pair(&mut mac, &mut power, || platform.disable_interrupts())
                .ok_or(EspHalMacInterruptRouteError::StorageInvariant)
        })
    }

    fn install(
        &mut self,
        platform: &EspHalRadioPeripheral,
        mac: MacInterruptRegisters,
        power: MacPowerInterruptRegisters,
    ) {
        critical_section::with(|cs| {
            *MAC_INTERRUPT_REGISTERS.borrow_ref_mut(cs) = Some(mac);
            *POWER_INTERRUPT_REGISTERS.borrow_ref_mut(cs) = Some(power);
            self.phase = Phase::Active(Cpu::current());
            platform.bind_interrupts(self.mac_handler, self.power_handler);
        });
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
    access: &RadioAccess<'_>,
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
        match self.phase {
            Phase::Active(_) => return Err((EspHalMacInterruptRouteError::AlreadyActive, setup)),
            Phase::Paused(_) => return Err((EspHalMacInterruptRouteError::Paused, setup)),
            Phase::Inactive => {}
        }
        if !self.storage_is_empty() {
            return Err((EspHalMacInterruptRouteError::StorageInvariant, setup));
        }
        let (mac, power) = setup.activate(event_mask);
        self.install(platform, mac, power);
        Ok(())
    }

    fn quiesce(&mut self, platform: &Self::Platform) -> Result<Self::Setup, Self::Error> {
        let (mac, power) = self.detach(platform)?;
        let setup = mac.deactivate(power);
        self.phase = Phase::Inactive;
        Ok(setup)
    }
}

impl MacInterruptPauseRoute for EspHalMacInterruptRoute {
    type Paused = MacInterruptCheckpoint;

    fn pause(&mut self, platform: &Self::Platform) -> Result<Self::Paused, Self::Error> {
        let (mac, power) = self.detach(platform)?;
        self.phase = Phase::Paused(Cpu::current());
        // No peripheral write: preserve source moderation, WDEVPWR policy and
        // latched events, including arrivals after the CPU route is detached.
        Ok(mac.checkpoint(power))
    }

    fn resume(
        &mut self,
        platform: &Self::Platform,
        paused: Self::Paused,
    ) -> Result<(), (Self::Error, Self::Paused)> {
        match self.phase {
            Phase::Paused(core) if core != Cpu::current() => {
                return Err((EspHalMacInterruptRouteError::WrongCore, paused));
            }
            Phase::Paused(_) => {}
            _ => return Err((EspHalMacInterruptRouteError::NotPaused, paused)),
        }
        if !self.storage_is_empty() {
            return Err((EspHalMacInterruptRouteError::StorageInvariant, paused));
        }
        // Restore stable storage before exposing the level routes. A latched
        // enabled event retriggers the normal bounded handler. A moderated RX
        // source stays masked until the consumer drains its descriptor frontier.
        let (mac, power) = paused.into_registers();
        self.install(platform, mac, power);
        Ok(())
    }

    fn finish_pause(&mut self, paused: Self::Paused) -> Self::Setup {
        assert!(matches!(self.phase, Phase::Paused(_)));
        let setup = paused.deactivate();
        self.phase = Phase::Inactive;
        setup
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
