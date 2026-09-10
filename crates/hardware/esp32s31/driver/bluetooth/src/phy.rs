//! Proof-preserving common-PHY prerequisite for standalone Bluetooth.
//!
//! Registration, Bluetooth-client acquisition and any due immediate tracking
//! are separate affine transitions. BTBB is reachable only after all lower
//! obligations have settled.

use crate::{
    common_phy_state::{
        ControllerPhyInitialized, ControllerPhyRegistered, PhyInitializationReport,
    },
    low_power::ControllerLowPowerHardwareInitialized,
};

use oer_esp32s31_phy::{
    PhyAsyncDelay, PhyCalibrationCache, PhyCalibrationIdentity, PhyTargetObserver,
    PhyTargetPortCounters, RegisteredBluetoothPhyClientAcquire,
    RegisteredBluetoothPhyClientAcquireFailure, RegisteredBluetoothPhyPendingTrack,
    RegisteredBluetoothPhyPendingTracking, TargetBluetoothPhyParamTrackingFailure,
    TargetBluetoothPhyRegisterConfig, TargetBluetoothPhyRegisterError,
    TargetBluetoothPhyRegisterFailure, TargetPhyParamTrackingError,
    run_target_bluetooth_phy_param_tracking, run_target_bluetooth_phy_register,
    state::client::{PhyClientAcquireError, PhyClientAcquireOrdering, PhyPllTrackClock},
    tracking::parameters::PhyParamTrackRequest,
};

type Controller<P, const MT: usize, const SC: usize> =
    ControllerLowPowerHardwareInitialized<P, MT, SC>;

/// Caller-owned inputs for one full common-PHY registration.
pub struct PhyInitializationConfig {
    calibration_identity: PhyCalibrationIdentity,
    calibration_cache: Option<PhyCalibrationCache>,
}

impl PhyInitializationConfig {
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

impl PhyInitializationReport {
    fn from_target(
        registration: oer_esp32s31_phy::PhyRegisterOutcome,
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
pub struct ControllerPhyInitializationFailure<P, const MT: usize, const SC: usize> {
    _controller: Controller<P, MT, SC>,
    failure: PhyInitializationFailure,
}

#[allow(
    clippy::large_enum_variant,
    reason = "the allocation-free failure retains the complete registration transition"
)]
enum PhyInitializationFailure {
    Power(oer_esp32s31_hal::power::PowerError),
    Registration(TargetBluetoothPhyRegisterFailure),
}

/// Exact failed boundary before a registered Bluetooth PHY can be issued.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyInitializationError {
    Power(oer_esp32s31_hal::power::PowerError),
    Registration(TargetBluetoothPhyRegisterError),
}

impl<P, const MT: usize, const SC: usize> ControllerPhyInitializationFailure<P, MT, SC> {
    /// Inspect the power prerequisite or lower target-registration failure.
    pub const fn error(&self) -> PhyInitializationError {
        match &self.failure {
            PhyInitializationFailure::Power(error) => PhyInitializationError::Power(*error),
            PhyInitializationFailure::Registration(failure) => {
                PhyInitializationError::Registration(failure.error())
            }
        }
    }

    /// Inspect registration operations completed before failure.
    /// Power-prerequisite failures have zero registration operations.
    pub fn port_counters(&self) -> PhyTargetPortCounters {
        match &self.failure {
            PhyInitializationFailure::Power(_) => PhyTargetPortCounters::default(),
            PhyInitializationFailure::Registration(failure) => failure.counters(),
        }
    }
}

/// Successful client acquisition before its tracking continuation is settled.
#[must_use = "Bluetooth client acquisition must advance or retain pending tracking"]
pub struct ControllerPhyClientAcquire<P, const MT: usize, const SC: usize> {
    controller: Controller<P, MT, SC>,
    acquisition: RegisteredBluetoothPhyClientAcquire,
    calibration_cache: Option<PhyCalibrationCache>,
    report: PhyInitializationReport,
}

impl<P, const MT: usize, const SC: usize> ControllerPhyClientAcquire<P, MT, SC> {
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
    ) -> Result<ControllerPhyInitialized<P, MT, SC>, ControllerPhyPendingTrack<P, MT, SC>> {
        let Self {
            controller,
            acquisition,
            calibration_cache,
            report,
        } = self;
        match acquisition.into_owner() {
            Ok(phy) => Ok(ControllerPhyInitialized {
                controller,
                phy,
                calibration_cache,
                report,
            }),
            Err(pending) => Err(ControllerPhyPendingTrack {
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
pub struct ControllerPhyClientAcquireFailure<P, const MT: usize, const SC: usize> {
    _controller: Controller<P, MT, SC>,
    failure: RegisteredBluetoothPhyClientAcquireFailure,
    _calibration_cache: Option<PhyCalibrationCache>,
    _report: PhyInitializationReport,
}

impl<P, const MT: usize, const SC: usize> ControllerPhyClientAcquireFailure<P, MT, SC> {
    /// Inspect the exact source-owned acquisition rejection.
    pub const fn error(&self) -> PhyClientAcquireError {
        self.failure.error()
    }
}

/// Pending immediate tracking retaining the complete Controller epoch.
#[must_use = "pending Bluetooth PHY tracking must begin"]
pub struct ControllerPhyPendingTrack<P, const MT: usize, const SC: usize> {
    controller: Controller<P, MT, SC>,
    pending: RegisteredBluetoothPhyPendingTrack,
    calibration_cache: Option<PhyCalibrationCache>,
    report: PhyInitializationReport,
}

impl<P, const MT: usize, const SC: usize> ControllerPhyPendingTrack<P, MT, SC> {
    /// Borrow the exact immediate tracking request.
    pub const fn request(&self) -> &PhyParamTrackRequest {
        self.pending.request()
    }

    /// Enter tracking with policy owned by the registered common-PHY epoch.
    pub fn begin_tracking(self) -> ControllerPhyPendingTracking<P, MT, SC> {
        ControllerPhyPendingTracking {
            controller: self.controller,
            tracking: self.pending.begin_tracking(),
            calibration_cache: self.calibration_cache,
            report: self.report,
        }
    }
}

/// In-flight immediate tracking retaining the complete Controller epoch.
#[must_use = "Bluetooth PHY tracking must be driven to a terminal result"]
pub struct ControllerPhyPendingTracking<P, const MT: usize, const SC: usize> {
    controller: Controller<P, MT, SC>,
    tracking: RegisteredBluetoothPhyPendingTracking,
    calibration_cache: Option<PhyCalibrationCache>,
    report: PhyInitializationReport,
}

/// Failed tracking retaining the outer Controller and poisoned lower owner.
#[must_use = "failed Bluetooth PHY tracking retains the poisoned powered epoch"]
pub struct ControllerPhyTrackingFailure<P, const MT: usize, const SC: usize> {
    _controller: Controller<P, MT, SC>,
    failure: TargetBluetoothPhyParamTrackingFailure,
    _calibration_cache: Option<PhyCalibrationCache>,
    _report: PhyInitializationReport,
}

impl<P, const MT: usize, const SC: usize> ControllerPhyTrackingFailure<P, MT, SC> {
    /// Inspect the exact target tracking failure.
    pub const fn error(&self) -> TargetPhyParamTrackingError {
        self.failure.error()
    }

    /// Borrow the poisoned lower owner without obtaining recovery authority.
    pub const fn lower_failure(&self) -> &TargetBluetoothPhyParamTrackingFailure {
        &self.failure
    }
}

impl<P, const MT: usize, const SC: usize> ControllerPhyPendingTracking<P, MT, SC> {
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
    ) -> Result<ControllerPhyInitialized<P, MT, SC>, ControllerPhyTrackingFailure<P, MT, SC>>
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
            let mut tracking = core::pin::pin!(run_target_bluetooth_phy_param_tracking::<P, D, O>(
                platform,
                &mut shared_phy,
                tracking,
                observer,
            ));
            core::future::poll_fn(|cx| poll_tracking(tracking.as_mut(), cx)).await
        };
        match result {
            Ok(success) => {
                let (phy, _outcome) = success.into_parts();
                Ok(ControllerPhyInitialized {
                    controller,
                    phy,
                    calibration_cache,
                    report,
                })
            }
            Err(failure) => Err(ControllerPhyTrackingFailure {
                _controller: controller,
                failure,
                _calibration_cache: calibration_cache,
                _report: report,
            }),
        }
    }
}

// Keep the lower PHY poll's calibration temporaries separate from the outer
// Controller ownership transfer. Pinning retains the same future and cancellation
// contract; this boundary needs neither allocation nor a larger stack allowance.
#[inline(never)]
fn poll_tracking<F: core::future::Future>(
    tracking: core::pin::Pin<&mut F>,
    cx: &mut core::task::Context<'_>,
) -> core::task::Poll<F::Output> {
    tracking.poll(cx)
}

impl<P, const MT: usize, const SC: usize> ControllerPhyRegistered<P, MT, SC> {
    /// Acquire the source-owned Bluetooth PHY client without skipping tracking.
    #[allow(
        clippy::result_large_err,
        reason = "failure retains the complete allocation-free Controller epoch"
    )]
    pub fn acquire_phy_client(
        self,
        clock: &mut impl PhyPllTrackClock,
    ) -> Result<ControllerPhyClientAcquire<P, MT, SC>, ControllerPhyClientAcquireFailure<P, MT, SC>>
    {
        let ControllerPhyRegistered {
            controller,
            phy,
            calibration_cache,
            report,
        } = self;
        match phy.acquire_phy_client(clock) {
            Ok(acquisition) => Ok(ControllerPhyClientAcquire {
                controller,
                acquisition,
                calibration_cache,
                report,
            }),
            Err(failure) => Err(ControllerPhyClientAcquireFailure {
                _controller: controller,
                failure,
                _calibration_cache: calibration_cache,
                _report: report,
            }),
        }
    }
}

impl<P, const MT: usize, const SC: usize> Controller<P, MT, SC> {
    /// Prepare common PHY power/clocks and run target registration.
    /// Bluetooth-client acquisition remains a separate transition.
    #[allow(
        clippy::result_large_err,
        reason = "failure retains the complete allocation-free Controller epoch"
    )]
    #[must_use = "common PHY registration must be driven to a terminal result"]
    pub async fn initialize_common_phy<D, O>(
        mut self,
        config: PhyInitializationConfig,
        observer: O,
    ) -> Result<ControllerPhyRegistered<P, MT, SC>, ControllerPhyInitializationFailure<P, MT, SC>>
    where
        D: PhyAsyncDelay,
        O: PhyTargetObserver,
    {
        if let Err(error) = self.common_phy_parts_mut().0.prepare_common_phy_power() {
            return Err(ControllerPhyInitializationFailure {
                _controller: self,
                failure: PhyInitializationFailure::Power(error),
            });
        }
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
                Ok(ControllerPhyRegistered {
                    controller: self,
                    phy,
                    calibration_cache,
                    report: PhyInitializationReport::from_target(registration, counters),
                })
            }
            Err(failure) => Err(ControllerPhyInitializationFailure {
                _controller: self,
                failure: PhyInitializationFailure::Registration(failure),
            }),
        }
    }
}
