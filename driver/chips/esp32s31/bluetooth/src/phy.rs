//! Full common-PHY prerequisite for the standalone Bluetooth lifecycle.
//!
//! The vendor Bluetooth enable path enters the same `register_chipv7_phy`
//! transition used by Wi-Fi before it touches BTBB or controller state. This
//! module reuses that one recovered transition and its target port; it does
//! not maintain a Bluetooth-specific shadow of common PHY initialization.

use embassy_sync::blocking_mutex::raw::RawMutex;
#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_phy::{
    PhyAsyncDelay, PhyRegisterRunError, PhyRegisterTransition, PhyTargetObserver,
    PhyTargetPortCounters, PhyTargetPortError, TargetPhyRegisterPort, run_phy_register,
};
use open_esp_radio_esp32s31_phy::{PhyCalibrationCache, PhyCalibrationIdentity};

use crate::{
    common_phy_state::{BluetoothControllerPhyInitialized, BluetoothPhyInitializationReport},
    hci::BluetoothControllerLowPowerHardwareInitialized,
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
pub struct BluetoothControllerPhyInitializationFailure<
    P,
    M,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> where
    M: RawMutex,
{
    _controller: BluetoothControllerLowPowerHardwareInitialized<
        P,
        M,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
    _transition: PhyRegisterTransition,
    port_counters: PhyTargetPortCounters,
    error: BluetoothPhyInitializationError,
}

#[cfg(target_arch = "riscv32")]
impl<
    P,
    M,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    BluetoothControllerPhyInitializationFailure<
        P,
        M,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
where
    M: RawMutex,
{
    /// Inspect the exact typed failure without releasing an owner.
    pub const fn error(&self) -> BluetoothPhyInitializationError {
        self.error
    }

    /// Inspect target operation counts completed before cleanup terminated.
    pub const fn port_counters(&self) -> PhyTargetPortCounters {
        self.port_counters
    }
}

/// Complete common-PHY initialization for this exact powered Controller.
///
/// The future must be driven to a terminal result. Once polled, cancellation
/// can strand a partially applied hardware edge; reset the chip before trying
/// to construct another radio owner.
impl<
    P,
    M,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    BluetoothControllerLowPowerHardwareInitialized<
        P,
        M,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
where
    M: RawMutex,
{
    #[must_use = "common PHY initialization must be driven to a terminal result"]
    pub async fn initialize_common_phy<D, O>(
        mut self,
        config: BluetoothPhyInitializationConfig,
        observer: O,
    ) -> Result<
        BluetoothControllerPhyInitialized<
            P,
            M,
            MODEM_TIMER_CAPACITY,
            SCHEDULER_CAPACITY,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        BluetoothControllerPhyInitializationFailure<
            P,
            M,
            MODEM_TIMER_CAPACITY,
            SCHEDULER_CAPACITY,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    >
    where
        D: PhyAsyncDelay,
        O: PhyTargetObserver,
    {
        let mut transition = PhyRegisterTransition::with_production_config_and_calibration(
            config.calibration_identity,
            config.calibration_cache,
        );

        let registration = {
            let (task, platform) = self.common_phy_parts_mut();
            let mut shared_phy = task.shared_phy_hal();
            let mut port =
                TargetPhyRegisterPort::<_, _, D, _>::new(platform, &mut shared_phy, observer);
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
                return Err(BluetoothControllerPhyInitializationFailure {
                    _controller: self,
                    _transition: transition,
                    port_counters,
                    error: BluetoothPhyInitializationError::Registration(error),
                });
            }
        };
        let (phy, calibration_cache) = match transition.into_model_parts() {
            Ok(parts) => parts,
            Err(transition) => {
                return Err(BluetoothControllerPhyInitializationFailure {
                    _controller: self,
                    _transition: transition,
                    port_counters,
                    error: BluetoothPhyInitializationError::MissingPhyOwner,
                });
            }
        };

        Ok(BluetoothControllerPhyInitialized {
            controller: self,
            phy,
            calibration_cache,
            report: BluetoothPhyInitializationReport::from_target(registration, port_counters),
        })
    }
}
