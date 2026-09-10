//! Complete calibration children on the admitted physical PHY partition.
//!
//! These executors borrow the existing maintenance capability. They neither
//! acquire radio access nor publish parent calibration state. A returned child
//! completion must still be accepted by its parent transition.

use super::{PhyTargetObserver, PhyTargetPortError, RF_OPERATION_LIMIT, TargetCompleter};
use crate::{
    target_executor::PhyAsyncDelay,
    tracking::calibration::{
        PhyCalibrationChannelTransition, PhyCalibrationDcodeTransition,
        PhyCalibrationPbusClearTransition, PhyCalibrationRxGainTransition,
        PhyCalibrationTrackingCompletion,
    },
};
use oer_esp32s31_hal::owner::{SharedPhyAccess, SharedPhyContext};

/// Run the bounded PBus-clear child, including readiness and work-mode settling.
/// Once polled, keep the physical maintenance borrow until terminal completion;
/// an error does not establish a safe state for resuming radio traffic.
pub async fn clear_pbus<D: PhyAsyncDelay, O: PhyTargetObserver>(
    mut child: PhyCalibrationPbusClearTransition,
    registers: &mut impl SharedPhyContext,
    observer: &mut O,
) -> Result<PhyCalibrationTrackingCompletion, PhyTargetPortError> {
    for _ in 0..RF_OPERATION_LIMIT {
        child = match child.commit() {
            Ok(completion) => return Ok(completion),
            Err(child) => child,
        };
        let binding = child
            .lower_external()
            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
        let completion = TargetCompleter::<D>::complete_rf(binding, registers, observer).await?;
        child
            .advance_external(completion)
            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
    }
    Err(PhyTargetPortError::RfOperationLimit)
}

/// Run the complete D-code child, retaining its nested PLL and I2C waits.
/// The caller retains the same maintenance and cancellation obligations as PBus clearing.
pub async fn dcode<D: PhyAsyncDelay, P, F: core::future::Future<Output = ()>>(
    mut child: PhyCalibrationDcodeTransition,
    platform: &mut P,
    registers: &mut impl SharedPhyAccess,
    mut delay: impl FnMut(crate::executor::wait::Scope, crate::executor::wait::Kind, u64) -> F,
    mut observe: impl FnMut(bool),
) -> Result<PhyCalibrationTrackingCompletion, PhyTargetPortError> {
    for _ in 0..RF_OPERATION_LIMIT {
        child = match child.commit() {
            Ok(completion) => return Ok(completion),
            Err(child) => child,
        };
        let binding = child
            .lower_external()
            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
        let completion = TargetCompleter::<D>::complete_dcode(
            binding,
            platform,
            registers,
            &mut delay,
            &mut observe,
        )
        .await?;
        child
            .advance(completion)
            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
    }
    Err(PhyTargetPortError::RfOperationLimit)
}

/// Run RX-DC calibration and publish both gain banks using the same admitted owner.
pub async fn rx_gain<D: PhyAsyncDelay, P>(
    mut child: PhyCalibrationRxGainTransition,
    platform: &mut P,
    registers: &mut impl SharedPhyContext,
) -> Result<PhyCalibrationTrackingCompletion, PhyTargetPortError> {
    rx_gain_init::<D, _>(child.transition_mut(), platform, registers).await?;
    child
        .commit()
        .map_err(|_| PhyTargetPortError::UnexpectedBinding)
}

/// Execute an RX-gain root through its terminal edge. Its caller owns state
/// publication; success here may still contain a typed calibration failure.
pub async fn rx_gain_init<D: PhyAsyncDelay, P>(
    child: &mut crate::rx::gain::PhyRxGainInitTransition,
    platform: &mut P,
    registers: &mut impl SharedPhyContext,
) -> Result<(), PhyTargetPortError> {
    use crate::rx::gain::{PhyRxGainInitAction, PhyRxGainInitExternalBinding};
    for _ in 0..RF_OPERATION_LIMIT {
        let action = child.action();
        if matches!(
            action,
            PhyRxGainInitAction::Complete(_) | PhyRxGainInitAction::Failed(_)
        ) {
            return Ok(());
        }
        let binding = PhyRxGainInitExternalBinding::lower(action)
            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
        let completion =
            TargetCompleter::<D>::complete_rx_gain(binding, platform, registers).await?;
        child
            .advance(completion)
            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
    }
    Err(PhyTargetPortError::RfOperationLimit)
}

/// Execute the complete TX-DC/PWDET root, including measurement and restoration.
/// The caller retains physical access until terminal completion. Returning an
/// executor error or a typed failed action does not establish a safe RF state.
pub async fn tx_dc_pwdet_init<D: PhyAsyncDelay, O: PhyTargetObserver>(
    child: &mut crate::tx::dc_power_detector::PhyTxDcPwdetTransition,
    registers: &mut impl SharedPhyContext,
    observer: &core::cell::RefCell<&mut O>,
) -> Result<(), PhyTargetPortError> {
    use crate::tx::dc_power_detector::{PhyTxDcPwdetAction, PhyTxDcPwdetExternalBinding};
    for _ in 0..RF_OPERATION_LIMIT {
        let action = child.action();
        if matches!(
            action,
            PhyTxDcPwdetAction::Complete(_) | PhyTxDcPwdetAction::Failed(_)
        ) {
            return Ok(());
        }
        let binding = PhyTxDcPwdetExternalBinding::lower(action)
            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
        let completion =
            TargetCompleter::<D>::complete_tx_dc_pwdet_with(binding, registers, observer).await?;
        child
            .advance(completion)
            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
    }
    Err(PhyTargetPortError::RfOperationLimit)
}

/// Restore the operating channel without acquiring or releasing physical access.
pub async fn channel<D: PhyAsyncDelay, P, O: PhyTargetObserver>(
    mut child: PhyCalibrationChannelTransition,
    platform: &mut P,
    registers: &mut impl SharedPhyAccess,
    observer: &mut O,
) -> Result<PhyCalibrationTrackingCompletion, PhyTargetPortError> {
    for _ in 0..RF_OPERATION_LIMIT {
        child = match child.commit() {
            Ok(completion) => return Ok(completion),
            Err(child) => child,
        };
        let binding = child
            .lower_external()
            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
        let completion =
            TargetCompleter::<D>::complete_channel(binding, platform, registers, observer).await?;
        child
            .advance(completion)
            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
    }
    Err(PhyTargetPortError::RfOperationLimit)
}
