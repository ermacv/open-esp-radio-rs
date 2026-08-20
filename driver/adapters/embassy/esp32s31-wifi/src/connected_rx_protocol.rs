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
    rx::RxPhyInfo,
    rx_ampdu::{
        RX_BLOCK_ACK_BANK_COUNT, RxAmpduError, RxAmpduMpdu, RxAmpduRelease, RxBlockAckReorderBanks,
    },
    rx_pool::{NetworkRxFrame, VENDOR_LARGE_RX_PAYLOAD_CAPACITY, VENDOR_LARGE_RX_SLOT_COUNT},
};
use open_esp_radio_esp32s31_wifi_sta::connected_rx::{
    ConnectedRxDispatch, ConnectedRxDispatcher, ConnectedRxEvent, ConnectedRxSink,
};
use open_esp_radio_ieee80211::data::EthernetFrameParts;
use open_esp_radio_wifi_embassy::connected_tasks::ConnectedTaskEndpoint;
use open_esp_radio_wifi_softmac::MacRxMetadata;

use crate::{
    embassy_irq::EmbassyMacIrqRuntime,
    ethernet_rx::PackedEthernetWriter,
    rx_pipeline_observer::{RxPipelineObservation, RxPipelineObserver},
    rx_reorder::{
        RX_REORDER_BACKING_SLOT_COUNT, RX_REORDER_CURRENT_SLOT, RX_REORDER_GAP_TIMEOUT_MICROS,
        RX_REORDER_SLOT_DOMAIN, RxReorderCommand, RxReorderCommandReceiver, RxReorderFrame,
        RxReorderFrameStorage, try_receive_rx_reorder_command,
    },
};

/// Maximum completed protocol dispatches in one cooperative service turn.
///
/// A count is deterministic and free of timer reads in the production
/// datapath. One dispatch always completes before yielding, so no staging or
/// reorder owner is split across executor turns. This matches the finite
/// staging arena: a smaller quantum fragmented service into more executor
/// turns without preventing hardware RX starvation under PSRAM-stack HIL.
const RX_PROTOCOL_DISPATCH_BUDGET: usize = 32;

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

/// Scratch and runtime-arena ownership returned only after a staged RX
/// protocol epoch stops.
///
/// The runtime arena is part of the stopped value deliberately. Losing it
/// would make a second connected epoch reinitialize one-shot static storage
/// instead of reusing the quiesced protocol state.
pub struct ConnectedRxProtocolStopped<'scratch, R> {
    shutdown: ConnectedRxProtocolShutdown,
    mpdu: &'scratch mut [u8],
    ethernet: &'scratch mut [u8],
    runtime: R,
}

impl<'scratch, R> ConnectedRxProtocolStopped<'scratch, R> {
    pub const fn shutdown(&self) -> ConnectedRxProtocolShutdown {
        self.shutdown
    }

    pub fn into_parts(self) -> (&'scratch mut [u8], &'scratch mut [u8], R) {
        (self.mpdu, self.ethernet, self.runtime)
    }
}

/// Concrete stopped owner returned by [`Esp32s31ConnectedRxProtocol`].
///
/// Composition roots should name this alias rather than restating the
/// internal runtime-arena reference type in executor control resources.
pub type Esp32s31ConnectedRxProtocolStopped<
    'scratch,
    'pool,
    const CAPACITY: usize = VENDOR_LARGE_RX_PAYLOAD_CAPACITY,
    const SLOTS: usize = VENDOR_LARGE_RX_SLOT_COUNT,
    const REORDER_SLOTS: usize = RX_REORDER_BACKING_SLOT_COUNT,
> = ConnectedRxProtocolStopped<
    'scratch,
    &'pool mut Esp32s31ConnectedRxProtocolStorage<'pool, CAPACITY, SLOTS, REORDER_SLOTS>,
>;

/// Async admission edge required by the staged protocol consumer.
///
/// The synchronous [`ConnectedRxSink`] callback remains useful for finite
/// parsing and control observers. This companion edge lets a network adapter
/// retain the staged frame until its bounded output queue has ownership.
pub trait ConnectedRxProtocolSink<
    const CAPACITY: usize = VENDOR_LARGE_RX_PAYLOAD_CAPACITY,
    const SLOTS: usize = VENDOR_LARGE_RX_SLOT_COUNT,
>: ConnectedRxSink
{
    /// Capacity edge for a frame that must be copied into adapter-owned
    /// storage, including A-MSDU subframes and reorder slow paths.
    fn wait_ready(&mut self) -> impl Future<Output = ()> + '_;

    /// Capacity edge for an ordinary frame still owning its staging slot.
    /// In-place sinks need no second slot and therefore return immediately.
    fn wait_staged_ready(&mut self) -> impl Future<Output = ()> + '_ {
        self.wait_ready()
    }

    /// Publish an ordinary Ethernet frame from its unique staging owner.
    /// The default preserves the copying behavior for test/control sinks.
    fn publish_staged(
        &mut self,
        frame: Esp32s31StagedRxFrame<'_, CAPACITY, SLOTS>,
        ethernet: StagedEthernetPublication,
    ) -> StagedRxDisposition {
        {
            let raw = frame.segment().buffer;
            let payload_end = ethernet
                .payload_offset
                .checked_add(ethernet.payload_length)
                .expect("captured Ethernet payload range cannot overflow");
            let payload = raw
                .get(ethernet.payload_offset..payload_end)
                .expect("captured Ethernet payload belongs to its staged frame");
            self.publish(ConnectedRxEvent::Ethernet {
                frame: EthernetFrameParts {
                    destination: ethernet.destination,
                    source: ethernet.source,
                    ether_type: ethernet.ether_type,
                    payload,
                },
                raw,
                amsdu: false,
                metadata: ethernet.metadata,
            });
        }
        drop(frame);
        StagedRxDisposition::Released
    }
}

/// Borrow-free description captured while the dispatcher validates one
/// ordinary Ethernet frame inside its staging slot.
#[derive(Clone, Copy, Debug)]
pub struct StagedEthernetPublication {
    pub destination: [u8; 6],
    pub source: [u8; 6],
    pub ether_type: u16,
    pub payload_offset: usize,
    pub payload_length: usize,
    pub metadata: MacRxMetadata<RxPhyInfo>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StagedRxDisposition {
    /// The sink dropped or copied the staging owner synchronously.
    Released,
    /// The network device retains the slot until its RX token is consumed.
    RetainedByNetwork,
}

/// Adapter for sinks whose `publish` operation cannot experience ownership
/// backpressure, such as fixed test observers.
pub struct AlwaysReadyConnectedRxSink<S>(pub S);

impl<S: ConnectedRxSink> ConnectedRxSink for AlwaysReadyConnectedRxSink<S> {
    fn publish(
        &mut self,
        event: open_esp_radio_esp32s31_wifi_sta::connected_rx::ConnectedRxEvent<'_>,
    ) {
        self.0.publish(event);
    }
}

impl<S: ConnectedRxSink, const CAPACITY: usize, const SLOTS: usize>
    ConnectedRxProtocolSink<CAPACITY, SLOTS> for AlwaysReadyConnectedRxSink<S>
{
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
    frames: PackedEthernetWriter<'storage>,
    metadata: Option<MacRxMetadata<RxPhyInfo>>,
}

impl<'storage> DeferredEthernetFrames<'storage> {
    fn new(storage: &'storage mut [u8]) -> Self {
        Self {
            frames: PackedEthernetWriter::new(storage),
            metadata: None,
        }
    }

    const fn used(&self) -> usize {
        self.frames.used()
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
        self.frames
            .push(frame)
            .expect("A-MSDU output fits the constructor-qualified scratch buffer");
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

/// Caller-owned state arena for one connected RX protocol instance.
///
/// This state is deliberately separate from [`Esp32s31ConnectedRxProtocol`]:
/// the retained lease table and eight BlockAck reorder machines are large,
/// long-lived data, while the protocol value is moved through composition and
/// async task boundaries. Embedded composition roots should place this arena
/// in static storage and pass a unique mutable borrow to each connected epoch.
pub struct Esp32s31ConnectedRxProtocolStorage<
    'pool,
    const CAPACITY: usize,
    const SLOTS: usize,
    const REORDER_SLOTS: usize = RX_REORDER_BACKING_SLOT_COUNT,
> {
    reorder_banks: RxBlockAckReorderBanks<RX_REORDER_SLOT_DOMAIN>,
    reorder_first_starts: [Option<u16>; RX_BLOCK_ACK_BANK_COUNT],
    gap_deadlines: [Option<Instant>; RX_BLOCK_ACK_BANK_COUNT],
    retained: [Option<RetainedRxFrame<'pool, CAPACITY, SLOTS, REORDER_SLOTS>>; REORDER_SLOTS],
}

impl<'pool, const CAPACITY: usize, const SLOTS: usize, const REORDER_SLOTS: usize>
    Esp32s31ConnectedRxProtocolStorage<'pool, CAPACITY, SLOTS, REORDER_SLOTS>
{
    pub const fn new() -> Self {
        Self {
            reorder_banks: RxBlockAckReorderBanks::new(),
            reorder_first_starts: [None; RX_BLOCK_ACK_BANK_COUNT],
            gap_deadlines: [None; RX_BLOCK_ACK_BANK_COUNT],
            retained: [const { None }; REORDER_SLOTS],
        }
    }
}

impl<'pool, const CAPACITY: usize, const SLOTS: usize, const REORDER_SLOTS: usize> Default
    for Esp32s31ConnectedRxProtocolStorage<'pool, CAPACITY, SLOTS, REORDER_SLOTS>
{
    fn default() -> Self {
        Self::new()
    }
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
    processor: Esp32s31ConnectedRxProcessor<
        'queue,
        'pool,
        'scratch,
        'irq,
        M,
        S,
        CAPACITY,
        SLOTS,
        REORDER_SLOTS,
    >,
}

/// Queue-independent connected-station RX protocol processor.
///
/// It owns parsing, reorder state, output scratch and the station sink, but no
/// source queue. Standalone STA scheduling wraps it in
/// [`Esp32s31ConnectedRxProtocol`]; same-channel STA+AP scheduling can feed an
/// already routed staging lease directly without manufacturing a second DMA
/// producer or compatibility queue.
pub struct Esp32s31ConnectedRxProcessor<
    'queue,
    'pool,
    'scratch,
    'irq,
    M: RawMutex,
    S,
    const CAPACITY: usize = VENDOR_LARGE_RX_PAYLOAD_CAPACITY,
    const SLOTS: usize = VENDOR_LARGE_RX_SLOT_COUNT,
    const REORDER_SLOTS: usize = RX_REORDER_BACKING_SLOT_COUNT,
> {
    irq: &'irq EmbassyMacIrqRuntime<M>,
    dispatcher: ConnectedRxDispatcher,
    sink: S,
    mpdu: &'scratch mut [u8],
    ethernet: &'scratch mut [u8],
    pipeline_observer: Option<&'queue dyn RxPipelineObserver>,
    reorder_commands: Option<RxReorderCommandReceiver<'queue, M>>,
    reorder_storage: Option<&'pool RxReorderFrameStorage<CAPACITY, REORDER_SLOTS>>,
    reorder_scratch: Option<&'scratch mut [u8]>,
    runtime: &'pool mut Esp32s31ConnectedRxProtocolStorage<'pool, CAPACITY, SLOTS, REORDER_SLOTS>,
}

mod dispatch;
mod owner;
mod reorder;
mod scheduler;

#[cfg(test)]
mod tests;
