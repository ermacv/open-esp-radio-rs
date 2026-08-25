//! Platform-neutral validation for the two ESP32-S31 Bluetooth CPU routes.

#![forbid(unsafe_code)]

pub(crate) const REQUIRED_PRIORITY_LEVEL: u8 = 3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BluetoothInterruptRouteError {
    PrimaryPriority,
    NrtPriority,
    WrongCore,
}

/// Validate the complete pair before either route can be installed.
pub(crate) const fn validate_route_priorities(
    primary: u8,
    nrt: u8,
) -> Result<(), BluetoothInterruptRouteError> {
    if primary != REQUIRED_PRIORITY_LEVEL {
        return Err(BluetoothInterruptRouteError::PrimaryPriority);
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
        BluetoothInterruptRouteError, REQUIRED_PRIORITY_LEVEL, validate_quiesce_core,
        validate_route_priorities,
    };

    #[test]
    fn complete_level_three_pair_is_accepted() {
        assert_eq!(
            validate_route_priorities(REQUIRED_PRIORITY_LEVEL, REQUIRED_PRIORITY_LEVEL),
            Ok(())
        );
    }

    #[test]
    fn either_invalid_priority_rejects_the_pair_before_binding() {
        assert_eq!(
            validate_route_priorities(2, REQUIRED_PRIORITY_LEVEL),
            Err(BluetoothInterruptRouteError::PrimaryPriority)
        );
        assert_eq!(
            validate_route_priorities(REQUIRED_PRIORITY_LEVEL, 2),
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
}
