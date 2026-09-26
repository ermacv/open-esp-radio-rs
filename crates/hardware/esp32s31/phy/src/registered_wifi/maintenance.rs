//! Due-only maintenance of the retained Wi-Fi registration epoch.

use super::{RegisteredWifiPhy, WifiPhyMaintenanceRequest};
use crate::{
    RegisteredPhyState,
    registered_route::PhyDomain,
    state::client::{PhyPendingTracking, PhyPllTrackClock, PhyTrackTimeError},
};

/// Why admission rejected the request before selecting any hardware work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Rejection {
    /// The registration no longer describes the admitted hardware.
    EpochMismatch,
    Clock(PhyTrackTimeError),
}

pub(super) enum Evaluation {
    Idle(RegisteredWifiPhy),
    Pending {
        registered: RegisteredPhyState,
        pending: PhyPendingTracking,
    },
}

impl RegisteredWifiPhy {
    #[allow(
        clippy::result_large_err,
        reason = "failure retains the unique registered owner"
    )]
    /// `registration_current` reports whether this registration still
    /// describes the admitted hardware; a stale registration selects nothing.
    pub(super) fn evaluate(
        self,
        request: WifiPhyMaintenanceRequest,
        clock: &mut impl PhyPllTrackClock,
        registration_current: bool,
    ) -> Result<Evaluation, (Self, Rejection)> {
        if !registration_current {
            return Err((self, Rejection::EpochMismatch));
        }
        let Self {
            domain: PhyDomain {
                registered,
                clients,
            },
        } = self;
        let operation = match request {
            WifiPhyMaintenanceRequest::Operation(operation)
            | WifiPhyMaintenanceRequest::ObservedOperation { operation, .. } => Some(operation),
            WifiPhyMaintenanceRequest::MeasureRfpll { .. } => {
                Some(crate::tracking::maintenance::Operation::Rfpll)
            }
            WifiPhyMaintenanceRequest::CalibrateCommon => {
                Some(crate::tracking::maintenance::Operation::CommonCalibration)
            }
            WifiPhyMaintenanceRequest::CalibrateTransmit => {
                Some(crate::tracking::maintenance::Operation::WifiTxCalibration)
            }
            _ => None,
        };
        if let Some(operation) = operation {
            // Validate the real scheduler clock without advancing any deadline.
            if let Err(error) = clients.snapshot().tracking_schedule_at(clock.now_micros()) {
                return Err((
                    Self {
                        domain: PhyDomain::new(registered, clients),
                    },
                    Rejection::Clock(error),
                ));
            }
            if !clients
                .snapshot()
                .contains(crate::state::client::PhyModemClient::Wifi)
            {
                return Ok(Evaluation::Idle(Self {
                    domain: PhyDomain::new(registered, clients),
                }));
            }
            let maximum_age = match request {
                WifiPhyMaintenanceRequest::ObservedOperation {
                    maximum_age_micros, ..
                }
                | WifiPhyMaintenanceRequest::MeasureRfpll { maximum_age_micros } => {
                    Some(maximum_age_micros)
                }
                _ => None,
            };
            if let Some(maximum_age_micros) = maximum_age
                && operation != crate::tracking::maintenance::Operation::Temperature
                && !matches!(
                    registered
                        .state()
                        .temperature_observation()
                        .freshness(clock.now_micros(), maximum_age_micros),
                    crate::tracking::temperature::Freshness::Fresh { .. }
                )
            {
                return Ok(Evaluation::Idle(Self {
                    domain: PhyDomain::new(registered, clients),
                }));
            }
            let pending = clients.begin_wifi_operation(request.policy(&registered), operation);
            return Ok(Evaluation::Pending {
                registered,
                pending,
            });
        }
        let evaluation = match clients.evaluate_immediate_tracking(clock) {
            Ok(evaluation) => evaluation,
            Err(failure) => {
                let error = failure.error();
                return Err((
                    Self {
                        domain: PhyDomain::new(registered, failure.into_owner()),
                    },
                    Rejection::Clock(error),
                ));
            }
        };
        Ok(match evaluation.into_owner() {
            Ok(clients) => Evaluation::Idle(Self {
                domain: PhyDomain::new(registered, clients),
            }),
            Err(pending) => {
                let policy = request.policy(&registered);
                Evaluation::Pending {
                    registered,
                    pending: pending.begin_tracking(policy),
                }
            }
        })
    }
}

#[cfg(target_arch = "riscv32")]
mod target {
    use super::*;
    use crate::{
        PhyAsyncDelay, PhyTargetObserver,
        executor::run_phy_param_tracking,
        state::client::PhyTrackPoisoned,
        target_port::{TargetPhyParamTrackingError, TargetPhyParamTrackingPort},
        tracking::parameters::PhyParamTrackingOutcome,
    };
    use oer_esp32s31_hal::owner::{
        MacInterruptSetup,
        maintenance::{InterruptAuthority, WifiAccess},
    };

    // The child stays pinned in the maintenance future. Keep hardware-step
    // temporaries out of the owner-transfer poll frame under fat LTO.
    // This adds no allocation, task, yield or ownership/cancellation change.
    #[inline(never)]
    fn poll_tracking<F: core::future::Future>(
        future: core::pin::Pin<&mut F>,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<F::Output> {
        type PollFn<F> = for<'future, 'context, 'wake> fn(
            core::pin::Pin<&'future mut F>,
            &'context mut core::task::Context<'wake>,
        ) -> core::task::Poll<
            <F as core::future::Future>::Output,
        >;
        // Coerce to a pointer before hiding it; a function item is zero-sized.
        let poll: PollFn<F> = F::poll;
        core::hint::black_box(poll)(future, cx)
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum WifiPhyMaintenanceError {
        /// The registration no longer describes the admitted hardware: the
        /// partition was registered again or returned to the cold root.
        EpochMismatch,
        Clock(PhyTrackTimeError),
        Tracking(TargetPhyParamTrackingError),
    }

    // Neither frontier may be converted into a usable PHY after failure.
    enum FailedOwner {
        Admission {
            _owner: RegisteredWifiPhy,
        },
        Tracking {
            _registered: RegisteredPhyState,
            _poisoned: PhyTrackPoisoned,
        },
    }

    #[must_use = "maintenance failure retains the unusable registered PHY epoch"]
    pub struct WifiPhyMaintenanceFailure<I: InterruptAuthority = MacInterruptSetup> {
        _owner: FailedOwner,
        _access: WifiAccess<I>,
        error: WifiPhyMaintenanceError,
    }

    impl<I: InterruptAuthority> WifiPhyMaintenanceFailure<I> {
        pub const fn error(&self) -> WifiPhyMaintenanceError {
            self.error
        }
    }

    impl RegisteredWifiPhy {
        /// Execute due tracking with exclusive PHY hardware access.
        ///
        /// The future owns both the registered PHY and physical access; the
        /// caller must retain stopped or paused descriptor resources. Success returns
        /// access with the same interrupt-owner type for MAC restoration and
        /// checked release. A paused checkpoint is never converted to a cold
        /// setup. A registration that no longer describes the admitted
        /// hardware fails before any access. This API does not supply a
        /// joint-radio coex grant. Cancellation or
        /// failure requires reset: neither PHY nor register/IRQ ownership can
        /// be recovered from the dropped future or an error.
        #[allow(
            clippy::result_large_err,
            reason = "failure retains the allocation-free PHY epoch"
        )]
        pub async fn maintain<P, D: PhyAsyncDelay, O: PhyTargetObserver, I: InterruptAuthority>(
            self,
            platform: &mut P,
            mut access: WifiAccess<I>,
            request: WifiPhyMaintenanceRequest,
            clock: &mut impl PhyPllTrackClock,
            observer: O,
        ) -> Result<
            (Self, WifiAccess<I>, Option<PhyParamTrackingOutcome>),
            WifiPhyMaintenanceFailure<I>,
        > {
            let registration_current = self.domain.clients.describes(&access.phy_hal());
            let (mut registered, mut pending) =
                match self.evaluate(request, clock, registration_current) {
                    Ok(Evaluation::Idle(owner)) => return Ok((owner, access, None)),
                    Ok(Evaluation::Pending {
                        registered,
                        pending,
                    }) => (registered, pending),
                    Err((owner, rejection)) => {
                        return Err(WifiPhyMaintenanceFailure {
                            _owner: FailedOwner::Admission { _owner: owner },
                            _access: access,
                            error: match rejection {
                                Rejection::EpochMismatch => WifiPhyMaintenanceError::EpochMismatch,
                                Rejection::Clock(error) => WifiPhyMaintenanceError::Clock(error),
                            },
                        });
                    }
                };
            let result = {
                let (mut registers, mut grant) = access.phy_hal_with_grant();
                let mut port = TargetPhyParamTrackingPort::<_, _, _, D, _>::new(
                    platform,
                    &mut registers,
                    &mut grant,
                    observer,
                );
                let mut tracking = core::pin::pin!(run_phy_param_tracking(
                    &mut pending,
                    registered.target_state_mut(),
                    &mut port,
                ));
                core::future::poll_fn(|cx| poll_tracking(tracking.as_mut(), cx)).await
            };
            let outcome = match result {
                Ok(outcome) => outcome,
                Err(error) => {
                    return Err(WifiPhyMaintenanceFailure {
                        _owner: FailedOwner::Tracking {
                            _registered: registered,
                            _poisoned: pending.fail(),
                        },
                        _access: access,
                        error: WifiPhyMaintenanceError::Tracking(TargetPhyParamTrackingError::Run(
                            error,
                        )),
                    });
                }
            };
            match pending.into_owner() {
                Ok(clients) => Ok((
                    Self {
                        domain: PhyDomain::new(registered, clients),
                    },
                    access,
                    Some(outcome),
                )),
                Err(pending) => Err(WifiPhyMaintenanceFailure {
                    _owner: FailedOwner::Tracking {
                        _registered: registered,
                        _poisoned: pending.fail(),
                    },
                    _access: access,
                    error: WifiPhyMaintenanceError::Tracking(
                        TargetPhyParamTrackingError::MissingCompletedOwner,
                    ),
                }),
            }
        }
    }
}
#[cfg(target_arch = "riscv32")]
pub use target::{WifiPhyMaintenanceError, WifiPhyMaintenanceFailure};

#[cfg(test)]
mod tests;

impl WifiPhyMaintenanceRequest {
    fn policy(
        self,
        registered: &RegisteredPhyState,
    ) -> crate::tracking::parameters::PhyParamTrackingPolicy {
        let mut policy = registered.tracking_policy();
        if matches!(
            self,
            Self::Calibrate | Self::CalibrateCommon | Self::CalibrateTransmit
        ) {
            policy.calibration_tracking_threshold = Some(0);
        }
        if matches!(self, Self::MeasureRfpll { .. }) {
            policy.rfpll_cap_tracking_threshold = Some(0);
        }
        policy
    }
}
