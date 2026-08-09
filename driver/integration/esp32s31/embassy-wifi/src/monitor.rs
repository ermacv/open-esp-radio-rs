//! Role-local monitor resources for the ESP32-S31 radio supervisor.
//!
//! This module does not own a second radio runner or interrupt handler. A
//! monitor epoch consumes these resources together with the role-neutral Wi-Fi
//! owner and returns both only after RX DMA and the shared IRQ route quiesce.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use open_esp_radio::{
    RadioSubsystemGeneration,
    esp32s31::wifi::{
        embassy::monitor::{
            Esp32s31MonitorControlResources, Esp32s31MonitorInterrupts, Esp32s31MonitorMemory,
            Esp32s31MonitorStoppedResources, Esp32s31MonitorTask, Esp32s31MonitorTaskBuildFailure,
            Esp32s31MonitorTaskResources, MonitorCaptureFrame, MonitorCapturePool,
            MonitorCaptureReceiver, MonitorCaptureResources, MonitorCaptureSink,
        },
        mac::rx::RxPhyInfo,
    },
};
use open_esp_radio_esp32s31_wifi_esp_hal::mac_interrupt_epoch::EspHalMacInterruptRoute;
use static_cell::{ConstStaticCell, StaticCell};

use crate::connected::monitor_interrupts;
use open_esp_radio_esp32s31_wifi_embassy::resource_profile::{
    ESP32S31_DEFAULT_RX_BUFFER_SIZE as RX_BUFFER_SIZE,
    ESP32S31_DEFAULT_RX_BUFFER_STORAGE_SIZE as RX_STORAGE_SIZE,
    ESP32S31_DEFAULT_RX_DESCRIPTOR_COUNT as RX_DESCRIPTOR_COUNT,
};

const CAPTURE_DEPTH: usize = 8;
const CAPTURE_SLOTS: usize = 8;
// Normalized monitor frames are views into one completed RX DMA segment. The
// segment cannot exceed the production RX buffer, so a larger retained slot
// only consumes internal SRAM without allowing an additional valid frame.
const CAPTURE_CAPACITY: usize = RX_BUFFER_SIZE;

type CapturePool = MonitorCapturePool<CAPTURE_CAPACITY, CAPTURE_SLOTS>;
pub(super) type CaptureResources = MonitorCaptureResources<
    'static,
    CriticalSectionRawMutex,
    RxPhyInfo,
    CAPTURE_DEPTH,
    CAPTURE_CAPACITY,
    CAPTURE_SLOTS,
>;
pub(super) type CaptureSink = MonitorCaptureSink<
    'static,
    'static,
    CriticalSectionRawMutex,
    RxPhyInfo,
    CAPTURE_DEPTH,
    CAPTURE_CAPACITY,
    CAPTURE_SLOTS,
>;
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
    EspHalMacInterruptRoute,
    CriticalSectionRawMutex,
    CaptureSink,
    RX_DESCRIPTOR_COUNT,
    RX_BUFFER_SIZE,
    RX_STORAGE_SIZE,
>;
pub(super) type MonitorMemory =
    Esp32s31MonitorMemory<RX_DESCRIPTOR_COUNT, RX_BUFFER_SIZE, RX_STORAGE_SIZE>;
type MonitorInterrupts =
    Esp32s31MonitorInterrupts<'static, EspHalMacInterruptRoute, CriticalSectionRawMutex>;
pub(super) type MonitorStoppedResources = Esp32s31MonitorStoppedResources<
    'static,
    EspHalMacInterruptRoute,
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
    pub fn try_receive(&self) -> Option<MonitorCaptureFrame<'static, RxPhyInfo, CAPTURE_CAPACITY>> {
        self.receiver.try_receive()
    }

    pub async fn receive(&self) -> MonitorCaptureFrame<'static, RxPhyInfo, CAPTURE_CAPACITY> {
        self.receiver.receive().await
    }
}

pub(super) struct ProductionMonitorResources {
    memory: MonitorMemory,
    sink: CaptureSink,
    interrupts: MonitorInterrupts,
    control: &'static Esp32s31MonitorControlResources<CriticalSectionRawMutex>,
}

impl ProductionMonitorResources {
    pub(super) fn bind(
        mut self,
        generation: RadioSubsystemGeneration,
        snapshot_length: Option<u16>,
    ) -> MonitorTaskResources {
        self.sink
            .configure(generation.value(), snapshot_length.map(usize::from));
        Esp32s31MonitorTaskResources::new(self.memory, self.sink, self.interrupts, self.control)
    }

    pub(super) fn from_stopped(resources: MonitorStoppedResources) -> Self {
        let parts = resources.into_parts();
        Self {
            memory: parts.memory,
            sink: parts.sink,
            interrupts: parts.interrupts,
            control: parts.control,
        }
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
            sink,
            interrupts: monitor_interrupts(),
            control: MONITOR_CONTROL.take(),
        },
        capture,
        frames: Esp32s31MonitorFrames { receiver },
    })
}
