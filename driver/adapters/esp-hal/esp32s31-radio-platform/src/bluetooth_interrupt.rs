//! Typed ESP-HAL routing primitives for all three Bluetooth interrupts.
//!
//! This module deliberately stops below a live interrupt epoch. The caller
//! must first publish the unique Bluetooth interrupt-register owner in stable
//! ISR storage; primary semantic fault/dynamic and opaque NRT acknowledgement plus the scheduler-list
//! drain remain incomplete. Keeping these primitives crate-private makes the
//! PAC-to-ESP-HAL mapping compile checked without exposing a safe API that
//! could enable any route prematurely.

#![forbid(unsafe_code)]
#![allow(
    dead_code,
    reason = "typed route primitives await stable ISR storage and complete three-source dispatch"
)]

use esp_hal::{
    interrupt::{self, InterruptHandler, Priority},
    peripherals::Interrupt,
    system::Cpu,
};
use open_esp_radio_esp32s31_bluetooth::BluetoothCpuInterruptRoutePolicy;

use crate::bluetooth_route_policy::{
    BluetoothInterruptRouteError, REQUIRED_PRIORITY_LEVEL, validate_quiesce_core,
    validate_route_priorities,
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
