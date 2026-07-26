//! Fixed-capacity, allocation-free TX descriptor transition trace.
//!
//! The trace is a diagnostic flight recorder. Writers never wait, allocate,
//! format text, invoke callbacks, or enter a critical section. Each slot uses
//! atomic fields and a release-published sequence number, so an executor-side
//! reader can reject a torn or concurrently overwritten entry in one bounded
//! pass. The ring deliberately overwrites its oldest entry until it is frozen.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

pub const TX_TRACE_CAPACITY: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum TxTraceEvent {
    Scenario = 1,
    SecurityInput = 2,
    SecurityPrepared = 3,
    SecurityRejected = 4,
    Submit = 5,
    CompletionInterrupt = 6,
    RetryDecision = 7,
    RetrySubmit = 8,
    TxDoneBegin = 9,
    RateControlDone = 10,
    TxDoneCommit = 11,
    Recycle = 12,
    PipelineRejected = 13,
}

impl TxTraceEvent {
    const fn from_raw(raw: u8) -> Option<Self> {
        match raw {
            1 => Some(Self::Scenario),
            2 => Some(Self::SecurityInput),
            3 => Some(Self::SecurityPrepared),
            4 => Some(Self::SecurityRejected),
            5 => Some(Self::Submit),
            6 => Some(Self::CompletionInterrupt),
            7 => Some(Self::RetryDecision),
            8 => Some(Self::RetrySubmit),
            9 => Some(Self::TxDoneBegin),
            10 => Some(Self::RateControlDone),
            11 => Some(Self::TxDoneCommit),
            12 => Some(Self::Recycle),
            13 => Some(Self::PipelineRejected),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxTraceEntry {
    pub sequence: u32,
    pub event: TxTraceEvent,
    pub queue: u8,
    pub rate: u8,
    pub response: u8,
    /// Low 32 bits of the frame address. This is an identity token only.
    pub frame: u32,
    pub frame_control: u16,
    pub descriptor_flags: u32,
    pub descriptor_control: u32,
    /// Event-specific observations documented at each recording site.
    pub status0: u32,
    pub status1: u32,
    pub status2: u32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TxTraceSnapshot {
    pub next_sequence: u32,
    pub oldest_sequence: u32,
    pub overwritten: u32,
    pub frozen: bool,
}

#[derive(Clone, Copy)]
pub(crate) struct TxTraceRecord {
    pub event: TxTraceEvent,
    pub queue: u8,
    pub rate: u8,
    pub response: u8,
    pub frame: u32,
    pub frame_control: u16,
    pub descriptor_flags: u32,
    pub descriptor_control: u32,
    pub status0: u32,
    pub status1: u32,
    pub status2: u32,
}

impl TxTraceRecord {
    pub(crate) const fn new(event: TxTraceEvent) -> Self {
        Self {
            event,
            queue: u8::MAX,
            rate: u8::MAX,
            response: 0,
            frame: 0,
            frame_control: 0,
            descriptor_flags: 0,
            descriptor_control: 0,
            status0: 0,
            status1: 0,
            status2: 0,
        }
    }
}

struct TxTraceSlot {
    // Zero means that the slot is being written or has never been committed.
    committed_sequence: AtomicU32,
    event_queue_rate_response: AtomicU32,
    frame: AtomicU32,
    frame_control: AtomicU32,
    descriptor_flags: AtomicU32,
    descriptor_control: AtomicU32,
    status0: AtomicU32,
    status1: AtomicU32,
    status2: AtomicU32,
}

impl TxTraceSlot {
    const fn new() -> Self {
        Self {
            committed_sequence: AtomicU32::new(0),
            event_queue_rate_response: AtomicU32::new(0),
            frame: AtomicU32::new(0),
            frame_control: AtomicU32::new(0),
            descriptor_flags: AtomicU32::new(0),
            descriptor_control: AtomicU32::new(0),
            status0: AtomicU32::new(0),
            status1: AtomicU32::new(0),
            status2: AtomicU32::new(0),
        }
    }
}

struct TxTrace {
    reserved: AtomicU32,
    frozen: AtomicBool,
    slots: [TxTraceSlot; TX_TRACE_CAPACITY],
}

impl TxTrace {
    const fn new() -> Self {
        Self {
            reserved: AtomicU32::new(0),
            frozen: AtomicBool::new(false),
            slots: [const { TxTraceSlot::new() }; TX_TRACE_CAPACITY],
        }
    }
}

#[cfg_attr(
    target_arch = "riscv32",
    link_section = ".critical.bss.wifi_strict.tx_trace"
)]
static TRACE: TxTrace = TxTrace::new();

/// Record one transition in a bounded number of atomic stores.
///
/// `fetch_add` reserves a unique slot generation for interrupt and executor
/// producers. There is no compare/retry loop. A reader either observes the
/// complete generation or returns `None`.
#[cfg_attr(
    target_arch = "riscv32",
    link_section = ".rwtext.wifi_strict.tx_trace_record"
)]
#[inline(never)]
pub(crate) fn record_tx_transition(record: TxTraceRecord) -> Option<u32> {
    if TRACE.frozen.load(Ordering::Relaxed) {
        return None;
    }

    let sequence = TRACE
        .reserved
        .fetch_add(1, Ordering::Relaxed)
        .wrapping_add(1);
    // A wrapped zero cannot serve as the in-progress marker. In practice this
    // requires more than four billion HIL records; drop that single generation.
    if sequence == 0 {
        return None;
    }
    let slot = &TRACE.slots[(sequence as usize - 1) % TX_TRACE_CAPACITY];
    slot.committed_sequence.store(0, Ordering::Relaxed);
    slot.event_queue_rate_response.store(
        u32::from(record.event as u8)
            | (u32::from(record.queue) << 8)
            | (u32::from(record.rate) << 16)
            | (u32::from(record.response) << 24),
        Ordering::Relaxed,
    );
    slot.frame.store(record.frame, Ordering::Relaxed);
    slot.frame_control
        .store(u32::from(record.frame_control), Ordering::Relaxed);
    slot.descriptor_flags
        .store(record.descriptor_flags, Ordering::Relaxed);
    slot.descriptor_control
        .store(record.descriptor_control, Ordering::Relaxed);
    slot.status0.store(record.status0, Ordering::Relaxed);
    slot.status1.store(record.status1, Ordering::Relaxed);
    slot.status2.store(record.status2, Ordering::Relaxed);
    slot.committed_sequence.store(sequence, Ordering::Release);
    Some(sequence)
}

/// Capture the common descriptor identity fields at a validated TX boundary.
///
/// # Safety
///
/// `descriptor` must point to a live descriptor containing the recovered S31
/// fields at offsets 0x00, 0x0c and 0x10.
#[cfg(target_arch = "riscv32")]
#[link_section = ".rwtext.wifi_strict.tx_trace_descriptor"]
#[inline(never)]
pub(crate) unsafe fn record_descriptor_transition(
    event: TxTraceEvent,
    frame: *mut u8,
    descriptor: *mut u8,
    frame_control: u16,
    queue: u8,
    response: u8,
    status0: u32,
    status1: u32,
    status2: u32,
) -> Option<u32> {
    let mut record = TxTraceRecord::new(event);
    record.queue = queue;
    record.rate = descriptor.add(0x0c).read();
    record.response = response;
    record.frame = frame.addr() as u32;
    record.frame_control = frame_control;
    record.descriptor_flags = descriptor.cast::<u32>().read_unaligned();
    record.descriptor_control = descriptor.add(0x10).cast::<u32>().read_unaligned();
    record.status0 = status0;
    record.status1 = status1;
    record.status2 = status2;
    record_tx_transition(record)
}

pub fn tx_trace_snapshot() -> TxTraceSnapshot {
    let reserved = TRACE.reserved.load(Ordering::Acquire);
    let next_sequence = reserved.wrapping_add(1);
    let retained = (TX_TRACE_CAPACITY as u32).min(reserved);
    TxTraceSnapshot {
        next_sequence,
        oldest_sequence: next_sequence.wrapping_sub(retained),
        overwritten: reserved.saturating_sub(TX_TRACE_CAPACITY as u32),
        frozen: TRACE.frozen.load(Ordering::Acquire),
    }
}

/// Read one exact generation without waiting or retrying.
pub fn tx_trace_entry(sequence: u32) -> Option<TxTraceEntry> {
    if sequence == 0 {
        return None;
    }
    let slot = &TRACE.slots[(sequence as usize - 1) % TX_TRACE_CAPACITY];
    if slot.committed_sequence.load(Ordering::Acquire) != sequence {
        return None;
    }

    let packed = slot.event_queue_rate_response.load(Ordering::Relaxed);
    let entry = TxTraceEntry {
        sequence,
        event: TxTraceEvent::from_raw(packed as u8)?,
        queue: (packed >> 8) as u8,
        rate: (packed >> 16) as u8,
        response: (packed >> 24) as u8,
        frame: slot.frame.load(Ordering::Relaxed),
        frame_control: slot.frame_control.load(Ordering::Relaxed) as u16,
        descriptor_flags: slot.descriptor_flags.load(Ordering::Relaxed),
        descriptor_control: slot.descriptor_control.load(Ordering::Relaxed),
        status0: slot.status0.load(Ordering::Relaxed),
        status1: slot.status1.load(Ordering::Relaxed),
        status2: slot.status2.load(Ordering::Relaxed),
    };
    if slot.committed_sequence.load(Ordering::Acquire) == sequence {
        Some(entry)
    } else {
        None
    }
}

/// Stop writers after the entries already in flight have committed.
pub fn freeze_tx_trace() {
    TRACE.frozen.store(true, Ordering::Release);
}

/// Add an application-defined scenario edge to the same sequence domain.
pub fn mark_tx_trace_scenario(tag: u32, detail: u32) -> Option<u32> {
    let mut record = TxTraceRecord::new(TxTraceEvent::Scenario);
    record.status0 = tag;
    record.status1 = detail;
    record_tx_transition(record)
}

#[cfg(test)]
mod tests {
    use super::{
        record_tx_transition, tx_trace_entry, tx_trace_snapshot, TxTraceEvent, TxTraceRecord,
        TX_TRACE_CAPACITY,
    };

    #[test]
    fn bounded_ring_retains_correlated_generations() {
        let start = tx_trace_snapshot().next_sequence;
        for value in 0..TX_TRACE_CAPACITY as u32 + 3 {
            let mut record = TxTraceRecord::new(TxTraceEvent::CompletionInterrupt);
            record.queue = 2;
            record.rate = 19;
            record.response = 0x7f;
            record.frame = 0x3fca_0000 + value;
            record.frame_control = 0x4288;
            record.descriptor_flags = 0x0200_2009;
            record.descriptor_control = 0x02a4_0348;
            record.status0 = value;
            record_tx_transition(record).unwrap();
        }

        let snapshot = tx_trace_snapshot();
        assert_eq!(
            snapshot.next_sequence.wrapping_sub(start),
            TX_TRACE_CAPACITY as u32 + 3
        );
        assert!(tx_trace_entry(start).is_none());
        let last = tx_trace_entry(snapshot.next_sequence - 1).unwrap();
        assert_eq!(last.event, TxTraceEvent::CompletionInterrupt);
        assert_eq!(last.queue, 2);
        assert_eq!(last.rate, 19);
        assert_eq!(last.frame_control, 0x4288);
        assert_eq!(last.descriptor_control, 0x02a4_0348);
        assert_eq!(last.status0, TX_TRACE_CAPACITY as u32 + 2);
    }
}
