//! Proof-preserving common-PHY prerequisite for standalone Bluetooth.
//!
//! Registration, Bluetooth-client acquisition and any due immediate tracking
//! are separate affine transitions. BTBB is reachable only after all lower
//! obligations have settled.

use crate::{
    common_phy_state::{
        ControllerPhyEntry, ControllerPhyInitialized, ControllerPhyRegistered,
        PhyInitializationReport,
    },
    low_power::ControllerLowPowerHardwareInitialized,
};

use oer_esp32s31_phy::{
    PhyAsyncDelay, PhyCalibrationCache, PhyCalibrationIdentity, PhyDomainRegisterFailure,
    PhyRegisterConfig, PhyTargetObserver, PhyTargetPortCounters, PhyTrackingFailure,
    RegisteredBluetoothPhy, RegisteredBluetoothPhyClientAcquire,
    RegisteredBluetoothPhyClientAcquireFailure, RegisteredBluetoothPhyPendingTrack,
    RegisteredBluetoothPhyPendingTracking, TargetPhyParamTrackingError, TargetPhyRegisterError,
    registered_route::{BluetoothRoute, PhyDomain},
    state::client::{PhyClientAcquireError, PhyClientAcquireOrdering, PhyPllTrackClock},
    tracking::PhyParamTrackRequest,
};

type Controller<P, const MT: usize> = ControllerLowPowerHardwareInitialized<P, MT>;

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

    fn into_target(self) -> PhyRegisterConfig {
        let target = PhyRegisterConfig::new(self.calibration_identity);
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
pub struct ControllerPhyInitializationFailure<P, const MT: usize> {
    _controller: Controller<P, MT>,
    failure: PhyInitializationFailure,
}

#[allow(
    clippy::large_enum_variant,
    reason = "the allocation-free failure retains the complete registration transition"
)]
enum PhyInitializationFailure {
    Power(oer_esp32s31_hal::power::PowerError),
    Registration(PhyDomainRegisterFailure),
}

/// Exact failed boundary before a registered Bluetooth PHY can be issued.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyInitializationError {
    Power(oer_esp32s31_hal::power::PowerError),
    Registration(TargetPhyRegisterError),
}

impl<P, const MT: usize> ControllerPhyInitializationFailure<P, MT> {
    /// Whether registration lacks a completed target failure cleanup.
    /// A power prerequisite rejection occurs before registration starts.
    pub fn phy_hardware_ambiguous(&self) -> bool {
        match &self.failure {
            PhyInitializationFailure::Power(_) => false,
            PhyInitializationFailure::Registration(failure) => !failure.failure_cleanup_completed(),
        }
    }

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
pub struct ControllerPhyClientAcquire<P, const MT: usize> {
    controller: Controller<P, MT>,
    acquisition: RegisteredBluetoothPhyClientAcquire,
    calibration_cache: Option<PhyCalibrationCache>,
    report: ControllerPhyEntry,
}

impl<P, const MT: usize> ControllerPhyClientAcquire<P, MT> {
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
    ) -> Result<ControllerPhyInitialized<P, MT>, ControllerPhyPendingTrack<P, MT>> {
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
pub struct ControllerPhyClientAcquireFailure<P, const MT: usize> {
    _controller: Controller<P, MT>,
    failure: RegisteredBluetoothPhyClientAcquireFailure,
    _calibration_cache: Option<PhyCalibrationCache>,
    _report: ControllerPhyEntry,
}

impl<P, const MT: usize> ControllerPhyClientAcquireFailure<P, MT> {
    /// Inspect the exact source-owned acquisition rejection.
    pub const fn error(&self) -> PhyClientAcquireError {
        self.failure.error()
    }
}

/// Pending immediate tracking retaining the complete Controller epoch.
#[must_use = "pending Bluetooth PHY tracking must begin"]
pub struct ControllerPhyPendingTrack<P, const MT: usize> {
    controller: Controller<P, MT>,
    pending: RegisteredBluetoothPhyPendingTrack,
    calibration_cache: Option<PhyCalibrationCache>,
    report: ControllerPhyEntry,
}

impl<P, const MT: usize> ControllerPhyPendingTrack<P, MT> {
    /// Borrow the exact immediate tracking request.
    pub const fn request(&self) -> &PhyParamTrackRequest {
        self.pending.request()
    }

    /// Enter tracking with policy owned by the registered common-PHY epoch.
    pub fn begin_tracking(self) -> ControllerPhyPendingTracking<P, MT> {
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
pub struct ControllerPhyPendingTracking<P, const MT: usize> {
    controller: Controller<P, MT>,
    tracking: RegisteredBluetoothPhyPendingTracking,
    calibration_cache: Option<PhyCalibrationCache>,
    report: ControllerPhyEntry,
}

/// Failed tracking retaining the outer Controller and poisoned lower owner.
#[must_use = "failed Bluetooth PHY tracking retains the poisoned powered epoch"]
pub struct ControllerPhyTrackingFailure<P, const MT: usize> {
    _controller: Controller<P, MT>,
    failure: PhyTrackingFailure<BluetoothRoute>,
    _calibration_cache: Option<PhyCalibrationCache>,
    _report: ControllerPhyEntry,
}

impl<P, const MT: usize> ControllerPhyTrackingFailure<P, MT> {
    /// Inspect the exact target tracking failure.
    pub const fn error(&self) -> TargetPhyParamTrackingError {
        self.failure.error()
    }

    /// Borrow the poisoned lower owner without obtaining recovery authority.
    pub const fn lower_failure(&self) -> &PhyTrackingFailure<BluetoothRoute> {
        &self.failure
    }
}

impl<P, const MT: usize> ControllerPhyPendingTracking<P, MT> {
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
    ) -> Result<ControllerPhyInitialized<P, MT>, ControllerPhyTrackingFailure<P, MT>>
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
            let (mut shared_phy, mut grant) = task.shared_phy_hal_with_grant();
            let mut tracking = core::pin::pin!(tracking.track::<P, _, D, O>(
                platform,
                &mut shared_phy,
                &mut grant,
                observer,
                None,
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

impl<P, const MT: usize> ControllerPhyRegistered<P, MT> {
    /// Acquire the source-owned Bluetooth PHY client without skipping tracking.
    #[allow(
        clippy::result_large_err,
        reason = "failure retains the complete allocation-free Controller epoch"
    )]
    pub fn acquire_phy_client(
        self,
        clock: &mut impl PhyPllTrackClock,
    ) -> Result<ControllerPhyClientAcquire<P, MT>, ControllerPhyClientAcquireFailure<P, MT>> {
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

impl<P, const MT: usize> Controller<P, MT> {
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
    ) -> Result<ControllerPhyRegistered<P, MT>, ControllerPhyInitializationFailure<P, MT>>
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
            let mut registration = core::pin::pin!(PhyDomain::register::<P, _, D, O>(
                platform,
                &mut shared_phy,
                config.into_target(),
                observer,
            ));
            core::future::poll_fn(|cx| poll_registration(registration.as_mut(), cx)).await
        };
        finish_registration(self, result)
    }
}

/// Rejected or failed retained-PHY resume retaining the outer Controller.
#[must_use = "failed retained PHY resume still owns Bluetooth hardware"]
pub struct ControllerPhyResumeFailure<P, const MT: usize> {
    _controller: Controller<P, MT>,
    failure: PhyResumeFailure,
}

enum PhyResumeFailure {
    NotRetained {
        _closed: oer_esp32s31_phy::RegisteredBluetoothPhyRfClosed,
    },
    Power {
        error: oer_esp32s31_hal::power::PowerError,
        _closed: oer_esp32s31_phy::RegisteredBluetoothPhyRfClosed,
    },
    Wake(oer_esp32s31_phy::BluetoothPhyRfWakeFailure),
}

/// Exact failed boundary of a retained-PHY resume.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyResumeError {
    /// This Controller powered its own common PHY; no retained epoch exists.
    NotRetained,
    /// The common power prerequisite was rejected before any wake.
    Power(oer_esp32s31_hal::power::PowerError),
    /// The closed domain belongs to another registration; no MMIO ran.
    EpochMismatch,
    /// The retained RF wake started and failed.
    Wake(oer_esp32s31_phy::PhyTargetPortError),
}

impl<P, const MT: usize> ControllerPhyResumeFailure<P, MT> {
    /// Whether the retained RF wake started and left the RF domain ambiguous.
    pub const fn phy_hardware_ambiguous(&self) -> bool {
        matches!(
            self.failure,
            PhyResumeFailure::Wake(oer_esp32s31_phy::BluetoothPhyRfWakeFailure::Poisoned(_))
        )
    }

    /// Inspect the rejected or failed resume boundary.
    pub const fn error(&self) -> PhyResumeError {
        match &self.failure {
            PhyResumeFailure::NotRetained { .. } => PhyResumeError::NotRetained,
            PhyResumeFailure::Power { error, .. } => PhyResumeError::Power(*error),
            PhyResumeFailure::Wake(oer_esp32s31_phy::BluetoothPhyRfWakeFailure::EpochMismatch(
                _,
            )) => PhyResumeError::EpochMismatch,
            PhyResumeFailure::Wake(oer_esp32s31_phy::BluetoothPhyRfWakeFailure::Poisoned(
                poisoned,
            )) => PhyResumeError::Wake(poisoned.error()),
        }
    }
}

impl<P, const MT: usize> Controller<P, MT> {
    /// Resume the registered common PHY another protocol route handed over,
    /// instead of powering and registering it.
    ///
    /// The Controller must have been booted from
    /// [`crate::resources::BluetoothStopped::from_retained`]; the common power
    /// sequence of the previous route stays in effect and only the retained
    /// RF wake runs. Bluetooth-client acquisition remains a separate
    /// transition, exactly as after [`Self::initialize_common_phy`].
    ///
    /// # Cancellation
    /// Once polled, drive this future to a terminal result. After the first
    /// wake edge every failure requires reset.
    #[allow(
        clippy::result_large_err,
        reason = "failure retains the complete allocation-free Controller epoch"
    )]
    #[must_use = "retained PHY resume must be driven to a terminal result"]
    pub async fn resume_common_phy<D: PhyAsyncDelay>(
        mut self,
        closed: oer_esp32s31_phy::RegisteredBluetoothPhyRfClosed,
        calibration_cache: Option<PhyCalibrationCache>,
    ) -> Result<ControllerPhyRegistered<P, MT>, ControllerPhyResumeFailure<P, MT>> {
        let task = self.common_phy_parts_mut().0;
        if !task.common_phy_inherited() {
            return Err(ControllerPhyResumeFailure {
                _controller: self,
                failure: PhyResumeFailure::NotRetained { _closed: closed },
            });
        }
        // Inherited common power returns without register access; the call
        // still retires cold reunion for this route.
        if let Err(error) = task.prepare_common_phy_power() {
            return Err(ControllerPhyResumeFailure {
                _controller: self,
                failure: PhyResumeFailure::Power {
                    error,
                    _closed: closed,
                },
            });
        }
        let result = {
            let mut shared_phy = self.common_phy_parts_mut().0.shared_phy_hal();
            closed.wake_rf::<_, D>(&mut shared_phy).await
        };
        match result {
            Ok(phy) => Ok(ControllerPhyRegistered {
                controller: self,
                phy,
                calibration_cache,
                report: ControllerPhyEntry::RetainedWake,
            }),
            Err(failure) => Err(ControllerPhyResumeFailure {
                _controller: self,
                failure: PhyResumeFailure::Wake(failure),
            }),
        }
    }
}

#[inline(never)]
#[allow(
    clippy::result_large_err,
    reason = "registration failure retains the actual Controller"
)]
fn finish_registration<P, const MT: usize>(
    controller: Controller<P, MT>,
    result: Result<oer_esp32s31_phy::PhyDomainRegistered, PhyDomainRegisterFailure>,
) -> Result<ControllerPhyRegistered<P, MT>, ControllerPhyInitializationFailure<P, MT>> {
    match result {
        Ok(success) => {
            let (phy, calibration_cache, registration, counters) =
                RegisteredBluetoothPhy::from_registration(success);
            Ok(ControllerPhyRegistered {
                controller,
                phy,
                calibration_cache,
                report: ControllerPhyEntry::Registered(PhyInitializationReport::from_target(
                    registration,
                    counters,
                )),
            })
        }
        Err(failure) => Err(ControllerPhyInitializationFailure {
            _controller: controller,
            failure: PhyInitializationFailure::Registration(failure),
        }),
    }
}

#[inline(never)]
fn poll_registration<F: core::future::Future>(
    future: core::pin::Pin<&mut F>,
    cx: &mut core::task::Context<'_>,
) -> core::task::Poll<F::Output> {
    let poll: fn(
        core::pin::Pin<&mut F>,
        &mut core::task::Context<'_>,
    ) -> core::task::Poll<F::Output> = F::poll;
    core::hint::black_box(poll)(future, cx)
}
