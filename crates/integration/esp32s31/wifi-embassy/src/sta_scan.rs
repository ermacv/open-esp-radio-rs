//! Finite ESP32-S31 station-scan transaction composition.
//!
//! The chip-independent lifecycle service owns channel-plan progress and
//! candidate policy. This module owns the ESP32-S31 transaction order shared
//! by cold scan and future running rescan. Concrete owners retain PAC, PHY,
//! RX-DMA, TX and observation storage and implement only the primitive port
//! operations below.

use core::{future::Future, marker::PhantomData};

use embassy_sync::blocking_mutex::raw::RawMutex;
use open_esp_radio_esp32s31_pac::MacInterruptSetup;
use open_esp_radio_esp32s31_wifi_mac::{
    rx::{
        RxDma, RxIngressConfig, RxReloadObservation, RxRingError, RxRingHalted, RxRingLive,
        RxRingStopped, RxSegment, extract_management,
    },
    tx::{TxCompletion, TxHardware},
};
use open_esp_radio_ieee80211::{
    management::ProbeRequest,
    scan::{ScanObservation, ScanTable},
};
use open_esp_radio_wifi_lifecycle::scan::{
    StaCandidateScanBackend, StaScanChannelContext, StaScanSelectionOutcome, StaScanStepOutcome,
};

use crate::{
    control_tx::{ControlTxError, Esp32s31ControlTx},
    embassy_rx::RxReloadDelay,
    ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile, WifiTxTimer},
    rx_backend::{
        ESP32S31_RX_WALKER_ENABLE_SETTLE_US, Esp32s31RxDmaBuffer, Esp32s31RxDmaStorage,
        Esp32s31RxEpochResources, Esp32s31StoppedRx,
    },
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

/// Complete inputs for one active-scan Probe Request publication.
pub struct Esp32s31ScanProbeRequest<'a> {
    pub source: [u8; 6],
    pub sequence_number: u16,
    pub ssid: &'a [u8],
    pub supported_rates: &'a [u8],
    pub current_channel: Option<u8>,
    pub descriptor_capacity: Option<u32>,
}

/// Detailed terminal observation retained for HIL evidence and telemetry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31ScanProbeReport {
    Transmitted(TxCompletion),
    PassiveWithoutAttempt,
    PassiveAfterCompletion(TxCompletion),
    PassiveAfterError(ControlTxError),
}

impl Esp32s31ScanProbeReport {
    pub const fn outcome(self) -> Esp32s31ActiveProbeOutcome {
        match self {
            Self::Transmitted(_) => Esp32s31ActiveProbeOutcome::Transmitted,
            Self::PassiveWithoutAttempt
            | Self::PassiveAfterCompletion(_)
            | Self::PassiveAfterError(_) => Esp32s31ActiveProbeOutcome::PassiveFallback,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Esp32s31ScanTxSummary {
    pub completions: u32,
    pub failures: u32,
}

/// Shared state machine for cold and running active-probe publication.
pub(crate) struct Esp32s31ScanTxState {
    active_probe_available: bool,
    summary: Esp32s31ScanTxSummary,
}

impl Esp32s31ScanTxState {
    pub(crate) const fn new() -> Self {
        Self {
            active_probe_available: true,
            summary: Esp32s31ScanTxSummary {
                completions: 0,
                failures: 0,
            },
        }
    }

    pub(crate) fn begin_scan(&mut self) {
        self.active_probe_available = true;
        self.summary = Esp32s31ScanTxSummary::default();
    }

    pub(crate) const fn active_probe_available(&self) -> bool {
        self.active_probe_available
    }

    pub(crate) fn classify(
        &mut self,
        result: Result<TxCompletion, ControlTxError>,
    ) -> Result<Esp32s31ScanProbeReport, ControlTxError> {
        match result {
            Ok(completion) => {
                self.summary.completions = self.summary.completions.saturating_add(1);
                if completion.status == 0 {
                    Ok(Esp32s31ScanProbeReport::Transmitted(completion))
                } else {
                    self.summary.failures = self.summary.failures.saturating_add(1);
                    self.active_probe_available = false;
                    Ok(Esp32s31ScanProbeReport::PassiveAfterCompletion(completion))
                }
            }
            Err(error) if error.retains_quiescent_owner() => {
                self.summary.failures = self.summary.failures.saturating_add(1);
                self.active_probe_available = false;
                Ok(Esp32s31ScanProbeReport::PassiveAfterError(error))
            }
            Err(error) => {
                self.summary.failures = self.summary.failures.saturating_add(1);
                self.active_probe_available = false;
                Err(error)
            }
        }
    }

    pub(crate) const fn summary(&self) -> Esp32s31ScanTxSummary {
        self.summary
    }
}

/// Polling TX owner for a running rescan after the MAC IRQ epoch is quiesced.
///
/// The connected teardown returns the exact ordinary descriptor and disables
/// both CPU and peripheral interrupt routes before this owner can exist. Probe
/// completion may therefore use the same finite polling transaction as the
/// pre-connected path without racing the connected runner. Re-entering a
/// connected epoch consumes the returned control owner and reactivates IRQs.
pub struct Esp32s31RunningScanTx<'slot, P, E, T, const BUFFER_SIZE: usize> {
    control: Esp32s31ControlTx<'slot, P, E, T, BUFFER_SIZE>,
    state: Esp32s31ScanTxState,
}

impl<'slot, P, E, T, const BUFFER_SIZE: usize> Esp32s31RunningScanTx<'slot, P, E, T, BUFFER_SIZE>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
{
    pub const fn new(
        control: Esp32s31ControlTx<'slot, P, E, T, BUFFER_SIZE>,
        _interrupt_setup: &MacInterruptSetup,
    ) -> Self {
        Self {
            control,
            state: Esp32s31ScanTxState::new(),
        }
    }

    pub fn begin_scan(&mut self) {
        self.state.begin_scan();
    }

    pub async fn transmit_probe_request<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        request: Esp32s31ScanProbeRequest<'_>,
    ) -> Result<Esp32s31ScanProbeReport, ControlTxError> {
        if !self.state.active_probe_available() {
            return Ok(Esp32s31ScanProbeReport::PassiveWithoutAttempt);
        }
        let Esp32s31ScanProbeRequest {
            source,
            sequence_number,
            ssid,
            supported_rates,
            current_channel,
            descriptor_capacity,
        } = request;
        let result = self
            .control
            .transmit_probe_request(
                hardware,
                ProbeRequest {
                    source,
                    sequence_number,
                    ssid,
                    supported_rates,
                },
                current_channel,
                descriptor_capacity,
            )
            .await;
        self.state.classify(result)
    }

    pub fn into_parts(
        self,
    ) -> (
        Esp32s31ControlTx<'slot, P, E, T, BUFFER_SIZE>,
        Esp32s31ScanTxSummary,
    ) {
        (self.control, self.state.summary())
    }
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

/// Running-scan RX owner which retains every connected-epoch resource.
///
/// A connected teardown returns more than a halted descriptor ring: the
/// staging pool, queue sender, reload delay and telemetry binding must survive
/// candidate refresh too. This owner separates the halted ring only while the
/// finite scan runs and can then return either exact join parts or the original
/// stopped connected owner without recreating static storage.
pub struct Esp32s31RunningScanRx<
    'storage,
    'pool,
    'queue,
    D,
    M: RawMutex,
    const QUEUE_DEPTH: usize,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> {
    scan: Esp32s31ScanRx<'storage, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    resources: Esp32s31RxEpochResources<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >,
}

impl<
    'storage,
    'pool,
    'queue,
    D,
    M: RawMutex,
    const QUEUE_DEPTH: usize,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
>
    Esp32s31RunningScanRx<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >
{
    pub fn from_stopped(
        stopped: Esp32s31StoppedRx<
            'storage,
            'pool,
            'queue,
            D,
            M,
            QUEUE_DEPTH,
            COUNT,
            STAGE_CAPACITY,
            STAGE_SLOTS,
            DMA_BUFFER_SIZE,
            DMA_STORAGE_SIZE,
        >,
    ) -> Self {
        let (ring, resources) = stopped.into_epoch_parts();
        let scan = Esp32s31ScanRx::from_halted(ring, resources.buffers());
        Self { scan, resources }
    }

    pub const fn phase(&self) -> Esp32s31ScanRxPhase {
        self.scan.phase()
    }

    /// Rebuild the first ring of this running scan from its halted connected
    /// frontier. Later channel visits use [`Self::prepare_next`] identically.
    pub fn prepare_initial<H: RxDma>(
        &mut self,
        hardware: &mut H,
    ) -> Result<(), Esp32s31ScanRxError> {
        self.scan.prepare_next(hardware)
    }

    pub async fn start<H: RxDma>(&mut self, hardware: &mut H) -> Result<(), Esp32s31ScanRxError>
    where
        D: RxReloadDelay,
    {
        self.resources
            .delay_mut()
            .after_micros(ESP32S31_RX_WALKER_ENABLE_SETTLE_US)
            .await;
        self.scan.start(hardware)
    }

    pub fn observe_management<H, O, const RECORDS: usize>(
        &mut self,
        hardware: &mut H,
        context: &mut Esp32s31ScanObservationContext<'_, O, RECORDS>,
    ) -> Result<Esp32s31ScanRxProgress, Esp32s31ScanRxError>
    where
        H: RxDma,
        O: Esp32s31ScanFrameObserver,
    {
        self.scan.observe_management(hardware, context)
    }

    pub fn stop<H: RxDma>(&mut self, hardware: &mut H) -> Result<(), Esp32s31ScanRxError> {
        self.scan.stop(hardware)
    }

    pub fn prepare_next<H: RxDma>(&mut self, hardware: &mut H) -> Result<(), Esp32s31ScanRxError> {
        self.scan.prepare_next(hardware)
    }

    /// Hand the halted ring and all peer-independent RX resources directly to
    /// Authentication/Association without manufacturing another capability.
    #[allow(clippy::type_complexity)]
    pub fn into_epoch_parts(
        self,
    ) -> Result<
        (
            RxRingHalted<'storage, COUNT>,
            Esp32s31RxEpochResources<
                'storage,
                'pool,
                'queue,
                D,
                M,
                QUEUE_DEPTH,
                COUNT,
                STAGE_CAPACITY,
                STAGE_SLOTS,
                DMA_BUFFER_SIZE,
                DMA_STORAGE_SIZE,
            >,
        ),
        Self,
    > {
        let Self { scan, resources } = self;
        match scan.into_halted() {
            Ok(ring) => Ok((ring, resources)),
            Err(scan) => Err(Self { scan, resources }),
        }
    }

    /// Restore the same stopped connected owner when a scan exits without
    /// crossing into a pre-connected protocol epoch.
    pub fn into_stopped(
        self,
    ) -> Result<
        Esp32s31StoppedRx<
            'storage,
            'pool,
            'queue,
            D,
            M,
            QUEUE_DEPTH,
            COUNT,
            STAGE_CAPACITY,
            STAGE_SLOTS,
            DMA_BUFFER_SIZE,
            DMA_STORAGE_SIZE,
        >,
        Self,
    > {
        self.into_epoch_parts()
            .map(|(ring, resources)| resources.with_halted_ring(ring))
    }
}

#[cfg(test)]
mod tests {
    use core::{
        future::ready,
        pin::{Pin, pin},
        task::{Context, Poll},
    };
    use std::vec::Vec;

    use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    use open_esp_radio_esp32s31_pac::{
        MacHeTxProgram, MacHtTxProgram, MacLegacyTxProgram, MacTxCompletionRegisters,
    };
    use open_esp_radio_esp32s31_wifi_mac::{
        rx_pool::RxStagePool, tx::TxSlot, tx_runtime::StaTxRuntimePolicy,
    };
    use open_esp_radio_wifi_lifecycle::scan::{StaCandidateScanExit, StaCandidateScanService};

    use super::*;
    use crate::{
        control_tx::{ControlTxConfig, WifiTxResources},
        ordinary_tx::WifiTxPowerPair,
        rx_backend::Esp32s31ConnectedRx,
        staged_rx::Esp32s31StagedRxQueue,
    };

    #[derive(Default)]
    struct ScanTxHardware {
        publications: u8,
        completion: Option<MacTxCompletionRegisters>,
    }

    impl TxHardware for ScanTxHardware {
        fn tx_descriptor_address(&self, _cpu_address: u32) -> u32 {
            0x2f00_1000
        }

        fn prepare_legacy_tx(&mut self, _queue: u8, _program: MacLegacyTxProgram) -> bool {
            true
        }

        fn start_legacy_tx(&mut self, _queue: u8, _plcp0: u32) {
            self.publications = self.publications.saturating_add(1);
        }

        fn prepare_ht_tx(&mut self, _queue: u8, _program: MacHtTxProgram) -> bool {
            true
        }

        fn start_ht_tx(&mut self, _queue: u8, _plcp0: u32) {
            self.publications = self.publications.saturating_add(1);
        }

        fn prepare_he_tx(&mut self, _queue: u8, _program: MacHeTxProgram) -> bool {
            false
        }

        fn start_he_tx(&mut self, _queue: u8, _plcp0: u32) {}

        fn take_tx_completion(&mut self, _queue: u8) -> Option<MacTxCompletionRegisters> {
            self.completion.take()
        }

        fn begin_tx_timeout_abort(&mut self, _queue: u8) -> bool {
            false
        }

        fn finish_tx_timeout_abort(&mut self, _queue: u8) -> Option<bool> {
            None
        }

        fn abort_tx_collision(&mut self, _queue: u8) -> bool {
            false
        }

        fn detach_completed_tx(&mut self, _queue: u8) -> bool {
            true
        }
    }

    #[derive(Clone, Copy)]
    struct ScanTxPower;

    impl WifiTxPowerProfile for ScanTxPower {
        fn power_pair(&self, _rate_code: u8) -> WifiTxPowerPair {
            WifiTxPowerPair {
                primary: 5,
                alternate: 6,
            }
        }
    }

    #[derive(Default)]
    struct ScanTxTimer {
        now: u64,
    }

    impl WifiTxTimer for ScanTxTimer {
        fn now_micros(&self) -> u64 {
            self.now
        }

        fn wait_until(&mut self, deadline_micros: u64) -> impl Future<Output = ()> + '_ {
            self.now = deadline_micros;
            ready(())
        }

        fn after_micros(&mut self, micros: u64) -> impl Future<Output = ()> + '_ {
            self.now = self.now.saturating_add(micros);
            ready(())
        }
    }

    fn scan_tx_completion(status: u8) -> MacTxCompletionRegisters {
        MacTxCompletionRegisters {
            aux_a: 0,
            aux_b: 0,
            aux_c: 0,
            primary: u32::from(status) << 12,
            alternate: 0,
            trigger_flow: false,
        }
    }

    fn running_scan_tx<'a>(
        slot: Pin<&'a mut TxSlot<256>>,
    ) -> Esp32s31RunningScanTx<'a, ScanTxPower, fn() -> u32, ScanTxTimer, 256> {
        fn entropy() -> u32 {
            0x1234_5678
        }
        Esp32s31RunningScanTx {
            control: Esp32s31ControlTx::new(
                WifiTxResources {
                    slot,
                    policy: StaTxRuntimePolicy::vendor_defaults(),
                    power: ScanTxPower,
                    entropy,
                    timer: ScanTxTimer::default(),
                },
                ControlTxConfig {
                    unicast_attempt_limit: 2,
                    completion_timeout_us: 10,
                    poll_interval_us: 1,
                },
            ),
            state: Esp32s31ScanTxState::new(),
        }
    }

    fn scan_probe_request() -> Esp32s31ScanProbeRequest<'static> {
        Esp32s31ScanProbeRequest {
            source: [2, 3, 4, 5, 6, 7],
            sequence_number: 9,
            ssid: b"",
            supported_rates: &[0x82, 0x84],
            current_channel: Some(6),
            descriptor_capacity: Some(256),
        }
    }

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

    #[test]
    fn running_scan_tx_returns_the_control_owner_after_a_probe() {
        let mut slot = pin!(TxSlot::<256>::new());
        let mut hardware = ScanTxHardware {
            completion: Some(scan_tx_completion(0)),
            ..ScanTxHardware::default()
        };
        let mut tx = running_scan_tx(slot.as_mut());
        tx.begin_scan();

        let report = block_on(tx.transmit_probe_request(&mut hardware, scan_probe_request()))
            .expect("completed running probe");
        assert!(matches!(report, Esp32s31ScanProbeReport::Transmitted(_)));
        let (mut control, summary) = tx.into_parts();
        assert_eq!(summary.completions, 1);
        assert_eq!(summary.failures, 0);

        hardware.completion = Some(scan_tx_completion(0));
        block_on(control.transmit_probe_request(
            &mut hardware,
            ProbeRequest {
                source: [2, 3, 4, 5, 6, 7],
                sequence_number: 10,
                ssid: b"",
                supported_rates: &[0x82, 0x84],
            },
            Some(6),
            Some(256),
        ))
        .expect("returned control owner remains usable");
        assert_eq!(hardware.publications, 2);
    }

    #[test]
    fn failed_running_probe_disables_further_active_attempts() {
        let mut slot = pin!(TxSlot::<256>::new());
        let mut hardware = ScanTxHardware {
            completion: Some(scan_tx_completion(1)),
            ..ScanTxHardware::default()
        };
        let mut tx = running_scan_tx(slot.as_mut());
        tx.begin_scan();

        let first = block_on(tx.transmit_probe_request(&mut hardware, scan_probe_request()))
            .expect("nonzero completion is a safe passive fallback");
        assert!(matches!(
            first,
            Esp32s31ScanProbeReport::PassiveAfterCompletion(_)
        ));
        let second = block_on(tx.transmit_probe_request(&mut hardware, scan_probe_request()))
            .expect("disabled active probe remains passive");
        assert_eq!(second, Esp32s31ScanProbeReport::PassiveWithoutAttempt);
        assert_eq!(hardware.publications, 1);

        let (_control, summary) = tx.into_parts();
        assert_eq!(summary.completions, 1);
        assert_eq!(summary.failures, 1);
    }

    #[test]
    fn running_scan_rx_returns_the_exact_connected_epoch_resources() {
        const STAGE_SLOTS: usize = 1;
        const STAGE_CAPACITY: usize = 64;
        struct TestDelay;

        impl RxReloadDelay for TestDelay {
            fn after_micros(&mut self, _micros: u32) -> impl Future<Output = ()> + '_ {
                ready(())
            }
        }

        let storage =
            Esp32s31RxDmaStorage::<RX_TEST_COUNT, RX_TEST_BUFFER_SIZE, RX_TEST_STORAGE_SIZE>::new();
        let mut hardware = MockRxDma::default();
        let stopped = RxRingStopped::prepare(
            &mut hardware,
            storage.descriptors(),
            RX_TEST_BASE,
            &RX_TEST_BUFFERS,
            RX_TEST_BUFFER_SIZE as u32,
            |_| Ok(()),
        )
        .unwrap();
        let ring = stopped.start(&mut hardware).unwrap();
        let pool = RxStagePool::<STAGE_SLOTS, STAGE_CAPACITY>::new();
        let queue =
            Esp32s31StagedRxQueue::<NoopRawMutex, STAGE_SLOTS, STAGE_CAPACITY, STAGE_SLOTS>::new();
        let (sender, _receiver) = queue.split();
        let connected = Esp32s31ConnectedRx::new(ring, storage.buffers(), &pool, TestDelay, sender);
        let stopped = connected
            .try_stop(&mut hardware)
            .unwrap_or_else(|_| panic!("mock connected ring must stop"));
        let pool_address = stopped.pool() as *const _;

        let mut running = Esp32s31RunningScanRx::from_stopped(stopped);
        assert_eq!(running.phase(), Esp32s31ScanRxPhase::Halted);
        running.prepare_initial(&mut hardware).unwrap();
        assert_eq!(running.phase(), Esp32s31ScanRxPhase::Prepared);
        block_on(running.start(&mut hardware)).unwrap();
        running.stop(&mut hardware).unwrap();

        let stopped = running
            .into_stopped()
            .unwrap_or_else(|_| panic!("halted running scan must restore connected resources"));
        assert_eq!(stopped.pool() as *const _, pool_address);
        assert_eq!(stopped.ring().descriptor_base(), RX_TEST_BASE);
        assert_eq!(stopped.ring().buffer_addresses(), &RX_TEST_BUFFERS);
        assert_eq!(stopped.buffers().as_ptr(), storage.buffers().as_ptr());
        assert_eq!(stopped.queued_frames(), 0);
    }
}
