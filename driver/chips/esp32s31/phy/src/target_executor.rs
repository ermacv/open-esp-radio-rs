//! ESP32-S31 target executors for finite PHY hardware edges.
//!
//! The recovered PHY transitions describe what must happen, while this module
//! owns the common polling contract for the target.  Executor-specific time is
//! injected through [`PhyAsyncDelay`], so neither Embassy nor an RTOS becomes a
//! dependency of the PHY crate.

use core::future::Future;

use open_esp_radio_esp32s31_hal::SharedPhyAccess;

use crate::{
    HARDWARE_EDGE_LIMIT,
    phy_bluetooth::{
        PhyBluetoothI2cAction, PhyBluetoothI2cBinding, PhyBluetoothPbusBinding,
        PhyBluetoothTxPowerCompletion,
    },
    phy_channel::{PhyChipChannelCompletion, PhyChipChannelI2cBinding},
    phy_cold::{PhyColdI2cAction, PhyColdI2cError, PhyColdI2cObservation},
    phy_dcode::{PhyDcodeCompletion, PhyDcodeI2cBinding},
    phy_i2c::{MaskedI2cWriteBinding, MaskedI2cWriteCompletion},
    phy_pbus::PhyPbusHardwareObservation,
    phy_register::{PhyRegisterCompletion, PhyRegisterFinalI2cBinding},
    phy_rfpll::{RfpllFrequencyCompletion, RfpllFrequencyI2cBinding},
    phy_rx_dco::{PhyRxDcoCompletion, PhyRxDcoPbusBinding},
    phy_rx_gain::{PhyRxGainPublishCompletion, PhyRxGainPublishPbusBinding},
    phy_rx_gain_cal::{
        PhyRxDcCalibrationCompletion, PhyRxDcCalibrationPbusBinding, PhyRxGainDcCompletion,
        PhyRxGainDcPbusBinding,
    },
    phy_rx_saturation::{PhyRxSaturationCompletion, PhyRxSaturationPbusBinding},
    phy_rxiq::{
        PhyRxIqAdjustedTxCompletion, PhyRxIqAdjustedTxI2cBinding, PhyRxIqGainCompletion,
        PhyRxIqGainI2cBinding, PhyRxIqGainPbusBinding, PhyRxIqInitCompletion,
        PhyRxIqInitI2cBinding, PhyRxIqInitPbusBinding,
    },
    phy_temperature::{PhyTemperatureCompletion, PhyTemperatureI2cBinding},
    phy_tx_cal::{PhyTxCalibrationEnvironmentCompletion, PhyTxCalibrationEnvironmentPbusBinding},
    phy_tx_power::{PhyTxPowerCompletion, PhyTxPowerI2cBinding},
    phy_txdc_pwdet::{
        PhyTxDcPwdetCompletion, PhyTxDcPwdetPbusBinding, PhyTxDcPwdetSearchCompletion,
        PhyTxDcPwdetSearchPbusBinding,
    },
    phy_txiq::{
        PhyTxIqCalibrationCompletion, PhyTxIqInitCompletion, PhyTxIqInitI2cBinding,
        PhyTxIqPbusBinding,
    },
};

/// Executor-independent asynchronous delay used by target PHY operations.
///
/// The delay has no borrowed state. Hardware integrations can therefore use a
/// zero-sized implementation backed by an executor timer or a dedicated
/// hardware timer without storing an executor object in the radio owner.
pub trait PhyAsyncDelay {
    fn after_micros(micros: u64) -> impl Future<Output = ()>;
}

/// Failure while completing a finite target PHY hardware operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTargetPortError {
    HardwareEdgeTimedOut,
    HardwareCapabilityUnavailable,
    HardwareInvariant,
    RfOperationLimit,
    UnexpectedBinding,
}

/// Complete the registration tail's bounded read-only PHY-I2C transaction.
///
/// This leaf is deliberately distinct from the general I2C executor: the
/// recovered registration tail permits only a read, and exhausting the
/// deadline produces a typed transition completion rather than an executor
/// error. Applications must not reinterpret a write action or select another
/// polling bound at this boundary.
pub async fn complete_final_i2c<D: PhyAsyncDelay>(
    mut binding: PhyRegisterFinalI2cBinding,
    registers: &mut impl SharedPhyAccess,
) -> Result<PhyRegisterCompletion, PhyTargetPortError> {
    for _ in 0..HARDWARE_EDGE_LIMIT {
        match binding.action() {
            PhyColdI2cAction::StartRead { .. } => match binding.start_target(registers) {
                Ok(()) => {}
                Err(PhyColdI2cError::BusyAtStart) => D::after_micros(1).await,
                Err(_) => return Err(PhyTargetPortError::UnexpectedBinding),
            },
            PhyColdI2cAction::AwaitReadCompletionEdge { .. } => {
                D::after_micros(1).await;
                match binding
                    .observe_target_edge(registers)
                    .map_err(|_| PhyTargetPortError::UnexpectedBinding)?
                {
                    PhyColdI2cObservation::EdgeConsumed | PhyColdI2cObservation::StillPending => {}
                }
            }
            PhyColdI2cAction::Complete(_) => {
                return binding
                    .into_completion()
                    .map_err(|_| PhyTargetPortError::UnexpectedBinding);
            }
            PhyColdI2cAction::StartWrite { .. }
            | PhyColdI2cAction::AwaitWriteCompletionEdge { .. } => {
                return Err(PhyTargetPortError::UnexpectedBinding);
            }
        }
    }
    Ok(binding.into_deadline_completion())
}

macro_rules! define_i2c_executor {
    ($function:ident, $binding:ty, $completion:ty) => {
        pub async fn $function<D: PhyAsyncDelay>(
            mut binding: $binding,
            registers: &mut impl SharedPhyAccess,
        ) -> Result<$completion, PhyTargetPortError> {
            for _ in 0..HARDWARE_EDGE_LIMIT {
                match binding.action() {
                    PhyColdI2cAction::StartRead { .. } | PhyColdI2cAction::StartWrite { .. } => {
                        match binding.start_target(registers) {
                            Ok(()) => {}
                            Err(PhyColdI2cError::BusyAtStart) => D::after_micros(1).await,
                            Err(_) => return Err(PhyTargetPortError::UnexpectedBinding),
                        }
                    }
                    PhyColdI2cAction::AwaitReadCompletionEdge { .. }
                    | PhyColdI2cAction::AwaitWriteCompletionEdge { .. } => {
                        D::after_micros(1).await;
                        match binding
                            .observe_target_edge(registers)
                            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?
                        {
                            PhyColdI2cObservation::EdgeConsumed
                            | PhyColdI2cObservation::StillPending => {}
                        }
                    }
                    PhyColdI2cAction::Complete(_) => {
                        return binding
                            .into_completion()
                            .map_err(|_| PhyTargetPortError::UnexpectedBinding);
                    }
                }
            }
            Err(PhyTargetPortError::HardwareEdgeTimedOut)
        }
    };
}

macro_rules! define_pbus_executor {
    ($function:ident, $binding:ty, $completion:ty) => {
        pub async fn $function<D: PhyAsyncDelay>(
            mut binding: $binding,
            registers: &mut impl SharedPhyAccess,
        ) -> Result<$completion, PhyTargetPortError> {
            let mut started = false;
            for _ in 0..HARDWARE_EDGE_LIMIT {
                if binding.start_target(registers).is_ok() {
                    started = true;
                    break;
                }
                D::after_micros(1).await;
            }
            if !started {
                return Err(PhyTargetPortError::HardwareEdgeTimedOut);
            }
            for _ in 0..HARDWARE_EDGE_LIMIT {
                D::after_micros(1).await;
                match binding
                    .observe_target_edge(registers)
                    .map_err(|_| PhyTargetPortError::UnexpectedBinding)?
                {
                    PhyPbusHardwareObservation::EdgeConsumed => {
                        return binding
                            .into_completion()
                            .map_err(|_| PhyTargetPortError::UnexpectedBinding);
                    }
                    PhyPbusHardwareObservation::StillPending => {}
                }
            }
            Err(PhyTargetPortError::HardwareEdgeTimedOut)
        }
    };
}

// These bindings model a PBus timeout as an ordinary completion consumed by
// their parent transition. Keep the finite wait and the timeout conversion in
// the driver: an application must not be able to silently select a different
// polling bound or turn the recovered fallback path into an executor error.
macro_rules! define_timeout_pbus_executor {
    ($function:ident, $binding:ty, $completion:ty) => {
        pub async fn $function<D: PhyAsyncDelay>(
            mut binding: $binding,
            registers: &mut impl SharedPhyAccess,
        ) -> Result<$completion, PhyTargetPortError> {
            let mut started = false;
            for _ in 0..HARDWARE_EDGE_LIMIT {
                if binding.start_target(registers).is_ok() {
                    started = true;
                    break;
                }
                D::after_micros(1).await;
            }
            if !started {
                return Ok(binding.into_timeout_completion());
            }

            for _ in 0..HARDWARE_EDGE_LIMIT {
                D::after_micros(1).await;
                match binding
                    .observe_target_edge(registers)
                    .map_err(|_| PhyTargetPortError::UnexpectedBinding)?
                {
                    PhyPbusHardwareObservation::EdgeConsumed => {
                        return binding
                            .into_completion()
                            .map_err(|_| PhyTargetPortError::UnexpectedBinding);
                    }
                    PhyPbusHardwareObservation::StillPending => {}
                }
            }
            Ok(binding.into_timeout_completion())
        }
    };
}

define_i2c_executor!(
    complete_rfpll_i2c,
    RfpllFrequencyI2cBinding,
    RfpllFrequencyCompletion
);
define_i2c_executor!(
    complete_masked_i2c,
    MaskedI2cWriteBinding,
    MaskedI2cWriteCompletion
);
define_i2c_executor!(
    complete_temperature_i2c,
    PhyTemperatureI2cBinding,
    PhyTemperatureCompletion
);
define_i2c_executor!(
    complete_tx_power_i2c,
    PhyTxPowerI2cBinding,
    PhyTxPowerCompletion
);
pub async fn complete_bluetooth_i2c<D: PhyAsyncDelay>(
    mut binding: PhyBluetoothI2cBinding,
    registers: &mut impl SharedPhyAccess,
) -> Result<PhyBluetoothTxPowerCompletion, PhyTargetPortError> {
    for _ in 0..HARDWARE_EDGE_LIMIT {
        match binding.action() {
            PhyBluetoothI2cAction::StartCommand => match binding.start_target(registers) {
                Ok(()) => {}
                Err(PhyColdI2cError::BusyAtStart) => D::after_micros(1).await,
                Err(_) => return Err(PhyTargetPortError::HardwareInvariant),
            },
            PhyBluetoothI2cAction::AwaitCompletionEdge => {
                D::after_micros(1).await;
                match binding
                    .observe_target_edge(registers)
                    .map_err(|_| PhyTargetPortError::HardwareInvariant)?
                {
                    PhyColdI2cObservation::EdgeConsumed | PhyColdI2cObservation::StillPending => {}
                }
            }
            PhyBluetoothI2cAction::Complete => {
                return binding
                    .into_completion()
                    .map_err(|_| PhyTargetPortError::UnexpectedBinding);
            }
        }
    }
    Err(PhyTargetPortError::HardwareEdgeTimedOut)
}
define_i2c_executor!(complete_dcode_i2c, PhyDcodeI2cBinding, PhyDcodeCompletion);
define_i2c_executor!(
    complete_txiq_init_i2c,
    PhyTxIqInitI2cBinding,
    PhyTxIqInitCompletion
);
define_i2c_executor!(
    complete_rxiq_adjusted_tx_i2c,
    PhyRxIqAdjustedTxI2cBinding,
    PhyRxIqAdjustedTxCompletion
);
define_i2c_executor!(
    complete_rxiq_gain_i2c,
    PhyRxIqGainI2cBinding,
    PhyRxIqGainCompletion
);
define_i2c_executor!(
    complete_rxiq_init_i2c,
    PhyRxIqInitI2cBinding,
    PhyRxIqInitCompletion
);
define_i2c_executor!(
    complete_channel_i2c,
    PhyChipChannelI2cBinding,
    PhyChipChannelCompletion
);
define_pbus_executor!(
    complete_tx_calibration_environment_pbus,
    PhyTxCalibrationEnvironmentPbusBinding,
    PhyTxCalibrationEnvironmentCompletion
);
define_timeout_pbus_executor!(
    complete_bluetooth_pbus,
    PhyBluetoothPbusBinding,
    PhyBluetoothTxPowerCompletion
);
define_timeout_pbus_executor!(
    complete_tx_dc_pwdet_search_pbus,
    PhyTxDcPwdetSearchPbusBinding,
    PhyTxDcPwdetSearchCompletion
);
define_timeout_pbus_executor!(
    complete_tx_dc_pwdet_pbus,
    PhyTxDcPwdetPbusBinding,
    PhyTxDcPwdetCompletion
);
define_timeout_pbus_executor!(
    complete_txiq_pbus,
    PhyTxIqPbusBinding,
    PhyTxIqCalibrationCompletion
);
define_timeout_pbus_executor!(
    complete_rx_dco_pbus,
    PhyRxDcoPbusBinding,
    PhyRxDcoCompletion
);
define_timeout_pbus_executor!(
    complete_rxiq_gain_pbus,
    PhyRxIqGainPbusBinding,
    PhyRxIqGainCompletion
);
define_timeout_pbus_executor!(
    complete_rxiq_init_pbus,
    PhyRxIqInitPbusBinding,
    PhyRxIqInitCompletion
);
define_timeout_pbus_executor!(
    complete_rx_saturation_pbus,
    PhyRxSaturationPbusBinding,
    PhyRxSaturationCompletion
);
define_timeout_pbus_executor!(
    complete_rx_dc_calibration_pbus,
    PhyRxDcCalibrationPbusBinding,
    PhyRxDcCalibrationCompletion
);
define_timeout_pbus_executor!(
    complete_rx_gain_dc_pbus,
    PhyRxGainDcPbusBinding,
    PhyRxGainDcCompletion
);
define_timeout_pbus_executor!(
    complete_rx_gain_publish_pbus,
    PhyRxGainPublishPbusBinding,
    PhyRxGainPublishCompletion
);
