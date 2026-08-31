//! Platform-neutral validation for the three ESP32-S31 Bluetooth CPU routes.

#![forbid(unsafe_code)]

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspHalBluetoothInterruptRouteError {
    /// A complete Bluetooth route epoch is already live process-wide.
    AlreadyBound,
    /// No complete Bluetooth route epoch is currently live.
    Inactive,
    /// Disable was attempted away from the route epoch's binding core.
    WrongCore,
}

/// Why both Bluetooth ISR owners could not be published atomically.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspHalBluetoothInterruptStorageError {
    /// Both slots already retain a preceding powered Controller epoch.
    AlreadyPublished,
    /// Exactly one slot was occupied, so the process-wide invariant is broken.
    StorageInvariant,
}

/// Validate both process-wide slots before either owner is moved into storage.
pub(crate) const fn validate_interrupt_storage(
    interrupt_occupied: bool,
    timer_occupied: bool,
) -> Result<(), EspHalBluetoothInterruptStorageError> {
    match (interrupt_occupied, timer_occupied) {
        (false, false) => Ok(()),
        (true, true) => Err(EspHalBluetoothInterruptStorageError::AlreadyPublished),
        _ => Err(EspHalBluetoothInterruptStorageError::StorageInvariant),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BluetoothModemLpTimerStoragePhase {
    Missing,
    Ready,
    SoftwarePending,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BluetoothModemLpTimerInterruptAdmission {
    Unavailable,
    ServiceRegisters,
    AwaitSoftware,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BluetoothModemLpTimerTaskTakeAdmission {
    Missing,
    NotSoftwarePending,
    Acquire,
}

pub(crate) const fn classify_modem_lp_timer_interrupt(
    phase: BluetoothModemLpTimerStoragePhase,
) -> BluetoothModemLpTimerInterruptAdmission {
    match phase {
        BluetoothModemLpTimerStoragePhase::Missing => {
            BluetoothModemLpTimerInterruptAdmission::Unavailable
        }
        BluetoothModemLpTimerStoragePhase::Ready => {
            BluetoothModemLpTimerInterruptAdmission::ServiceRegisters
        }
        BluetoothModemLpTimerStoragePhase::SoftwarePending => {
            BluetoothModemLpTimerInterruptAdmission::AwaitSoftware
        }
    }
}

pub(crate) const fn classify_modem_lp_timer_task_take(
    phase: BluetoothModemLpTimerStoragePhase,
) -> BluetoothModemLpTimerTaskTakeAdmission {
    match phase {
        BluetoothModemLpTimerStoragePhase::Missing => {
            BluetoothModemLpTimerTaskTakeAdmission::Missing
        }
        BluetoothModemLpTimerStoragePhase::Ready => {
            BluetoothModemLpTimerTaskTakeAdmission::NotSoftwarePending
        }
        BluetoothModemLpTimerStoragePhase::SoftwarePending => {
            BluetoothModemLpTimerTaskTakeAdmission::Acquire
        }
    }
}

pub(crate) const fn ready_owner_restore_is_admitted(slot_occupied: bool) -> bool {
    !slot_occupied
}

pub(crate) fn service_stable_owner<Owner, Output>(
    slot: &mut Option<Owner>,
    service: impl FnOnce(&mut Owner) -> Output,
) -> Option<Output> {
    slot.as_mut().map(service)
}

/// Host-testable ownership model for one process-wide interrupt-route epoch.
///
/// The target adapter stores the dispatch function beside the same binding
/// core. This model owns the transition policy without depending on ESP-HAL.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BluetoothInterruptRouteState<Core> {
    bound_core: Option<Core>,
}

impl<Core: Copy + Eq> BluetoothInterruptRouteState<Core> {
    #[cfg(test)]
    pub(crate) const fn inactive() -> Self {
        Self { bound_core: None }
    }

    pub(crate) const fn from_bound_core(bound_core: Option<Core>) -> Self {
        Self { bound_core }
    }

    pub(crate) fn bind(&mut self, core: Core) -> Result<(), EspHalBluetoothInterruptRouteError> {
        if self.bound_core.is_some() {
            return Err(EspHalBluetoothInterruptRouteError::AlreadyBound);
        }
        self.bound_core = Some(core);
        Ok(())
    }

    #[cfg(test)]
    pub(crate) const fn dispatch_is_live(&self) -> bool {
        self.bound_core.is_some()
    }

    pub(crate) fn disable(
        &mut self,
        current_core: Core,
    ) -> Result<(), EspHalBluetoothInterruptRouteError> {
        let Some(bound_core) = self.bound_core else {
            return Err(EspHalBluetoothInterruptRouteError::Inactive);
        };
        if current_core != bound_core {
            return Err(EspHalBluetoothInterruptRouteError::WrongCore);
        }
        self.bound_core = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BluetoothInterruptRouteState, BluetoothModemLpTimerInterruptAdmission,
        BluetoothModemLpTimerStoragePhase, BluetoothModemLpTimerTaskTakeAdmission,
        EspHalBluetoothInterruptRouteError, EspHalBluetoothInterruptStorageError,
        classify_modem_lp_timer_interrupt, classify_modem_lp_timer_task_take,
        ready_owner_restore_is_admitted, service_stable_owner, validate_interrupt_storage,
    };

    #[test]
    fn route_epoch_binds_once_and_dispatches_only_while_live() {
        let mut state = BluetoothInterruptRouteState::from_bound_core(None);
        assert!(!state.dispatch_is_live());
        assert_eq!(state.bind(0_u8), Ok(()));
        assert!(state.dispatch_is_live());
        assert_eq!(
            state.bind(0),
            Err(EspHalBluetoothInterruptRouteError::AlreadyBound)
        );
    }

    #[test]
    fn route_epoch_disable_is_same_core_and_consumes_live_dispatch() {
        let mut state = BluetoothInterruptRouteState::inactive();
        assert_eq!(state.bind(1_u8), Ok(()));
        assert_eq!(
            state.disable(0),
            Err(EspHalBluetoothInterruptRouteError::WrongCore)
        );
        assert!(state.dispatch_is_live());
        assert_eq!(state.disable(1), Ok(()));
        assert!(!state.dispatch_is_live());
        assert_eq!(
            state.disable(1),
            Err(EspHalBluetoothInterruptRouteError::Inactive)
        );
    }

    #[test]
    fn both_isr_owner_slots_publish_or_reject_as_one_invariant() {
        assert_eq!(validate_interrupt_storage(false, false), Ok(()));
        assert_eq!(
            validate_interrupt_storage(true, true),
            Err(EspHalBluetoothInterruptStorageError::AlreadyPublished)
        );
        assert_eq!(
            validate_interrupt_storage(true, false),
            Err(EspHalBluetoothInterruptStorageError::StorageInvariant)
        );
        assert_eq!(
            validate_interrupt_storage(false, true),
            Err(EspHalBluetoothInterruptStorageError::StorageInvariant)
        );
    }

    #[test]
    fn source_127_register_entry_stops_while_software_owns_the_timer() {
        assert_eq!(
            classify_modem_lp_timer_interrupt(BluetoothModemLpTimerStoragePhase::Missing),
            BluetoothModemLpTimerInterruptAdmission::Unavailable
        );
        assert_eq!(
            classify_modem_lp_timer_interrupt(BluetoothModemLpTimerStoragePhase::Ready),
            BluetoothModemLpTimerInterruptAdmission::ServiceRegisters
        );
        assert_eq!(
            classify_modem_lp_timer_interrupt(BluetoothModemLpTimerStoragePhase::SoftwarePending),
            BluetoothModemLpTimerInterruptAdmission::AwaitSoftware
        );
    }

    #[test]
    fn source_127_task_can_take_only_pending_work_and_restore_only_into_the_empty_slot() {
        assert_eq!(
            classify_modem_lp_timer_task_take(BluetoothModemLpTimerStoragePhase::Missing),
            BluetoothModemLpTimerTaskTakeAdmission::Missing
        );
        assert_eq!(
            classify_modem_lp_timer_task_take(BluetoothModemLpTimerStoragePhase::Ready),
            BluetoothModemLpTimerTaskTakeAdmission::NotSoftwarePending
        );
        assert_eq!(
            classify_modem_lp_timer_task_take(BluetoothModemLpTimerStoragePhase::SoftwarePending),
            BluetoothModemLpTimerTaskTakeAdmission::Acquire
        );
        assert!(ready_owner_restore_is_admitted(false));
        assert!(!ready_owner_restore_is_admitted(true));
    }

    #[test]
    fn shared_interrupt_service_retains_and_reuses_the_stable_owner() {
        let mut slot = Some(0_u8);

        assert_eq!(
            service_stable_owner(&mut slot, |owner| {
                *owner += 1;
                *owner
            }),
            Some(1)
        );
        assert_eq!(
            service_stable_owner(&mut slot, |owner| {
                *owner += 1;
                *owner
            }),
            Some(2)
        );
        assert_eq!(slot, Some(2));

        let mut missing: Option<u8> = None;
        assert_eq!(
            service_stable_owner(&mut missing, |_| panic!(
                "missing owner must not be serviced"
            )),
            None
        );
    }
}
