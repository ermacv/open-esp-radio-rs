//! Ownership handoff between the PAC/DMA RX actor and protocol processing.
//!
//! The producer queue stores unique staging-pool leases, never DMA pointers.
//! This lets the radio owner finish one finite completion epoch and return to
//! TX/control arbitration while a separate future performs 802.11 parsing and
//! publishes Ethernet or connected-control effects.

use core::future::{Future, ready};

use embassy_sync::channel::{Channel, Receiver, Sender};
use open_esp_radio_embassy_net::RawMutex;
use open_esp_radio_esp32s31_wifi_mac::{
    connected_rx::{ConnectedRxDispatch, ConnectedRxDispatcher, ConnectedRxEvent, ConnectedRxSink},
    rx_pool::{NetworkRxFrame, VENDOR_LARGE_RX_PAYLOAD_CAPACITY, VENDOR_LARGE_RX_SLOT_COUNT},
};
use open_esp_radio_ieee80211::data::EthernetFrameParts;

use crate::{embassy_irq::EmbassyMacIrqRuntime, rx_telemetry::RxPipelineCounters};

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
}

impl<'storage> DeferredEthernetFrames<'storage> {
    fn new(storage: &'storage mut [u8]) -> Self {
        Self { storage, used: 0 }
    }
}

impl ConnectedRxSink for DeferredEthernetFrames<'_> {
    fn publish(&mut self, event: ConnectedRxEvent<'_>) {
        let ConnectedRxEvent::Ethernet { frame, .. } = event else {
            return;
        };
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
> {
    frames: Receiver<'queue, M, Esp32s31StagedRxFrame<'pool, CAPACITY, SLOTS>, DEPTH>,
    irq: &'irq EmbassyMacIrqRuntime<M>,
    dispatcher: ConnectedRxDispatcher,
    sink: S,
    mpdu: &'scratch mut [u8],
    ethernet: &'scratch mut [u8],
    pipeline_counters: Option<&'queue RxPipelineCounters>,
}

impl<
    'queue,
    'pool,
    'scratch,
    'irq,
    M: RawMutex,
    S,
    const DEPTH: usize,
    const CAPACITY: usize,
    const SLOTS: usize,
> Esp32s31ConnectedRxProtocol<'queue, 'pool, 'scratch, 'irq, M, S, DEPTH, CAPACITY, SLOTS>
where
    S: ConnectedRxProtocolSink,
{
    pub fn new(
        frames: Receiver<'queue, M, Esp32s31StagedRxFrame<'pool, CAPACITY, SLOTS>, DEPTH>,
        irq: &'irq EmbassyMacIrqRuntime<M>,
        dispatcher: ConnectedRxDispatcher,
        sink: S,
        mpdu: &'scratch mut [u8],
        ethernet: &'scratch mut [u8],
    ) -> Self {
        assert!(
            CAPACITY <= usize::from(u16::MAX),
            "staged RX capacity must fit the deferred record length"
        );
        assert!(
            ethernet.len() >= CAPACITY,
            "A-MSDU output scratch must cover one complete staged RX unit"
        );
        Self {
            frames,
            irq,
            dispatcher,
            sink,
            mpdu,
            ethernet,
            pipeline_counters: None,
        }
    }

    pub fn with_pipeline_counters(mut self, counters: &'queue RxPipelineCounters) -> Self {
        self.pipeline_counters = Some(counters);
        self
    }

    pub const fn dispatcher(&self) -> &ConnectedRxDispatcher {
        &self.dispatcher
    }

    pub const fn sink(&self) -> &S {
        &self.sink
    }

    pub fn sink_mut(&mut self) -> &mut S {
        &mut self.sink
    }

    pub fn queue_len(&self) -> usize {
        self.frames.len()
    }

    /// Wait for and dispatch one independently owned staged frame.
    pub async fn dispatch_next(&mut self) -> ConnectedRxDispatch {
        let frame = self.frames.receive().await;
        if self.dispatcher.may_publish_amsdu(frame.segment()) {
            return self.dispatch_amsdu(frame).await;
        }
        if self.dispatcher.may_publish_ethernet(frame.segment()) {
            // Keep the staging lease until the single-MSDU path owns its one
            // network output slot. A-MSDU uses the deferred streaming path
            // above and acquires one slot per decoded subframe.
            let wait_started = self.pipeline_counters.map(RxPipelineCounters::now_micros);
            self.sink.wait_ready().await;
            if let (Some(counters), Some(started)) = (self.pipeline_counters, wait_started) {
                counters.record_network_ready_wait(counters.elapsed_micros_since(started));
            }
        }
        let dispatch_started = self.pipeline_counters.map(RxPipelineCounters::now_micros);
        let result =
            self.dispatcher
                .dispatch(frame.segment(), self.mpdu, self.ethernet, &mut self.sink);
        if let (Some(counters), Some(started)) = (self.pipeline_counters, dispatch_started) {
            counters.record_dispatch(
                matches!(result, ConnectedRxDispatch::Data { .. }),
                counters.elapsed_micros_since(started),
            );
        }
        drop(frame);
        self.irq.notify_rx_capacity();
        result
    }

    async fn dispatch_amsdu(
        &mut self,
        frame: Esp32s31StagedRxFrame<'pool, CAPACITY, SLOTS>,
    ) -> ConnectedRxDispatch {
        let dispatch_started = self.pipeline_counters.map(RxPipelineCounters::now_micros);
        let mut deferred = DeferredEthernetFrames::new(self.ethernet);
        let result = self
            .dispatcher
            .dispatch(frame.segment(), self.mpdu, &mut [], &mut deferred);
        let used = deferred.used;
        drop(deferred);
        if let (Some(counters), Some(started)) = (self.pipeline_counters, dispatch_started) {
            counters.record_dispatch(
                matches!(result, ConnectedRxDispatch::Data { .. }),
                counters.elapsed_micros_since(started),
            );
        }
        let raw = frame.segment().buffer;
        let mut offset = 0_usize;
        while offset < used {
            let length = usize::from(u16::from_be_bytes([
                self.ethernet[offset],
                self.ethernet[offset + 1],
            ]));
            let start = offset + 2;
            let end = start + length;
            let wait_started = self.pipeline_counters.map(RxPipelineCounters::now_micros);
            self.sink.wait_ready().await;
            if let (Some(counters), Some(started)) = (self.pipeline_counters, wait_started) {
                counters.record_network_ready_wait(counters.elapsed_micros_since(started));
            }
            let ethernet = &self.ethernet[start..end];
            self.sink.publish(ConnectedRxEvent::Ethernet {
                frame: EthernetFrameParts {
                    destination: ethernet[..6]
                        .try_into()
                        .expect("deferred Ethernet destination has six bytes"),
                    source: ethernet[6..12]
                        .try_into()
                        .expect("deferred Ethernet source has six bytes"),
                    ether_type: u16::from_be_bytes([ethernet[12], ethernet[13]]),
                    payload: &ethernet[14..],
                },
                raw,
                amsdu: true,
            });
            offset = end;
        }

        drop(frame);
        self.irq.notify_rx_capacity();
        result
    }

    /// Run protocol processing independently from the PAC/DMA owner.
    pub async fn run(&mut self) -> ! {
        loop {
            self.dispatch_next().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deferred_ethernet_frames_pack_complete_ordered_records() {
        let first_payload = [1, 2, 3];
        let second_payload = [4, 5];
        let first = EthernetFrameParts {
            destination: [0x10; 6],
            source: [0x20; 6],
            ether_type: 0x0800,
            payload: &first_payload,
        };
        let second = EthernetFrameParts {
            destination: [0x30; 6],
            source: [0x40; 6],
            ether_type: 0x0806,
            payload: &second_payload,
        };
        let mut storage = [0_u8; 64];
        let mut deferred = DeferredEthernetFrames::new(&mut storage);

        deferred.publish(ConnectedRxEvent::Ethernet {
            frame: first,
            raw: &[],
            amsdu: true,
        });
        deferred.publish(ConnectedRxEvent::Ethernet {
            frame: second,
            raw: &[],
            amsdu: true,
        });

        let first_end = 2 + first.length();
        assert_eq!(
            &deferred.storage[..2],
            &(first.length() as u16).to_be_bytes()
        );
        assert_eq!(&deferred.storage[2..8], &first.destination);
        assert_eq!(&deferred.storage[8..14], &first.source);
        assert_eq!(&deferred.storage[14..16], &first.ether_type.to_be_bytes());
        assert_eq!(&deferred.storage[16..first_end], &first_payload);
        assert_eq!(
            &deferred.storage[first_end..first_end + 2],
            &(second.length() as u16).to_be_bytes()
        );
        assert_eq!(deferred.used, first_end + 2 + second.length());
    }
}
