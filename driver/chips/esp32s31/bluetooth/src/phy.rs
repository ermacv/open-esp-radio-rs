//! Proof-preserving common-PHY prerequisite for standalone Bluetooth.
//!
//! Registration, Bluetooth-client acquisition and any due immediate tracking
//! are separate affine transitions. BTBB is reachable only after all lower
//! obligations have settled.

use open_esp_radio_esp32s31_phy::{
    PhyAsyncDelay, PhyCalibrationCache, PhyCalibrationIdentity, PhyTargetObserver,
    PhyTargetPortCounters, RegisteredBluetoothPhyClientAcquire,
    RegisteredBluetoothPhyClientAcquireFailure, RegisteredBluetoothPhyPendingTrack,
    RegisteredBluetoothPhyPendingTracking, TargetBluetoothPhyParamTrackingFailure,
    TargetBluetoothPhyRegisterConfig, TargetBluetoothPhyRegisterError,
    TargetBluetoothPhyRegisterFailure, TargetPhyParamTrackingError,
    run_target_bluetooth_phy_param_tracking, run_target_bluetooth_phy_register,
};
use open_esp_radio_esp32s31_phy::{
    phy_client::{PhyClientAcquireError, PhyClientAcquireOrdering, PhyPllTrackClock},
    phy_param_tracking::PhyParamTrackRequest,
};

use crate::{
    common_phy_state::{
        BluetoothControllerPhyInitialized, BluetoothControllerPhyRegistered,
        BluetoothPhyInitializationReport,
    },
    hci::BluetoothControllerLowPowerHardwareInitialized,
};

type Controller<P, const MT: usize, const SC: usize> =
    BluetoothControllerLowPowerHardwareInitialized<P, MT, SC>;

/// Caller-owned inputs for one full common-PHY registration.
pub struct BluetoothPhyInitializationConfig {
    calibration_identity: PhyCalibrationIdentity,
    calibration_cache: Option<PhyCalibrationCache>,
}

impl BluetoothPhyInitializationConfig {
    /// Request a fresh full target registration and calibration cache.
    pub const fn new(calibration_identity: PhyCalibrationIdentity) -> Self {
        Self {
            calibration_identity,
            calibration_cache: None,
        }
    }

    /// Supply retained calibration data as validation input to the full run.
    pub fn with_calibration_cache(mut self, cache: PhyCalibrationCache) -> Self {
        self.calibration_cache = Some(cache);
        self
    }

    fn into_target(self) -> TargetBluetoothPhyRegisterConfig {
        let target = TargetBluetoothPhyRegisterConfig::new(self.calibration_identity);
        match self.calibration_cache {
            Some(cache) => target.with_calibration_cache(cache),
            None => target,
        }
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

/// Failed target registration retaining the complete outer Controller.
#[must_use = "failed common PHY registration still owns Bluetooth hardware"]
pub struct BluetoothControllerPhyInitializationFailure<P, const MT: usize, const SC: usize> {
    _controller: Controller<P, MT, SC>,
    failure: TargetBluetoothPhyRegisterFailure,
}

impl<P, const MT: usize, const SC: usize> BluetoothControllerPhyInitializationFailure<P, MT, SC> {
    /// Inspect the exact lower target-registration failure.
    pub const fn error(&self) -> TargetBluetoothPhyRegisterError {
        self.failure.error()
    }

    /// Inspect target operations completed before failure.
    pub const fn port_counters(&self) -> PhyTargetPortCounters {
        self.failure.counters()
    }
}

/// Successful client acquisition before its tracking continuation is settled.
#[must_use = "Bluetooth client acquisition must advance or retain pending tracking"]
pub struct BluetoothControllerPhyClientAcquire<P, const MT: usize, const SC: usize> {
    controller: Controller<P, MT, SC>,
    acquisition: RegisteredBluetoothPhyClientAcquire,
    calibration_cache: Option<PhyCalibrationCache>,
    report: BluetoothPhyInitializationReport,
}

impl<P, const MT: usize, const SC: usize> BluetoothControllerPhyClientAcquire<P, MT, SC> {
    /// Return the source-reviewed first-client acquisition ordering.
    pub const fn ordering(&self) -> PhyClientAcquireOrdering {
        self.acquisition.ordering()
    }

    /// Borrow the immediate tracking request, when one is due.
    pub const fn request(&self) -> Option<&PhyParamTrackRequest> {
        self.acquisition.request()
    }

    /// Settle the client owner or retain the exact pending tracking request.
    #[allow(
        clippy::result_large_err,
        reason = "pending work retains the complete allocation-free Controller epoch"
    )]
    pub fn into_owner(
        self,
    ) -> Result<
        BluetoothControllerPhyInitialized<P, MT, SC>,
        BluetoothControllerPhyPendingTrack<P, MT, SC>,
    > {
        let Self {
            controller,
            acquisition,
            calibration_cache,
            report,
        } = self;
        match acquisition.into_owner() {
            Ok(phy) => Ok(BluetoothControllerPhyInitialized {
                controller,
                phy,
                calibration_cache,
                report,
            }),
            Err(pending) => Err(BluetoothControllerPhyPendingTrack {
                controller,
                pending,
                calibration_cache,
                report,
            }),
        }
    }
}

/// Rejected client acquisition retaining the complete powered epoch.
///
/// This is fail-stop and exposes no lower recovery edge.
#[must_use = "failed Bluetooth client acquisition retains the powered Controller"]
pub struct BluetoothControllerPhyClientAcquireFailure<P, const MT: usize, const SC: usize> {
    _controller: Controller<P, MT, SC>,
    failure: RegisteredBluetoothPhyClientAcquireFailure,
    _calibration_cache: Option<PhyCalibrationCache>,
    _report: BluetoothPhyInitializationReport,
}

impl<P, const MT: usize, const SC: usize> BluetoothControllerPhyClientAcquireFailure<P, MT, SC> {
    /// Inspect the exact source-owned acquisition rejection.
    pub const fn error(&self) -> PhyClientAcquireError {
        self.failure.error()
    }
}

/// Pending immediate tracking retaining the complete Controller epoch.
#[must_use = "pending Bluetooth PHY tracking must begin"]
pub struct BluetoothControllerPhyPendingTrack<P, const MT: usize, const SC: usize> {
    controller: Controller<P, MT, SC>,
    pending: RegisteredBluetoothPhyPendingTrack,
    calibration_cache: Option<PhyCalibrationCache>,
    report: BluetoothPhyInitializationReport,
}

impl<P, const MT: usize, const SC: usize> BluetoothControllerPhyPendingTrack<P, MT, SC> {
    /// Borrow the exact immediate tracking request.
    pub const fn request(&self) -> &PhyParamTrackRequest {
        self.pending.request()
    }

    /// Enter tracking with policy owned by the registered common-PHY epoch.
    pub fn begin_tracking(self) -> BluetoothControllerPhyPendingTracking<P, MT, SC> {
        BluetoothControllerPhyPendingTracking {
            controller: self.controller,
            tracking: self.pending.begin_tracking(),
            calibration_cache: self.calibration_cache,
            report: self.report,
        }
    }
}

/// In-flight immediate tracking retaining the complete Controller epoch.
#[must_use = "Bluetooth PHY tracking must be driven to a terminal result"]
pub struct BluetoothControllerPhyPendingTracking<P, const MT: usize, const SC: usize> {
    controller: Controller<P, MT, SC>,
    tracking: RegisteredBluetoothPhyPendingTracking,
    calibration_cache: Option<PhyCalibrationCache>,
    report: BluetoothPhyInitializationReport,
}

/// Failed tracking retaining the outer Controller and poisoned lower owner.
#[must_use = "failed Bluetooth PHY tracking retains the poisoned powered epoch"]
pub struct BluetoothControllerPhyTrackingFailure<P, const MT: usize, const SC: usize> {
    _controller: Controller<P, MT, SC>,
    failure: TargetBluetoothPhyParamTrackingFailure,
    _calibration_cache: Option<PhyCalibrationCache>,
    _report: BluetoothPhyInitializationReport,
}

impl<P, const MT: usize, const SC: usize> BluetoothControllerPhyTrackingFailure<P, MT, SC> {
    /// Inspect the exact target tracking failure.
    pub const fn error(&self) -> TargetPhyParamTrackingError {
        self.failure.error()
    }

    /// Borrow the poisoned lower owner without obtaining recovery authority.
    pub const fn lower_failure(&self) -> &TargetBluetoothPhyParamTrackingFailure {
        &self.failure
    }
}

impl<P, const MT: usize, const SC: usize> BluetoothControllerPhyPendingTracking<P, MT, SC> {
    /// Complete due tracking through the borrowed concrete target port.
    ///
    /// Once polled, cancellation releases no reusable Controller state; an
    /// out-of-band hardware reset is required.
    #[allow(
        clippy::result_large_err,
        reason = "failure retains the complete allocation-free Controller epoch"
    )]
    pub async fn complete_tracking<D, O>(
        self,
        observer: O,
    ) -> Result<
        BluetoothControllerPhyInitialized<P, MT, SC>,
        BluetoothControllerPhyTrackingFailure<P, MT, SC>,
    >
    where
        D: PhyAsyncDelay,
        O: PhyTargetObserver,
    {
        let Self {
            mut controller,
            tracking,
            calibration_cache,
            report,
        } = self;
        let result = {
            let (task, platform) = controller.common_phy_parts_mut();
            let mut shared_phy = task.shared_phy_hal();
            run_target_bluetooth_phy_param_tracking::<P, D, O>(
                platform,
                &mut shared_phy,
                tracking,
                observer,
            )
            .await
        };
        match result {
            Ok(success) => {
                let (phy, _outcome) = success.into_parts();
                Ok(BluetoothControllerPhyInitialized {
                    controller,
                    phy,
                    calibration_cache,
                    report,
                })
            }
            Err(failure) => Err(BluetoothControllerPhyTrackingFailure {
                _controller: controller,
                failure,
                _calibration_cache: calibration_cache,
                _report: report,
            }),
        }
    }
}

impl<P, const MT: usize, const SC: usize> BluetoothControllerPhyRegistered<P, MT, SC> {
    /// Acquire the source-owned Bluetooth PHY client without skipping tracking.
    #[allow(
        clippy::result_large_err,
        reason = "failure retains the complete allocation-free Controller epoch"
    )]
    pub fn acquire_phy_client(
        self,
        clock: &mut impl PhyPllTrackClock,
    ) -> Result<
        BluetoothControllerPhyClientAcquire<P, MT, SC>,
        BluetoothControllerPhyClientAcquireFailure<P, MT, SC>,
    > {
        let BluetoothControllerPhyRegistered {
            controller,
            phy,
            calibration_cache,
            report,
        } = self;
        match phy.acquire_phy_client(clock) {
            Ok(acquisition) => Ok(BluetoothControllerPhyClientAcquire {
                controller,
                acquisition,
                calibration_cache,
                report,
            }),
            Err(failure) => Err(BluetoothControllerPhyClientAcquireFailure {
                _controller: controller,
                failure,
                _calibration_cache: calibration_cache,
                _report: report,
            }),
        }
    }
}

impl<P, const MT: usize, const SC: usize> Controller<P, MT, SC> {
    /// Run target registration without treating it as Bluetooth-client enable.
    #[allow(
        clippy::result_large_err,
        reason = "failure retains the complete allocation-free Controller epoch"
    )]
    #[must_use = "common PHY registration must be driven to a terminal result"]
    pub async fn initialize_common_phy<D, O>(
        mut self,
        config: BluetoothPhyInitializationConfig,
        observer: O,
    ) -> Result<
        BluetoothControllerPhyRegistered<P, MT, SC>,
        BluetoothControllerPhyInitializationFailure<P, MT, SC>,
    >
    where
        D: PhyAsyncDelay,
        O: PhyTargetObserver,
    {
        let result = {
            let (task, platform) = self.common_phy_parts_mut();
            let mut shared_phy = task.shared_phy_hal();
            run_target_bluetooth_phy_register::<P, D, O>(
                platform,
                &mut shared_phy,
                config.into_target(),
                observer,
            )
            .await
        };
        match result {
            Ok(success) => {
                let (phy, calibration_cache, registration, counters) = success.into_parts();
                Ok(BluetoothControllerPhyRegistered {
                    controller: self,
                    phy,
                    calibration_cache,
                    report: BluetoothPhyInitializationReport::from_target(registration, counters),
                })
            }
            Err(failure) => Err(BluetoothControllerPhyInitializationFailure {
                _controller: self,
                failure,
            }),
        }
    }
}
