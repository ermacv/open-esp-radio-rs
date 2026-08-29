//! Typed ESP-HAL routing primitives for all three Bluetooth interrupts.
//!
//! Stable publication is deliberately separate from a live interrupt epoch.
//! Both unique HAL owners are installed atomically before any CPU route can be
//! enabled. Primary semantic fault/dynamic dispatch, source-127 software work,
//! opaque NRT acknowledgement plus the scheduler-list drain remain incomplete.

#![forbid(unsafe_code)]
#![allow(
    dead_code,
    reason = "typed route primitives await stable ISR storage and complete three-source dispatch"
)]

use core::cell::RefCell;

use critical_section::Mutex;
use esp_hal::{
    interrupt::{self, InterruptHandler, Priority},
    peripherals::Interrupt,
    system::Cpu,
};
use open_esp_radio_esp32s31_bluetooth::{
    BluetoothCpuInterruptRoutePolicy, BluetoothInterruptOwnerStorage,
};
use open_esp_radio_esp32s31_hal::{
    BluetoothInterruptRegistersOwner, BluetoothModemLpTimerInterruptReadyOwner,
};

use crate::bluetooth_route_policy::{
    BluetoothInterruptRouteError, EspHalBluetoothInterruptStorageError, REQUIRED_PRIORITY_LEVEL,
    validate_interrupt_storage, validate_quiesce_core, validate_route_priorities,
};

pub(crate) const PRIMARY_INTERRUPT: Interrupt = Interrupt::BT_MAC;
pub(crate) const MODEM_LP_TIMER_INTERRUPT: Interrupt = Interrupt::MODEM_LP_TIMER;
pub(crate) const NRT_INTERRUPT: Interrupt = Interrupt::BT_MAC_INT1;
const ROUTE_PRIORITY: Priority = Priority::Priority3;

// Fail compilation if either the reviewed chip policy or the generated PAC
// identity moves independently. No raw interrupt number reaches ESP-HAL.
const _: () = assert!(
    PRIMARY_INTERRUPT as u16 == BluetoothCpuInterruptRoutePolicy::PRIMARY.source().number()
);
const _: () = assert!(
    MODEM_LP_TIMER_INTERRUPT as u16
        == BluetoothCpuInterruptRoutePolicy::MODEM_LP_TIMER
            .source()
            .number()
);
const _: () =
    assert!(NRT_INTERRUPT as u16 == BluetoothCpuInterruptRoutePolicy::NRT.source().number());
const _: () = assert!(BluetoothCpuInterruptRoutePolicy::PRIMARY.priority_level() == 3);
const _: () = assert!(BluetoothCpuInterruptRoutePolicy::MODEM_LP_TIMER.priority_level() == 3);
const _: () = assert!(BluetoothCpuInterruptRoutePolicy::NRT.priority_level() == 3);
const _: () = assert!(ROUTE_PRIORITY as u8 == REQUIRED_PRIORITY_LEVEL);

static INTERRUPT_REGISTERS: Mutex<RefCell<Option<BluetoothInterruptRegistersOwner>>> =
    Mutex::new(RefCell::new(None));
static MODEM_LP_TIMER: Mutex<RefCell<Option<BluetoothModemLpTimerInterruptReadyOwner>>> =
    Mutex::new(RefCell::new(None));

/// Stable process-wide slots for both Bluetooth ISR register owners.
///
/// Constructing this value performs no claim. Publication rejects any second
/// value while either process-wide slot is occupied.
#[derive(Clone, Copy, Debug, Default)]
pub struct EspHalBluetoothInterruptStorage;

impl EspHalBluetoothInterruptStorage {
    /// Construct an unclaimed reference to the process-wide storage boundary.
    pub const fn new() -> Self {
        Self
    }
}

/// Affine proof that both Bluetooth owners remain in stable ISR storage.
///
/// Dropping the lease is fail-stop: the owners remain published because no
/// verified live-route shutdown and Controller teardown path exists yet.
#[must_use = "published Bluetooth ISR owners remain process-wide until verified teardown"]
pub struct PublishedEspHalBluetoothInterruptOwners {
    _private: (),
}

impl BluetoothInterruptOwnerStorage for EspHalBluetoothInterruptStorage {
    type Published = PublishedEspHalBluetoothInterruptOwners;
    type Error = EspHalBluetoothInterruptStorageError;

    fn publish(
        self,
        interrupts: BluetoothInterruptRegistersOwner,
        timer: BluetoothModemLpTimerInterruptReadyOwner,
    ) -> Result<
        Self::Published,
        (
            Self::Error,
            Self,
            BluetoothInterruptRegistersOwner,
            BluetoothModemLpTimerInterruptReadyOwner,
        ),
    > {
        critical_section::with(|critical_section| {
            let mut interrupt_slot = INTERRUPT_REGISTERS.borrow_ref_mut(critical_section);
            let mut timer_slot = MODEM_LP_TIMER.borrow_ref_mut(critical_section);
            match validate_interrupt_storage(interrupt_slot.is_some(), timer_slot.is_some()) {
                Ok(()) => {
                    *interrupt_slot = Some(interrupts);
                    *timer_slot = Some(timer);
                    Ok(PublishedEspHalBluetoothInterruptOwners { _private: () })
                }
                Err(error) => Err((error, self, interrupts, timer)),
            }
        })
    }
}

/// Proof that all Bluetooth routes were bound on the same CPU core.
///
/// The value must stay owned by the future interrupt epoch until all routes
/// have been disabled. It intentionally exposes no individual-line teardown.
#[must_use = "all Bluetooth CPU routes must be disabled before ISR storage is recovered"]
pub struct BoundBluetoothInterruptRoutes {
    core: Cpu,
}

/// Bind the complete primary/modem-timer/NRT set using typed PAC identities.
///
/// Every handler priority is checked before the first interrupt is enabled,
/// so an invalid set leaves the interrupt matrix unchanged. The caller must
/// already have published the shared register owner and must retain it until
/// [`BoundBluetoothInterruptRoutes::disable`] returns successfully.
pub(crate) fn bind(
    primary_handler: InterruptHandler,
    modem_lp_timer_handler: InterruptHandler,
    nrt_handler: InterruptHandler,
) -> Result<BoundBluetoothInterruptRoutes, BluetoothInterruptRouteError> {
    validate_route_priorities(
        primary_handler.priority() as u8,
        modem_lp_timer_handler.priority() as u8,
        nrt_handler.priority() as u8,
    )?;

    let core = Cpu::current();
    interrupt::bind_handler(PRIMARY_INTERRUPT, primary_handler);
    interrupt::bind_handler(MODEM_LP_TIMER_INTERRUPT, modem_lp_timer_handler);
    interrupt::bind_handler(NRT_INTERRUPT, nrt_handler);
    Ok(BoundBluetoothInterruptRoutes { core })
}

impl BoundBluetoothInterruptRoutes {
    /// Disable all routes on the core where the set was installed.
    ///
    /// The timer is closed first so no deadline callback can enter while the
    /// MAC routes are being quiesced. NRT follows because its opaque
    /// acknowledgement path has no controller-side baseline mask. Teardown on
    /// any other core returns the intact route owner without changing the
    /// matrix. After success, no handler can begin a new same-core epoch and
    /// the caller may recover shared storage.
    pub(crate) fn disable(
        self,
    ) -> Result<(), (BluetoothInterruptRouteError, BoundBluetoothInterruptRoutes)> {
        if let Err(error) = validate_quiesce_core(Cpu::current() == self.core) {
            return Err((error, self));
        }
        interrupt::disable(self.core, MODEM_LP_TIMER_INTERRUPT);
        interrupt::disable(self.core, NRT_INTERRUPT);
        interrupt::disable(self.core, PRIMARY_INTERRUPT);
        Ok(())
    }
}
