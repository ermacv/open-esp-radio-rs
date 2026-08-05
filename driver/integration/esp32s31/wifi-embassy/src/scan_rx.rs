//! Concrete ESP32-S31 station-scan RX ownership and DMA composition.
//!
//! Cold scan and running rescan use different surrounding radio owners, but
//! both carry the same typed RX ring through prepared, live and halted phases.
//! This module owns that DMA lifecycle and management-frame observation only;
//! scan policy and active-probe TX live in their respective modules.

#![forbid(unsafe_code)]

use embassy_sync::blocking_mutex::raw::RawMutex;
use open_esp_radio_esp32s31_wifi_lmac::rx::{
    RxDma, RxIngressConfig, RxReloadObservation, RxRingError, RxRingHalted, RxRingLive,
    RxRingStopped, extract_management,
};
use open_esp_radio_ieee80211::scan::{ScanObservation, ScanTable};

use crate::{
    embassy_rx::RxReloadDelay,
    rx_backend::{
        ESP32S31_RX_WALKER_ENABLE_SETTLE_US, Esp32s31RxDmaStorage, Esp32s31RxEpochResources,
        Esp32s31StoppedRx,
    },
};

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

    /// Publish one already-extracted management frame through the owned scan
    /// table and its non-retaining observer.
    ///
    /// Concrete RX ports use this boundary when frame extraction is provided
    /// by another typed DMA owner. The frame borrow is never retained beyond
    /// this call.
    pub fn observe_management_frame(&mut self, frame: &[u8], rssi: i8) -> ScanObservation
    where
        O: Esp32s31ScanFrameObserver,
    {
        let outcome = self.table.observe_management(frame, self.channel, rssi);
        self.observer.observe(frame, rssi, outcome);
        outcome
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
    storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
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
        let ring = storage.prepare_ring(hardware, descriptor_base, buffer_addresses)?;
        Ok(Self {
            state: Esp32s31ScanRxState::Prepared(ring),
            storage,
        })
    }

    /// Reuse a hardware-confirmed halted ring for a running rescan.
    pub const fn from_halted(
        ring: RxRingHalted<'storage, COUNT>,
        storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    ) -> Self {
        Self {
            state: Esp32s31ScanRxState::Halted(ring),
            storage,
        }
    }

    pub const fn phase(&self) -> Esp32s31ScanRxPhase {
        self.state.phase()
    }

    /// Admit either the first already-prepared cold scan or a later complete
    /// retry whose final channel returned the same ring halted.
    ///
    /// This is deliberately not an implicit live-ring restart: a caller that
    /// still owns a live scan epoch has violated the finite scan boundary and
    /// receives the exact phase error.
    pub fn prepare_initial_or_retry<H: RxDma>(
        &mut self,
        hardware: &mut H,
    ) -> Result<(), Esp32s31ScanRxError> {
        match self.phase() {
            Esp32s31ScanRxPhase::Prepared => Ok(()),
            Esp32s31ScanRxPhase::Halted => self.prepare_next(hardware),
            actual @ Esp32s31ScanRxPhase::Live => Err(Esp32s31ScanRxError::InvalidPhase {
                expected: Esp32s31ScanRxPhase::Prepared,
                actual,
            }),
        }
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
            let Some(completed) = self.storage.take_completed(ring, index)? else {
                continue;
            };
            progress.completed_descriptors = progress.completed_descriptors.saturating_add(1);
            let segment = completed.segment();
            let buffer = segment.buffer;
            let rssi = buffer[0] as i8;
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
            && let Some(append) = self
                .storage
                .recycle_completed_prefix::<COUNT, _>(ring, hardware)?
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
        match self.storage.prepare_halted(ring, hardware) {
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
            Esp32s31ScanRxState::Prepared(ring) => Ok(ring.into_halted()),
            state => Err(Self {
                state,
                storage: self.storage,
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
        let scan = Esp32s31ScanRx::from_halted(ring, resources.storage());
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
    use super::*;
    use crate::{rx_backend::Esp32s31ConnectedRx, staged_rx::Esp32s31StagedRxQueue};
    use core::{
        future::{Future, ready},
        pin::pin,
        task::{Context, Poll},
    };
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    use open_esp_radio_esp32s31_wifi_lmac::rx_pool::RxStagePool;

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

    fn write_test_beacon(
        storage: &mut Esp32s31RxDmaStorage<
            RX_TEST_COUNT,
            RX_TEST_BUFFER_SIZE,
            RX_TEST_STORAGE_SIZE,
        >,
    ) {
        const FRAME_LENGTH: usize = 43;
        const SIGNAL_LENGTH: usize = FRAME_LENGTH + 4;
        const FRAME_OFFSET: usize = 0x40;

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
        storage
            .buffer_mut(0)
            .expect("test RX buffer exists")
            .copy_from_slice(&bytes);
    }

    fn complete_test_beacon(
        storage: &Esp32s31RxDmaStorage<RX_TEST_COUNT, RX_TEST_BUFFER_SIZE, RX_TEST_STORAGE_SIZE>,
    ) {
        use open_esp_radio_esp32s31_wifi_lmac::descriptor::{BIT_30, BIT_31, LENGTH_SHIFT};

        const FRAME_LENGTH: usize = 43;
        const SIGNAL_LENGTH: usize = FRAME_LENGTH + 4;
        const FRAME_OFFSET: usize = 0x40;
        const RECEIVED_LENGTH: usize = FRAME_OFFSET + SIGNAL_LENGTH;

        storage.descriptors()[0].write_word0(
            RX_TEST_BUFFER_SIZE as u32 | (RECEIVED_LENGTH as u32) << LENGTH_SHIFT | BIT_30 | BIT_31,
        );
    }

    #[test]
    fn scan_rx_hands_the_exact_halted_ring_to_the_next_phase() {
        let mut storage =
            Esp32s31RxDmaStorage::<RX_TEST_COUNT, RX_TEST_BUFFER_SIZE, RX_TEST_STORAGE_SIZE>::new();
        write_test_beacon(&mut storage);
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
    fn complete_cold_scan_can_prepare_the_same_ring_for_a_retry() {
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

        rx.prepare_initial_or_retry(&mut hardware).unwrap();
        assert_eq!(rx.phase(), Esp32s31ScanRxPhase::Prepared);
        rx.start(&mut hardware).unwrap();
        rx.stop(&mut hardware).unwrap();
        assert_eq!(rx.phase(), Esp32s31ScanRxPhase::Halted);

        rx.prepare_initial_or_retry(&mut hardware).unwrap();
        assert_eq!(rx.phase(), Esp32s31ScanRxPhase::Prepared);
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
        let connected = Esp32s31ConnectedRx::new(ring, &storage, &pool, TestDelay, sender);
        let stopped = connected
            .try_stop(&mut hardware)
            .unwrap_or_else(|_| panic!("mock connected ring must stop"));
        let pool_address = stopped.pool() as *const _;

        let mut running = Esp32s31RunningScanRx::from_stopped(stopped);
        assert_eq!(running.phase(), Esp32s31ScanRxPhase::Halted);
        running.prepare_initial(&mut hardware).unwrap();
        assert_eq!(running.phase(), Esp32s31ScanRxPhase::Prepared);

        let stopped = running
            .into_stopped()
            .unwrap_or_else(|_| panic!("prepared running scan must discard its unstarted epoch"));
        assert_eq!(stopped.pool() as *const _, pool_address);
        assert_eq!(stopped.ring().descriptor_base(), RX_TEST_BASE);

        let mut running = Esp32s31RunningScanRx::from_stopped(stopped);
        running.prepare_initial(&mut hardware).unwrap();
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
