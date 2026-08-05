//! ESP-HAL routing and stable ISR storage for finite MAC interrupt epochs.

use core::sync::atomic::{AtomicPtr, Ordering};

use esp_hal::interrupt::InterruptHandler;
use open_esp_radio_esp32s31_registers::{
    MacInterruptRegisters, MacInterruptSetup, MacPowerInterruptRegisters,
};
use open_esp_radio_esp32s31_wifi_lmac::irq::{
    IrqSink, MacInterruptRoute, PowerIrqSink, handle_mac_irq, handle_power_irq,
};
use static_cell::StaticCell;

use crate::EspHalRadioPeripheral;

const MAX_ISR_SNAPSHOTS: u8 = 32;

static MAC_INTERRUPT_REGISTERS: StaticCell<MacInterruptRegisters> = StaticCell::new();
static MAC_INTERRUPT_STORAGE_PTR: AtomicPtr<MacInterruptRegisters> =
    AtomicPtr::new(core::ptr::null_mut());
static MAC_INTERRUPT_ACTIVE_PTR: AtomicPtr<MacInterruptRegisters> =
    AtomicPtr::new(core::ptr::null_mut());
static POWER_INTERRUPT_REGISTERS: StaticCell<MacPowerInterruptRegisters> = StaticCell::new();
static POWER_INTERRUPT_STORAGE_PTR: AtomicPtr<MacPowerInterruptRegisters> =
    AtomicPtr::new(core::ptr::null_mut());
static POWER_INTERRUPT_ACTIVE_PTR: AtomicPtr<MacPowerInterruptRegisters> =
    AtomicPtr::new(core::ptr::null_mut());

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

    fn storage_state(
        &self,
    ) -> Result<
        (*mut MacInterruptRegisters, *mut MacPowerInterruptRegisters),
        EspHalMacInterruptRouteError,
    > {
        let mac_storage = MAC_INTERRUPT_STORAGE_PTR.load(Ordering::Acquire);
        let power_storage = POWER_INTERRUPT_STORAGE_PTR.load(Ordering::Acquire);
        let mac_active = MAC_INTERRUPT_ACTIVE_PTR.load(Ordering::Acquire);
        let power_active = POWER_INTERRUPT_ACTIVE_PTR.load(Ordering::Acquire);
        let storage_is_cold = mac_storage.is_null() && power_storage.is_null();
        let storage_is_reusable = !mac_storage.is_null() && !power_storage.is_null();
        if (!storage_is_cold && !storage_is_reusable)
            || !mac_active.is_null()
            || !power_active.is_null()
        {
            return Err(EspHalMacInterruptRouteError::StorageInvariant);
        }
        Ok((mac_storage, power_storage))
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
        let (mac_storage, power_storage) = match self.storage_state() {
            Ok(storage) => storage,
            Err(error) => return Err((error, setup)),
        };
        let (mac, power) = setup.activate(event_mask);
        let (mac_storage, power_storage) = if mac_storage.is_null() {
            let mac_storage = MAC_INTERRUPT_REGISTERS.init(mac) as *mut _;
            let power_storage = POWER_INTERRUPT_REGISTERS.init(power) as *mut _;
            MAC_INTERRUPT_STORAGE_PTR.store(mac_storage, Ordering::Release);
            POWER_INTERRUPT_STORAGE_PTR.store(power_storage, Ordering::Release);
            (mac_storage, power_storage)
        } else {
            // SAFETY: both active pointers are null and the CPU routes remain
            // disabled after the previous quiescence. That transition moved
            // both values out of these exact stable locations.
            unsafe {
                mac_storage.write(mac);
                power_storage.write(power);
            }
            (mac_storage, power_storage)
        };
        MAC_INTERRUPT_ACTIVE_PTR.store(mac_storage, Ordering::Release);
        POWER_INTERRUPT_ACTIVE_PTR.store(power_storage, Ordering::Release);
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
        let mac = MAC_INTERRUPT_ACTIVE_PTR.load(Ordering::Acquire);
        let power = POWER_INTERRUPT_ACTIVE_PTR.load(Ordering::Acquire);
        if mac.is_null()
            || power.is_null()
            || mac != MAC_INTERRUPT_STORAGE_PTR.load(Ordering::Acquire)
            || power != POWER_INTERRUPT_STORAGE_PTR.load(Ordering::Acquire)
        {
            return Err(EspHalMacInterruptRouteError::StorageInvariant);
        }
        let recovered_mac = MAC_INTERRUPT_ACTIVE_PTR.swap(core::ptr::null_mut(), Ordering::AcqRel);
        let recovered_power =
            POWER_INTERRUPT_ACTIVE_PTR.swap(core::ptr::null_mut(), Ordering::AcqRel);
        debug_assert_eq!(recovered_mac, mac);
        debug_assert_eq!(recovered_power, power);
        // SAFETY: CPU routing is disabled, active pointers are null and these
        // stable locations contain the unique values published by `activate`.
        let mac = unsafe { recovered_mac.read() };
        let power = unsafe { recovered_power.read() };
        let setup = mac.deactivate(power);
        self.active = false;
        Ok(setup)
    }
}

/// Service the active MAC register capability in recovered vendor priority
/// until the status bank is empty or the finite ISR budget is exhausted.
#[inline]
pub fn service_mac_interrupt<S: IrqSink>(sink: &S) -> EspHalMacInterruptServiceReport {
    let registers = MAC_INTERRUPT_ACTIVE_PTR.load(Ordering::Acquire);
    if registers.is_null() {
        return EspHalMacInterruptServiceReport::default();
    }
    // SAFETY: the epoch publishes this unique PAC owner before CPU routing.
    // Task-side recovery first disables the same-core route and cannot overlap
    // this handler invocation.
    let interrupt = unsafe { &mut *registers };
    let mut report = EspHalMacInterruptServiceReport::default();
    for _ in 0..MAX_ISR_SNAPSHOTS {
        let (_, snapshot) = handle_mac_irq(&mut *interrupt, sink);
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
}

/// Service the active power-interrupt bank with the same finite ISR budget.
#[inline]
pub fn service_power_interrupt<S: PowerIrqSink>(sink: &S) -> EspHalPowerInterruptServiceReport {
    let registers = POWER_INTERRUPT_ACTIVE_PTR.load(Ordering::Acquire);
    if registers.is_null() {
        return EspHalPowerInterruptServiceReport::default();
    }
    // SAFETY: both ISR capabilities are published before either route opens;
    // quiescence disables the same-core route before moving this value out.
    let interrupt = unsafe { &mut *registers };
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
}
