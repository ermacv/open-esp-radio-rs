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

/// Failure reported while driving a caller-owned registration transition.
///
/// The transition is borrowed rather than consumed. After any error, the
/// caller still owns it and can inspect its state. After a terminal radio
/// failure or success, `PhyRegisterTransition::into_state` recovers the unique
/// Rust owner of the PHY parameter state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRegisterRunError<E> {
    Lowering(PhyRegisterBindingError),
    Port(E),
    Transition(PhyRegisterTransitionError),
    Radio(PhyRegisterFailure),
}

/// Drive `register_chipv7_phy` as a Rust async state machine.
///
/// Local arithmetic and ownership transitions run synchronously because they
/// cannot wait on hardware. Every external edge is first lowered to an
/// exhaustive source-owned binding and then awaited through `port`.
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

#[cfg(test)]
mod tests {
    use core::{
        future::{Future, Ready, ready},
        task::{Context, Poll, Waker},
    };
    use std::{sync::Arc, task::Wake};

    use super::{PhyRegisterPort, PhyRegisterRunError, run_phy_register};
    use crate::{
        PhyRegisterAction,
        phy_register::{
            PhyRegisterCompletion, PhyRegisterExternalBinding, PhyRegisterMmioCompletion,
            PhyRegisterTransition,
        },
    };

    struct WakeWithoutSideEffects;

    impl Wake for WakeWithoutSideEffects {
        fn wake(self: Arc<Self>) {}
    }

    fn run_ready<F: Future>(future: F) -> F::Output {
        let waker = Waker::from(Arc::new(WakeWithoutSideEffects));
        let mut context = Context::from_waker(&waker);
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
        ) -> Ready<Result<PhyRegisterCompletion, Self::Error>> {
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
        let mut transition = PhyRegisterTransition::with_default_profile();
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
}
