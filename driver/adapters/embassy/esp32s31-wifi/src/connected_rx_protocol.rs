//! Ownership handoff between the PAC/DMA RX actor and protocol processing.
//!
//! The producer queue stores unique staging-pool leases, never DMA pointers.
//! This lets the radio owner finish one finite completion epoch and return to
//! TX/control arbitration while a separate future performs 802.11 parsing and
//! publishes Ethernet or connected-control effects.

use core::future::{Future, ready};

use embassy_futures::select::{Either, select};
use embassy_sync::channel::{Channel, Receiver, Sender};
use embassy_time::{Duration, Instant, Timer};
use open_esp_radio_embassy_net::RawMutex;
use open_esp_radio_esp32s31_wifi_mac::{
    connected_rx::{ConnectedRxDispatch, ConnectedRxDispatcher, ConnectedRxEvent, ConnectedRxSink},
    rx::RxPhyInfo,
    rx_ampdu::{RxAmpduError, RxAmpduMpdu, RxAmpduRelease, RxBlockAckReorderState},
    rx_pool::{NetworkRxFrame, VENDOR_LARGE_RX_PAYLOAD_CAPACITY, VENDOR_LARGE_RX_SLOT_COUNT},
};
use open_esp_radio_ieee80211::data::EthernetFrameParts;
use open_esp_radio_wifi_softmac::MacRxMetadata;

use crate::{
    embassy_irq::EmbassyMacIrqRuntime,
    rx_pipeline_observer::{RxPipelineObservation, RxPipelineObserver},
    rx_reorder::{
        RX_REORDER_BACKING_SLOT_COUNT, RX_REORDER_CURRENT_SLOT, RX_REORDER_GAP_TIMEOUT_MICROS,
        RX_REORDER_SLOT_DOMAIN, RxReorderCommand, RxReorderCommandReceiver, RxReorderFrame,
        RxReorderFrameStorage, try_receive_rx_reorder_command,
    },
};

const RX_BLOCK_ACK_TID_COUNT: usize = 8;

/// Ownership released when a connected staged-RX epoch is stopped.
///
/// The counts are diagnostic evidence for an outer station lifecycle. A
/// nonzero value is legal: disconnect may race with already staged input, but
/// every counted frame or command has been discarded before this value is
/// returned.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ConnectedRxProtocolShutdown {
    pub queued_frames: usize,
    pub retained_frames: usize,
    pub reorder_commands: usize,
    pub active_reorders: usize,
}

/// Scratch ownership returned only after a staged RX protocol epoch stops.
pub struct ConnectedRxProtocolStopped<'scratch> {
    shutdown: ConnectedRxProtocolShutdown,
    mpdu: &'scratch mut [u8],
    ethernet: &'scratch mut [u8],
}

impl<'scratch> ConnectedRxProtocolStopped<'scratch> {
    pub const fn shutdown(&self) -> ConnectedRxProtocolShutdown {
        self.shutdown
    }

    pub fn into_scratch(self) -> (&'scratch mut [u8], &'scratch mut [u8]) {
        (self.mpdu, self.ethernet)
    }
}

/// Async admission edge required by the staged protocol consumer.
///
/// The synchronous [`ConnectedRxSink`] callback remains useful for finite
/// parsing and control observers. This companion edge lets a network adapter
/// retain the staged frame until its bounded output queue has ownership.
pub trait ConnectedRxProtocolSink: ConnectedRxSink {
    fn wait_ready(&mut self) -> impl Future<Output = ()> + '_;
}

/// Adapter for sinks whose `publish` operation cannot experience ownership
/// backpressure, such as fixed test observers.
pub struct AlwaysReadyConnectedRxSink<S>(pub S);

impl<S: ConnectedRxSink> ConnectedRxSink for AlwaysReadyConnectedRxSink<S> {
    fn publish(
        &mut self,
        event: open_esp_radio_esp32s31_wifi_mac::connected_rx::ConnectedRxEvent<'_>,
    ) {
        self.0.publish(event);
    }
}

impl<S: ConnectedRxSink> ConnectedRxProtocolSink for AlwaysReadyConnectedRxSink<S> {
    fn wait_ready(&mut self) -> impl Future<Output = ()> + '_ {
        ready(())
    }
}

/// Scratch-backed A-MSDU publication plan.
///
/// Each record is a big-endian `u16` Ethernet length followed by the complete
/// frame. Decapsulation removes the eight-byte LLC/SNAP header from every
/// subframe, so the two-byte record prefix still leaves the packed output
/// strictly smaller than its valid A-MSDU input.
struct DeferredEthernetFrames<'storage> {
    storage: &'storage mut [u8],
    used: usize,
    metadata: Option<MacRxMetadata<RxPhyInfo>>,
}

impl<'storage> DeferredEthernetFrames<'storage> {
    fn new(storage: &'storage mut [u8]) -> Self {
        Self {
            storage,
            used: 0,
            metadata: None,
        }
    }
}

impl ConnectedRxSink for DeferredEthernetFrames<'_> {
    fn publish(&mut self, event: ConnectedRxEvent<'_>) {
        let ConnectedRxEvent::Ethernet {
            frame, metadata, ..
        } = event
        else {
            return;
        };
        if let Some(previous) = self.metadata {
            debug_assert_eq!(previous, metadata);
        } else {
            self.metadata = Some(metadata);
        }
        let encoded_length = u16::try_from(frame.length())
            .expect("staged RX capacity bounds a deferred Ethernet frame");
        let record_length = frame
            .length()
            .checked_add(2)
            .expect("deferred Ethernet record length cannot overflow");
        let end = self
            .used
            .checked_add(record_length)
            .expect("deferred A-MSDU plan length cannot overflow");
        let record = self
            .storage
            .get_mut(self.used..end)
            .expect("A-MSDU output fits the constructor-qualified scratch buffer");
        record[..2].copy_from_slice(&encoded_length.to_be_bytes());
        frame
            .copy_to(&mut record[2..])
            .expect("deferred record has the exact Ethernet frame length");
        self.used = end;
    }
}

/// Unique owner of one staged RX unit.
///
/// The default retains the ordinary vendor large-RX profile. A platform that
/// negotiates the 3,839-byte A-MSDU class must select a correspondingly larger
/// capacity instead of silently discarding a valid multi-MSDU receive unit.
pub type Esp32s31StagedRxFrame<
    'pool,
    const CAPACITY: usize = VENDOR_LARGE_RX_PAYLOAD_CAPACITY,
    const SLOTS: usize = VENDOR_LARGE_RX_SLOT_COUNT,
> = NetworkRxFrame<'pool, SLOTS, CAPACITY>;

enum RetainedRxFrame<'pool, const CAPACITY: usize, const SLOTS: usize, const REORDER_SLOTS: usize> {
    Hot(Esp32s31StagedRxFrame<'pool, CAPACITY, SLOTS>),
    Cold(RxReorderFrame<'pool, CAPACITY, REORDER_SLOTS>),
}

/// Static bounded storage for the radio-to-protocol ownership handoff.
///
/// Queue depth is a memory/resource limit, not a per-poll processing budget.
/// The useful maximum cannot exceed the staging-pool slot count.
pub struct Esp32s31StagedRxQueue<
    'pool,
    M: RawMutex,
    const DEPTH: usize,
    const CAPACITY: usize = VENDOR_LARGE_RX_PAYLOAD_CAPACITY,
    const SLOTS: usize = VENDOR_LARGE_RX_SLOT_COUNT,
    const REORDER_SLOTS: usize = RX_REORDER_BACKING_SLOT_COUNT,
> {
    frames: Channel<M, Esp32s31StagedRxFrame<'pool, CAPACITY, SLOTS>, DEPTH>,
}

impl<'pool, M: RawMutex, const DEPTH: usize, const CAPACITY: usize, const SLOTS: usize>
    Esp32s31StagedRxQueue<'pool, M, DEPTH, CAPACITY, SLOTS>
{
    pub const fn new() -> Self {
        assert!(DEPTH != 0, "staged RX queue must not be empty");
        assert!(
            DEPTH <= SLOTS,
            "staged RX queue cannot outgrow its ownership pool"
        );
        Self {
            frames: Channel::new(),
        }
    }

    pub fn split(
        &self,
    ) -> (
        Sender<'_, M, Esp32s31StagedRxFrame<'pool, CAPACITY, SLOTS>, DEPTH>,
        Receiver<'_, M, Esp32s31StagedRxFrame<'pool, CAPACITY, SLOTS>, DEPTH>,
    ) {
        (self.frames.sender(), self.frames.receiver())
    }
}

impl<'pool, M: RawMutex, const DEPTH: usize, const CAPACITY: usize, const SLOTS: usize> Default
    for Esp32s31StagedRxQueue<'pool, M, DEPTH, CAPACITY, SLOTS>
{
    fn default() -> Self {
        Self::new()
    }
}

/// Protocol-side consumer of staged RX ownership.
///
/// `dispatch_next` contains no PAC access. Dropping the frame after dispatch
/// returns its staging credit and wakes a radio actor that previously stopped
/// at natural ownership backpressure.
pub struct Esp32s31ConnectedRxProtocol<
    'queue,
    'pool,
    'scratch,
    'irq,
    M: RawMutex,
    S,
    const DEPTH: usize,
    const CAPACITY: usize = VENDOR_LARGE_RX_PAYLOAD_CAPACITY,
    const SLOTS: usize = VENDOR_LARGE_RX_SLOT_COUNT,
    const REORDER_SLOTS: usize = RX_REORDER_BACKING_SLOT_COUNT,
> {
    frames: Receiver<'queue, M, Esp32s31StagedRxFrame<'pool, CAPACITY, SLOTS>, DEPTH>,
    irq: &'irq EmbassyMacIrqRuntime<M>,
    dispatcher: ConnectedRxDispatcher,
    sink: S,
    mpdu: &'scratch mut [u8],
    ethernet: &'scratch mut [u8],
    pipeline_observer: Option<&'queue dyn RxPipelineObserver>,
    reorder_commands: Option<RxReorderCommandReceiver<'queue, M>>,
    reorder_storage: Option<&'pool RxReorderFrameStorage<CAPACITY, REORDER_SLOTS>>,
    reorder_scratch: Option<&'scratch mut [u8]>,
    reorders: [Option<RxBlockAckReorderState<RX_REORDER_SLOT_DOMAIN>>; RX_BLOCK_ACK_TID_COUNT],
    reorder_first_starts: [Option<u16>; RX_BLOCK_ACK_TID_COUNT],
    gap_deadlines: [Option<Instant>; RX_BLOCK_ACK_TID_COUNT],
    retained: [Option<RetainedRxFrame<'pool, CAPACITY, SLOTS, REORDER_SLOTS>>; REORDER_SLOTS],
}

mod dispatch;
mod owner;
mod reorder;
mod scheduler;

#[cfg(test)]
mod tests;
