//! Executor-independent ESP32-S31 station scan transaction.
//!
//! The chip-independent STA service owns channel-plan progress and candidate
//! policy. This module owns the ESP32-S31 transaction order shared by cold
//! scan and running rescan. Concrete integrations retain PHY, RX-DMA, TX,
//! timer and observation storage and implement only the primitive port below.

use core::{future::Future, marker::PhantomData};

use open_esp_radio_wifi_sta::scan::{
    StaCandidateScanBackend, StaScanChannelContext, StaScanSelectionOutcome, StaScanStepOutcome,
};

/// Result of the optional active-probe edge.
///
/// Probe failure is deliberately not necessarily a scan failure. A concrete
/// port must close any failed TX publication before returning
/// [`PassiveFallback`](Self::PassiveFallback); the bounded receive dwell then
/// continues as a passive scan.
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
    ActiveProbe(E),
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

/// Bounded executor-neutral policy for one channel visit.
///
/// A tick has no duration at this layer. The integration port maps it to its
/// clock and executor, so the transaction is also usable by a non-Embassy
/// runtime.
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
    ) -> impl Future<Output = Result<Esp32s31ActiveProbeOutcome, Self::Error>> + '_;

    fn observe_receive(
        &mut self,
        context: StaScanChannelContext<Self::Channel>,
    ) -> Result<(), Self::Error>;

    fn wait_dwell_tick(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_;

    fn stop_receive(
        &mut self,
        context: StaScanChannelContext<Self::Channel>,
    ) -> impl Future<Output = Result<(), Self::Error>> + '_;

    fn prepare_next_ring(
        &mut self,
        context: StaScanChannelContext<Self::Channel>,
    ) -> Result<(), Self::Error>;

    fn select_candidate(&mut self) -> Result<Option<Self::Candidate>, Self::Error>;
}

/// ESP32-S31 transaction adapter for `StaCandidateScanService`.
///
/// The owner type remains explicit so the compiler cannot conflate a cold
/// radio owner with a later running-rescan owner. Both nevertheless reuse this
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

    async fn begin_scan(
        &mut self,
        mut owner: Self::Owner,
    ) -> StaScanStepOutcome<Self::Owner, Self::Error> {
        match owner.begin_scan().await {
            Ok(()) => StaScanStepOutcome::Completed { owner },
            Err(error) => StaScanStepOutcome::Failed {
                owner,
                error: Esp32s31StaScanError::Begin(error),
            },
        }
    }

    async fn scan_channel(
        &mut self,
        mut owner: Self::Owner,
        context: StaScanChannelContext<Self::Channel>,
    ) -> StaScanStepOutcome<Self::Owner, Self::Error> {
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

        let mut transaction_failure = match owner.transmit_active_probe(context).await {
            Ok(_probe) => None,
            Err(error) => Some(Esp32s31StaScanError::ActiveProbe(error)),
        };
        if transaction_failure.is_none() {
            for _ in 0..self.config.dwell_ticks() {
                if let Err(error) = owner.observe_receive(context) {
                    transaction_failure = Some(Esp32s31StaScanError::ReceiveObserve(error));
                    break;
                }
                if let Err(error) = owner.wait_dwell_tick().await {
                    transaction_failure = Some(Esp32s31StaScanError::DwellWait(error));
                    break;
                }
            }
        }

        // Always try to close a live RX epoch after dwell began. A stop
        // failure takes precedence because descriptor ownership is then
        // uncertain and no ring mutation or retry is safe.
        if let Err(error) = owner.stop_receive(context).await {
            return StaScanStepOutcome::Failed {
                owner,
                error: Esp32s31StaScanError::ReceiveStop(error),
            };
        }
        if let Some(error) = transaction_failure {
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
mod tests;
