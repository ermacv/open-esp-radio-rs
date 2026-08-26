//! Allocation-free async driver for the Rust-owned PHY registration graph.
//!
//! This module deliberately does not know how an ESP executor represents a
//! timer or interrupt future.  The board integration owns that policy through
//! [`PhyRegisterPort`].  The state machine can therefore be used with Embassy,
//! a custom interrupt executor, or a test harness without importing an RTOS.

use core::future::Future;

use crate::phy_register::{
    PhyRegisterBindingError, PhyRegisterCompletion, PhyRegisterExternalBinding, PhyRegisterFailure,
    PhyRegisterLocalStep, PhyRegisterOutcome, PhyRegisterTransition, PhyRegisterTransitionError,
};

const CALIBRATION_TRACKING_PARENT_EDGE_LIMIT: u8 = 32;
const PARAM_TRACKING_PARENT_EDGE_LIMIT: u8 = 16;

/// Async completion boundary for one identity-bound PHY hardware operation.
///
/// Implementations must consume the binding exactly once. MMIO-only bindings
/// may return `Ready` after their finite register sequence. Timer, readiness,
/// and measurement bindings must become ready from a Rust-owned timer or
/// interrupt event; they must not busy-wait, call an RTOS primitive, allocate,
/// or invoke a vendor/ROM radio parent.
///
/// The returned future is an associated, statically dispatched type. No
/// `Box`, trait object, or allocator is required by this interface.
pub trait PhyRegisterPort {
    type Error;

    fn complete(
        &mut self,
        binding: PhyRegisterExternalBinding,
    ) -> impl Future<Output = Result<PhyRegisterCompletion, Self::Error>> + '_;
}

/// Async completion boundary for one complete periodic-calibration child.
///
/// The port receives the retained live-state owner rather than a forgeable
/// action/completion pair. Opaque child proofs can therefore be produced only
/// by driving the selected typed child to its terminal edge.
pub trait PhyCalibrationTrackingPort {
    type Error;

    fn complete<'port, 'state>(
        &'port mut self,
        transition: &'port mut crate::phy_param_tracking::PhyParamTrackingCalibrationTransition<
            'state,
        >,
    ) -> impl Future<
        Output = Result<crate::phy_cal_tracking::PhyCalibrationTrackingCompletion, Self::Error>,
    > + 'port;
}

/// Async completion boundary for one complete outer periodic-tracking child.
pub trait PhyParamTrackingPort {
    type Error;

    fn complete<'port>(
        &'port mut self,
        pending: &'port mut crate::phy_client::PhyPendingTracking,
        state: &'port mut crate::PhyState,
    ) -> impl Future<
        Output = Result<crate::phy_param_tracking::PhyParamTrackingCompletion, Self::Error>,
    > + 'port;
}

/// Failure reported while driving a caller-owned registration transition.
///
/// This generic executor is also the host-model boundary: safe test ports may
/// synthesize completions without touching hardware. Consequently, completing
/// this runner never creates target-registration authority. A completed model
/// transition can recover only ordinary state through
/// [`PhyRegisterTransition::into_model_parts`].
///
/// The transition is borrowed rather than consumed. After any error, the
/// caller still owns it and can inspect its state. After a terminal radio
/// failure, [`PhyRegisterTransition::into_failed_parts`] recovers the ordinary
/// PHY owner and retry input. On ESP32-S31 targets, only the concrete opaque
/// target-attempt runner can yield [`crate::RegisteredPhyState`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRegisterRunError<E> {
    Lowering(PhyRegisterBindingError),
    Port(E),
    Transition(PhyRegisterTransitionError),
    Radio(PhyRegisterFailure),
}

/// Failure while driving the bounded parent of `phy_cal_param_track`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyCalibrationTrackingRunError<E> {
    Port(E),
    Transition(crate::phy_cal_tracking::PhyCalibrationTrackingTransitionError),
    Radio(crate::phy_cal_tracking::PhyCalibrationTrackingFailure),
    ParentEdgeLimit,
}

/// Failure while driving one complete `phy_param_track_tot` request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyParamTrackingRunError<E> {
    Port(E),
    Transition(crate::phy_param_tracking::PhyParamTrackingTransitionError),
    ParentEdgeLimit,
}

/// Drive `register_chipv7_phy` as a Rust async state machine or host model.
///
/// Local arithmetic and ownership transitions run synchronously because they
/// cannot wait on hardware. Every external edge is first lowered to an
/// exhaustive source-owned binding and then awaited through `port`.
///
/// `port` is a caller-provided completion oracle. This function deliberately
/// returns only a value outcome and cannot mint hardware-registration proof.
pub async fn run_phy_register<P: PhyRegisterPort>(
    transition: &mut PhyRegisterTransition,
    port: &mut P,
) -> Result<PhyRegisterOutcome, PhyRegisterRunError<P::Error>> {
    loop {
        match transition
            .step_local()
            .map_err(PhyRegisterRunError::Transition)?
        {
            PhyRegisterLocalStep::StateAdvanced => {}
            PhyRegisterLocalStep::External(action) => {
                let binding = PhyRegisterExternalBinding::lower(action)
                    .map_err(PhyRegisterRunError::Lowering)?;
                let completion = port
                    .complete(binding)
                    .await
                    .map_err(PhyRegisterRunError::Port)?;
                transition
                    .advance_external(completion)
                    .map_err(PhyRegisterRunError::Transition)?;
            }
            PhyRegisterLocalStep::Complete(outcome) => return Ok(outcome),
            PhyRegisterLocalStep::Failed(failure) => {
                return Err(PhyRegisterRunError::Radio(failure));
            }
        }
    }
}

/// Drive one complete periodic calibration parent through identity-bound
/// children without exposing its live [`crate::PhyState`] owner.
///
/// Terminal success leaves `transition` ready for its consuming `commit`.
/// Every error leaves the exact transition and state borrow with the caller;
/// the surrounding PHY-client owner must then enter its fail-stop state.
pub async fn run_phy_calibration_tracking<P: PhyCalibrationTrackingPort>(
    transition: &mut crate::phy_param_tracking::PhyParamTrackingCalibrationTransition<'_>,
    port: &mut P,
) -> Result<(), PhyCalibrationTrackingRunError<P::Error>> {
    for _ in 0..CALIBRATION_TRACKING_PARENT_EDGE_LIMIT {
        match transition.action() {
            crate::phy_cal_tracking::PhyCalibrationTrackingAction::Complete(_) => return Ok(()),
            crate::phy_cal_tracking::PhyCalibrationTrackingAction::Failed(failure) => {
                return Err(PhyCalibrationTrackingRunError::Radio(failure));
            }
            _ => {
                let completion = port
                    .complete(transition)
                    .await
                    .map_err(PhyCalibrationTrackingRunError::Port)?;
                transition
                    .advance(completion)
                    .map_err(PhyCalibrationTrackingRunError::Transition)?;
            }
        }
    }
    Err(PhyCalibrationTrackingRunError::ParentEdgeLimit)
}

/// Drive every selected child of one unique pending PHY-client request.
///
/// `EnterCritical`/`ExitCritical` are ownership boundaries rather than CPU
/// interrupt masking: the affine [`crate::phy_client::PhyPendingTracking`]
/// owner excludes a concurrent radio transition while asynchronous hardware
/// timers and interrupts remain able to make progress.
///
/// On error, callers must consume `pending` through its fail-stop path. A
/// terminal success leaves it ready for `into_owner`.
pub async fn run_phy_param_tracking<P: PhyParamTrackingPort>(
    pending: &mut crate::phy_client::PhyPendingTracking,
    state: &mut crate::PhyState,
    port: &mut P,
) -> Result<crate::phy_param_tracking::PhyParamTrackingOutcome, PhyParamTrackingRunError<P::Error>>
{
    for _ in 0..PARAM_TRACKING_PARENT_EDGE_LIMIT {
        match pending.action() {
            crate::phy_param_tracking::PhyParamTrackingAction::Complete(outcome) => {
                return Ok(outcome);
            }
            _ => {
                let completion = port
                    .complete(pending, state)
                    .await
                    .map_err(PhyParamTrackingRunError::Port)?;
                pending
                    .advance(completion)
                    .map_err(PhyParamTrackingRunError::Transition)?;
            }
        }
    }
    Err(PhyParamTrackingRunError::ParentEdgeLimit)
}

#[cfg(test)]
mod tests {
    use core::{
        future::{Future, ready},
        task::{Context, Poll, Waker},
    };

    use super::{
        PhyCalibrationTrackingPort, PhyParamTrackingPort, PhyRegisterPort, PhyRegisterRunError,
        run_phy_calibration_tracking, run_phy_param_tracking, run_phy_register,
    };
    use crate::{
        PhyRegisterAction,
        phy_register::{
            PhyRegisterCompletion, PhyRegisterExternalBinding, PhyRegisterMmioCompletion,
            PhyRegisterTransition,
        },
    };

    fn run_ready<F: Future>(future: F) -> F::Output {
        let mut context = Context::from_waker(Waker::noop());
        let mut future = std::pin::pin!(future);
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => output,
            Poll::Pending => panic!("test port unexpectedly waited"),
        }
    }

    struct StopAfterFirstMmio {
        calls: u8,
    }

    impl PhyRegisterPort for StopAfterFirstMmio {
        type Error = u8;

        fn complete(
            &mut self,
            binding: PhyRegisterExternalBinding,
        ) -> impl Future<Output = Result<PhyRegisterCompletion, Self::Error>> + '_ {
            self.calls += 1;
            if self.calls != 1 {
                return ready(Err(self.calls));
            }
            let PhyRegisterExternalBinding::Mmio(binding) = binding else {
                return ready(Err(0xff));
            };
            ready(Ok(PhyRegisterCompletion::Mmio(PhyRegisterMmioCompletion {
                action: binding.action(),
            })))
        }
    }

    #[test]
    fn executor_awaits_only_lowered_identity_bound_operations() {
        let mut transition = PhyRegisterTransition::with_production_config();
        let mut port = StopAfterFirstMmio { calls: 0 };

        assert_eq!(
            run_ready(run_phy_register(&mut transition, &mut port)),
            Err(PhyRegisterRunError::Port(2))
        );
        assert_eq!(port.calls, 2);
        assert!(transition.state().is_some());
        assert!(matches!(
            transition.step_local(),
            Ok(crate::phy_register::PhyRegisterLocalStep::External(
                PhyRegisterAction::Mmio(_)
            ))
        ));
    }

    struct RestoreOnlyCalibrationPort {
        calls: u8,
    }

    impl PhyCalibrationTrackingPort for RestoreOnlyCalibrationPort {
        type Error = ();

        fn complete<'port, 'state>(
            &'port mut self,
            transition: &'port mut crate::phy_param_tracking::PhyParamTrackingCalibrationTransition<
                'state,
            >,
        ) -> impl Future<
            Output = Result<crate::phy_cal_tracking::PhyCalibrationTrackingCompletion, Self::Error>,
        > + 'port {
            self.calls += 1;
            ready(match transition.action() {
                crate::phy_cal_tracking::PhyCalibrationTrackingAction::RestoreTxGainCompensation => {
                    Ok(crate::phy_cal_tracking::PhyCalibrationTrackingCompletion::TxGainCompensationRestored)
                }
                _ => Err(()),
            })
        }
    }

    #[test]
    fn calibration_executor_retains_live_state_until_terminal_child_commit() {
        let parameters = crate::phy_param_tracking::PhyParamTrackingParameters {
            tracking_inhibited: false,
            rfpll_cap_tracking_enabled: false,
            rfpll_cap_tracking_threshold: None,
            calibration_tracking_threshold: None,
            shared_tracking_control: 7,
            bluetooth_ieee802154_power_control: 3,
            calibration_tracking_enabled: true,
            relaxed_power_tracking_threshold: false,
        };
        let mut outer = crate::phy_param_tracking::PhyParamTrackingTransition::new(
            crate::phy_param_tracking::PhyParamTrackRequest::new(false, true),
            parameters,
        );
        outer
            .advance(crate::phy_param_tracking::PhyParamTrackingCompletion::EnteredCritical)
            .unwrap();
        outer
            .advance(
                crate::phy_param_tracking::PhyParamTrackingCompletion::BluetoothIeee802154TxPowerTracked {
                    power_control: 3,
                    shared_tracking_control: 7,
                },
            )
            .unwrap();

        let mut state = crate::PhyState::new(crate::PhyConfig::production());
        let mut child = outer.begin_calibration_tracking(&mut state).unwrap();
        let mut port = RestoreOnlyCalibrationPort { calls: 0 };
        assert_eq!(
            run_ready(run_phy_calibration_tracking(&mut child, &mut port)),
            Ok(())
        );
        assert_eq!(port.calls, 1);
        assert!(matches!(
            child.action(),
            crate::phy_cal_tracking::PhyCalibrationTrackingAction::Complete(_)
        ));
        let completion = child.commit().unwrap();
        outer.advance(completion).unwrap();
        assert_eq!(
            outer.action(),
            crate::phy_param_tracking::PhyParamTrackingAction::TemperatureRead
        );
    }

    struct CriticalOnlyTrackingPort {
        calls: u8,
    }

    impl PhyParamTrackingPort for CriticalOnlyTrackingPort {
        type Error = ();

        fn complete<'port>(
            &'port mut self,
            pending: &'port mut crate::phy_client::PhyPendingTracking,
            _state: &'port mut crate::PhyState,
        ) -> impl Future<
            Output = Result<crate::phy_param_tracking::PhyParamTrackingCompletion, Self::Error>,
        > + 'port {
            self.calls += 1;
            ready(match pending.action() {
                crate::phy_param_tracking::PhyParamTrackingAction::EnterCritical => {
                    Ok(crate::phy_param_tracking::PhyParamTrackingCompletion::EnteredCritical)
                }
                crate::phy_param_tracking::PhyParamTrackingAction::ExitCritical => {
                    Ok(crate::phy_param_tracking::PhyParamTrackingCompletion::ExitedCritical)
                }
                _ => Err(()),
            })
        }
    }

    #[test]
    fn outer_executor_holds_affine_client_owner_across_software_critical_section() {
        let request = crate::phy_param_tracking::PhyParamTrackRequest::new(true, false);
        let parameters = crate::phy_param_tracking::PhyParamTrackingParameters {
            tracking_inhibited: true,
            rfpll_cap_tracking_enabled: true,
            rfpll_cap_tracking_threshold: None,
            calibration_tracking_threshold: None,
            shared_tracking_control: 7,
            bluetooth_ieee802154_power_control: 3,
            calibration_tracking_enabled: true,
            relaxed_power_tracking_threshold: false,
        };
        let mut pending = crate::phy_client::PhyPendingTracking::for_test(request, parameters);
        let mut state = crate::PhyState::new(crate::PhyConfig::production());
        let mut port = CriticalOnlyTrackingPort { calls: 0 };
        let outcome =
            run_ready(run_phy_param_tracking(&mut pending, &mut state, &mut port)).unwrap();
        assert_eq!(outcome.clients, request);
        assert!(outcome.tracking_inhibited);
        assert_eq!(port.calls, 2);
        assert!(pending.into_owner().is_ok());
    }

    struct FailingTrackingPort;

    impl PhyParamTrackingPort for FailingTrackingPort {
        type Error = u8;

        fn complete<'port>(
            &'port mut self,
            _pending: &'port mut crate::phy_client::PhyPendingTracking,
            _state: &'port mut crate::PhyState,
        ) -> impl Future<
            Output = Result<crate::phy_param_tracking::PhyParamTrackingCompletion, Self::Error>,
        > + 'port {
            ready(Err(9))
        }
    }

    #[test]
    fn outer_executor_error_preserves_pending_owner_for_explicit_poisoning() {
        let request = crate::phy_param_tracking::PhyParamTrackRequest::new(false, true);
        let parameters = crate::phy_param_tracking::PhyParamTrackingParameters {
            tracking_inhibited: false,
            rfpll_cap_tracking_enabled: false,
            rfpll_cap_tracking_threshold: None,
            calibration_tracking_threshold: None,
            shared_tracking_control: 7,
            bluetooth_ieee802154_power_control: 3,
            calibration_tracking_enabled: true,
            relaxed_power_tracking_threshold: false,
        };
        let mut pending = crate::phy_client::PhyPendingTracking::for_test(request, parameters);
        let mut state = crate::PhyState::new(crate::PhyConfig::production());
        assert_eq!(
            run_ready(run_phy_param_tracking(
                &mut pending,
                &mut state,
                &mut FailingTrackingPort,
            )),
            Err(super::PhyParamTrackingRunError::Port(9))
        );
        assert_eq!(
            pending.action(),
            crate::phy_param_tracking::PhyParamTrackingAction::EnterCritical
        );
        assert_eq!(pending.fail().request(), &request);
    }
}
