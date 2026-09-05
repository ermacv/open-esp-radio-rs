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
