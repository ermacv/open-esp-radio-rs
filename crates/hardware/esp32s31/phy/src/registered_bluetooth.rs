//! Target-registered common-PHY ownership for the Bluetooth client.
//!
//! Bluetooth already owns its outer Controller and platform epoch. These
//! wrappers therefore retain only the target-issued PHY-registration proof and
//! the source-owned shared-PHY client set. The concrete target runners borrow
//! the outer platform and [`oer_esp32s31_hal::owner::SharedPhyHal`] for one
//! terminal operation; neither resource can escape through this module.
//!
//! Registration and Bluetooth-client acquisition are deliberately separate.
//! A completed `register_chipv7_phy` transition does not imply that the
//! Bluetooth client bit was acquired, and a pending initial tracking request
//! must finish before the client owner can advance into BTBB initialization.
//! None of these states claims RF qualification or operational Link Layer
//! readiness.

#[cfg(target_arch = "riscv32")]
use crate::state::client::PhyPendingTracking;
use crate::{
    PhyState,
    registered_route::{
        BluetoothRoute, PhyClientAcquire, PhyClientAcquireFailureOwner, PhyClientRelease,
        PhyClientReleaseFailureOwner, PhyDomain, PhyPendingTrackOwner, PhyPendingTrackingOwner,
        PhyTrackEvaluationFailureOwner, PhyTrackEvaluationOwner, PhyTrackPoisonedOwner,
    },
    state::client::{PhyClientSnapshot, PhyModemClient, PhyPllTrackClock, PhyTrackTimeError},
};

/// Target-registered common PHY before the Bluetooth client is acquired.
///
/// The private fields prevent safe code from replacing the registered state or
/// supplying an unrelated client-set image. This owner is neither `Copy` nor
/// `Clone` and exposes no decomposer.
#[must_use = "the target-registered Bluetooth PHY owner is unique"]
pub struct RegisteredBluetoothPhy {
    domain: PhyDomain,
}

impl RegisteredBluetoothPhy {
    /// Borrow the registered PHY domain this owner holds.
    pub const fn domain(&self) -> &PhyDomain {
        &self.domain
    }

    /// Mint the Bluetooth owner after one concrete target registration run.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn from_target_completion(
        state: PhyState,
        witness: crate::target_port::TargetRegistrationWitness,
    ) -> Self {
        Self {
            domain: PhyDomain::from_target_completion(state, witness),
        }
    }

    /// Borrow the target-registered PHY state without mutable authority.
    pub const fn phy_state(&self) -> &PhyState {
        self.domain.phy_state()
    }

    /// Inspect the source-owned client set without exposing its bit image.
    pub const fn client_snapshot(&self) -> PhyClientSnapshot {
        self.domain.client_snapshot()
    }

    /// Acquire exactly the Bluetooth shared-PHY client.
    ///
    /// A due initial tracking request remains affine with the registered state
    /// and must pass through the target tracking runner before the caller can
    /// obtain [`RegisteredBluetoothPhyClient`].
    #[allow(
        clippy::result_large_err,
        reason = "the allocation-free failure retains the registered PHY and client owner"
    )]
    pub fn acquire_phy_client(
        self,
        clock: &mut impl PhyPllTrackClock,
    ) -> Result<RegisteredBluetoothPhyClientAcquire, RegisteredBluetoothPhyClientAcquireFailure>
    {
        crate::registered_route::acquire::<BluetoothRoute>((), self.domain, clock)
    }
}

/// Successful Bluetooth-client acquisition retaining target registration.
pub type RegisteredBluetoothPhyClientAcquire = PhyClientAcquire<BluetoothRoute>;

/// Rejected Bluetooth-client acquisition retaining the unchanged owner.
pub type RegisteredBluetoothPhyClientAcquireFailure = PhyClientAcquireFailureOwner<BluetoothRoute>;

/// Target-registered PHY with the Bluetooth client acquired and settled.
///
/// This is the lower owner that a Bluetooth Controller may retain across BTBB
/// and BLE-engine initialization for an always-awake profile. It proves neither
/// a per-event RF-ready instant nor operational radio-engine readiness.
#[must_use = "the registered Bluetooth PHY client owner is unique"]
pub struct RegisteredBluetoothPhyClient {
    domain: PhyDomain,
}

impl RegisteredBluetoothPhyClient {
    /// Borrow the registered PHY domain this owner holds.
    pub const fn domain(&self) -> &PhyDomain {
        &self.domain
    }

    /// Change only the diagnostic thermal thresholds, returning their old policy.
    /// This does not mutate samples, acknowledge demand or grant RF access.
    /// The caller retains exclusive client authority and must arrange quiescence.
    #[cfg(target_arch = "riscv32")]
    pub fn set_tracking_debug(
        &mut self,
        debug: crate::state::PhyTemperatureTrackingDebug,
    ) -> crate::state::PhyTemperatureTrackingDebug {
        let state = self.domain.registered.target_state_mut();
        let old = state.temperature_tracking_debug();
        state.set_temperature_tracking_debug(debug.first, debug.second);
        old
    }

    /// Borrow the target-registered PHY state without mutable authority.
    pub const fn phy_state(&self) -> &PhyState {
        self.domain.phy_state()
    }

    /// Inspect the settled source-owned client set.
    pub const fn client_snapshot(&self) -> PhyClientSnapshot {
        self.domain.client_snapshot()
    }

    /// Release the Bluetooth client while retaining the registered common-PHY
    /// state and the source-owned last-client fact.
    ///
    /// This is a software ownership transition only. The outer Controller must
    /// still quiesce BTBB, timers and interrupts and reunite its physical radio
    /// owners before any RF close or cold release may begin.
    #[allow(
        clippy::result_large_err,
        reason = "failure retains the allocation-free registered Bluetooth owner"
    )]
    pub fn release_phy_client(
        self,
    ) -> Result<RegisteredBluetoothPhyClientRelease, RegisteredBluetoothPhyClientReleaseFailure>
    {
        crate::registered_route::release::<BluetoothRoute>((), self.domain)
    }

    /// Inspect registered-policy conditions without sampling temperature,
    /// advancing deadlines or acquiring the shared RF hardware.
    pub fn inspect_tracking(
        &self,
        now_micros: u64,
    ) -> Result<crate::tracking::inspection::Inspection, PhyTrackTimeError> {
        self.domain.inspect_tracking(now_micros)
    }

    /// Evaluate one source-compatible periodic callback for this Bluetooth
    /// client without acquiring physical RF access.
    #[allow(
        clippy::result_large_err,
        reason = "failure retains the allocation-free registered Bluetooth owner"
    )]
    pub fn evaluate_periodic_tracking(
        self,
        clock: &mut impl PhyPllTrackClock,
    ) -> Result<RegisteredBluetoothPhyTrackEvaluation, RegisteredBluetoothPhyTrackEvaluationFailure>
    {
        crate::registered_route::evaluate::<BluetoothRoute>((), self.domain, clock, false)
    }

    /// Recheck an absolute deadline after a timer or another Controller event
    /// wakes the owner. An early wake returns the unchanged settled owner.
    /// A due request still requires Controller quiescence and physical shared-
    /// PHY admission before target execution begins.
    #[allow(
        clippy::result_large_err,
        reason = "failure retains the allocation-free registered Bluetooth owner"
    )]
    pub fn evaluate_due_tracking(
        self,
        clock: &mut impl PhyPllTrackClock,
    ) -> Result<RegisteredBluetoothPhyTrackEvaluation, RegisteredBluetoothPhyTrackEvaluationFailure>
    {
        crate::registered_route::evaluate::<BluetoothRoute>((), self.domain, clock, true)
    }

    /// Wait for scheduling demand while borrowing, rather than transferring,
    /// the registered Bluetooth owner to the timer.
    ///
    /// The result is an observation only. It neither refreshes timestamps nor
    /// grants access to Controller, BTBB or common-PHY hardware.
    pub async fn wait_for_tracking_demand(
        &self,
        timer: &mut impl crate::state::client::PhyTrackingTimer,
    ) -> Result<Option<crate::tracking::schedule::Demand>, PhyTrackTimeError> {
        self.domain.wait_for_tracking_demand(timer).await
    }
}

/// Successful Bluetooth-client release retaining its registered PHY epoch.
///
/// The result is detached from the Controller's physical owners. It proves
/// only the source-owned client transition and cannot close RF by itself.
pub type RegisteredBluetoothPhyClientRelease = PhyClientRelease<BluetoothRoute>;

/// Registered PHY domain after the complete target RF-close graph, with an
/// empty client set.
///
/// Controller, timer, temperature power and platform clocks still belong to
/// the outer lifecycle. The domain can retire with the cold release, move
/// with the retained radio root to another route through
/// [`crate::RetainedPhy`], or wake RF again on the Bluetooth route without
/// a new registration.
#[must_use = "retain the closed registration until physical owner reunion"]
pub struct RegisteredBluetoothPhyRfClosed {
    pub(crate) domain: PhyDomain,
}

impl RegisteredBluetoothPhyRfClosed {
    /// Read the final state while retaining its physical-close provenance.
    pub const fn state(&self) -> &PhyState {
        self.domain.phy_state()
    }

    /// Borrow the closed registered PHY domain.
    pub const fn domain(&self) -> &PhyDomain {
        &self.domain
    }

    /// Final calibrated state, including the pre-close temperature observation.
    #[cfg(target_arch = "riscv32")]
    pub fn into_retired_state(self) -> PhyState {
        self.domain.registered.into_retired_state()
    }

    /// Restore the closed RF domain on the Bluetooth route while retaining
    /// the exact registered calibration epoch.
    ///
    /// This is the Bluetooth counterpart of the Wi-Fi retained wake: no
    /// registration, calibration or common power sequence runs, and no client
    /// is acquired. A shared-PHY borrow that this registration no longer
    /// describes is rejected before MMIO and returns the unchanged owner.
    ///
    /// # Cancellation
    /// Once polled, drive this future to a terminal result. After the first
    /// wake edge every failure is fail-stop and requires reset.
    #[cfg(target_arch = "riscv32")]
    #[allow(
        clippy::result_large_err,
        reason = "failure retains the allocation-free registered PHY domain"
    )]
    pub async fn wake_rf<D: crate::PhyAsyncDelay>(
        self,
        registers: &mut oer_esp32s31_hal::owner::SharedPhyHal<
            '_,
            oer_esp32s31_hal::owner::route::Bluetooth,
        >,
    ) -> Result<RegisteredBluetoothPhy, BluetoothPhyRfWakeFailure> {
        debug_assert!(self.domain.client_snapshot().is_empty());
        if !self.domain.clients.describes(&*registers) {
            return Err(BluetoothPhyRfWakeFailure::EpochMismatch(self));
        }
        if let Err(error) =
            crate::target_port::wake_bluetooth_rf::<D>(registers, self.domain.phy_state()).await
        {
            return Err(BluetoothPhyRfWakeFailure::Poisoned(
                RegisteredBluetoothPhyRfWakePoisoned {
                    _domain: self.domain,
                    error,
                },
            ));
        }
        Ok(RegisteredBluetoothPhy {
            domain: self.domain,
        })
    }
}

/// Bluetooth retained RF wake that did not return a powered owner.
#[cfg(target_arch = "riscv32")]
#[must_use = "failed RF wake retains the registered PHY frontier"]
pub enum BluetoothPhyRfWakeFailure {
    /// The shared-PHY borrow belongs to another registration; no MMIO ran.
    EpochMismatch(RegisteredBluetoothPhyRfClosed),
    /// The wake graph started and failed; the RF domain is ambiguous.
    Poisoned(RegisteredBluetoothPhyRfWakePoisoned),
}

/// Fail-stop Bluetooth epoch after retained RF wake started but did not
/// complete.
#[cfg(target_arch = "riscv32")]
#[must_use = "partially restored RF hardware requires reset"]
pub struct RegisteredBluetoothPhyRfWakePoisoned {
    _domain: PhyDomain,
    error: crate::PhyTargetPortError,
}

#[cfg(target_arch = "riscv32")]
impl RegisteredBluetoothPhyRfWakePoisoned {
    /// The first failing wake operation.
    pub const fn error(&self) -> crate::PhyTargetPortError {
        self.error
    }
}

/// RF-close failure retaining the released client and registered epoch.
#[must_use = "failed RF close retains the physical shutdown obligation"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothPhyRfCloseFailure {
    _owner: RegisteredBluetoothPhyClientRelease,
    error: crate::PhyTargetPortError,
    retryable: bool,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothPhyRfCloseFailure {
    /// Whether preparation or RF close left an ambiguous hardware epoch.
    /// A false result retains the unchanged release; it does not prove RF off.
    pub const fn hardware_ambiguous(&self) -> bool {
        !self.retryable
    }

    /// The first failing preparation or hardware operation.
    pub const fn error(&self) -> crate::PhyTargetPortError {
        self.error
    }

    /// Recover the exact release only when preparation completed without any
    /// ambiguous hardware operation. A started close remains owned by failure.
    #[allow(
        clippy::result_large_err,
        reason = "both branches retain the affine PHY registration"
    )]
    pub fn into_retry(self) -> Result<RegisteredBluetoothPhyClientRelease, Self> {
        if self.retryable {
            Ok(self._owner)
        } else {
            Err(self)
        }
    }
}

/// Failed Bluetooth maintenance retaining the exact failed PHY frontier.
///
/// Evaluation fails before any hardware access and retains the settled
/// owner; tracking failure retains the poisoned epoch. Neither variant exposes
/// a runnable client.
#[cfg(target_arch = "riscv32")]
#[must_use = "failed Bluetooth PHY maintenance retains the failed PHY frontier"]
pub enum BluetoothPhyMaintenanceFailure {
    /// The tracking clock was rejected before any hardware access.
    Evaluation(RegisteredBluetoothPhyTrackEvaluationFailure),
    /// The admitted tracking transaction failed.
    Tracking(crate::TargetBluetoothPhyParamTrackingFailure),
}

#[cfg(target_arch = "riscv32")]
impl RegisteredBluetoothPhyClient {
    /// Service due tracking under checked Bluetooth maintenance access.
    ///
    /// This is the Bluetooth counterpart of the Wi-Fi maintenance entry: the
    /// access proves Controller quiescence and lends the shared PHY only under
    /// the Bluetooth route. An early wake returns the unchanged client and no
    /// outcome without touching hardware. `deadline` guards execution in
    /// `D`'s clock domain; `None` runs without a transaction deadline.
    ///
    /// # Cancellation
    /// Once polled, drive this future to a terminal result. Cancellation or
    /// failure requires out-of-band reset; neither returns a runnable client.
    #[allow(
        clippy::result_large_err,
        reason = "failure retains the allocation-free Bluetooth PHY frontier"
    )]
    pub async fn maintain<P, D, O>(
        self,
        platform: &mut P,
        access: &mut impl oer_esp32s31_hal::owner::maintenance::PhyMaintenanceAccess<
            Route = oer_esp32s31_hal::owner::route::Bluetooth,
        >,
        clock: &mut impl PhyPllTrackClock,
        observer: O,
        deadline: Option<crate::tracking::deadline::TrackingDeadline>,
    ) -> Result<
        (Self, Option<crate::tracking::PhyParamTrackingOutcome>),
        BluetoothPhyMaintenanceFailure,
    >
    where
        D: crate::PhyAsyncDelay,
        O: crate::PhyTargetObserver,
    {
        let evaluation = self
            .evaluate_due_tracking(clock)
            .map_err(BluetoothPhyMaintenanceFailure::Evaluation)?;
        let pending = match evaluation.into_owner() {
            Ok(client) => return Ok((client, None)),
            Err(pending) => pending.begin_tracking(),
        };
        let mut registers = access.phy_hal();
        let result = match deadline {
            Some(deadline) => {
                crate::run_target_bluetooth_phy_param_tracking_until::<P, D, O>(
                    platform,
                    &mut registers,
                    pending,
                    observer,
                    deadline,
                )
                .await
            }
            None => {
                crate::run_target_bluetooth_phy_param_tracking::<P, D, O>(
                    platform,
                    &mut registers,
                    pending,
                    observer,
                )
                .await
            }
        };
        match result {
            Ok(success) => {
                let (client, outcome) = success.into_parts();
                Ok((client, Some(outcome)))
            }
            Err(failure) => Err(BluetoothPhyMaintenanceFailure::Tracking(failure)),
        }
    }
}

#[cfg(target_arch = "riscv32")]
impl RegisteredBluetoothPhyClientRelease {
    /// Close the last client's physical RF through the outer stopped Controller.
    ///
    /// The caller retains the matching platform, stopped Controller/BTBB,
    /// inactive IRQ routes and drained timers for this exact registration.
    /// No second client may use RF during the operation. Non-final release and
    /// a registration that no longer describes `registers` are rejected before
    /// MMIO. The temperature preflight and close graph are the
    /// same target implementation used by the Wi-Fi radio lifecycle.
    ///
    /// # Cancellation
    /// Once polled, drive this future to completion and retain the outer owners.
    /// A completed preparation failure may return the original release for retry;
    /// cancellation or ambiguous hardware failure never authorizes reuse.
    #[allow(
        clippy::result_large_err,
        reason = "failure retains the complete registered PHY epoch without allocation"
    )]
    pub async fn close_rf<P, D: crate::PhyAsyncDelay>(
        mut self,
        platform: &mut P,
        registers: &mut oer_esp32s31_hal::owner::SharedPhyHal<
            '_,
            oer_esp32s31_hal::owner::route::Bluetooth,
        >,
    ) -> Result<RegisteredBluetoothPhyRfClosed, BluetoothPhyRfCloseFailure> {
        if !self.is_last() {
            return Err(BluetoothPhyRfCloseFailure {
                _owner: self,
                error: crate::PhyTargetPortError::HardwareInvariant,
                retryable: true,
            });
        }
        if !self.outcome.owner().describes(&*registers) {
            return Err(BluetoothPhyRfCloseFailure {
                _owner: self,
                error: crate::PhyTargetPortError::RegistrationEpochMismatch,
                retryable: false,
            });
        }
        if let Err(error) = crate::target_port::close_bluetooth_rf::<P, D>(
            platform,
            registers,
            self.registered.target_state_mut(),
        )
        .await
        {
            let (error, retryable) = match error {
                crate::target_port::PhyRfCloseTemperatureFailure::Recoverable(error) => {
                    (error, true)
                }
                crate::target_port::PhyRfCloseTemperatureFailure::HardwareAmbiguous(error) => {
                    (error, false)
                }
            };
            return Err(BluetoothPhyRfCloseFailure {
                _owner: self,
                error,
                retryable,
            });
        }
        Ok(RegisteredBluetoothPhyRfClosed {
            domain: PhyDomain::new(self.registered, self.outcome.into_owner()),
        })
    }
}

impl RegisteredBluetoothPhyClientRelease {
    /// Recover the registered pre-acquisition owner only when this release
    /// removed the last client.
    ///
    /// A future shared-radio composition may retain another protocol client;
    /// that case returns this exact release unchanged instead of erasing the
    /// saved-mask disposition.
    #[allow(
        clippy::result_large_err,
        reason = "the no-alloc error is the exact affine release owner and cannot be boxed or reduced"
    )]
    pub fn into_registered_phy(self) -> Result<RegisteredBluetoothPhy, Self> {
        if !self.outcome.is_last() {
            return Err(self);
        }
        let Self {
            registered,
            outcome,
            ..
        } = self;
        let clients = outcome.into_owner();
        debug_assert!(clients.snapshot().is_empty());
        Ok(RegisteredBluetoothPhy {
            domain: PhyDomain::new(registered, clients),
        })
    }
}

/// Rejected Bluetooth-client release retaining the unchanged registered owner.
pub type RegisteredBluetoothPhyClientReleaseFailure = PhyClientReleaseFailureOwner<BluetoothRoute>;

/// Scheduler evaluation retaining the exact registered Bluetooth epoch.
pub type RegisteredBluetoothPhyTrackEvaluation = PhyTrackEvaluationOwner<BluetoothRoute>;

/// Invalid clock evaluation retaining the unchanged Bluetooth PHY owner.
pub type RegisteredBluetoothPhyTrackEvaluationFailure =
    PhyTrackEvaluationFailureOwner<BluetoothRoute>;

/// Pending immediate tracking after Bluetooth-client acquisition.
pub type RegisteredBluetoothPhyPendingTrack = PhyPendingTrackOwner<BluetoothRoute>;

/// In-flight Bluetooth tracking retaining the target registration proof.
pub type RegisteredBluetoothPhyPendingTracking = PhyPendingTrackingOwner<BluetoothRoute>;

impl RegisteredBluetoothPhyPendingTracking {
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn target_tracking_parts(&mut self) -> (&mut PhyState, &mut PhyPendingTracking) {
        (self.registered.target_state_mut(), &mut self.pending)
    }
}

/// Fail-stop Bluetooth PHY epoch after ambiguous tracking hardware work.
pub type RegisteredBluetoothPhyTrackPoisoned = PhyTrackPoisonedOwner<BluetoothRoute>;

impl crate::registered_route::sealed::PhyRoute for BluetoothRoute {
    type Hardware = ();
    type Unclaimed = RegisteredBluetoothPhy;
    type Client = RegisteredBluetoothPhyClient;
    const CLIENT: PhyModemClient = PhyModemClient::Bluetooth;

    fn unclaimed((): (), domain: PhyDomain) -> RegisteredBluetoothPhy {
        RegisteredBluetoothPhy { domain }
    }

    fn client((): (), domain: PhyDomain) -> RegisteredBluetoothPhyClient {
        RegisteredBluetoothPhyClient { domain }
    }
}

#[cfg(test)]
mod tests;
