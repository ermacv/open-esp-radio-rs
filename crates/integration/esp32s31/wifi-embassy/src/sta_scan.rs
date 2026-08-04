//! Finite ESP32-S31 station-scan transaction composition.
//!
//! The chip-independent lifecycle service owns channel-plan progress and
//! candidate policy. This module owns the ESP32-S31 transaction order shared
//! by cold scan and future running rescan. Concrete owners retain PAC, PHY,
//! RX-DMA, TX and observation storage and implement only the primitive port
//! operations below.

use core::{future::Future, marker::PhantomData};

use open_esp_radio_esp32s31_wifi_mac::rx::{
    RxDma, RxIngressConfig, RxReloadObservation, RxRingError, RxRingHalted, RxRingLive,
    RxRingStopped, RxSegment, extract_management,
};
use open_esp_radio_ieee80211::scan::{ScanObservation, ScanTable};
use open_esp_radio_wifi_lifecycle::scan::{
    StaCandidateScanBackend, StaScanChannelContext, StaScanSelectionOutcome, StaScanStepOutcome,
};

use crate::rx_backend::{Esp32s31RxDmaBuffer, Esp32s31RxDmaStorage};

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
    ) -> impl Future<Output = Result<Esp32s31ActiveProbeOutcome, Self::Error>> + '_;

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
            if let Err(error) = owner.stop_receive(context) {
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

/// Hardware state held by [`Esp32s31ScanRx`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31ScanRxPhase {
    Prepared,
    Live,
    Halted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31ScanRxError {
    InvalidPhase {
        expected: Esp32s31ScanRxPhase,
        actual: Esp32s31ScanRxPhase,
    },
    Ring(RxRingError),
}

impl From<RxRingError> for Esp32s31ScanRxError {
    fn from(error: RxRingError) -> Self {
        Self::Ring(error)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Esp32s31ScanRxProgress {
    pub completed_descriptors: u32,
    pub parsed_management_frames: u32,
    pub inserted_records: u32,
    pub updated_records: u32,
    pub malformed_or_irrelevant_frames: u32,
    pub recycled_descriptors: u32,
    pub reload_pending: bool,
}

/// Optional observer for successfully extracted scan frames.
///
/// The frame borrow ends before any descriptor can be recycled. Production
/// policy should normally use the owned `ScanTable`; this hook exists for
/// qualification counters and diagnostics which must inspect the addressed
/// Probe Response without retaining DMA-backed memory.
pub trait Esp32s31ScanFrameObserver {
    fn observe(&mut self, frame: &[u8], rssi: i8, table_outcome: ScanObservation);
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NoopEsp32s31ScanFrameObserver;

impl Esp32s31ScanFrameObserver for NoopEsp32s31ScanFrameObserver {
    fn observe(&mut self, _frame: &[u8], _rssi: i8, _table_outcome: ScanObservation) {}
}

/// One channel's borrowed observation destinations.
///
/// Bundling them prevents the RX hot path from growing another positional
/// argument list as scan telemetry evolves.
pub struct Esp32s31ScanObservationContext<'a, O, const RECORDS: usize> {
    channel: u8,
    frame: &'a mut [u8],
    table: &'a mut ScanTable<RECORDS>,
    observer: &'a mut O,
}

impl<'a, O, const RECORDS: usize> Esp32s31ScanObservationContext<'a, O, RECORDS> {
    pub fn new(
        channel: u8,
        frame: &'a mut [u8],
        table: &'a mut ScanTable<RECORDS>,
        observer: &'a mut O,
    ) -> Self {
        Self {
            channel,
            frame,
            table,
            observer,
        }
    }
}

enum Esp32s31ScanRxState<'storage, const COUNT: usize> {
    Prepared(RxRingStopped<'storage, COUNT>),
    Live(RxRingLive<'storage, COUNT>),
    Halted(RxRingHalted<'storage, COUNT>),
    Vacant,
}

impl<const COUNT: usize> Esp32s31ScanRxState<'_, COUNT> {
    const fn phase(&self) -> Esp32s31ScanRxPhase {
        match self {
            Self::Prepared(_) => Esp32s31ScanRxPhase::Prepared,
            Self::Live(_) => Esp32s31ScanRxPhase::Live,
            Self::Halted(_) => Esp32s31ScanRxPhase::Halted,
            Self::Vacant => unreachable!(),
        }
    }
}

/// Production RX-ring owner shared by cold scan and running rescan.
///
/// Unlike the former HIL mask, this value carries the MAC crate's unique ring
/// capability across `Prepared -> Live -> Halted`. A completed scan can hand
/// the exact halted ring to Authentication; no later phase needs to recreate
/// descriptor authority from addresses.
pub struct Esp32s31ScanRx<
    'storage,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> {
    state: Esp32s31ScanRxState<'storage, COUNT>,
    buffers: &'storage [Esp32s31RxDmaBuffer<DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>; COUNT],
}

impl<'storage, const COUNT: usize, const DMA_BUFFER_SIZE: usize, const DMA_STORAGE_SIZE: usize>
    Esp32s31ScanRx<'storage, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
{
    /// Prepare the first cold scan epoch under the caller's unique PAC owner.
    pub fn prepare_initial<H: RxDma>(
        hardware: &mut H,
        storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        descriptor_base: u32,
        buffer_addresses: &'storage [u32; COUNT],
    ) -> Result<Self, RxRingError> {
        if DMA_BUFFER_SIZE > u32::MAX as usize {
            return Err(RxRingError::Size);
        }
        let ring = RxRingStopped::prepare(
            hardware,
            storage.descriptors(),
            descriptor_base,
            buffer_addresses,
            DMA_BUFFER_SIZE as u32,
            |index| {
                // SAFETY: `prepare` first confirms that the walker is stopped
                // and transfers each matching buffer to this closure.
                unsafe { storage.buffers()[index].prepare_for_recycle() }
            },
        )?;
        Ok(Self {
            state: Esp32s31ScanRxState::Prepared(ring),
            buffers: storage.buffers(),
        })
    }

    /// Reuse a hardware-confirmed halted ring for a running rescan.
    pub const fn from_halted(
        ring: RxRingHalted<'storage, COUNT>,
        buffers: &'storage [Esp32s31RxDmaBuffer<DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>; COUNT],
    ) -> Self {
        Self {
            state: Esp32s31ScanRxState::Halted(ring),
            buffers,
        }
    }

    pub const fn phase(&self) -> Esp32s31ScanRxPhase {
        self.state.phase()
    }

    pub fn start<H: RxDma>(&mut self, hardware: &mut H) -> Result<(), Esp32s31ScanRxError> {
        let state = core::mem::replace(&mut self.state, Esp32s31ScanRxState::Vacant);
        let Esp32s31ScanRxState::Prepared(ring) = state else {
            let actual = state.phase();
            self.state = state;
            return Err(Esp32s31ScanRxError::InvalidPhase {
                expected: Esp32s31ScanRxPhase::Prepared,
                actual,
            });
        };
        match ring.try_start(hardware) {
            Ok(ring) => {
                self.state = Esp32s31ScanRxState::Live(ring);
                Ok(())
            }
            Err((ring, error)) => {
                self.state = Esp32s31ScanRxState::Prepared(ring);
                Err(error.into())
            }
        }
    }

    /// Drain the current completion frontier, copy scan frames into bounded
    /// caller storage and promptly recycle the contiguous observed prefix.
    pub fn observe_management<H, O, const RECORDS: usize>(
        &mut self,
        hardware: &mut H,
        context: &mut Esp32s31ScanObservationContext<'_, O, RECORDS>,
    ) -> Result<Esp32s31ScanRxProgress, Esp32s31ScanRxError>
    where
        H: RxDma,
        O: Esp32s31ScanFrameObserver,
    {
        let actual = self.state.phase();
        let Esp32s31ScanRxState::Live(ring) = &mut self.state else {
            return Err(Esp32s31ScanRxError::InvalidPhase {
                expected: Esp32s31ScanRxPhase::Live,
                actual,
            });
        };
        let mut progress = Esp32s31ScanRxProgress::default();
        progress.reload_pending =
            ring.poll_pending_reload(hardware)? == RxReloadObservation::Pending;

        for index in 0..COUNT {
            let Some(completed) = ring.take_completed(index) else {
                continue;
            };
            progress.completed_descriptors = progress.completed_descriptors.saturating_add(1);
            let buffer = unsafe {
                // The live ring transferred this completed descriptor and its
                // matching buffer to the unique scan owner.
                self.buffers[completed.index()].completed()
            };
            let rssi = buffer[0] as i8;
            let segment = RxSegment {
                descriptor_address: completed.descriptor_address(),
                descriptor_word0: completed.word0(),
                buffer,
                next_descriptor_address: completed.next_descriptor_address(),
            };
            match extract_management(
                core::slice::from_ref(&segment),
                RxIngressConfig {
                    ring_entry_limit: 1,
                    csi_config: 0,
                    flags: 0,
                },
                context.frame,
            ) {
                Ok(frame) => {
                    progress.parsed_management_frames =
                        progress.parsed_management_frames.saturating_add(1);
                    let frame = &context.frame[..frame.length];
                    let table_outcome =
                        context
                            .table
                            .observe_management(frame, context.channel, rssi);
                    match table_outcome {
                        ScanObservation::Inserted { .. } => {
                            progress.inserted_records = progress.inserted_records.saturating_add(1)
                        }
                        ScanObservation::Updated { .. } => {
                            progress.updated_records = progress.updated_records.saturating_add(1)
                        }
                        _ => {}
                    }
                    context.observer.observe(frame, rssi, table_outcome);
                }
                Err(_) => {
                    progress.malformed_or_irrelevant_frames =
                        progress.malformed_or_irrelevant_frames.saturating_add(1);
                }
            }
        }

        if !progress.reload_pending
            && let Some(append) =
                ring.recycle_completed_prefix::<COUNT, _, _>(hardware, |index| {
                    // SAFETY: the live ring invokes this only for an observed
                    // descriptor immediately before republishing it to DMA.
                    unsafe { self.buffers[index].prepare_for_recycle() }
                })?
        {
            progress.recycled_descriptors = append.descriptor_count as u32;
        }
        Ok(progress)
    }

    pub fn stop<H: RxDma>(&mut self, hardware: &mut H) -> Result<(), Esp32s31ScanRxError> {
        let state = core::mem::replace(&mut self.state, Esp32s31ScanRxState::Vacant);
        let Esp32s31ScanRxState::Live(ring) = state else {
            let actual = state.phase();
            self.state = state;
            return Err(Esp32s31ScanRxError::InvalidPhase {
                expected: Esp32s31ScanRxPhase::Live,
                actual,
            });
        };
        match ring.try_stop(hardware) {
            Ok(ring) => {
                self.state = Esp32s31ScanRxState::Halted(ring);
                Ok(())
            }
            Err((ring, error)) => {
                self.state = Esp32s31ScanRxState::Live(ring);
                Err(error.into())
            }
        }
    }

    pub fn prepare_next<H: RxDma>(&mut self, hardware: &mut H) -> Result<(), Esp32s31ScanRxError> {
        let state = core::mem::replace(&mut self.state, Esp32s31ScanRxState::Vacant);
        let Esp32s31ScanRxState::Halted(ring) = state else {
            let actual = state.phase();
            self.state = state;
            return Err(Esp32s31ScanRxError::InvalidPhase {
                expected: Esp32s31ScanRxPhase::Halted,
                actual,
            });
        };
        match ring.prepare(hardware, DMA_BUFFER_SIZE as u32, |index| {
            // SAFETY: the halted ring proves that DMA released this buffer.
            unsafe { self.buffers[index].prepare_for_recycle() }
        }) {
            Ok(ring) => {
                self.state = Esp32s31ScanRxState::Prepared(ring);
                Ok(())
            }
            Err((ring, error)) => {
                self.state = Esp32s31ScanRxState::Halted(ring);
                Err(error.into())
            }
        }
    }

    pub fn into_halted(self) -> Result<RxRingHalted<'storage, COUNT>, Self> {
        match self.state {
            Esp32s31ScanRxState::Halted(ring) => Ok(ring),
            state => Err(Self {
                state,
                buffers: self.buffers,
            }),
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
        ) -> impl Future<Output = Result<Esp32s31ActiveProbeOutcome, Self::Error>> + '_ {
            self.actions.push(Action::Probe(context.channel));
            let outcome = if self.probe_fallback {
                Esp32s31ActiveProbeOutcome::PassiveFallback
            } else {
                Esp32s31ActiveProbeOutcome::Transmitted
            };
            ready(if self.fail == Some(Action::Probe(context.channel)) {
                Err(Action::Probe(context.channel))
            } else {
                Ok(outcome)
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
    fn fatal_probe_failure_still_closes_the_live_rx_epoch() {
        let mut owner = Owner::new(17);
        owner.fail = Some(Action::Probe(3));
        let mut service = StaCandidateScanService::new(backend());

        let exit = block_on(service.run(owner, &[3]));
        let StaCandidateScanExit::Failed { owner, error, .. } = exit else {
            panic!("fatal active-probe failure must be returned")
        };

        assert_eq!(error, Esp32s31StaScanError::ActiveProbe(Action::Probe(3)));
        assert_eq!(
            owner.actions,
            [
                Action::Begin,
                Action::Switch(3),
                Action::Start(3),
                Action::Probe(3),
                Action::Stop(3),
            ]
        );
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
            ) -> impl Future<Output = Result<Esp32s31ActiveProbeOutcome, Self::Error>> + '_
            {
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

    const RX_TEST_COUNT: usize = 2;
    const RX_TEST_BUFFER_SIZE: usize = 128;
    const RX_TEST_STORAGE_SIZE: usize = RX_TEST_BUFFER_SIZE + 4;
    const RX_TEST_BASE: u32 = 0x2f00_1000;
    const RX_TEST_BUFFERS: [u32; RX_TEST_COUNT] = [0x2f00_2000, 0x2f00_2080];

    #[derive(Default)]
    struct MockRxDma {
        walker: bool,
        fail_enable: bool,
        fail_disable: bool,
        descriptor_base: u32,
        reload_requests: u32,
    }

    impl RxDma for MockRxDma {
        fn last_descriptor_low(&mut self) -> u32 {
            0
        }

        fn next_descriptor_low(&mut self) -> u32 {
            RX_TEST_BASE + 12
        }

        fn walker_enabled(&mut self) -> bool {
            self.walker
        }

        fn reload_pending(&mut self) -> bool {
            false
        }

        fn set_descriptor_high_window(&mut self, _address_high: u16) {}

        fn write_descriptor_base(&mut self, address: u32) {
            self.descriptor_base = address;
        }

        fn publish_walker_enable(&mut self) {
            self.walker = true;
        }

        fn request_reload(&mut self) {
            self.reload_requests = self.reload_requests.saturating_add(1);
        }

        fn try_enable_walker(&mut self) -> bool {
            if self.fail_enable {
                false
            } else {
                self.walker = true;
                true
            }
        }

        fn try_disable_walker(&mut self) -> bool {
            if self.fail_disable {
                false
            } else {
                self.walker = false;
                true
            }
        }

        fn fence(&mut self) {}
    }

    #[derive(Default)]
    struct FrameObserver {
        frames: u32,
    }

    impl Esp32s31ScanFrameObserver for FrameObserver {
        fn observe(&mut self, _frame: &[u8], _rssi: i8, _table_outcome: ScanObservation) {
            self.frames = self.frames.saturating_add(1);
        }
    }

    fn complete_test_beacon(
        storage: &Esp32s31RxDmaStorage<RX_TEST_COUNT, RX_TEST_BUFFER_SIZE, RX_TEST_STORAGE_SIZE>,
    ) {
        use open_esp_radio_esp32s31_wifi_mac::descriptor::{BIT_30, BIT_31, LENGTH_SHIFT};

        const FRAME_LENGTH: usize = 43;
        const SIGNAL_LENGTH: usize = FRAME_LENGTH + 4;
        const FRAME_OFFSET: usize = 0x40;
        const RECEIVED_LENGTH: usize = FRAME_OFFSET + SIGNAL_LENGTH;

        let mut bytes = [0_u8; RX_TEST_BUFFER_SIZE];
        bytes[0] = (-42_i8) as u8;
        bytes[0x38..0x3c].copy_from_slice(
            &(((SIGNAL_LENGTH + 4) as u32) << 16 | SIGNAL_LENGTH as u32).to_le_bytes(),
        );
        let frame = &mut bytes[FRAME_OFFSET..FRAME_OFFSET + FRAME_LENGTH];
        frame[0] = 0x80;
        frame[10..16].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
        frame[16..22].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
        frame[32..34].copy_from_slice(&100_u16.to_le_bytes());
        frame[36..40].copy_from_slice(&[0, 2, b'a', b'p']);
        frame[40..43].copy_from_slice(&[3, 1, 6]);
        unsafe { storage.buffers()[0].write_test_bytes(0, &bytes) };
        storage.descriptors()[0].write_word0(
            RX_TEST_BUFFER_SIZE as u32 | (RECEIVED_LENGTH as u32) << LENGTH_SHIFT | BIT_30 | BIT_31,
        );
    }

    #[test]
    fn scan_rx_hands_the_exact_halted_ring_to_the_next_phase() {
        let storage =
            Esp32s31RxDmaStorage::<RX_TEST_COUNT, RX_TEST_BUFFER_SIZE, RX_TEST_STORAGE_SIZE>::new();
        let mut hardware = MockRxDma::default();
        let mut rx = Esp32s31ScanRx::prepare_initial(
            &mut hardware,
            &storage,
            RX_TEST_BASE,
            &RX_TEST_BUFFERS,
        )
        .unwrap();
        assert_eq!(rx.phase(), Esp32s31ScanRxPhase::Prepared);

        rx.start(&mut hardware).unwrap();
        complete_test_beacon(&storage);
        let mut table = ScanTable::<4>::new();
        let mut frame = [0_u8; 64];
        let mut observer = FrameObserver::default();
        let mut context =
            Esp32s31ScanObservationContext::new(6, &mut frame, &mut table, &mut observer);
        let progress = rx.observe_management(&mut hardware, &mut context).unwrap();

        assert_eq!(progress.completed_descriptors, 1);
        assert_eq!(progress.parsed_management_frames, 1);
        assert_eq!(progress.inserted_records, 1);
        assert_eq!(progress.recycled_descriptors, 1);
        assert_eq!(observer.frames, 1);
        assert_eq!(table.records()[0].ssid_bytes(), b"ap");
        assert_eq!(table.records()[0].channel, 6);

        rx.stop(&mut hardware).unwrap();
        let halted = match rx.into_halted() {
            Ok(halted) => halted,
            Err(_) => panic!("completed scan must expose its halted owner"),
        };
        assert_eq!(halted.descriptor_base(), RX_TEST_BASE);
        assert_eq!(halted.buffer_addresses(), &RX_TEST_BUFFERS);
    }

    #[test]
    fn scan_rx_retains_its_typed_phase_across_enable_and_disable_failure() {
        let storage =
            Esp32s31RxDmaStorage::<RX_TEST_COUNT, RX_TEST_BUFFER_SIZE, RX_TEST_STORAGE_SIZE>::new();
        let mut hardware = MockRxDma::default();
        let mut rx = Esp32s31ScanRx::prepare_initial(
            &mut hardware,
            &storage,
            RX_TEST_BASE,
            &RX_TEST_BUFFERS,
        )
        .unwrap();

        hardware.fail_enable = true;
        assert_eq!(
            rx.start(&mut hardware),
            Err(Esp32s31ScanRxError::Ring(RxRingError::Busy))
        );
        assert_eq!(rx.phase(), Esp32s31ScanRxPhase::Prepared);

        hardware.fail_enable = false;
        rx.start(&mut hardware).unwrap();
        hardware.fail_disable = true;
        assert_eq!(
            rx.stop(&mut hardware),
            Err(Esp32s31ScanRxError::Ring(RxRingError::Busy))
        );
        assert_eq!(rx.phase(), Esp32s31ScanRxPhase::Live);

        hardware.fail_disable = false;
        rx.stop(&mut hardware).unwrap();
        assert_eq!(rx.phase(), Esp32s31ScanRxPhase::Halted);
    }
}
