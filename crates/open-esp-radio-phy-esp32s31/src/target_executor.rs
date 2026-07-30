//! ESP32-S31 target executors for finite PHY hardware edges.
//!
//! The recovered PHY transitions describe what must happen, while this module
//! owns the common polling contract for the target.  Executor-specific time is
//! injected through [`PhyAsyncDelay`], so neither Embassy nor an RTOS becomes a
//! dependency of the PHY crate.

use core::future::Future;

use open_esp_radio_hal_esp32s31::{phy_i2c::PhyI2cMasterControl, RadioRegisters};

use crate::{
    phy_channel::{PhyChipChannelCompletion, PhyChipChannelI2cBinding},
    phy_cold::{PhyColdI2cAction, PhyColdI2cError, PhyColdI2cObservation},
    phy_dcode::{PhyDcodeCompletion, PhyDcodeI2cBinding},
    phy_i2c::{MaskedI2cWriteBinding, MaskedI2cWriteCompletion},
    phy_pbus::PhyPbusHardwareObservation,
    phy_rfpll::{RfpllFrequencyCompletion, RfpllFrequencyI2cBinding},
    phy_rxiq::{
        PhyRxIqAdjustedTxCompletion, PhyRxIqAdjustedTxI2cBinding, PhyRxIqGainCompletion,
        PhyRxIqGainI2cBinding, PhyRxIqInitCompletion, PhyRxIqInitI2cBinding,
    },
    phy_temperature::{PhyTemperatureCompletion, PhyTemperatureI2cBinding},
    phy_tx_cal::{PhyTxCalibrationEnvironmentCompletion, PhyTxCalibrationEnvironmentPbusBinding},
    phy_tx_power::{PhyTxPowerCompletion, PhyTxPowerI2cBinding},
    phy_txiq::{PhyTxIqInitCompletion, PhyTxIqInitI2cBinding},
};

/// Maximum number of one-microsecond samples used for a finite hardware edge.
///
/// This is the exact bound used by the first hardware-tested open PHY port in
/// `esp32s31_rust`.  Keeping it in the driver makes timeout behavior identical
/// across applications instead of silently changing with board integration.
pub const HARDWARE_EDGE_LIMIT: u16 = 10_000;

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
    RfOperationLimit,
    UnexpectedBinding,
}

macro_rules! define_i2c_executor {
    ($function:ident, $binding:ty, $completion:ty) => {
        pub async fn $function<D: PhyAsyncDelay>(
            mut binding: $binding,
            platform: &mut impl PhyI2cMasterControl,
        ) -> Result<$completion, PhyTargetPortError> {
            for _ in 0..HARDWARE_EDGE_LIMIT {
                match binding.action() {
                    PhyColdI2cAction::StartRead { .. } | PhyColdI2cAction::StartWrite { .. } => {
                        match binding.start_target(platform) {
                            Ok(()) => {}
                            Err(PhyColdI2cError::BusyAtStart) => D::after_micros(1).await,
                            Err(_) => return Err(PhyTargetPortError::UnexpectedBinding),
                        }
                    }
                    PhyColdI2cAction::AwaitReadCompletionEdge { .. }
                    | PhyColdI2cAction::AwaitWriteCompletionEdge { .. } => {
                        D::after_micros(1).await;
                        match binding
                            .observe_target_edge(platform)
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
            registers: &mut RadioRegisters,
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
