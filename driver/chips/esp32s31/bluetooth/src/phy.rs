//! Full common-PHY prerequisite for the standalone Bluetooth lifecycle.
//!
//! The vendor Bluetooth enable path enters the same `register_chipv7_phy`
//! transition used by Wi-Fi before it touches BTBB or controller state. This
//! module reuses that one recovered transition and its target port; it does
//! not maintain a Bluetooth-specific shadow of common PHY initialization.

use open_esp_radio_esp32s31_hal::{
    analog_i2c::PhyPmuControl, phy_i2c::PhyI2cMasterControl,
    phy_prelude::PhyPreludePlatformControl, phy_temperature::PhyTemperatureSystemControl,
    power_detector_platform::PhyPowerDetectorPlatformControl, wifi_bb::PhyWifiBbControl,
};
#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_phy::{
    PhyAsyncDelay, PhyRegisterRunError, PhyRegisterTransition, PhyTargetObserver,
    PhyTargetPortCounters, PhyTargetPortError, TargetPhyRegisterPort, run_phy_register,
};
use open_esp_radio_esp32s31_phy::{
    PhyCalibrationCache, PhyCalibrationIdentity, PhyRegisterOutcome, PhyState,
};

use crate::{
    BluetoothClockControl, BluetoothClockedResources, BluetoothPhysicalResources,
    clock::disable_owned,
    resources::{BluetoothInterruptBankOwner, BluetoothTaskResources},
};

/// Complete platform capability set consumed by common PHY initialization.
///
/// This is a composition contract only. Each parent trait remains the owner
/// of one official system-peripheral operation family; Bluetooth gains no raw
/// platform-register access through this marker.
pub trait BluetoothPhyPlatform:
    PhyPreludePlatformControl
    + PhyPmuControl
    + PhyWifiBbControl
    + PhyPowerDetectorPlatformControl
    + PhyTemperatureSystemControl
    + PhyI2cMasterControl
{
}

impl<T> BluetoothPhyPlatform for T where
    T: PhyPreludePlatformControl
        + PhyPmuControl
        + PhyWifiBbControl
        + PhyPowerDetectorPlatformControl
        + PhyTemperatureSystemControl
        + PhyI2cMasterControl
{
}

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

/// Observable, value-only result of the full common-PHY transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothPhyInitializationReport {
    /// Terminal result returned by the shared `register_chipv7_phy` model.
    pub registration: PhyRegisterOutcome,
    /// Number of recovered MMIO actions completed by the target port.
    pub mmio_operations: u16,
    /// Number of recovered delay actions completed by the target port.
    pub delays: u16,
    /// Number of reset-readback samples completed by the target port.
    pub reset_samples: u16,
    /// Number of bounded RF operations completed by the target port.
    pub rf_operations: u32,
    /// Number of bounded baseband operations completed by the target port.
    pub baseband_operations: u32,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothPhyInitializationReport {
    fn from_target(registration: PhyRegisterOutcome, counters: PhyTargetPortCounters) -> Self {
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

/// Bluetooth hardware after the complete shared PHY registration/calibration.
///
/// Construction is possible only through [`BluetoothClockedResources::initialize_common_phy`].
/// The unique PHY software state and both Bluetooth PAC partitions remain
/// private. Later BTBB/controller transitions may borrow the task partition
/// only inside this crate and must retain this whole state by value.
#[must_use = "initialized common PHY state retains every Bluetooth hardware owner"]
pub struct BluetoothPhyInitialized<P> {
    pub(crate) task: BluetoothTaskResources,
    pub(crate) interrupts: BluetoothInterruptBankOwner,
    pub(crate) platform: P,
    phy: PhyState,
    calibration_cache: Option<PhyCalibrationCache>,
    report: BluetoothPhyInitializationReport,
}

impl<P> BluetoothPhyInitialized<P> {
    /// Inspect the value-only result without obtaining hardware authority.
    pub const fn report(&self) -> BluetoothPhyInitializationReport {
        self.report
    }

    /// Borrow the retained calibration cache for caller-selected persistence.
    pub const fn calibration_cache(&self) -> Option<&PhyCalibrationCache> {
        self.calibration_cache.as_ref()
    }
}

impl<P: BluetoothClockControl> BluetoothPhyInitialized<P> {
    /// Gate the Bluetooth clock prerequisite and recover both cold owners.
    ///
    /// No guessed inverse of common calibration is performed. The retained
    /// software PHY state is consumed and the next cold start re-establishes
    /// hardware state through its normal clock/reset and registration path.
    pub fn disable_clocks(self) -> (BluetoothPhysicalResources, P) {
        let Self {
            task,
            interrupts,
            mut platform,
            phy: _phy,
            calibration_cache: _calibration_cache,
            report: _,
        } = self;
        let resources = task.reunite(interrupts);
        disable_owned(&mut platform);
        (resources, platform)
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
#[cfg(target_arch = "riscv32")]
#[must_use = "failed common PHY initialization still owns Bluetooth hardware"]
pub struct BluetoothPhyInitializationFailure<P> {
    task: BluetoothTaskResources,
    interrupts: BluetoothInterruptBankOwner,
    platform: P,
    transition: PhyRegisterTransition,
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

#[cfg(target_arch = "riscv32")]
impl<P: BluetoothClockControl> BluetoothPhyInitializationFailure<P> {
    /// Complete clock rollback and recover both cold owners after failure.
    pub fn disable_clocks(self) -> (BluetoothPhysicalResources, P) {
        let Self {
            task,
            interrupts,
            mut platform,
            transition: _transition,
            port_counters: _,
            error: _,
        } = self;
        let resources = task.reunite(interrupts);
        disable_owned(&mut platform);
        (resources, platform)
    }
}

impl<P> BluetoothClockedResources<P> {
    /// Run the complete shared ESP32-S31 PHY registration/calibration path.
    ///
    /// The initial Wi-Fi-baseband condition is sampled from the platform
    /// owner before the shared PHY borrow is created. The recovered common
    /// transition may temporarily drive that physical bit even for Bluetooth;
    /// the local observation follows those official platform edges and is not
    /// inferred from the selected protocol route.
    #[cfg(target_arch = "riscv32")]
    pub async fn initialize_common_phy<D, O>(
        self,
        config: BluetoothPhyInitializationConfig,
        observer: O,
    ) -> Result<BluetoothPhyInitialized<P>, BluetoothPhyInitializationFailure<P>>
    where
        P: BluetoothPhyPlatform,
        D: PhyAsyncDelay,
        O: PhyTargetObserver,
    {
        let (resources, mut platform) = self.into_parts();
        let wifi_baseband =
            open_esp_radio_esp32s31_hal::WifiBasebandEnableObservation::from_platform_readback(
                platform.wifi_baseband_is_enabled(),
            );
        let (mut task, interrupts) = resources.separate_interrupt_owner();
        let mut transition = PhyRegisterTransition::with_production_config_and_calibration(
            config.calibration_identity,
            config.calibration_cache,
        );

        let registration = {
            let mut shared_phy = task.shared_phy_hal(wifi_baseband);
            let mut port =
                TargetPhyRegisterPort::<_, _, D, _>::new(&mut platform, &mut shared_phy, observer);
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
                    task,
                    interrupts,
                    platform,
                    transition,
                    port_counters,
                    error: BluetoothPhyInitializationError::Registration(error),
                });
            }
        };
        let calibration_cache = transition.take_calibration_cache();
        let phy = match transition.into_state() {
            Ok(phy) => phy,
            Err(transition) => {
                return Err(BluetoothPhyInitializationFailure {
                    task,
                    interrupts,
                    platform,
                    transition,
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
}
