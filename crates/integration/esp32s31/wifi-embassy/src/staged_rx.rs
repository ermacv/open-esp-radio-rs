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
    connected_rx::{ConnectedRxDispatch, ConnectedRxDispatcher, ConnectedRxSink},
    rx_pool::{NetworkRxFrame, VENDOR_LARGE_RX_PAYLOAD_CAPACITY, VENDOR_LARGE_RX_SLOT_COUNT},
};

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

/// Unique owner of one vendor-profile staged RX unit.
pub type Esp32s31StagedRxFrame<'pool> =
    NetworkRxFrame<'pool, VENDOR_LARGE_RX_SLOT_COUNT, VENDOR_LARGE_RX_PAYLOAD_CAPACITY>;

/// Static bounded storage for the radio-to-protocol ownership handoff.
///
/// Queue depth is a memory/resource limit, not a per-poll processing budget.
/// The useful maximum cannot exceed the staging-pool slot count.
pub struct Esp32s31StagedRxQueue<'pool, M: RawMutex, const DEPTH: usize> {
    frames: Channel<M, Esp32s31StagedRxFrame<'pool>, DEPTH>,
}

impl<'pool, M: RawMutex, const DEPTH: usize> Esp32s31StagedRxQueue<'pool, M, DEPTH> {
    pub const fn new() -> Self {
        assert!(DEPTH != 0, "staged RX queue must not be empty");
        assert!(
            DEPTH <= VENDOR_LARGE_RX_SLOT_COUNT,
            "staged RX queue cannot outgrow its ownership pool"
        );
        Self {
            frames: Channel::new(),
        }
    }

    pub fn split(
        &self,
    ) -> (
        Sender<'_, M, Esp32s31StagedRxFrame<'pool>, DEPTH>,
        Receiver<'_, M, Esp32s31StagedRxFrame<'pool>, DEPTH>,
    ) {
        (self.frames.sender(), self.frames.receiver())
    }
}

impl<'pool, M: RawMutex, const DEPTH: usize> Default for Esp32s31StagedRxQueue<'pool, M, DEPTH> {
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
> {
    frames: Receiver<'queue, M, Esp32s31StagedRxFrame<'pool>, DEPTH>,
    irq: &'irq EmbassyMacIrqRuntime<M>,
    dispatcher: ConnectedRxDispatcher,
    sink: S,
    mpdu: &'scratch mut [u8],
    ethernet: &'scratch mut [u8],
    pipeline_counters: Option<&'queue RxPipelineCounters>,
}

impl<'queue, 'pool, 'scratch, 'irq, M: RawMutex, S, const DEPTH: usize>
    Esp32s31ConnectedRxProtocol<'queue, 'pool, 'scratch, 'irq, M, S, DEPTH>
where
    S: ConnectedRxProtocolSink,
{
    pub fn new(
        frames: Receiver<'queue, M, Esp32s31StagedRxFrame<'pool>, DEPTH>,
        irq: &'irq EmbassyMacIrqRuntime<M>,
        dispatcher: ConnectedRxDispatcher,
        sink: S,
        mpdu: &'scratch mut [u8],
        ethernet: &'scratch mut [u8],
    ) -> Self {
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
        if self.dispatcher.may_publish_ethernet(frame.segment()) {
            // Keep the staging lease until the next network output owner
            // exists. Connected operation currently negotiates A-MSDU off, so
            // one credit admits every data MPDU. If multi-subframe A-MSDU is
            // enabled later, its output reservation must become a
            // multi-credit transaction.
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

    /// Run protocol processing independently from the PAC/DMA owner.
    pub async fn run(&mut self) -> ! {
        loop {
            self.dispatch_next().await;
        }
    }
}
