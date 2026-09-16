use embassy_futures::{block_on, join::join};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use oer_esp32s31_coex::{
    CoexClient, CoexClockSelector, CoexPti, CoexPtiTable, CoexTimerClock, CoexTimerIndex,
};

use super::*;

mod recovery;

#[derive(Default)]
struct Hardware {
    enabled: u8,
    disabled: u8,
    fail_enable_once: bool,
    fail_disable_once: bool,
}

impl CoexTimerHardware for Hardware {
    fn configure_request(
        &mut self,
        _index: CoexTimerIndex,
        _client: CoexClient,
        _pti: CoexPti,
    ) -> Result<(), CoexError> {
        Ok(())
    }

    fn set_primary_target(
        &mut self,
        _index: CoexTimerIndex,
        _tick_image: u32,
    ) -> Result<(), CoexError> {
        Ok(())
    }

    fn set_secondary_target(
        &mut self,
        _index: CoexTimerIndex,
        _tick_image: u32,
    ) -> Result<(), CoexError> {
        Ok(())
    }

    fn enable(&mut self, index: CoexTimerIndex) -> Result<(), CoexError> {
        self.enabled |= 1 << index.value();
        if core::mem::take(&mut self.fail_enable_once) {
            return Err(CoexError::Hardware);
        }
        Ok(())
    }

    fn disable(&mut self, index: CoexTimerIndex) -> Result<(), CoexError> {
        if core::mem::take(&mut self.fail_disable_once) {
            return Err(CoexError::Hardware);
        }
        self.disabled |= 1 << index.value();
        self.enabled &= !(1 << index.value());
        Ok(())
    }

    fn force(&mut self, _index: CoexTimerIndex) -> Result<(), CoexError> {
        Ok(())
    }

    fn unforce(&mut self, _index: CoexTimerIndex) -> Result<(), CoexError> {
        Ok(())
    }
}

struct Clock(CoexTimerClock);

impl CoexClockHardware for Clock {
    fn sample(&mut self) -> Result<CoexTimerClock, CoexError> {
        Ok(self.0)
    }
}

#[test]
fn single_owner_serializes_request_release_and_shutdown() {
    let mut resources = CoexResources::<NoopRawMutex, 2>::new();
    let (mut control, owner) = resources.split();
    let mut core = CoexCore::new(CoexPtiTable::reviewed_vendor());
    let mut hardware = Hardware::default();
    let mut clock = Clock(CoexTimerClock::from_hardware_fields(
        CoexClockSelector::Selector8,
        0,
        40,
        true,
    ));
    let request = CoexClientRequest {
        event: CoexEventId::new(1).unwrap(),
        latency: 1_000,
        duration: 2_000,
    };

    block_on(join(
        async {
            assert_eq!(
                control.execute(CoexCommand::Enable).await,
                Ok(CoexOutcome::Status(CoexStatus {
                    enabled: true,
                    active_timers: 0,
                    uncertain_timers: 0,
                }))
            );
            assert_eq!(
                control.execute(CoexCommand::WifiRequest(request)).await,
                Ok(CoexOutcome::Request(CoexTimerIndex::new(0).unwrap()))
            );
            assert_eq!(
                control.execute(CoexCommand::Release(request.event)).await,
                Ok(CoexOutcome::Release(CoexTimerIndex::new(0).unwrap()))
            );
            assert_eq!(
                control.execute(CoexCommand::Shutdown).await,
                Ok(CoexOutcome::Stopped)
            );
        },
        owner.run(&mut core, &mut hardware, &mut clock),
    ))
    .1
    .unwrap();

    assert_eq!(hardware.enabled, 0);
    assert_eq!(hardware.disabled, 1);
}

#[test]
fn cancelled_commands_settle_before_publishing_the_next_command() {
    use core::{
        future::Future,
        task::{Context, Poll, Waker},
    };
    let mut resources = CoexResources::<NoopRawMutex, 2>::new();
    let (mut control, owner) = resources.split();
    let mut core = CoexCore::new(CoexPtiTable::reviewed_vendor());
    let mut hardware = Hardware::default();
    let mut clock = Clock(CoexTimerClock::from_hardware_fields(
        CoexClockSelector::Selector8,
        0,
        40,
        true,
    ));
    let mut context = Context::from_waker(Waker::noop());
    {
        let mut enable = core::pin::pin!(control.execute(CoexCommand::Enable));
        assert!(enable.as_mut().poll(&mut context).is_pending());
    }
    // Cancelling another call while it settles the old response must neither
    // publish Disable nor forget that Enable remains outstanding.
    {
        let mut disable = core::pin::pin!(control.execute(CoexCommand::Disable));
        assert!(disable.as_mut().poll(&mut context).is_pending());
    }
    let mut runner = core::pin::pin!(owner.run(&mut core, &mut hardware, &mut clock));
    assert!(runner.as_mut().poll(&mut context).is_pending());
    {
        let mut disable = core::pin::pin!(control.execute(CoexCommand::Disable));
        assert!(disable.as_mut().poll(&mut context).is_pending());
        assert!(runner.as_mut().poll(&mut context).is_pending());
        assert_eq!(
            disable.as_mut().poll(&mut context),
            Poll::Ready(Ok(CoexOutcome::Status(CoexStatus {
                enabled: false,
                active_timers: 0,
                uncertain_timers: 0,
            })))
        );
    }
    let mut shutdown = core::pin::pin!(control.execute(CoexCommand::Shutdown));
    assert!(shutdown.as_mut().poll(&mut context).is_pending());
    assert_eq!(runner.as_mut().poll(&mut context), Poll::Ready(Ok(())));
    assert_eq!(
        shutdown.as_mut().poll(&mut context),
        Poll::Ready(Ok(CoexOutcome::Stopped))
    );
}

#[test]
fn a_new_epoch_discards_abandoned_commands_and_responses() {
    use core::{
        future::Future,
        task::{Context, Poll, Waker},
    };
    for process_old_command in [false, true] {
        let mut resources = CoexResources::<NoopRawMutex, 2>::new();
        let mut core = CoexCore::new(CoexPtiTable::reviewed_vendor());
        let mut hardware = Hardware::default();
        let mut clock = Clock(CoexTimerClock::from_hardware_fields(
            CoexClockSelector::Selector8,
            0,
            40,
            true,
        ));
        let mut context = Context::from_waker(Waker::noop());
        {
            let (mut control, owner) = resources.split();
            {
                let mut enable = core::pin::pin!(control.execute(CoexCommand::Enable));
                assert!(enable.as_mut().poll(&mut context).is_pending());
            }
            if process_old_command {
                let mut runner = core::pin::pin!(owner.run(&mut core, &mut hardware, &mut clock));
                assert!(runner.as_mut().poll(&mut context).is_pending());
            }
        }
        let (mut control, owner) = resources.split();
        let mut runner = core::pin::pin!(owner.run(&mut core, &mut hardware, &mut clock));
        let mut disable = core::pin::pin!(control.execute(CoexCommand::Disable));
        assert!(disable.as_mut().poll(&mut context).is_pending());
        assert!(runner.as_mut().poll(&mut context).is_pending());
        assert_eq!(
            disable.as_mut().poll(&mut context),
            Poll::Ready(Ok(CoexOutcome::Status(CoexStatus {
                enabled: false,
                active_timers: 0,
                uncertain_timers: 0,
            })))
        );
    }
}

#[test]
fn cancelled_shutdown_ends_the_control_epoch_without_waiting_for_a_dead_owner() {
    use core::{
        future::Future,
        task::{Context, Poll, Waker},
    };
    let mut resources = CoexResources::<NoopRawMutex, 2>::new();
    let (mut control, owner) = resources.split();
    let mut core = CoexCore::new(CoexPtiTable::reviewed_vendor());
    let mut hardware = Hardware::default();
    let mut clock = Clock(CoexTimerClock::from_hardware_fields(
        CoexClockSelector::Selector8,
        0,
        40,
        true,
    ));
    let mut context = Context::from_waker(Waker::noop());
    {
        let mut shutdown = core::pin::pin!(control.execute(CoexCommand::Shutdown));
        assert!(shutdown.as_mut().poll(&mut context).is_pending());
    }
    let mut runner = core::pin::pin!(owner.run(&mut core, &mut hardware, &mut clock));
    assert_eq!(runner.as_mut().poll(&mut context), Poll::Ready(Ok(())));
    for _ in 0..2 {
        let mut status = core::pin::pin!(control.execute(CoexCommand::Status));
        assert_eq!(
            status.as_mut().poll(&mut context),
            Poll::Ready(Err(CoexControlError::Stopped))
        );
    }
}

#[test]
fn dropping_owner_wakes_the_request_and_permanently_closes_control() {
    use core::{
        future::Future,
        task::{Context, Poll},
    };
    use std::{
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        task::Wake,
    };
    struct Wakes(AtomicUsize);
    impl Wake for Wakes {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
        fn wake_by_ref(self: &Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    for before_request in [false, true] {
        let mut resources = CoexResources::<NoopRawMutex, 1>::new();
        let (mut control, owner) = resources.split();
        let mut owner = Some(owner);
        if before_request {
            drop(owner.take());
        }
        let wakes = Arc::new(Wakes(AtomicUsize::new(0)));
        let waker = wakes.clone().into();
        let mut context = Context::from_waker(&waker);
        {
            let mut request = core::pin::pin!(control.execute(CoexCommand::Status));
            if !before_request {
                assert!(request.as_mut().poll(&mut context).is_pending());
                drop(owner.take());
                assert!(wakes.0.load(Ordering::SeqCst) > 0);
            }
            assert_eq!(
                request.as_mut().poll(&mut context),
                Poll::Ready(Err(CoexControlError::OwnerGone))
            );
        }
        let mut another = core::pin::pin!(control.execute(CoexCommand::Shutdown));
        assert_eq!(
            another.as_mut().poll(&mut context),
            Poll::Ready(Err(CoexControlError::OwnerGone))
        );
    }
}

#[test]
fn cancellation_preserves_a_published_result_but_does_not_claim_cleanup() {
    use core::{
        future::Future,
        task::{Context, Poll, Waker},
    };
    let mut resources = CoexResources::<NoopRawMutex, 1>::new();
    let (mut control, owner) = resources.split();
    let mut core = CoexCore::new(CoexPtiTable::reviewed_vendor());
    let mut hardware = Hardware::default();
    let mut clock = Clock(CoexTimerClock::from_hardware_fields(
        CoexClockSelector::Selector8,
        0,
        40,
        true,
    ));
    let mut context = Context::from_waker(Waker::noop());
    let mut enable = core::pin::pin!(control.execute(CoexCommand::Enable));
    assert!(enable.as_mut().poll(&mut context).is_pending());
    {
        let mut runner = core::pin::pin!(owner.run(&mut core, &mut hardware, &mut clock));
        assert!(runner.as_mut().poll(&mut context).is_pending());
    }
    assert!(core.status().enabled);
    assert_eq!(
        enable.as_mut().poll(&mut context),
        Poll::Ready(Ok(CoexOutcome::Status(core.status())))
    );
}
