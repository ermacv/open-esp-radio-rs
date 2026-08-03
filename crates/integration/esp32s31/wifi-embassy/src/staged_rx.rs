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
    rx_ampdu::{RxAmpduError, RxAmpduMpdu, RxAmpduRelease, RxBlockAckReorderState},
    rx_pool::{NetworkRxFrame, VENDOR_LARGE_RX_PAYLOAD_CAPACITY, VENDOR_LARGE_RX_SLOT_COUNT},
};
use open_esp_radio_ieee80211::data::EthernetFrameParts;

use crate::{
    embassy_irq::EmbassyMacIrqRuntime,
    rx_reorder::{
        RX_REORDER_BACKING_SLOT_COUNT, RX_REORDER_CURRENT_SLOT, RX_REORDER_GAP_TIMEOUT_MICROS,
        RX_REORDER_SLOT_DOMAIN, RxReorderCommand, RxReorderCommandReceiver, RxReorderFrame,
        RxReorderFrameStorage, try_receive_rx_reorder_command,
    },
    rx_telemetry::RxPipelineCounters,
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

enum RetainedRxFrame<'pool, const CAPACITY: usize, const SLOTS: usize> {
    Hot(Esp32s31StagedRxFrame<'pool, CAPACITY, SLOTS>),
    Cold(RxReorderFrame<'pool, CAPACITY>),
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
    reorder_commands: Option<RxReorderCommandReceiver<'queue, M>>,
    reorder_storage: Option<&'pool RxReorderFrameStorage<CAPACITY>>,
    reorder_scratch: Option<&'scratch mut [u8]>,
    reorders: [Option<RxBlockAckReorderState<RX_REORDER_SLOT_DOMAIN>>; RX_BLOCK_ACK_TID_COUNT],
    reorder_first_starts: [Option<u16>; RX_BLOCK_ACK_TID_COUNT],
    gap_deadlines: [Option<Instant>; RX_BLOCK_ACK_TID_COUNT],
    retained: [Option<RetainedRxFrame<'pool, CAPACITY, SLOTS>>; RX_REORDER_BACKING_SLOT_COUNT],
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
        assert!(SLOTS != 0, "staged RX pool must not be empty");
        assert!(
            SLOTS <= usize::from(u8::MAX) + 1,
            "reorder slot identity must fit the MAC token"
        );
        Self {
            frames,
            irq,
            dispatcher,
            sink,
            mpdu,
            ethernet,
            pipeline_counters: None,
            reorder_commands: None,
            reorder_storage: None,
            reorder_scratch: None,
            reorders: core::array::from_fn(|_| None),
            reorder_first_starts: [None; RX_BLOCK_ACK_TID_COUNT],
            gap_deadlines: [None; RX_BLOCK_ACK_TID_COUNT],
            retained: core::array::from_fn(|_| None),
        }
    }

    pub fn with_rx_reorder_commands(
        mut self,
        commands: RxReorderCommandReceiver<'queue, M>,
    ) -> Self {
        self.reorder_commands = Some(commands);
        self
    }

    /// Install cold backing for the MPDUs that actually cross a sequence gap.
    /// In-order frames continue directly from the SRAM staging lease.
    pub fn with_rx_reorder_storage(
        mut self,
        storage: &'pool RxReorderFrameStorage<CAPACITY>,
    ) -> Self {
        self.reorder_storage = Some(storage);
        self
    }

    /// Install one internal-SRAM readback scratch for a retained ordinary
    /// MPDU. This avoids repeatedly parsing the cold PSRAM backing in place.
    /// A-MSDU keeps its distinct output-scratch path.
    pub fn with_rx_reorder_scratch(mut self, scratch: &'scratch mut [u8]) -> Self {
        assert!(
            scratch.len() >= CAPACITY,
            "reorder readback scratch must cover one complete staged RX unit"
        );
        self.reorder_scratch = Some(scratch);
        self
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

    /// Discard all ownership retained by the completed connected epoch.
    ///
    /// The connected control producer must already be stopped, otherwise it
    /// could publish a new reorder command after the mailbox is drained. This
    /// operation performs no PAC access and no sink publication: reconnect
    /// teardown must not block on a full network queue merely to return RX
    /// staging and cold-reorder leases.
    pub fn shutdown_discard(&mut self) -> ConnectedRxProtocolShutdown {
        let mut shutdown = ConnectedRxProtocolShutdown::default();
        while let Ok(frame) = self.frames.try_receive() {
            drop(frame);
            shutdown.queued_frames = shutdown.queued_frames.saturating_add(1);
        }
        if let Some(commands) = &self.reorder_commands {
            while try_receive_rx_reorder_command(commands).is_some() {
                shutdown.reorder_commands = shutdown.reorder_commands.saturating_add(1);
            }
        }
        for reorder in &mut self.reorders {
            if reorder.take().is_some() {
                shutdown.active_reorders = shutdown.active_reorders.saturating_add(1);
            }
        }
        self.reorder_first_starts.fill(None);
        self.gap_deadlines.fill(None);
        for retained in &mut self.retained {
            if retained.take().is_some() {
                shutdown.retained_frames = shutdown.retained_frames.saturating_add(1);
            }
        }
        if shutdown.queued_frames != 0 || shutdown.retained_frames != 0 {
            self.irq.notify_rx_capacity();
        }
        shutdown
    }

    /// Wait for and dispatch one independently owned staged frame.
    pub async fn dispatch_next(&mut self) -> ConnectedRxDispatch {
        loop {
            if let Some(command) = self
                .reorder_commands
                .as_ref()
                .and_then(try_receive_rx_reorder_command)
            {
                if let Some(result) = self.apply_reorder_command(command).await {
                    return result;
                }
                continue;
            }

            let next_gap = self.next_gap_deadline();
            let frame = if let Some(commands) = &self.reorder_commands {
                if let Some((tid, deadline)) = next_gap {
                    match select(
                        select(commands.receive(), self.frames.receive()),
                        Timer::at(deadline),
                    )
                    .await
                    {
                        Either::First(Either::First(command)) => {
                            if let Some(result) = self.apply_reorder_command(command).await {
                                return result;
                            }
                            continue;
                        }
                        Either::First(Either::Second(frame)) => frame,
                        Either::Second(()) => {
                            if let Some(result) = self.expire_reorder_gap(tid).await {
                                return result;
                            }
                            continue;
                        }
                    }
                } else {
                    match select(commands.receive(), self.frames.receive()).await {
                        Either::First(command) => {
                            if let Some(result) = self.apply_reorder_command(command).await {
                                return result;
                            }
                            continue;
                        }
                        Either::Second(frame) => frame,
                    }
                }
            } else if let Some((tid, deadline)) = next_gap {
                match select(self.frames.receive(), Timer::at(deadline)).await {
                    Either::First(frame) => frame,
                    Either::Second(()) => {
                        if let Some(result) = self.expire_reorder_gap(tid).await {
                            return result;
                        }
                        continue;
                    }
                }
            } else {
                self.frames.receive().await
            };
            if let Some(result) = self.accept_frame(frame).await {
                return result;
            }
        }
    }

    async fn accept_frame(
        &mut self,
        frame: Esp32s31StagedRxFrame<'pool, CAPACITY, SLOTS>,
    ) -> Option<ConnectedRxDispatch> {
        let Some(key) = self.dispatcher.reorder_key(frame.segment()) else {
            return Some(self.dispatch_owned_frame(frame).await);
        };
        let tid = usize::from(key.tid);
        if tid >= self.reorders.len() || self.reorders[tid].is_none() {
            return Some(self.dispatch_owned_frame(frame).await);
        }
        if let Some(start) = self.reorder_first_starts[tid].take()
            && let Some(counters) = self.pipeline_counters
        {
            counters.record_reorder_first(key.tid, start, key.sequence);
        }

        let retain = match self.reorders[tid]
            .as_ref()
            .expect("active TID was checked above")
            .retains_on_ingest(key.sequence)
        {
            Ok(retain) => retain,
            Err(error) => {
                drop(frame);
                self.irq.notify_rx_capacity();
                return Some(if matches!(error, RxAmpduError::DuplicateSequence(_)) {
                    ConnectedRxDispatch::Duplicate
                } else {
                    ConnectedRxDispatch::Ignored
                });
            }
        };
        // A 64-slot hot pool can retain the maximum 31 out-of-order frames,
        // admit the next 32-descriptor hardware burst and still own the
        // current frontier frame. Smaller compositions keep the independent
        // cold backing so one sequence gap cannot exhaust DMA staging.
        let retain_hot = retain && SLOTS == RX_REORDER_BACKING_SLOT_COUNT;
        let reservation = if retain && !retain_hot {
            let Some(storage) = self.reorder_storage else {
                // An agreement must never retain the finite hot staging pool
                // when its independent backing was omitted by the composition.
                drop(frame);
                self.irq.notify_rx_capacity();
                return Some(ConnectedRxDispatch::Ignored);
            };
            match storage.try_reserve() {
                Ok(reservation) => Some(reservation),
                Err(_) => {
                    drop(frame);
                    self.irq.notify_rx_capacity();
                    return Some(ConnectedRxDispatch::Ignored);
                }
            }
        } else {
            None
        };
        let slot = reservation.as_ref().map_or_else(
            || {
                if retain_hot {
                    frame.slot()
                } else {
                    RX_REORDER_CURRENT_SLOT
                }
            },
            |reservation| reservation.slot(),
        );
        let mpdu = RxAmpduMpdu {
            sequence: key.sequence,
            slot: slot as u8,
        };
        let release = match self.reorders[tid]
            .as_mut()
            .expect("active TID was checked above")
            .ingest(mpdu)
        {
            Ok(release) => release,
            Err(error) => {
                drop(reservation);
                drop(frame);
                self.irq.notify_rx_capacity();
                return Some(if matches!(error, RxAmpduError::DuplicateSequence(_)) {
                    ConnectedRxDispatch::Duplicate
                } else {
                    ConnectedRxDispatch::Ignored
                });
            }
        };
        self.update_gap_deadline(tid);
        self.record_reorder_occupied();
        if release.buffered {
            if retain_hot {
                debug_assert!(slot < self.retained.len());
                debug_assert!(self.retained[slot].is_none());
                self.retained[slot] = Some(RetainedRxFrame::Hot(frame));
                return self.dispatch_release(release).await;
            }
            let reservation = reservation.expect("predicted retained frame owns backing");
            let retained = match reservation.copy_from(frame.segment()) {
                Ok(retained) => retained,
                Err((_error, reservation)) => {
                    let mut reorder = self.reorders[tid]
                        .take()
                        .expect("active reorder owns the failed retained copy");
                    let rollback = reorder.stop();
                    self.gap_deadlines[tid] = None;
                    self.reorder_first_starts[tid] = None;
                    drop(reservation);
                    return self
                        .dispatch_release_with_current(rollback, slot, frame)
                        .await
                        .or(Some(ConnectedRxDispatch::Ignored));
                }
            };
            debug_assert_eq!(retained.slot(), slot);
            debug_assert!(self.retained[slot].is_none());
            self.retained[slot] = Some(RetainedRxFrame::Cold(retained));
            drop(frame);
            self.irq.notify_rx_capacity();
            self.dispatch_release(release).await
        } else {
            drop(reservation);
            self.dispatch_release_with_current(release, slot, frame)
                .await
        }
    }

    async fn apply_reorder_command(
        &mut self,
        command: RxReorderCommand,
    ) -> Option<ConnectedRxDispatch> {
        match command {
            RxReorderCommand::Start {
                tid,
                starting_sequence,
                window,
            } => {
                let tid = usize::from(tid);
                if tid >= self.reorders.len() {
                    return None;
                }
                let released = self.stop_reorder(tid).await;
                self.reorders[tid] = RxBlockAckReorderState::<RX_REORDER_SLOT_DOMAIN>::new(
                    starting_sequence,
                    window,
                )
                .ok();
                self.reorder_first_starts[tid] = Some(starting_sequence);
                self.gap_deadlines[tid] = None;
                if let Some(counters) = self.pipeline_counters {
                    counters.record_reorder_start(tid as u8, starting_sequence, window);
                }
                released
            }
            RxReorderCommand::Stop { tid } => {
                let tid = usize::from(tid);
                if tid >= self.reorders.len() {
                    None
                } else {
                    self.stop_reorder(tid).await
                }
            }
            RxReorderCommand::StopAll => {
                let mut result = None;
                for tid in 0..self.reorders.len() {
                    if let Some(released) = self.stop_reorder(tid).await {
                        result = Some(released);
                    }
                }
                result
            }
        }
    }

    async fn stop_reorder(&mut self, tid: usize) -> Option<ConnectedRxDispatch> {
        self.gap_deadlines[tid] = None;
        self.reorder_first_starts[tid] = None;
        let mut reorder = self.reorders[tid].take()?;
        let release = reorder.stop();
        if let Some(counters) = self.pipeline_counters {
            counters.record_reorder_stop();
        }
        self.record_reorder_occupied();
        self.dispatch_release(release).await
    }

    fn next_gap_deadline(&self) -> Option<(usize, Instant)> {
        self.gap_deadlines
            .iter()
            .copied()
            .enumerate()
            .filter_map(|(tid, deadline)| deadline.map(|deadline| (tid, deadline)))
            .min_by_key(|(_, deadline)| *deadline)
    }

    fn update_gap_deadline(&mut self, tid: usize) {
        if self.reorders[tid]
            .as_ref()
            .is_some_and(|reorder| reorder.occupied() != 0)
        {
            self.gap_deadlines[tid].get_or_insert_with(|| {
                Instant::now() + Duration::from_micros(RX_REORDER_GAP_TIMEOUT_MICROS)
            });
        } else {
            self.gap_deadlines[tid] = None;
        }
    }

    async fn expire_reorder_gap(&mut self, tid: usize) -> Option<ConnectedRxDispatch> {
        self.gap_deadlines[tid] = None;
        let release = self.reorders[tid].as_mut()?.expire_gap();
        if let Some(counters) = self.pipeline_counters {
            counters.record_reorder_gap_expiry();
        }
        self.update_gap_deadline(tid);
        self.record_reorder_occupied();
        self.dispatch_release(release).await
    }

    fn record_reorder_occupied(&self) {
        let Some(counters) = self.pipeline_counters else {
            return;
        };
        let occupied = self
            .reorders
            .iter()
            .flatten()
            .map(RxBlockAckReorderState::occupied)
            .sum();
        counters.record_reorder_occupied(occupied);
    }

    async fn dispatch_release(&mut self, release: RxAmpduRelease) -> Option<ConnectedRxDispatch> {
        if let Some(counters) = self.pipeline_counters {
            counters.record_reorder_release(
                release.buffered,
                release.count,
                release.missing,
                release.rejected.is_some(),
            );
        }
        let mut result = None;
        for released in release.iter() {
            let slot = usize::from(released.slot);
            let frame = self.retained[slot]
                .take()
                .expect("reorder release must reference one retained frame lease");
            result = Some(self.dispatch_retained_frame(frame).await);
        }
        if let Some(rejected) = release.rejected {
            self.release_retained_slot(usize::from(rejected.slot));
            result = Some(ConnectedRxDispatch::Duplicate);
        }
        result
    }

    async fn dispatch_release_with_current(
        &mut self,
        release: RxAmpduRelease,
        current_slot: usize,
        current_frame: Esp32s31StagedRxFrame<'pool, CAPACITY, SLOTS>,
    ) -> Option<ConnectedRxDispatch> {
        if let Some(counters) = self.pipeline_counters {
            counters.record_reorder_release(
                release.buffered,
                release.count,
                release.missing,
                release.rejected.is_some(),
            );
        }
        let mut current_frame = Some(current_frame);
        let mut result = None;
        for released in release.iter() {
            let slot = usize::from(released.slot);
            result = Some(if slot == current_slot {
                self.dispatch_owned_frame(
                    current_frame
                        .take()
                        .expect("current reorder release is unique"),
                )
                .await
            } else {
                let frame = self.retained[slot]
                    .take()
                    .expect("reorder release references retained cold backing");
                self.dispatch_retained_frame(frame).await
            });
        }
        if let Some(rejected) = release.rejected {
            let slot = usize::from(rejected.slot);
            if slot == current_slot {
                drop(current_frame.take());
                self.irq.notify_rx_capacity();
            } else {
                self.release_retained_slot(slot);
            }
            result = Some(ConnectedRxDispatch::Duplicate);
        }
        debug_assert!(current_frame.is_none());
        result
    }

    fn release_retained_slot(&mut self, slot: usize) {
        if let Some(frame) = self.retained[slot].take() {
            let hot = matches!(&frame, RetainedRxFrame::Hot(_));
            drop(frame);
            if hot {
                self.irq.notify_rx_capacity();
            }
        }
    }

    async fn dispatch_retained_frame(
        &mut self,
        frame: RetainedRxFrame<'pool, CAPACITY, SLOTS>,
    ) -> ConnectedRxDispatch {
        match frame {
            RetainedRxFrame::Hot(frame) => self.dispatch_owned_frame(frame).await,
            RetainedRxFrame::Cold(frame) => self.dispatch_reordered_frame(frame).await,
        }
    }

    async fn dispatch_reordered_frame(
        &mut self,
        frame: RxReorderFrame<'pool, CAPACITY>,
    ) -> ConnectedRxDispatch {
        let source = frame.segment();
        let ordinary = !self.dispatcher.may_publish_amsdu(source);
        let result = if ordinary {
            if let Some(scratch) = self.reorder_scratch.as_deref_mut() {
                let length = source.buffer.len();
                scratch[..length].copy_from_slice(source.buffer);
                let segment = open_esp_radio_esp32s31_wifi_mac::rx::RxSegment {
                    descriptor_address: source.descriptor_address,
                    descriptor_word0: source.descriptor_word0,
                    buffer: &scratch[..length],
                    next_descriptor_address: source.next_descriptor_address,
                };
                dispatch_non_amsdu_segment(
                    &mut self.dispatcher,
                    &mut self.sink,
                    self.mpdu,
                    segment,
                    self.pipeline_counters,
                )
                .await
            } else {
                self.dispatch_segment(source).await
            }
        } else {
            self.dispatch_segment(source).await
        };
        drop(frame);
        result
    }

    async fn dispatch_owned_frame(
        &mut self,
        frame: Esp32s31StagedRxFrame<'pool, CAPACITY, SLOTS>,
    ) -> ConnectedRxDispatch {
        let result = self.dispatch_segment(frame.segment()).await;
        drop(frame);
        self.irq.notify_rx_capacity();
        result
    }

    async fn dispatch_segment(
        &mut self,
        segment: open_esp_radio_esp32s31_wifi_mac::rx::RxSegment<'_>,
    ) -> ConnectedRxDispatch {
        if self.dispatcher.may_publish_amsdu(segment) {
            return self.dispatch_amsdu(segment).await;
        }
        dispatch_non_amsdu_segment(
            &mut self.dispatcher,
            &mut self.sink,
            self.mpdu,
            segment,
            self.pipeline_counters,
        )
        .await
    }

    async fn dispatch_amsdu(
        &mut self,
        segment: open_esp_radio_esp32s31_wifi_mac::rx::RxSegment<'_>,
    ) -> ConnectedRxDispatch {
        let dispatch_started = self.pipeline_counters.map(RxPipelineCounters::now_micros);
        let mut deferred = DeferredEthernetFrames::new(self.ethernet);
        let result = self
            .dispatcher
            .dispatch(segment, self.mpdu, &mut [], &mut deferred);
        let used = deferred.used;
        drop(deferred);
        if let (Some(counters), Some(started)) = (self.pipeline_counters, dispatch_started) {
            let (data, amsdu, amsdu_subframes) = match result {
                ConnectedRxDispatch::Data {
                    ethernet_frames,
                    amsdu,
                } => (true, amsdu, ethernet_frames),
                _ => (false, false, 0),
            };
            counters.record_dispatch(
                data,
                amsdu,
                amsdu_subframes,
                segment.buffer.len(),
                counters.elapsed_micros_since(started),
            );
        }
        let raw = segment.buffer;
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
        result
    }

    /// Run protocol processing independently from the PAC/DMA owner.
    pub async fn run(&mut self) -> ! {
        loop {
            self.dispatch_next().await;
        }
    }

    /// Run until an outer connected-epoch owner requests teardown.
    ///
    /// The stop future is polled first, so a simultaneous frame/stop edge does
    /// not publish new network input after disconnect. Cancelling the current
    /// `dispatch_next` future drops its local staging lease; the explicit
    /// shutdown then drains queued and retained ownership before returning.
    pub async fn run_until<F: Future<Output = ()>>(
        &mut self,
        stop: F,
    ) -> ConnectedRxProtocolShutdown {
        let mut stop = core::pin::pin!(stop);
        loop {
            match select(stop.as_mut(), self.dispatch_next()).await {
                Either::First(()) => return self.shutdown_discard(),
                Either::Second(_) => {}
            }
        }
    }
}

async fn dispatch_non_amsdu_segment<S: ConnectedRxProtocolSink>(
    dispatcher: &mut ConnectedRxDispatcher,
    sink: &mut S,
    mpdu: &mut [u8],
    segment: open_esp_radio_esp32s31_wifi_mac::rx::RxSegment<'_>,
    pipeline_counters: Option<&RxPipelineCounters>,
) -> ConnectedRxDispatch {
    if dispatcher.may_publish_ethernet(segment) {
        // Keep the staging lease until the single-MSDU path owns its one
        // network output slot. A-MSDU uses the deferred streaming path
        // above and acquires one slot per decoded subframe.
        let wait_started = pipeline_counters.map(RxPipelineCounters::now_micros);
        sink.wait_ready().await;
        if let (Some(counters), Some(started)) = (pipeline_counters, wait_started) {
            counters.record_network_ready_wait(counters.elapsed_micros_since(started));
        }
    }
    let dispatch_started = pipeline_counters.map(RxPipelineCounters::now_micros);
    let result = dispatcher.dispatch(segment, mpdu, &mut [], sink);
    if let (Some(counters), Some(started)) = (pipeline_counters, dispatch_started) {
        let (data, amsdu, amsdu_subframes) = match result {
            ConnectedRxDispatch::Data {
                ethernet_frames,
                amsdu,
            } => (true, amsdu, ethernet_frames),
            _ => (false, false, 0),
        };
        counters.record_dispatch(
            data,
            amsdu,
            amsdu_subframes,
            segment.buffer.len(),
            counters.elapsed_micros_since(started),
        );
    }
    result
}

#[cfg(test)]
mod tests {
    use open_esp_radio_embassy_net::NoopRawMutex;
    use open_esp_radio_esp32s31_wifi_mac::{connected_rx::ConnectedRxConfig, rx::RxIngressConfig};

    use super::*;

    struct Sink;

    impl ConnectedRxSink for Sink {
        fn publish(&mut self, _event: ConnectedRxEvent<'_>) {}
    }

    impl ConnectedRxProtocolSink for Sink {
        fn wait_ready(&mut self) -> impl Future<Output = ()> + '_ {
            ready(())
        }
    }

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

    #[test]
    fn stop_edge_returns_an_empty_reusable_protocol_epoch() {
        let queue = Esp32s31StagedRxQueue::<NoopRawMutex, 1, 64, 1>::new();
        let (_sender, receiver) = queue.split();
        let irq = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
        let dispatcher = ConnectedRxDispatcher::new(ConnectedRxConfig {
            station_address: [2, 3, 4, 5, 6, 7],
            bssid: [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
            association_id: 1,
            ingress: RxIngressConfig {
                ring_entry_limit: 1,
                csi_config: 0,
                flags: 0,
            },
        });
        let mut mpdu = [0_u8; 64];
        let mut ethernet = [0_u8; 64];
        let mut protocol = Esp32s31ConnectedRxProtocol::new(
            receiver,
            &irq,
            dispatcher,
            Sink,
            &mut mpdu,
            &mut ethernet,
        );

        assert_eq!(
            embassy_futures::block_on(protocol.run_until(ready(()))),
            ConnectedRxProtocolShutdown::default()
        );
        assert_eq!(protocol.queue_len(), 0);
    }
}
