//! Platform-neutral validation for the three ESP32-S31 Bluetooth CPU routes.

#![forbid(unsafe_code)]

pub(crate) const REQUIRED_PRIORITY_LEVEL: u8 = 3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BluetoothInterruptRouteError {
    PrimaryPriority,
    ModemLpTimerPriority,
    NrtPriority,
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

/// Validate the complete route set before any route can be installed.
pub(crate) const fn validate_route_priorities(
    primary: u8,
    modem_lp_timer: u8,
    nrt: u8,
) -> Result<(), BluetoothInterruptRouteError> {
    if primary != REQUIRED_PRIORITY_LEVEL {
        return Err(BluetoothInterruptRouteError::PrimaryPriority);
    }
    if modem_lp_timer != REQUIRED_PRIORITY_LEVEL {
        return Err(BluetoothInterruptRouteError::ModemLpTimerPriority);
    }
    if nrt != REQUIRED_PRIORITY_LEVEL {
        return Err(BluetoothInterruptRouteError::NrtPriority);
    }
    Ok(())
}

/// Reject teardown from a CPU other than the route's binding core.
pub(crate) const fn validate_quiesce_core(
    is_binding_core: bool,
) -> Result<(), BluetoothInterruptRouteError> {
    if !is_binding_core {
        return Err(BluetoothInterruptRouteError::WrongCore);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        BluetoothInterruptRouteError, EspHalBluetoothInterruptStorageError,
        REQUIRED_PRIORITY_LEVEL, validate_interrupt_storage, validate_quiesce_core,
        validate_route_priorities,
    };

    #[test]
    fn complete_level_three_route_set_is_accepted() {
        assert_eq!(
            validate_route_priorities(
                REQUIRED_PRIORITY_LEVEL,
                REQUIRED_PRIORITY_LEVEL,
                REQUIRED_PRIORITY_LEVEL,
            ),
            Ok(())
        );
    }

    #[test]
    fn any_invalid_priority_rejects_the_route_set_before_binding() {
        assert_eq!(
            validate_route_priorities(2, REQUIRED_PRIORITY_LEVEL, REQUIRED_PRIORITY_LEVEL),
            Err(BluetoothInterruptRouteError::PrimaryPriority)
        );
        assert_eq!(
            validate_route_priorities(REQUIRED_PRIORITY_LEVEL, 2, REQUIRED_PRIORITY_LEVEL),
            Err(BluetoothInterruptRouteError::ModemLpTimerPriority)
        );
        assert_eq!(
            validate_route_priorities(REQUIRED_PRIORITY_LEVEL, REQUIRED_PRIORITY_LEVEL, 2),
            Err(BluetoothInterruptRouteError::NrtPriority)
        );
    }

    #[test]
    fn quiesce_is_affine_to_the_binding_core() {
        assert_eq!(validate_quiesce_core(true), Ok(()));
        assert_eq!(
            validate_quiesce_core(false),
            Err(BluetoothInterruptRouteError::WrongCore)
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
}
