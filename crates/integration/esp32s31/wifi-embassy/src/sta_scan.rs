//! Finite ESP32-S31 station-scan transaction composition.
//!
//! The chip-independent lifecycle service owns channel-plan progress and
//! candidate policy. This module owns the ESP32-S31 transaction order shared
//! by cold scan and future running rescan. Concrete owners retain PAC, PHY,
//! RX-DMA, TX and observation storage and implement only the primitive port
//! operations below.

use core::{future::Future, marker::PhantomData};

use open_esp_radio_wifi_lifecycle::scan::{
    StaCandidateScanBackend, StaScanChannelContext, StaScanSelectionOutcome, StaScanStepOutcome,
};

/// Result of the optional active-probe edge.
///
/// Probe failure is deliberately not a scan failure. The concrete owner must
/// close any failed TX publication before returning `PassiveFallback`; the
/// bounded receive dwell then continues as a passive scan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31ActiveProbeOutcome {
    Transmitted,
    PassiveFallback,
}

/// Exact mandatory transaction edge which failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31StaScanError<E> {
    Begin(E),
    ChannelSwitch(E),
    ReceiveStart(E),
    ReceiveObserve(E),
    DwellWait(E),
    ReceiveStop(E),
    PrepareNextRing(E),
    CandidateSelection(E),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31StaScanConfigError {
    ZeroDwellTicks,
}

/// Bounded executor policy for one channel visit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31StaScanConfig {
    dwell_ticks: u16,
}

impl Esp32s31StaScanConfig {
    pub const fn new(dwell_ticks: u16) -> Result<Self, Esp32s31StaScanConfigError> {
        if dwell_ticks == 0 {
            Err(Esp32s31StaScanConfigError::ZeroDwellTicks)
        } else {
            Ok(Self { dwell_ticks })
        }
    }

    pub const fn dwell_ticks(self) -> u16 {
        self.dwell_ticks
    }
}

/// Primitive operations retained by a concrete cold or running scan owner.
///
/// `start_receive` must either establish a live RX epoch or leave the walker
/// stopped on error. `observe_receive` performs one finite drain/recycle pass;
/// it must not wait. `stop_receive` must confirm that DMA released descriptor
/// ownership before returning success. `prepare_next_ring` runs only after
/// that confirmation and must leave a stopped ring ready for the next channel.
pub trait Esp32s31StaScanPort {
    type Channel: Copy;
    type Candidate;
    type Error;

    fn begin_scan(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_;

    fn switch_channel(
        &mut self,
        context: StaScanChannelContext<Self::Channel>,
    ) -> impl Future<Output = Result<(), Self::Error>> + '_;

    fn start_receive(
        &mut self,
        context: StaScanChannelContext<Self::Channel>,
    ) -> impl Future<Output = Result<(), Self::Error>> + '_;

    fn transmit_active_probe(
        &mut self,
        context: StaScanChannelContext<Self::Channel>,
    ) -> impl Future<Output = Esp32s31ActiveProbeOutcome> + '_;

    fn observe_receive(
        &mut self,
        context: StaScanChannelContext<Self::Channel>,
    ) -> Result<(), Self::Error>;

    fn wait_dwell_tick(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_;

    fn stop_receive(
        &mut self,
        context: StaScanChannelContext<Self::Channel>,
    ) -> Result<(), Self::Error>;

    fn prepare_next_ring(
        &mut self,
        context: StaScanChannelContext<Self::Channel>,
    ) -> Result<(), Self::Error>;

    fn select_candidate(&mut self) -> Result<Option<Self::Candidate>, Self::Error>;
}

/// Production ESP32-S31 transaction adapter for `StaCandidateScanService`.
///
/// The owner type remains explicit so the compiler cannot conflate a cold PAC
/// owner with a later running-rescan owner. Both can nevertheless reuse this
/// exact ordering and failure cleanup.
pub struct Esp32s31StaScanBackend<O> {
    config: Esp32s31StaScanConfig,
    _owner: PhantomData<fn() -> O>,
}

impl<O> Esp32s31StaScanBackend<O> {
    pub const fn new(config: Esp32s31StaScanConfig) -> Self {
        Self {
            config,
            _owner: PhantomData,
        }
    }

    pub const fn config(&self) -> Esp32s31StaScanConfig {
        self.config
    }
}

impl<O> StaCandidateScanBackend for Esp32s31StaScanBackend<O>
where
    O: Esp32s31StaScanPort,
{
    type Owner = O;
    type Channel = O::Channel;
    type Candidate = O::Candidate;
    type Error = Esp32s31StaScanError<O::Error>;

    fn begin_scan(
        &mut self,
        mut owner: Self::Owner,
    ) -> impl Future<Output = StaScanStepOutcome<Self::Owner, Self::Error>> + '_ {
        async move {
            match owner.begin_scan().await {
                Ok(()) => StaScanStepOutcome::Completed { owner },
                Err(error) => StaScanStepOutcome::Failed {
                    owner,
                    error: Esp32s31StaScanError::Begin(error),
                },
            }
        }
    }

    fn scan_channel(
        &mut self,
        mut owner: Self::Owner,
        context: StaScanChannelContext<Self::Channel>,
    ) -> impl Future<Output = StaScanStepOutcome<Self::Owner, Self::Error>> + '_ {
        async move {
            if let Err(error) = owner.switch_channel(context).await {
                return StaScanStepOutcome::Failed {
                    owner,
                    error: Esp32s31StaScanError::ChannelSwitch(error),
                };
            }
            if let Err(error) = owner.start_receive(context).await {
                return StaScanStepOutcome::Failed {
                    owner,
                    error: Esp32s31StaScanError::ReceiveStart(error),
                };
            }

            let _probe = owner.transmit_active_probe(context).await;
            let mut dwell_failure = None;
            for _ in 0..self.config.dwell_ticks() {
                if let Err(error) = owner.observe_receive(context) {
                    dwell_failure = Some(Esp32s31StaScanError::ReceiveObserve(error));
                    break;
                }
                if let Err(error) = owner.wait_dwell_tick().await {
                    dwell_failure = Some(Esp32s31StaScanError::DwellWait(error));
                    break;
                }
            }

            // Always try to close a live RX epoch after dwell began. A stop
            // failure takes precedence because descriptor ownership is then
            // uncertain and no ring mutation or retry is safe.
            if let Err(error) = owner.stop_receive(context) {
                return StaScanStepOutcome::Failed {
                    owner,
                    error: Esp32s31StaScanError::ReceiveStop(error),
                };
            }
            if let Some(error) = dwell_failure {
                return StaScanStepOutcome::Failed { owner, error };
            }

            if !context.is_last()
                && let Err(error) = owner.prepare_next_ring(context)
            {
                return StaScanStepOutcome::Failed {
                    owner,
                    error: Esp32s31StaScanError::PrepareNextRing(error),
                };
            }
            StaScanStepOutcome::Completed { owner }
        }
    }

    fn select_candidate(
        &mut self,
        mut owner: Self::Owner,
    ) -> StaScanSelectionOutcome<Self::Owner, Self::Candidate, Self::Error> {
        match owner.select_candidate() {
            Ok(Some(candidate)) => StaScanSelectionOutcome::Selected { owner, candidate },
            Ok(None) => StaScanSelectionOutcome::NoCandidate { owner },
            Err(error) => StaScanSelectionOutcome::Failed {
                owner,
                error: Esp32s31StaScanError::CandidateSelection(error),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use core::{
        future::ready,
        pin::pin,
        task::{Context, Poll},
    };
    use std::vec::Vec;

    use open_esp_radio_wifi_lifecycle::scan::{StaCandidateScanExit, StaCandidateScanService};

    use super::*;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Action {
        Begin,
        Switch(u8),
        Start(u8),
        Probe(u8),
        Observe(u8),
        Wait,
        Stop(u8),
        Prepare(u8),
        Select,
    }

    struct Owner {
        identity: u32,
        actions: Vec<Action>,
        fail: Option<Action>,
        probe_fallback: bool,
        candidate: Option<u8>,
    }

    impl Owner {
        fn new(identity: u32) -> Self {
            Self {
                identity,
                actions: Vec::new(),
                fail: None,
                probe_fallback: false,
                candidate: Some(11),
            }
        }

        fn record(&mut self, action: Action) -> Result<(), Action> {
            self.actions.push(action);
            if self.fail == Some(action) {
                Err(action)
            } else {
                Ok(())
            }
        }
    }

    impl Esp32s31StaScanPort for Owner {
        type Channel = u8;
        type Candidate = u8;
        type Error = Action;

        fn begin_scan(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_ {
            ready(self.record(Action::Begin))
        }

        fn switch_channel(
            &mut self,
            context: StaScanChannelContext<Self::Channel>,
        ) -> impl Future<Output = Result<(), Self::Error>> + '_ {
            ready(self.record(Action::Switch(context.channel)))
        }

        fn start_receive(
            &mut self,
            context: StaScanChannelContext<Self::Channel>,
        ) -> impl Future<Output = Result<(), Self::Error>> + '_ {
            ready(self.record(Action::Start(context.channel)))
        }

        fn transmit_active_probe(
            &mut self,
            context: StaScanChannelContext<Self::Channel>,
        ) -> impl Future<Output = Esp32s31ActiveProbeOutcome> + '_ {
            self.actions.push(Action::Probe(context.channel));
            ready(if self.probe_fallback {
                Esp32s31ActiveProbeOutcome::PassiveFallback
            } else {
                Esp32s31ActiveProbeOutcome::Transmitted
            })
        }

        fn observe_receive(
            &mut self,
            context: StaScanChannelContext<Self::Channel>,
        ) -> Result<(), Self::Error> {
            self.record(Action::Observe(context.channel))
        }

        fn wait_dwell_tick(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_ {
            ready(self.record(Action::Wait))
        }

        fn stop_receive(
            &mut self,
            context: StaScanChannelContext<Self::Channel>,
        ) -> Result<(), Self::Error> {
            self.record(Action::Stop(context.channel))
        }

        fn prepare_next_ring(
            &mut self,
            context: StaScanChannelContext<Self::Channel>,
        ) -> Result<(), Self::Error> {
            self.record(Action::Prepare(context.channel))
        }

        fn select_candidate(&mut self) -> Result<Option<Self::Candidate>, Self::Error> {
            self.record(Action::Select)?;
            Ok(self.candidate)
        }
    }

    fn block_on<F: Future>(future: F) -> F::Output {
        let mut future = pin!(future);
        let mut context = Context::from_waker(core::task::Waker::noop());
        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(output) => return output,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    fn backend() -> Esp32s31StaScanBackend<Owner> {
        Esp32s31StaScanBackend::new(Esp32s31StaScanConfig::new(2).unwrap())
    }

    #[test]
    fn two_channels_preserve_the_complete_transaction_order() {
        let mut service = StaCandidateScanService::new(backend());
        let exit = block_on(service.run(Owner::new(41), &[1, 6]));
        let StaCandidateScanExit::Selected {
            owner, candidate, ..
        } = exit
        else {
            panic!("scan must select the planned candidate")
        };

        assert_eq!(owner.identity, 41);
        assert_eq!(candidate, 11);
        assert_eq!(
            owner.actions,
            [
                Action::Begin,
                Action::Switch(1),
                Action::Start(1),
                Action::Probe(1),
                Action::Observe(1),
                Action::Wait,
                Action::Observe(1),
                Action::Wait,
                Action::Stop(1),
                Action::Prepare(1),
                Action::Switch(6),
                Action::Start(6),
                Action::Probe(6),
                Action::Observe(6),
                Action::Wait,
                Action::Observe(6),
                Action::Wait,
                Action::Stop(6),
                Action::Select,
            ]
        );
    }

    #[test]
    fn passive_probe_fallback_does_not_abort_the_receive_dwell() {
        let mut owner = Owner::new(7);
        owner.probe_fallback = true;
        let mut service = StaCandidateScanService::new(backend());

        let exit = block_on(service.run(owner, &[3]));

        assert!(matches!(
            exit,
            StaCandidateScanExit::Selected {
                owner: Owner { identity: 7, .. },
                candidate: 11,
                ..
            }
        ));
    }

    #[test]
    fn dwell_failure_still_stops_rx_and_returns_the_exact_owner() {
        let mut owner = Owner::new(99);
        owner.fail = Some(Action::Observe(4));
        let mut service = StaCandidateScanService::new(backend());

        let exit = block_on(service.run(owner, &[4]));
        let StaCandidateScanExit::Failed { owner, error, .. } = exit else {
            panic!("planned dwell failure must be returned")
        };

        assert_eq!(owner.identity, 99);
        assert_eq!(
            error,
            Esp32s31StaScanError::ReceiveObserve(Action::Observe(4))
        );
        assert_eq!(
            owner.actions,
            [
                Action::Begin,
                Action::Switch(4),
                Action::Start(4),
                Action::Probe(4),
                Action::Observe(4),
                Action::Stop(4),
            ]
        );
    }

    #[test]
    fn stop_failure_takes_precedence_over_an_earlier_dwell_failure() {
        struct StopFailureOwner(Owner);

        impl Esp32s31StaScanPort for StopFailureOwner {
            type Channel = u8;
            type Candidate = u8;
            type Error = Action;

            fn begin_scan(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_ {
                self.0.begin_scan()
            }

            fn switch_channel(
                &mut self,
                context: StaScanChannelContext<Self::Channel>,
            ) -> impl Future<Output = Result<(), Self::Error>> + '_ {
                self.0.switch_channel(context)
            }

            fn start_receive(
                &mut self,
                context: StaScanChannelContext<Self::Channel>,
            ) -> impl Future<Output = Result<(), Self::Error>> + '_ {
                self.0.start_receive(context)
            }

            fn transmit_active_probe(
                &mut self,
                context: StaScanChannelContext<Self::Channel>,
            ) -> impl Future<Output = Esp32s31ActiveProbeOutcome> + '_ {
                self.0.transmit_active_probe(context)
            }

            fn observe_receive(
                &mut self,
                context: StaScanChannelContext<Self::Channel>,
            ) -> Result<(), Self::Error> {
                self.0.record(Action::Observe(context.channel))?;
                Err(Action::Observe(context.channel))
            }

            fn wait_dwell_tick(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_ {
                self.0.wait_dwell_tick()
            }

            fn stop_receive(
                &mut self,
                context: StaScanChannelContext<Self::Channel>,
            ) -> Result<(), Self::Error> {
                self.0.actions.push(Action::Stop(context.channel));
                Err(Action::Stop(context.channel))
            }

            fn prepare_next_ring(
                &mut self,
                context: StaScanChannelContext<Self::Channel>,
            ) -> Result<(), Self::Error> {
                self.0.prepare_next_ring(context)
            }

            fn select_candidate(&mut self) -> Result<Option<Self::Candidate>, Self::Error> {
                self.0.select_candidate()
            }
        }

        let owner = StopFailureOwner(Owner::new(123));
        let backend = Esp32s31StaScanBackend::new(Esp32s31StaScanConfig::new(1).unwrap());
        let mut service = StaCandidateScanService::new(backend);
        let exit = block_on(service.run(owner, &[9]));

        assert!(matches!(
            exit,
            StaCandidateScanExit::Failed {
                owner: StopFailureOwner(Owner { identity: 123, .. }),
                error: Esp32s31StaScanError::ReceiveStop(Action::Stop(9)),
                ..
            }
        ));
    }
}
