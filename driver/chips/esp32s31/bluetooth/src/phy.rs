//! Full common-PHY prerequisite for the standalone Bluetooth lifecycle.
//!
//! The vendor Bluetooth enable path enters the same `register_chipv7_phy`
//! transition used by Wi-Fi before it touches BTBB or controller state. This
//! module reuses that one recovered transition and its target port; it does
//! not maintain a Bluetooth-specific shadow of common PHY initialization.

#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_phy::{
    PhyAsyncDelay, PhyRegisterRunError, PhyRegisterTransition, PhyTargetObserver,
    PhyTargetPortCounters, PhyTargetPortError, TargetPhyRegisterPort, run_phy_register,
};
use open_esp_radio_esp32s31_phy::{PhyCalibrationCache, PhyCalibrationIdentity};

use crate::{
    common_phy_state::{BluetoothPhyInitializationReport, BluetoothPhyInitialized},
    resources::{
        BluetoothInterruptBankOwner, BluetoothTaskResources, BluetoothTeardownPendingPlatform,
    },
};

/// Caller-owned inputs for one full common-PHY registration.
pub struct BluetoothPhyInitializationConfig {
    calibration_identity: PhyCalibrationIdentity,
    calibration_cache: Option<PhyCalibrationCache>,
}

impl BluetoothPhyInitializationConfig {
    /// Request a fresh full calibration and retain its resulting cache.
    pub const fn new(calibration_identity: PhyCalibrationIdentity) -> Self {
        Self {
            calibration_identity,
            calibration_cache: None,
        }
    }

    /// Supply a caller-owned retained cache as validation input.
    ///
    /// The common transition currently performs full calibration even when a
    /// cache is supplied; it never silently treats retained data as hardware
    /// truth before complete replay is implemented.
    pub fn with_calibration_cache(mut self, cache: PhyCalibrationCache) -> Self {
        self.calibration_cache = Some(cache);
        self
    }
}

impl BluetoothPhyInitializationReport {
    fn from_target(
        registration: open_esp_radio_esp32s31_phy::PhyRegisterOutcome,
        counters: PhyTargetPortCounters,
    ) -> Self {
        Self {
            registration,
            mmio_operations: counters.mmio,
            delays: counters.delays,
            reset_samples: counters.reset_samples,
            rf_operations: counters.rf_operations,
            baseband_operations: counters.baseband_operations,
        }
    }
}

/// Exact reason the common-PHY transition did not produce its typestate.
#[cfg(target_arch = "riscv32")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothPhyInitializationError {
    /// A recovered external edge or the shared state machine failed.
    Registration(PhyRegisterRunError<PhyTargetPortError>),
    /// The transition reported success but did not return its unique state.
    MissingPhyOwner,
}

/// Failed common-PHY initialization retaining every unique hardware owner.
///
/// No recovery method is exposed yet. A partially executed common-PHY
/// transition cannot be represented as cold radio ownership until the full
/// PHY teardown transaction is recovered and verified. Dropping the failure
/// is likewise fail-stop and does not release the platform lease.
#[cfg(target_arch = "riscv32")]
#[must_use = "failed common PHY initialization still owns Bluetooth hardware"]
pub struct BluetoothPhyInitializationFailure<P> {
    _task: BluetoothTaskResources,
    _interrupts: BluetoothInterruptBankOwner,
    _platform: BluetoothTeardownPendingPlatform<P>,
    _transition: PhyRegisterTransition,
    port_counters: PhyTargetPortCounters,
    error: BluetoothPhyInitializationError,
}

#[cfg(target_arch = "riscv32")]
impl<P> BluetoothPhyInitializationFailure<P> {
    /// Inspect the exact typed failure without releasing an owner.
    pub const fn error(&self) -> BluetoothPhyInitializationError {
        self.error
    }

    /// Inspect target operation counts completed before cleanup terminated.
    pub const fn port_counters(&self) -> PhyTargetPortCounters {
        self.port_counters
    }
}

/// Execute the already recovered common-PHY enable component for a future
/// owner that has completed every controller-init prerequisite.
///
/// This helper is intentionally crate-private and currently has no caller.
/// Attaching it to the scheduler-prefix state would falsely skip the remaining
/// task, LP, BLE-stack and HCI initialization stages.
#[cfg(target_arch = "riscv32")]
#[allow(
    dead_code,
    reason = "verified enable-stage component awaits a complete controller-init owner"
)]
pub(crate) async fn initialize_common_phy<D, O, P>(
    mut task: BluetoothTaskResources,
    interrupts: BluetoothInterruptBankOwner,
    mut platform: BluetoothTeardownPendingPlatform<P>,
    config: BluetoothPhyInitializationConfig,
    observer: O,
) -> Result<BluetoothPhyInitialized<P>, BluetoothPhyInitializationFailure<P>>
where
    D: PhyAsyncDelay,
    O: PhyTargetObserver,
{
    let mut transition = PhyRegisterTransition::with_production_config_and_calibration(
        config.calibration_identity,
        config.calibration_cache,
    );

    let registration = {
        let mut shared_phy = task.shared_phy_hal();
        let mut port = TargetPhyRegisterPort::<_, _, D, _>::new(
            platform.platform_mut(),
            &mut shared_phy,
            observer,
        );
        let result = run_phy_register(&mut transition, &mut port).await;
        let port_counters = port.counters();
        match result {
            Ok(registration) => Ok((registration, port_counters)),
            Err(error) => Err((error, port_counters)),
        }
    };

    let (registration, port_counters) = match registration {
        Ok(result) => result,
        Err((error, port_counters)) => {
            return Err(BluetoothPhyInitializationFailure {
                _task: task,
                _interrupts: interrupts,
                _platform: platform,
                _transition: transition,
                port_counters,
                error: BluetoothPhyInitializationError::Registration(error),
            });
        }
    };
    let (phy, calibration_cache) = match transition.into_model_parts() {
        Ok(parts) => parts,
        Err(transition) => {
            return Err(BluetoothPhyInitializationFailure {
                _task: task,
                _interrupts: interrupts,
                _platform: platform,
                _transition: transition,
                port_counters,
                error: BluetoothPhyInitializationError::MissingPhyOwner,
            });
        }
    };

    Ok(BluetoothPhyInitialized {
        task,
        interrupts,
        platform,
        phy,
        calibration_cache,
        report: BluetoothPhyInitializationReport::from_target(registration, port_counters),
    })
}
