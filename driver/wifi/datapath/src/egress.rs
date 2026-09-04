//! Synchronous radio-selected egress ownership.
//!
//! This module does not prescribe a network stack or an executor. A producer
//! retains canonical work, reports durable per-key demand and materializes
//! only the key selected by the radio into an already reserved physical batch.

use core::num::{NonZeroU16, NonZeroU32};

use open_esp_radio_network::NetworkInterfaceId;

/// Valid 802.11 QoS traffic identifier.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct TrafficIdentifier(u8);

impl TrafficIdentifier {
    pub const fn new(value: u8) -> Result<Self, TrafficIdentifierError> {
        if value <= 15 {
            Ok(Self(value))
        } else {
            Err(TrafficIdentifierError)
        }
    }

    pub const fn value(self) -> u8 {
        self.0
    }
}

/// A traffic identifier outside the four-bit 802.11 QoS space.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TrafficIdentifierError;

/// Destination identity owned by the radio lifecycle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RadioPeer {
    Unicast { slot: u8, generation: u32 },
    Group { generation: u32 },
}

/// Complete identity of one independently schedulable radio data flow.
///
/// A network destination is deliberately absent. The radio classifier maps a
/// resolved link route onto the current interface/peer generation and TID.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RadioEgressKey {
    interface: NetworkInterfaceId,
    link_epoch: u32,
    peer: RadioPeer,
    traffic_identifier: TrafficIdentifier,
}

/// Resource class applied independently of peer/TID fairness identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdmissionClass {
    Bulk,
    LatencySensitive,
    LinkControl,
    Emergency,
}

/// Software scheduling key: radio aggregation identity plus bounded resource
/// policy. Distinct admission classes never become one accidental FIFO.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EgressFlowKey {
    pub radio: RadioEgressKey,
    pub admission: AdmissionClass,
}

impl RadioEgressKey {
    pub const fn new(
        interface: NetworkInterfaceId,
        link_epoch: u32,
        peer: RadioPeer,
        traffic_identifier: TrafficIdentifier,
    ) -> Self {
        Self {
            interface,
            link_epoch,
            peer,
            traffic_identifier,
        }
    }

    pub const fn interface(self) -> NetworkInterfaceId {
        self.interface
    }

    pub const fn link_epoch(self) -> u32 {
        self.link_epoch
    }

    pub const fn peer(self) -> RadioPeer {
        self.peer
    }

    pub const fn traffic_identifier(self) -> TrafficIdentifier {
        self.traffic_identifier
    }
}

/// Durable queue-level state observed by radio scheduling policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EgressDemand {
    pub key: EgressFlowKey,
    pub ready_frames: u16,
    pub ready_bytes: u32,
    pub oldest_enqueue_micros: u64,
}

/// One bounded decision retained privately by the radio scheduler.
///
/// Airtime reservations are intentionally absent. The radio owns them and
/// reconciles actual completion against this selection outside the network
/// provider.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EgressSelection {
    pub key: EgressFlowKey,
    pub max_frames: NonZeroU16,
    pub max_bytes: NonZeroU32,
}

/// Canonical packet/transport work which can construct its final Ethernet
/// representation only after radio selection.
pub trait DeferredTxWork {
    type WriteError;

    fn egress_key(&self) -> EgressFlowKey;

    fn enqueue_micros(&self) -> u64;

    fn frame_length(&self) -> u16;

    fn write_frame(&self, destination: &mut [u8]) -> Result<(), Self::WriteError>;
}

/// Why an already reserved destination could not accept one frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BatchWriteError<WriteError> {
    Exhausted,
    Write(WriteError),
}

/// Physical slots reserved before a selected source prefix is consumed.
///
/// `try_write` must make writer failure transactional for the current slot:
/// it releases or retains that destination without publishing it, while every
/// earlier successful call remains committed to the batch.
pub trait ReservedTxBatch {
    fn remaining(&self) -> usize;

    fn try_write<WriteError>(
        &mut self,
        length: usize,
        write: impl FnOnce(&mut [u8]) -> Result<(), WriteError>,
    ) -> Result<(), BatchWriteError<WriteError>>;
}

/// Natural reason one synchronous fill turn stopped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FillStopReason {
    SelectionSatisfied,
    SourceDrained,
    DestinationExhausted,
    ByteBudget,
}

/// Work committed by one synchronous fill turn.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FillOutcome {
    pub frames: u16,
    pub bytes: u32,
    pub source_remaining: u16,
    pub stop: FillStopReason,
}

/// A writer error retains the current source owner and reports the prefix
/// which was already committed before that error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FillFailure<WriteError> {
    pub error: WriteError,
    pub committed_frames: u16,
    pub committed_bytes: u32,
    pub source_remaining: u16,
}

/// Radio-facing contract implemented by a network/transport owner.
pub trait EgressWorkProvider {
    type WriteError;

    fn visit_demands(&self, visitor: impl FnMut(EgressDemand));

    fn fill_selected<Batch: ReservedTxBatch>(
        &mut self,
        selection: EgressSelection,
        batch: &mut Batch,
    ) -> Result<FillOutcome, FillFailure<Self::WriteError>>;
}

/// Bounded admission failure which returns the exact work owner.
#[derive(Debug)]
pub enum EnqueueError<Work> {
    WorkCapacity(Work),
    FlowCapacity(Work),
}

struct WorkSlot<Work> {
    work: Option<Work>,
    next: Option<u16>,
}

impl<Work> WorkSlot<Work> {
    const fn empty() -> Self {
        Self {
            work: None,
            next: None,
        }
    }
}

#[derive(Clone, Copy)]
struct FlowState {
    key: EgressFlowKey,
    head: u16,
    tail: u16,
    ready_frames: u16,
    ready_bytes: u32,
    oldest_enqueue_micros: u64,
}

/// Fixed-memory per-radio-key queue for deferred transmission work.
///
/// Queue storage may be placed in PSRAM by a concrete integration. Flow
/// metadata is separate from the physical SRAM pool and neither capacity is
/// derived from the number of associated peers or a BA window.
pub struct FixedEgressQueue<Work, const FLOW_CAPACITY: usize, const WORK_CAPACITY: usize> {
    flows: [Option<FlowState>; FLOW_CAPACITY],
    slots: [WorkSlot<Work>; WORK_CAPACITY],
    free_head: Option<u16>,
    queued: usize,
}

impl<Work, const FLOW_CAPACITY: usize, const WORK_CAPACITY: usize>
    FixedEgressQueue<Work, FLOW_CAPACITY, WORK_CAPACITY>
{
    pub const fn new() -> Self {
        assert!(FLOW_CAPACITY > 0, "egress queue needs at least one flow");
        assert!(
            WORK_CAPACITY > 0,
            "egress queue needs at least one work slot"
        );
        assert!(
            WORK_CAPACITY <= u16::MAX as usize,
            "egress demand frame counts must fit in u16"
        );

        let mut slots = [const { WorkSlot::empty() }; WORK_CAPACITY];
        let mut index = 0;
        while index + 1 < WORK_CAPACITY {
            slots[index].next = Some((index + 1) as u16);
            index += 1;
        }
        Self {
            flows: [const { None }; FLOW_CAPACITY],
            slots,
            free_head: Some(0),
            queued: 0,
        }
    }

    pub const fn len(&self) -> usize {
        self.queued
    }

    pub const fn is_empty(&self) -> bool {
        self.queued == 0
    }
}

impl<Work: DeferredTxWork, const FLOW_CAPACITY: usize, const WORK_CAPACITY: usize>
    FixedEgressQueue<Work, FLOW_CAPACITY, WORK_CAPACITY>
{
    pub fn try_enqueue(&mut self, work: Work) -> Result<(), EnqueueError<Work>> {
        let Some(free_index) = self.free_head else {
            return Err(EnqueueError::WorkCapacity(work));
        };
        let key = work.egress_key();
        let flow_index = if let Some(index) = self.flow_index(key) {
            index
        } else if let Some(index) = self.flows.iter().position(Option::is_none) {
            index
        } else {
            return Err(EnqueueError::FlowCapacity(work));
        };

        self.free_head = self.slots[usize::from(free_index)].next.take();
        let frame_length = u32::from(work.frame_length());
        let enqueue_micros = work.enqueue_micros();
        self.slots[usize::from(free_index)].work = Some(work);

        match &mut self.flows[flow_index] {
            Some(flow) => {
                debug_assert_eq!(flow.key, key);
                self.slots[usize::from(flow.tail)].next = Some(free_index);
                flow.tail = free_index;
                flow.ready_frames = flow
                    .ready_frames
                    .checked_add(1)
                    .expect("work capacity bounds per-flow frame count");
                flow.ready_bytes = flow
                    .ready_bytes
                    .checked_add(frame_length)
                    .expect("bounded Ethernet work cannot overflow u32");
            }
            slot @ None => {
                *slot = Some(FlowState {
                    key,
                    head: free_index,
                    tail: free_index,
                    ready_frames: 1,
                    ready_bytes: frame_length,
                    oldest_enqueue_micros: enqueue_micros,
                });
            }
        }
        self.queued += 1;
        Ok(())
    }

    fn flow_index(&self, key: EgressFlowKey) -> Option<usize> {
        self.flows
            .iter()
            .position(|flow| flow.is_some_and(|flow| flow.key == key))
    }

    fn pop_head(&mut self, flow_index: usize) -> Work {
        let flow = self.flows[flow_index].expect("selected flow remains live");
        let head = flow.head;
        let slot = &mut self.slots[usize::from(head)];
        let work = slot.work.take().expect("flow head owns queued work");
        let next = slot.next.take();
        slot.next = self.free_head;
        self.free_head = Some(head);
        self.queued -= 1;

        if let Some(next) = next {
            let next_work = self.slots[usize::from(next)]
                .work
                .as_ref()
                .expect("next flow slot owns work");
            let state = self.flows[flow_index]
                .as_mut()
                .expect("nonempty flow remains installed");
            state.head = next;
            state.ready_frames -= 1;
            state.ready_bytes -= u32::from(work.frame_length());
            state.oldest_enqueue_micros = next_work.enqueue_micros();
        } else {
            self.flows[flow_index] = None;
        }
        work
    }
}

impl<Work: DeferredTxWork, const FLOW_CAPACITY: usize, const WORK_CAPACITY: usize>
    EgressWorkProvider for FixedEgressQueue<Work, FLOW_CAPACITY, WORK_CAPACITY>
{
    type WriteError = Work::WriteError;

    fn visit_demands(&self, mut visitor: impl FnMut(EgressDemand)) {
        for flow in self.flows.iter().flatten() {
            visitor(EgressDemand {
                key: flow.key,
                ready_frames: flow.ready_frames,
                ready_bytes: flow.ready_bytes,
                oldest_enqueue_micros: flow.oldest_enqueue_micros,
            });
        }
    }

    fn fill_selected<Batch: ReservedTxBatch>(
        &mut self,
        selection: EgressSelection,
        batch: &mut Batch,
    ) -> Result<FillOutcome, FillFailure<Self::WriteError>> {
        let Some(flow_index) = self.flow_index(selection.key) else {
            return Ok(FillOutcome {
                frames: 0,
                bytes: 0,
                source_remaining: 0,
                stop: FillStopReason::SourceDrained,
            });
        };
        let mut frames = 0u16;
        let mut bytes = 0u32;
        let stop = loop {
            let Some(flow) = self.flows[flow_index] else {
                break FillStopReason::SourceDrained;
            };
            if frames == selection.max_frames.get() {
                break FillStopReason::SelectionSatisfied;
            }
            let work = self.slots[usize::from(flow.head)]
                .work
                .as_ref()
                .expect("flow head owns queued work");
            let frame_length = u32::from(work.frame_length());
            if bytes.saturating_add(frame_length) > selection.max_bytes.get() {
                break FillStopReason::ByteBudget;
            }
            match batch.try_write(usize::from(work.frame_length()), |destination| {
                work.write_frame(destination)
            }) {
                Ok(()) => {}
                Err(BatchWriteError::Exhausted) => break FillStopReason::DestinationExhausted,
                Err(BatchWriteError::Write(error)) => {
                    return Err(FillFailure {
                        error,
                        committed_frames: frames,
                        committed_bytes: bytes,
                        source_remaining: flow.ready_frames,
                    });
                }
            }
            let committed = self.pop_head(flow_index);
            frames += 1;
            bytes += frame_length;
            drop(committed);
        };
        let source_remaining = self.flows[flow_index].map_or(0, |flow| flow.ready_frames);
        Ok(FillOutcome {
            frames,
            bytes,
            source_remaining,
            stop,
        })
    }
}

impl<Work, const FLOW_CAPACITY: usize, const WORK_CAPACITY: usize> Default
    for FixedEgressQueue<Work, FLOW_CAPACITY, WORK_CAPACITY>
{
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use core::num::{NonZeroU16, NonZeroU32};
    use std::vec::Vec;

    use super::*;

    #[derive(Debug, Eq, PartialEq)]
    struct TestWork {
        key: EgressFlowKey,
        sequence: u8,
        enqueued: u64,
        fail: bool,
    }

    impl DeferredTxWork for TestWork {
        type WriteError = u8;

        fn egress_key(&self) -> EgressFlowKey {
            self.key
        }

        fn enqueue_micros(&self) -> u64 {
            self.enqueued
        }

        fn frame_length(&self) -> u16 {
            4
        }

        fn write_frame(&self, destination: &mut [u8]) -> Result<(), Self::WriteError> {
            if self.fail {
                return Err(self.sequence);
            }
            destination.copy_from_slice(&[self.sequence; 4]);
            Ok(())
        }
    }

    struct TestBatch {
        capacity: usize,
        frames: Vec<Vec<u8>>,
    }

    impl ReservedTxBatch for TestBatch {
        fn remaining(&self) -> usize {
            self.capacity - self.frames.len()
        }

        fn try_write<WriteError>(
            &mut self,
            length: usize,
            write: impl FnOnce(&mut [u8]) -> Result<(), WriteError>,
        ) -> Result<(), BatchWriteError<WriteError>> {
            if self.remaining() == 0 {
                return Err(BatchWriteError::Exhausted);
            }
            let mut frame = std::vec![0; length];
            write(&mut frame).map_err(BatchWriteError::Write)?;
            self.frames.push(frame);
            Ok(())
        }
    }

    fn key(peer: u8) -> EgressFlowKey {
        EgressFlowKey {
            radio: RadioEgressKey::new(
                NetworkInterfaceId::new(1),
                7,
                RadioPeer::Unicast {
                    slot: peer,
                    generation: 3,
                },
                TrafficIdentifier::new(0).unwrap(),
            ),
            admission: AdmissionClass::Bulk,
        }
    }

    fn selection(key: EgressFlowKey, frames: u16, bytes: u32) -> EgressSelection {
        EgressSelection {
            key,
            max_frames: NonZeroU16::new(frames).unwrap(),
            max_bytes: NonZeroU32::new(bytes).unwrap(),
        }
    }

    #[test]
    fn interleaved_work_is_materialized_as_one_selected_contiguous_burst() {
        let mut queue = FixedEgressQueue::<TestWork, 2, 6>::new();
        for (peer, sequence) in [(1, 1), (2, 9), (1, 2), (2, 10), (1, 3)] {
            queue
                .try_enqueue(TestWork {
                    key: key(peer),
                    sequence,
                    enqueued: u64::from(sequence),
                    fail: false,
                })
                .unwrap();
        }

        let mut batch = TestBatch {
            capacity: 3,
            frames: Vec::new(),
        };
        let outcome = queue
            .fill_selected(selection(key(1), 3, 12), &mut batch)
            .unwrap();

        assert_eq!(outcome.frames, 3);
        assert_eq!(outcome.stop, FillStopReason::SourceDrained);
        assert_eq!(batch.frames, [[1; 4], [2; 4], [3; 4]]);
        let mut demands = Vec::new();
        queue.visit_demands(|demand| demands.push(demand));
        assert_eq!(demands.len(), 1);
        assert_eq!(demands[0].key, key(2));
        assert_eq!(demands[0].ready_frames, 2);
    }

    #[test]
    fn writer_failure_keeps_the_current_owner_and_committed_prefix_exact() {
        let mut queue = FixedEgressQueue::<TestWork, 1, 3>::new();
        for (sequence, fail) in [(1, false), (2, true), (3, false)] {
            queue
                .try_enqueue(TestWork {
                    key: key(1),
                    sequence,
                    enqueued: u64::from(sequence),
                    fail,
                })
                .unwrap();
        }
        let mut batch = TestBatch {
            capacity: 3,
            frames: Vec::new(),
        };

        let failure = queue
            .fill_selected(selection(key(1), 3, 12), &mut batch)
            .unwrap_err();
        assert_eq!(failure.error, 2);
        assert_eq!(failure.committed_frames, 1);
        assert_eq!(failure.source_remaining, 2);
        assert_eq!(batch.frames, [[1; 4]]);
        assert_eq!(queue.len(), 2);
    }

    #[test]
    fn exhausted_destination_never_consumes_the_unwritten_source() {
        let mut queue = FixedEgressQueue::<TestWork, 1, 2>::new();
        for sequence in [1, 2] {
            queue
                .try_enqueue(TestWork {
                    key: key(1),
                    sequence,
                    enqueued: u64::from(sequence),
                    fail: false,
                })
                .unwrap();
        }
        let mut batch = TestBatch {
            capacity: 1,
            frames: Vec::new(),
        };

        let outcome = queue
            .fill_selected(selection(key(1), 2, 8), &mut batch)
            .unwrap();
        assert_eq!(outcome.stop, FillStopReason::DestinationExhausted);
        assert_eq!(outcome.frames, 1);
        assert_eq!(outcome.source_remaining, 1);
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn capacity_failure_returns_the_exact_owner() {
        let mut queue = FixedEgressQueue::<TestWork, 1, 1>::new();
        queue
            .try_enqueue(TestWork {
                key: key(1),
                sequence: 1,
                enqueued: 1,
                fail: false,
            })
            .unwrap();
        let rejected = match queue.try_enqueue(TestWork {
            key: key(1),
            sequence: 2,
            enqueued: 2,
            fail: false,
        }) {
            Err(EnqueueError::WorkCapacity(work)) => work,
            _ => panic!("work capacity must reject the exact second owner"),
        };
        assert_eq!(rejected.sequence, 2);
    }
}
