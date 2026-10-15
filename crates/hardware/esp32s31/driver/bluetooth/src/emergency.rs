//! Independently owned emergency authority and counted ordinary task borrows.
//!
//! Quarantine is permanent for both scheduler outcomes. It neither resets
//! shared modules nor returns memory, PHY membership, coexistence or clocks.

pub use oer_esp32s31_hal::bluetooth::emergency::{
    EmergencyClosed, EmergencyDrained, EmergencyEntry, EmergencyGate, EmergencyOutcome,
    EmergencyUnconfirmed,
};

/// Platform boundary for the installation's three BLE CPU routes.
///
/// # Safety
///
/// Every ISR, register-owner access and task callback of this controller
/// installation must retain an entry in the returned gate through its last
/// access and wake publication. Closing that gate must permanently prevent
/// new accesses and route rebinding. The stored register owners and all
/// hardware-visible backing remain durable after closure. These obligations
/// serialize the emergency PAC views with the original interrupt views.
#[allow(
    unsafe_code,
    reason = "the implementation proves exclusion of aliased emergency PAC views"
)]
pub unsafe trait EmergencyInterruptStorage {
    /// One gate shared by this epoch's ISR and task-side hardware operations.
    fn emergency_gate(&self) -> &EmergencyGate;
    /// Close only routes 127, 133 and 124, in that order, on their bound CPU.
    /// The caller has already closed admission. This does not wait for an ISR.
    fn mask_emergency_routes(&self);
}

/// Synchronous, idempotent emergency authority kept outside the driver.
pub trait PortEmergencyStop {
    /// Close admission, mask routes, attempt bounded stopping and quarantine.
    fn stop(&mut self) -> EmergencyOutcome;
}

/// CPU observation budget for joining an entered ISR/task operation.
///
/// This is an open implementation policy, using the same finite ceiling as
/// the 10,000-attempt vendor command-drain budget. It is not a vendor claim
/// about CPU/ISR latency; exhausting it retains the complete installation.
pub const EMERGENCY_ENTRY_OBSERVATIONS: u32 = 10_000;

#[cfg(any(target_arch = "riscv32", test))]
enum StopState {
    Armed,
    Quarantined(EmergencyOutcome),
}

#[cfg(any(target_arch = "riscv32", test))]
impl StopState {
    fn stop(
        &mut self,
        gate: &EmergencyGate,
        mask_routes: impl FnOnce(),
        stop_registers: impl FnOnce(&EmergencyDrained<'_>) -> EmergencyOutcome,
    ) -> EmergencyOutcome {
        if let Self::Quarantined(outcome) = *self {
            return outcome;
        }
        let closed = gate.close();
        mask_routes();
        let outcome = match closed.drain(EMERGENCY_ENTRY_OBSERVATIONS) {
            Err(cause) => EmergencyOutcome::Unconfirmed(cause),
            Ok(drained) => stop_registers(&drained),
        };
        *self = Self::Quarantined(outcome);
        outcome
    }
}

#[cfg(target_arch = "riscv32")]
mod live {
    use super::*;
    use crate::{
        runtime_resources::ControllerPoweredTaskRuntime, scheduler::DIAGNOSTIC_READ_BUDGET,
    };
    use core::ops::{Deref, DerefMut};
    use oer_esp32s31_hal::bluetooth::emergency::EmergencyRegisters;

    /// Affine emergency owner minted with exactly one ordinary task owner.
    #[must_use = "keep the emergency owner in the backend, outside the driver"]
    pub struct ControllerEmergencyStop<'a, S> {
        registers: EmergencyRegisters,
        storage: &'a S,
        gate: &'a EmergencyGate,
        state: StopState,
    }

    impl<S: EmergencyInterruptStorage> PortEmergencyStop for ControllerEmergencyStop<'_, S> {
        fn stop(&mut self) -> EmergencyOutcome {
            self.state.stop(
                self.gate,
                || self.storage.mask_emergency_routes(),
                |drained| self.registers.stop(drained, DIAGNOSTIC_READ_BUDGET),
            )
        }
    }

    /// Ordinary task registers can be borrowed only under counted admission.
    #[must_use = "the task rejoins only its own unused emergency authority"]
    pub struct PortController<'a> {
        task: ControllerPoweredTaskRuntime<'a>,
        gate: &'a EmergencyGate,
    }

    /// A finite counted borrow; its admission cannot outlive the task borrow.
    pub struct PortControllerBorrow<'p, 'a> {
        task: &'p mut ControllerPoweredTaskRuntime<'a>,
        _entry: EmergencyEntry<'p>,
    }
    impl<'a> Deref for PortControllerBorrow<'_, 'a> {
        type Target = ControllerPoweredTaskRuntime<'a>;
        fn deref(&self) -> &Self::Target {
            self.task
        }
    }
    impl DerefMut for PortControllerBorrow<'_, '_> {
        fn deref_mut(&mut self) -> &mut Self::Target {
            self.task
        }
    }

    impl<'a> ControllerPoweredTaskRuntime<'a> {
        /// Hide the consumed ordinary owner behind the storage's admission and
        /// mint its independent emergency owner. No register is read or written.
        #[allow(
            unsafe_code,
            reason = "the consumed task is exposed only by counted borrows or paired reunion"
        )]
        pub fn separate_emergency<S: EmergencyInterruptStorage>(
            mut self,
            storage: &'a S,
        ) -> (PortController<'a>, ControllerEmergencyStop<'a, S>) {
            // SAFETY: the original task is moved into the private PortController.
            // All its borrows retain a gate entry. The emergency owner closes
            // and drains that gate before using its views; paired reunion is
            // rejected forever after closure, and consumes the views otherwise.
            let registers = unsafe { self.task.emergency_registers() };
            let gate = storage.emergency_gate();
            (
                PortController { task: self, gate },
                ControllerEmergencyStop {
                    registers,
                    storage,
                    gate,
                    state: StopState::Armed,
                },
            )
        }
    }

    impl<'a> PortController<'a> {
        /// Borrow the ordinary task only while admission remains open.
        pub fn enter(&mut self) -> Result<PortControllerBorrow<'_, 'a>, EmergencyClosed> {
            let entry = self.gate.enter()?;
            Ok(PortControllerBorrow {
                task: &mut self.task,
                _entry: entry,
            })
        }

        /// Immutable scheduler policy; this reads no hardware or DMA memory.
        pub fn scheduler_config(&self) -> crate::scheduler::SchedulerSoftwareConfig {
            self.task.scheduler_config()
        }

        /// Immutable time scale; this reads no hardware or DMA memory.
        pub fn controller_time_scale(
            &self,
        ) -> oer_esp32s31_hal::bluetooth::BluetoothControllerTimeScale {
            self.task.controller_time_scale()
        }

        /// Recover ordinary ownership only with the same unused emergency
        /// owner. A quarantined or mismatched pair retains both owners.
        #[allow(
            clippy::result_large_err,
            reason = "rejected reunion retains both affine owners"
        )]
        pub fn reunite<S: EmergencyInterruptStorage>(
            self,
            emergency: ControllerEmergencyStop<'a, S>,
        ) -> Result<ControllerPoweredTaskRuntime<'a>, (Self, ControllerEmergencyStop<'a, S>)>
        {
            if !core::ptr::eq(self.gate, emergency.gate)
                || self.gate.is_closed()
                || !matches!(emergency.state, StopState::Armed)
            {
                return Err((self, emergency));
            }
            // Consuming the emergency views before returning the ordinary task
            // ends their exclusive-admission obligation. No reset is performed.
            drop(emergency);
            Ok(self.task)
        }
    }
}

#[cfg(target_arch = "riscv32")]
pub use live::{ControllerEmergencyStop, PortController, PortControllerBorrow};

#[cfg(test)]
mod tests;
