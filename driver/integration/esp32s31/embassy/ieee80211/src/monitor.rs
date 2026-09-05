//! Role-local monitor resources for the ESP32-S31 radio supervisor.
//!
//! This module does not own a second radio runner or interrupt handler. A
//! monitor epoch consumes these resources together with the role-neutral Wi-Fi
//! owner and returns both only after RX DMA and the shared IRQ route quiesce.

use core::sync::atomic::{AtomicU32, Ordering};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use open_esp_radio::RadioSubsystemGeneration;
pub use open_esp_radio::{
    MONITOR_CHANNEL_SEQUENCE_CAPACITY, MonitorCapturePolicy, MonitorChannelPolicy,
    MonitorChannelSequence, MonitorChannelSequenceError, MonitorRequest,
};
use open_esp_radio_esp32s31_wifi_embassy::roles::monitor::{
    Esp32s31MonitorControlResources, Esp32s31MonitorMemory, Esp32s31MonitorRxRing,
    Esp32s31MonitorStoppedResources, Esp32s31MonitorTask, Esp32s31MonitorTaskBuildFailure,
    Esp32s31MonitorTaskResources,
};
use open_esp_radio_esp32s31_wifi_esp_hal::mac_interrupt_epoch::EspHalMacInterruptRoute;
use open_esp_radio_esp32s31_wifi_mac::rx::RxPhyInfo;
pub use open_esp_radio_esp32s31_wifi_mac::rx::{
    RxBasebandFormat as Esp32s31MonitorBasebandFormat, RxPhyInfo as Esp32s31MonitorPhyInfo,
};
use open_esp_radio_wifi_embassy::{
    MonitorCaptureFrame, MonitorCapturePool, MonitorCaptureReceiver, MonitorCaptureResources,
    MonitorCaptureSink,
};
use open_esp_radio_wifi_softmac::{
    MonitorDropReason, MonitorFrame, MonitorInjectionChannelBinding, MonitorPublishOutcome,
    MonitorSink, WifiChannel,
};
use static_cell::{ConstStaticCell, StaticCell};

use crate::resources::profile::{
    ESP32S31_DEFAULT_RX_BUFFER_SIZE as RX_BUFFER_SIZE,
    ESP32S31_DEFAULT_RX_BUFFER_STORAGE_SIZE as RX_STORAGE_SIZE,
    ESP32S31_DEFAULT_RX_DESCRIPTOR_COUNT as RX_DESCRIPTOR_COUNT,
};
use crate::supervisor::ProductionRxRing;

const CAPTURE_DEPTH: usize = 8;
const CAPTURE_SLOTS: usize = 8;
// Normalized monitor frames are views into one completed RX DMA segment. The
// segment cannot exceed the production RX buffer, so a larger retained slot
// only consumes internal SRAM without allowing an additional valid frame.
pub const ESP32S31_MONITOR_CAPTURE_CAPACITY: usize = RX_BUFFER_SIZE;
const CAPTURE_CAPACITY: usize = ESP32S31_MONITOR_CAPTURE_CAPACITY;

pub type Esp32s31MonitorFrame =
    MonitorCaptureFrame<'static, RxPhyInfo, ESP32S31_MONITOR_CAPTURE_CAPACITY>;

type CapturePool = MonitorCapturePool<CAPTURE_CAPACITY, CAPTURE_SLOTS>;
pub(super) type CaptureResources = MonitorCaptureResources<
    'static,
    CriticalSectionRawMutex,
    RxPhyInfo,
    CAPTURE_DEPTH,
    CAPTURE_CAPACITY,
    CAPTURE_SLOTS,
>;
type RawCaptureSink = MonitorCaptureSink<
    'static,
    'static,
    CriticalSectionRawMutex,
    RxPhyInfo,
    CAPTURE_DEPTH,
    CAPTURE_CAPACITY,
    CAPTURE_SLOTS,
>;

/// Value-only capture accounting for the single production monitor stream.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Esp32s31MonitorCaptureStatistics {
    pub generation: u32,
    pub published_frames: u32,
    pub full_drops: u32,
    pub oversized_drops: u32,
    pub discarded_frames: u32,
}

struct MonitorCaptureCounters {
    generation: AtomicU32,
    published_frames: AtomicU32,
    full_drops: AtomicU32,
    oversized_drops: AtomicU32,
    discarded_frames: AtomicU32,
}

impl MonitorCaptureCounters {
    const fn new() -> Self {
        Self {
            generation: AtomicU32::new(0),
            published_frames: AtomicU32::new(0),
            full_drops: AtomicU32::new(0),
            oversized_drops: AtomicU32::new(0),
            discarded_frames: AtomicU32::new(0),
        }
    }

    fn begin_epoch(&self, generation: u32) {
        self.published_frames.store(0, Ordering::Relaxed);
        self.full_drops.store(0, Ordering::Relaxed);
        self.oversized_drops.store(0, Ordering::Relaxed);
        self.discarded_frames.store(0, Ordering::Relaxed);
        self.generation.store(generation, Ordering::Release);
    }

    fn snapshot(&self) -> Esp32s31MonitorCaptureStatistics {
        Esp32s31MonitorCaptureStatistics {
            generation: self.generation.load(Ordering::Acquire),
            published_frames: self.published_frames.load(Ordering::Relaxed),
            full_drops: self.full_drops.load(Ordering::Relaxed),
            oversized_drops: self.oversized_drops.load(Ordering::Relaxed),
            discarded_frames: self.discarded_frames.load(Ordering::Relaxed),
        }
    }
}

static CAPTURE_COUNTERS: MonitorCaptureCounters = MonitorCaptureCounters::new();

pub(super) fn record_discarded_monitor_frames(discarded: usize) {
    CAPTURE_COUNTERS.discarded_frames.fetch_add(
        u32::try_from(discarded).unwrap_or(u32::MAX),
        Ordering::Relaxed,
    );
}

/// Transparent sink wrapper: accounting stays in one static instead of adding
/// a pointer to every monitor owner carried through the radio async state.
/// Keeping this handle the same size as `RawCaptureSink` avoids large
/// by-value owner states becoming transient executor-stack allocations. With
/// the counters pointer embedded, the ESP32-S31 release build's radio poll
/// frame grew from `0x2dd0` to `0x66d90` bytes and overwrote static station
/// memory before crossing the RAM boundary.
pub(super) struct CaptureSink {
    inner: RawCaptureSink,
}

const _: () = assert!(
    core::mem::size_of::<CaptureSink>() == core::mem::size_of::<RawCaptureSink>(),
    "monitor accounting must not enlarge the role owner carried by the radio future",
);

impl CaptureSink {
    fn configure(&mut self, generation: u32, snapshot_length: Option<usize>) {
        self.inner.configure(generation, snapshot_length);
    }
}

impl MonitorSink<RxPhyInfo> for CaptureSink {
    fn begin_channel_epoch(&mut self, channel: WifiChannel) {
        self.inner.begin_channel_epoch(channel);
    }

    fn end_channel_epoch(&mut self) {
        self.inner.end_channel_epoch();
    }

    fn injection_channel_binding(&self) -> Option<MonitorInjectionChannelBinding> {
        self.inner.injection_channel_binding()
    }

    fn try_publish(&mut self, frame: MonitorFrame<'_, RxPhyInfo>) -> MonitorPublishOutcome {
        let outcome = self.inner.try_publish(frame);
        let counter = match outcome {
            MonitorPublishOutcome::Published => &CAPTURE_COUNTERS.published_frames,
            MonitorPublishOutcome::Dropped(MonitorDropReason::Full) => &CAPTURE_COUNTERS.full_drops,
            MonitorPublishOutcome::Dropped(MonitorDropReason::TooLong) => {
                &CAPTURE_COUNTERS.oversized_drops
            }
            MonitorPublishOutcome::Dropped(MonitorDropReason::Filtered) => return outcome,
        };
        counter.fetch_add(1, Ordering::Relaxed);
        outcome
    }
}
type CaptureReceiver = MonitorCaptureReceiver<
    'static,
    'static,
    CriticalSectionRawMutex,
    RxPhyInfo,
    CAPTURE_DEPTH,
    CAPTURE_CAPACITY,
>;

pub(super) type MonitorTaskResources = Esp32s31MonitorTaskResources<
    'static,
    CriticalSectionRawMutex,
    CaptureSink,
    RX_DESCRIPTOR_COUNT,
    RX_BUFFER_SIZE,
    RX_STORAGE_SIZE,
>;
pub(super) type MonitorMemory =
    Esp32s31MonitorMemory<RX_DESCRIPTOR_COUNT, RX_BUFFER_SIZE, RX_STORAGE_SIZE>;
pub(super) type MonitorStoppedResources = Esp32s31MonitorStoppedResources<
    'static,
    CriticalSectionRawMutex,
    CaptureSink,
    RX_DESCRIPTOR_COUNT,
    RX_BUFFER_SIZE,
    RX_STORAGE_SIZE,
>;
pub(super) type ProductionMonitorTask = Esp32s31MonitorTask<
    'static,
    open_esp_radio_esp32s31_wifi_esp_hal::EspHalRadioPeripheral,
    EspHalMacInterruptRoute,
    CriticalSectionRawMutex,
    CaptureSink,
    RX_DESCRIPTOR_COUNT,
    RX_BUFFER_SIZE,
    RX_STORAGE_SIZE,
>;
pub(super) type ProductionMonitorBuildFailure = Esp32s31MonitorTaskBuildFailure<
    'static,
    open_esp_radio_esp32s31_wifi_esp_hal::EspHalRadioPeripheral,
    EspHalMacInterruptRoute,
    CriticalSectionRawMutex,
    CaptureSink,
    RX_DESCRIPTOR_COUNT,
    RX_BUFFER_SIZE,
    RX_STORAGE_SIZE,
>;

static CAPTURE_POOL: ConstStaticCell<CapturePool> = ConstStaticCell::new(CapturePool::new());
static CAPTURE_RESOURCES: StaticCell<CaptureResources> = StaticCell::new();
static MONITOR_CONTROL: ConstStaticCell<Esp32s31MonitorControlResources<CriticalSectionRawMutex>> =
    ConstStaticCell::new(Esp32s31MonitorControlResources::new());

/// Application-owned monitor capture stream. Frames carry their supervisor
/// generation, so a lease retained across a role transition is unambiguous.
pub struct Esp32s31MonitorFrames {
    receiver: CaptureReceiver,
}

impl Esp32s31MonitorFrames {
    pub fn try_receive(&self) -> Option<Esp32s31MonitorFrame> {
        self.receiver.try_receive()
    }

    pub async fn receive(&self) -> Esp32s31MonitorFrame {
        self.receiver.receive().await
    }

    /// Snapshot capture-pool publication and loss for the current generation.
    pub fn statistics(&self) -> Esp32s31MonitorCaptureStatistics {
        CAPTURE_COUNTERS.snapshot()
    }
}

pub(super) struct ProductionMonitorResources {
    memory: MonitorMemory,
    sink: CaptureSink,
    control: &'static Esp32s31MonitorControlResources<CriticalSectionRawMutex>,
}

impl ProductionMonitorResources {
    pub(super) fn bind(
        mut self,
        generation: RadioSubsystemGeneration,
        snapshot_length: Option<u16>,
        rx_ring: Option<ProductionRxRing>,
    ) -> MonitorTaskResources {
        CAPTURE_COUNTERS.begin_epoch(generation.value());
        self.sink
            .configure(generation.value(), snapshot_length.map(usize::from));
        Esp32s31MonitorTaskResources::new(
            self.memory,
            rx_ring.map(|ring| match ring {
                ProductionRxRing::Halted(ring) => Esp32s31MonitorRxRing::Halted(ring),
                ProductionRxRing::Live(ring) => Esp32s31MonitorRxRing::Live(ring),
            }),
            self.sink,
            self.control,
        )
    }

    pub(super) fn from_stopped(resources: MonitorStoppedResources) -> (Self, ProductionRxRing) {
        let parts = resources.into_parts();
        (
            Self {
                memory: parts.memory,
                sink: parts.sink,
                control: parts.control,
            },
            ProductionRxRing::Live(parts.live_ring),
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MonitorResourcesError {
    InUse,
}

pub(super) struct MonitorProductResources {
    pub(super) role: ProductionMonitorResources,
    pub(super) capture: &'static CaptureResources,
    pub(super) frames: Esp32s31MonitorFrames,
}

pub(super) fn initialize_monitor_resources(
    memory: MonitorMemory,
) -> Result<MonitorProductResources, MonitorResourcesError> {
    let capture = CAPTURE_RESOURCES
        .try_init_with(|| CaptureResources::new(CAPTURE_POOL.take()))
        .ok_or(MonitorResourcesError::InUse)?;
    let (sink, receiver) = capture.split();
    Ok(MonitorProductResources {
        role: ProductionMonitorResources {
            memory,
            sink: CaptureSink { inner: sink },
            control: MONITOR_CONTROL.take(),
        },
        capture,
        frames: Esp32s31MonitorFrames { receiver },
    })
}
