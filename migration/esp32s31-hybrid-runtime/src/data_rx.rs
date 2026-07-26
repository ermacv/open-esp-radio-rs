//! Fixed, owned STA/AP data receive boundary.

use core::{
    cell::UnsafeCell,
    future::Future,
    pin::Pin,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    task::{Context, Poll},
};

use crate::channel::BoundedChannel;

pub const WIFI_DATA_RX_CAPACITY: usize = 32;
pub const WIFI_DATA_RX_FRAME_CAPACITY: usize = 1600;
#[cfg(target_arch = "riscv32")]
const WIFI_DATA_RX_COPY_CAPACITY: usize = 8;
#[cfg(not(target_arch = "riscv32"))]
const WIFI_DATA_RX_COPY_CAPACITY: usize = WIFI_DATA_RX_CAPACITY;
#[cfg(target_arch = "riscv32")]
const WIFI_DATA_RX_COPY_FRAME_CAPACITY: usize = 512;
// The default profile keeps one kind-7 object beyond the complete 16-frame
// reorder window. The deeper profile also reserves eight objects for frames
// concurrently traversing the lower-MAC receive/recycle pipeline. That
// pipeline completes asynchronously after the input callback returns, so a
// single incoming-frame reserve can still exhaust the hardware descriptor
// chain during a reconnect burst.
#[cfg(target_arch = "riscv32")]
const WIFI_DATA_RX_PIPELINE_RESERVE: usize = if crate::rx_ampdu::RX_ESF_SLOT_ID_CAPACITY > 32 {
    8
} else {
    1
};
#[cfg(target_arch = "riscv32")]
const WIFI_DATA_RX_ZERO_COPY_LIMIT: usize = crate::rx_ampdu::RX_ESF_SLOT_ID_CAPACITY
    - crate::rx_ampdu::RX_AMPDU_SLOT_CAPACITY
    - WIFI_DATA_RX_PIPELINE_RESERVE;
#[cfg(not(target_arch = "riscv32"))]
const WIFI_DATA_RX_COPY_FRAME_CAPACITY: usize = WIFI_DATA_RX_FRAME_CAPACITY;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiDataInterface {
    Station,
    AccessPoint,
}

struct RxSlotData {
    length: usize,
    bytes: [u8; WIFI_DATA_RX_COPY_FRAME_CAPACITY],
}

struct RxSlot {
    occupied: AtomicBool,
    data: UnsafeCell<RxSlotData>,
}

impl RxSlot {
    const fn new() -> Self {
        Self {
            occupied: AtomicBool::new(false),
            data: UnsafeCell::new(RxSlotData {
                length: 0,
                bytes: [0; WIFI_DATA_RX_COPY_FRAME_CAPACITY],
            }),
        }
    }
}

// `occupied` transfers exclusive access from the callback to the channel
// consumer. A slot is never written again until its token is dropped.
unsafe impl Sync for RxSlot {}

#[cfg_attr(
    target_arch = "riscv32",
    link_section = ".critical.bss.wifi_strict.data_rx_slots"
)]
static RX_SLOTS: [RxSlot; WIFI_DATA_RX_COPY_CAPACITY] =
    [const { RxSlot::new() }; WIFI_DATA_RX_COPY_CAPACITY];
#[cfg_attr(
    target_arch = "riscv32",
    link_section = ".critical.bss.wifi_strict.data_rx_channel"
)]
static RX_CHANNEL: BoundedChannel<RxSlotToken, WIFI_DATA_RX_CAPACITY> = BoundedChannel::new();
static REJECTED_RX_FRAMES: AtomicUsize = AtomicUsize::new(0);
static RX_ENQUEUED: AtomicUsize = AtomicUsize::new(0);
static RX_DEQUEUED: AtomicUsize = AtomicUsize::new(0);
static RX_RELEASED: AtomicUsize = AtomicUsize::new(0);
static RX_REJECTED_INVALID: AtomicUsize = AtomicUsize::new(0);
static RX_REJECTED_SLOTS_FULL: AtomicUsize = AtomicUsize::new(0);
static RX_REJECTED_CHANNEL_CONTENDED: AtomicUsize = AtomicUsize::new(0);
static RX_OCCUPIED: AtomicUsize = AtomicUsize::new(0);
static RX_OCCUPIED_HIGH_WATER: AtomicUsize = AtomicUsize::new(0);
static RX_LATENCY_SAMPLES: AtomicUsize = AtomicUsize::new(0);
static RX_QUEUE_TICKS_SUM: AtomicUsize = AtomicUsize::new(0);
static RX_QUEUE_CYCLES_MAX: AtomicUsize = AtomicUsize::new(0);
static RX_QUEUE_OVER_1MS: AtomicUsize = AtomicUsize::new(0);
static RX_PROCESSING_TICKS_SUM: AtomicUsize = AtomicUsize::new(0);
static RX_PROCESSING_CYCLES_MAX: AtomicUsize = AtomicUsize::new(0);
static RX_PROCESSING_OVER_100US: AtomicUsize = AtomicUsize::new(0);
#[cfg(target_arch = "riscv32")]
static RX_CALLBACKS_INSTALLED: AtomicBool = AtomicBool::new(false);

#[cfg(all(target_arch = "riscv32", feature = "rx-pipeline-telemetry"))]
const RX_TELEMETRY_SAMPLE_MASK: usize = 255;
#[cfg(all(target_arch = "riscv32", feature = "rx-pipeline-telemetry"))]
const S31_CYCLES_PER_MILLISECOND: u32 = 320_000;
#[cfg(all(target_arch = "riscv32", feature = "rx-pipeline-telemetry"))]
const S31_CYCLES_PER_100_MICROSECONDS: u32 = 32_000;

#[cfg(all(target_arch = "riscv32", feature = "rx-pipeline-telemetry"))]
#[inline(always)]
fn cycle_count() -> u32 {
    let value: u32;
    unsafe {
        core::arch::asm!(
            "csrr {value}, mcycle",
            value = out(reg) value,
            options(nomem, nostack)
        )
    };
    value
}

fn record_high_water(counter: &AtomicUsize, value: usize) {
    let observed = counter.load(Ordering::Relaxed);
    if value > observed {
        // The interrupt producer gets one attempt; diagnostics must never
        // introduce a CAS retry loop into the packet path.
        let _ = counter.compare_exchange(observed, value, Ordering::Relaxed, Ordering::Relaxed);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiDataRxSnapshot {
    pub claimed: usize,
    pub enqueued: usize,
    pub dequeued: usize,
    pub released: usize,
    pub rejected: usize,
    pub rejected_invalid: usize,
    pub rejected_slots_full: usize,
    pub rejected_channel_contended: usize,
    pub occupied: usize,
    pub occupied_high_water: usize,
    pub queued: usize,
    pub latency_samples: usize,
    /// Sum of sampled callback-to-dequeue latency in 256-cycle ticks.
    pub queue_ticks_sum: usize,
    pub queue_cycles_max: usize,
    pub queue_over_1ms: usize,
    /// Sum of sampled dequeue-to-release latency in 256-cycle ticks.
    pub processing_ticks_sum: usize,
    pub processing_cycles_max: usize,
    pub processing_over_100us: usize,
}

pub fn wifi_data_rx_snapshot() -> WifiDataRxSnapshot {
    let enqueued = RX_ENQUEUED.load(Ordering::Acquire);
    let rejected_slots_full = RX_REJECTED_SLOTS_FULL.load(Ordering::Acquire);
    let rejected_channel_contended = RX_REJECTED_CHANNEL_CONTENDED.load(Ordering::Acquire);
    WifiDataRxSnapshot {
        // Every valid token is either enqueued or rejected by one of the two
        // bounded admission conditions, so this cumulative value needs no
        // separate mutable counter.
        claimed: enqueued + rejected_slots_full + rejected_channel_contended,
        enqueued,
        dequeued: RX_DEQUEUED.load(Ordering::Acquire),
        released: RX_RELEASED.load(Ordering::Acquire),
        rejected: REJECTED_RX_FRAMES.load(Ordering::Acquire),
        rejected_invalid: RX_REJECTED_INVALID.load(Ordering::Acquire),
        rejected_slots_full,
        rejected_channel_contended,
        occupied: RX_OCCUPIED.load(Ordering::Acquire),
        occupied_high_water: RX_OCCUPIED_HIGH_WATER.load(Ordering::Acquire),
        queued: RX_CHANNEL.len(),
        latency_samples: RX_LATENCY_SAMPLES.load(Ordering::Acquire),
        queue_ticks_sum: RX_QUEUE_TICKS_SUM.load(Ordering::Acquire),
        queue_cycles_max: RX_QUEUE_CYCLES_MAX.load(Ordering::Acquire),
        queue_over_1ms: RX_QUEUE_OVER_1MS.load(Ordering::Acquire),
        processing_ticks_sum: RX_PROCESSING_TICKS_SUM.load(Ordering::Acquire),
        processing_cycles_max: RX_PROCESSING_CYCLES_MAX.load(Ordering::Acquire),
        processing_over_100us: RX_PROCESSING_OVER_100US.load(Ordering::Acquire),
    }
}

enum RxStorage {
    Copied {
        index: usize,
    },
    #[cfg(target_arch = "riscv32")]
    LargeEsf(crate::esf::OwnedLargeRxNetworkFrame),
}

struct RxSlotToken {
    interface: WifiDataInterface,
    storage: RxStorage,
    #[cfg(all(target_arch = "riscv32", feature = "rx-pipeline-telemetry"))]
    enqueued_cycle: u32,
    #[cfg(all(target_arch = "riscv32", feature = "rx-pipeline-telemetry"))]
    dequeued_cycle: u32,
    #[cfg(all(target_arch = "riscv32", feature = "rx-pipeline-telemetry"))]
    telemetry_sample: bool,
}

impl Drop for RxSlotToken {
    fn drop(&mut self) {
        #[cfg(all(target_arch = "riscv32", feature = "rx-pipeline-telemetry"))]
        if self.telemetry_sample {
            let cycles = cycle_count().wrapping_sub(self.dequeued_cycle);
            RX_PROCESSING_TICKS_SUM.fetch_add((cycles >> 8) as usize, Ordering::Relaxed);
            record_high_water(&RX_PROCESSING_CYCLES_MAX, cycles as usize);
            if cycles > S31_CYCLES_PER_100_MICROSECONDS {
                RX_PROCESSING_OVER_100US.fetch_add(1, Ordering::Relaxed);
            }
        }
        match &self.storage {
            RxStorage::Copied { index } => {
                RX_SLOTS[*index].occupied.store(false, Ordering::Release);
            }
            #[cfg(target_arch = "riscv32")]
            RxStorage::LargeEsf(_) => {}
        }
        RX_RELEASED.fetch_add(1, Ordering::Relaxed);
        RX_OCCUPIED.fetch_sub(1, Ordering::AcqRel);
    }
}

pub struct OwnedWifiDataFrame {
    token: RxSlotToken,
}

impl OwnedWifiDataFrame {
    pub fn interface(&self) -> WifiDataInterface {
        self.token.interface
    }

    pub fn as_bytes(&self) -> &[u8] {
        match &self.token.storage {
            RxStorage::Copied { index } => {
                let data = unsafe { &*RX_SLOTS[*index].data.get() };
                &data.bytes[..data.length]
            }
            #[cfg(target_arch = "riscv32")]
            RxStorage::LargeEsf(frame) => frame.as_bytes(),
        }
    }

    /// Mutable packet view for a network-stack receive token.
    ///
    /// Ownership of the slot token guarantees exclusive access until this
    /// frame is dropped and returns the slot to the interrupt producer.
    pub fn as_bytes_mut(&mut self) -> &mut [u8] {
        match &mut self.token.storage {
            RxStorage::Copied { index } => {
                let data = unsafe { &mut *RX_SLOTS[*index].data.get() };
                &mut data.bytes[..data.length]
            }
            #[cfg(target_arch = "riscv32")]
            RxStorage::LargeEsf(frame) => frame.as_bytes_mut(),
        }
    }
}

pub fn try_receive_wifi_data() -> Option<OwnedWifiDataFrame> {
    RX_CHANNEL.try_receive().map(|mut token| {
        record_dequeue(&mut token);
        OwnedWifiDataFrame { token }
    })
}

pub async fn receive_wifi_data() -> OwnedWifiDataFrame {
    let mut token = RX_CHANNEL.receive().await;
    record_dequeue(&mut token);
    OwnedWifiDataFrame { token }
}

/// Register an executor waker and receive one owned Ethernet frame if ready.
///
/// This is the synchronous poll boundary required by `embassy-net-driver`;
/// the interrupt producer wakes it through the same bounded channel used by
/// [`receive_wifi_data`].
pub fn poll_receive_wifi_data(cx: &mut Context<'_>) -> Poll<OwnedWifiDataFrame> {
    let mut receive = RX_CHANNEL.receive();
    Pin::new(&mut receive).poll(cx).map(|mut token| {
        record_dequeue(&mut token);
        OwnedWifiDataFrame { token }
    })
}

fn record_dequeue(token: &mut RxSlotToken) {
    let sequence = RX_DEQUEUED.fetch_add(1, Ordering::Relaxed);
    #[cfg(not(all(target_arch = "riscv32", feature = "rx-pipeline-telemetry")))]
    let _ = (sequence, token);
    #[cfg(all(target_arch = "riscv32", feature = "rx-pipeline-telemetry"))]
    if sequence & RX_TELEMETRY_SAMPLE_MASK == 0 {
        let now = cycle_count();
        let cycles = now.wrapping_sub(token.enqueued_cycle);
        token.dequeued_cycle = now;
        token.telemetry_sample = true;
        RX_LATENCY_SAMPLES.fetch_add(1, Ordering::Relaxed);
        RX_QUEUE_TICKS_SUM.fetch_add((cycles >> 8) as usize, Ordering::Relaxed);
        record_high_water(&RX_QUEUE_CYCLES_MAX, cycles as usize);
        if cycles > S31_CYCLES_PER_MILLISECOND {
            RX_QUEUE_OVER_1MS.fetch_add(1, Ordering::Relaxed);
        }
    }
}

pub fn rejected_wifi_data_frames() -> usize {
    REJECTED_RX_FRAMES.load(Ordering::Acquire)
}

#[cfg(any(test, target_arch = "riscv32"))]
#[cfg_attr(
    target_arch = "riscv32",
    link_section = ".rwtext.wifi_strict.data_rx_copy"
)]
unsafe fn copy_into_slot(interface: WifiDataInterface, buffer: *const u8, length: usize) -> bool {
    if buffer.is_null() || length == 0 || length > WIFI_DATA_RX_COPY_FRAME_CAPACITY {
        REJECTED_RX_FRAMES.fetch_add(1, Ordering::Relaxed);
        RX_REJECTED_INVALID.fetch_add(1, Ordering::Relaxed);
        return false;
    }
    let Some((index, slot)) = RX_SLOTS.iter().enumerate().find(|(_, slot)| {
        slot.occupied
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }) else {
        REJECTED_RX_FRAMES.fetch_add(1, Ordering::Relaxed);
        RX_REJECTED_SLOTS_FULL.fetch_add(1, Ordering::Relaxed);
        return false;
    };

    let data = &mut *slot.data.get();
    data.length = length;
    core::ptr::copy_nonoverlapping(buffer, data.bytes.as_mut_ptr(), length);
    enqueue_token(RxSlotToken {
        interface,
        storage: RxStorage::Copied { index },
        #[cfg(all(target_arch = "riscv32", feature = "rx-pipeline-telemetry"))]
        enqueued_cycle: 0,
        #[cfg(all(target_arch = "riscv32", feature = "rx-pipeline-telemetry"))]
        dequeued_cycle: 0,
        #[cfg(all(target_arch = "riscv32", feature = "rx-pipeline-telemetry"))]
        telemetry_sample: false,
    })
}

fn enqueue_token(mut token: RxSlotToken) -> bool {
    #[cfg(not(all(target_arch = "riscv32", feature = "rx-pipeline-telemetry")))]
    let _ = &mut token;
    #[cfg(all(target_arch = "riscv32", feature = "rx-pipeline-telemetry"))]
    {
        token.enqueued_cycle = cycle_count();
    }
    let occupied = RX_OCCUPIED.fetch_add(1, Ordering::AcqRel) + 1;
    record_high_water(&RX_OCCUPIED_HIGH_WATER, occupied);
    if let Err(error) = RX_CHANNEL.try_send(token) {
        REJECTED_RX_FRAMES.fetch_add(1, Ordering::Relaxed);
        if RX_CHANNEL.len() >= WIFI_DATA_RX_CAPACITY {
            RX_REJECTED_SLOTS_FULL.fetch_add(1, Ordering::Relaxed);
        } else {
            RX_REJECTED_CHANNEL_CONTENDED.fetch_add(1, Ordering::Relaxed);
        }
        drop(error.0);
        return false;
    }
    RX_ENQUEUED.fetch_add(1, Ordering::Release);
    true
}

#[cfg(target_arch = "riscv32")]
fn reject_owned_token_at_capacity(token: RxSlotToken) -> bool {
    let occupied = RX_OCCUPIED.fetch_add(1, Ordering::AcqRel) + 1;
    record_high_water(&RX_OCCUPIED_HIGH_WATER, occupied);
    REJECTED_RX_FRAMES.fetch_add(1, Ordering::Relaxed);
    RX_REJECTED_SLOTS_FULL.fetch_add(1, Ordering::Relaxed);
    drop(token);
    false
}

#[cfg(target_arch = "riscv32")]
mod target {
    use core::ffi::c_void;

    use esp_wifi_sys_esp32s31::include::{
        esp_wifi_internal_reg_rxcb, wifi_interface_t_WIFI_IF_AP, wifi_interface_t_WIFI_IF_STA,
        ESP_ERR_NO_MEM, ESP_OK,
    };

    use super::*;

    unsafe extern "C" {
        static mut sta_rxcb: usize;
        static mut ap_rxcb: usize;
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum WifiDataRxInstallError {
        AlreadyInstalled,
        StationRegistration(i32),
        AccessPointRegistration(i32),
    }

    #[no_mangle]
    #[link_section = ".rwtext.wifi_strict.data_rx_sta"]
    pub unsafe extern "C" fn __esp_wifi_async_data_rx_sta(
        buffer: *mut c_void,
        length: u16,
        vendor_buffer: *mut c_void,
    ) -> i32 {
        receive(WifiDataInterface::Station, buffer, length, vendor_buffer)
    }

    #[no_mangle]
    #[link_section = ".rwtext.wifi_strict.data_rx_ap"]
    pub unsafe extern "C" fn __esp_wifi_async_data_rx_ap(
        buffer: *mut c_void,
        length: u16,
        vendor_buffer: *mut c_void,
    ) -> i32 {
        receive(
            WifiDataInterface::AccessPoint,
            buffer,
            length,
            vendor_buffer,
        )
    }

    #[link_section = ".rwtext.wifi_strict.data_rx_dispatch"]
    unsafe fn receive(
        interface: WifiDataInterface,
        buffer: *mut c_void,
        length: u16,
        vendor_buffer: *mut c_void,
    ) -> i32 {
        let length = usize::from(length);
        let large_esf = if length <= WIFI_DATA_RX_FRAME_CAPACITY && !vendor_buffer.is_null() {
            Some(crate::esf::adopt_large_rx_for_network(
                vendor_buffer.cast(),
                buffer.cast(),
                length,
            ))
        } else {
            None
        };
        let accepted = if let Some(Ok(frame)) = large_esf {
            // Transfer the live kind-7 object into the bounded safe channel.
            // Its typed token performs the sole Network -> Free transition on
            // Drop, so raw ESF pointers cannot cross into embassy-net.
            let token = RxSlotToken {
                interface,
                storage: RxStorage::LargeEsf(frame),
                #[cfg(feature = "rx-pipeline-telemetry")]
                enqueued_cycle: 0,
                #[cfg(feature = "rx-pipeline-telemetry")]
                dequeued_cycle: 0,
                #[cfg(feature = "rx-pipeline-telemetry")]
                telemetry_sample: false,
            };
            if RX_CHANNEL.len() >= WIFI_DATA_RX_ZERO_COPY_LIMIT {
                reject_owned_token_at_capacity(token)
            } else {
                enqueue_token(token)
            }
        } else if matches!(
            large_esf,
            Some(Err(crate::esf::LargeRxNetworkAdoptionError::NotRadioOwned))
        ) {
            // A duplicate/stale callback must not recycle a frame already
            // owned by the network task. Fail closed without creating an
            // alias or freeing live storage.
            REJECTED_RX_FRAMES.fetch_add(1, Ordering::Relaxed);
            RX_REJECTED_INVALID.fetch_add(1, Ordering::Relaxed);
            false
        } else {
            // Small vendor-static frames cannot be returned to their intrusive
            // free list from an arbitrary network task. Copy them into the
            // compact owned pool, then recycle while still on the radio owner.
            let accepted = copy_into_slot(interface, buffer.cast(), length);
            if !vendor_buffer.is_null() {
                crate::esf::wifi_strict_esp_wifi_internal_free_rx_buffer(vendor_buffer);
            }
            accepted
        };
        if accepted {
            ESP_OK as i32
        } else {
            ESP_ERR_NO_MEM as i32
        }
    }

    pub fn async_wifi_data_rx_installed() -> bool {
        RX_CALLBACKS_INSTALLED.load(Ordering::Acquire)
            && unsafe {
                core::ptr::addr_of!(sta_rxcb).read()
                    == __esp_wifi_async_data_rx_sta as *const () as usize
                    && core::ptr::addr_of!(ap_rxcb).read()
                        == __esp_wifi_async_data_rx_ap as *const () as usize
            }
    }

    /// Install owned, nonblocking data callbacks for both STA and AP.
    ///
    /// # Safety
    /// Call after Wi-Fi initialization and before either interface can receive
    /// data. No other code may replace these callbacks while strict runtime is
    /// active. The callbacks must only run under the virtual Wi-Fi task
    /// identity established by `VendorPpDispatcher`.
    pub unsafe fn install_async_wifi_data_rx() -> Result<(), WifiDataRxInstallError> {
        if RX_CALLBACKS_INSTALLED.load(Ordering::Acquire) {
            return Err(WifiDataRxInstallError::AlreadyInstalled);
        }
        let result = esp_wifi_internal_reg_rxcb(
            wifi_interface_t_WIFI_IF_STA,
            Some(__esp_wifi_async_data_rx_sta),
        );
        if result != ESP_OK as i32 {
            return Err(WifiDataRxInstallError::StationRegistration(result));
        }
        let result = esp_wifi_internal_reg_rxcb(
            wifi_interface_t_WIFI_IF_AP,
            Some(__esp_wifi_async_data_rx_ap),
        );
        if result != ESP_OK as i32 {
            let _ = esp_wifi_internal_reg_rxcb(wifi_interface_t_WIFI_IF_STA, None);
            return Err(WifiDataRxInstallError::AccessPointRegistration(result));
        }
        RX_CALLBACKS_INSTALLED.store(true, Ordering::Release);
        Ok(())
    }
}

#[cfg(target_arch = "riscv32")]
pub use target::{
    async_wifi_data_rx_installed, install_async_wifi_data_rx, WifiDataRxInstallError,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_slot_owns_bytes_until_frame_drop() {
        let bytes = [1_u8, 2, 3, 4];
        assert!(unsafe {
            copy_into_slot(WifiDataInterface::AccessPoint, bytes.as_ptr(), bytes.len())
        });
        let frame = try_receive_wifi_data().unwrap();
        assert_eq!(frame.interface(), WifiDataInterface::AccessPoint);
        assert_eq!(frame.as_bytes(), &bytes);
        drop(frame);
        assert!(unsafe { copy_into_slot(WifiDataInterface::Station, bytes.as_ptr(), bytes.len()) });
        drop(try_receive_wifi_data().unwrap());
    }

    #[test]
    fn invalid_length_is_rejected_without_claiming_slot() {
        let byte = 0_u8;
        assert!(!unsafe { copy_into_slot(WifiDataInterface::Station, &byte, 0) });
        assert!(!unsafe {
            copy_into_slot(
                WifiDataInterface::Station,
                &byte,
                WIFI_DATA_RX_FRAME_CAPACITY + 1,
            )
        });
    }
}
