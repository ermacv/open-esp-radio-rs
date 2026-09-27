//! The post-mortem record a boot leaves in reset-retained memory.
//!
//! The runtime places one [`Record`] in memory that survives software,
//! watchdog, JTAG and brownout resets but not a power-on. During a boot it
//! appends named checkpoints to a ring and, when the boot ends in a hang or
//! a panic, the fault. The next boot calls [`Record::begin_boot`], which
//! returns what the previous boot left and starts a fresh ring.
//!
//! Every field is an integer or a byte array, so any bit pattern is a value
//! and the record can be read before anything initialized it. Integrity is
//! checked piecewise, so a checkpoint costs one small CRC rather than one over
//! the whole record: the header, each checkpoint and the fault carry their
//! own CRC, and a part whose CRC fails is dropped. After a power-on the header
//! fails and nothing is reported.
//!
//! [`Record::checkpoint`] is for named phase transitions, not hot loops: each
//! call takes the runtime's critical section and computes a CRC.
//! [`RateMonitor`] reports a caller that checkpoints too often.

use crc::{CRC_32_ISCSI, Crc};
use oer_hil_protocol::{
    CHECKPOINT_NAME_BYTES, Checkpoint, Fault, HangFault, HartState, POST_MORTEM_CHECKPOINT_PAGE,
    POST_MORTEM_CHECKPOINTS, PanicFault, PostMortemCheckpoints, PostMortemSummary,
};

const CRC: Crc<u32> = Crc::<u32>::new(&CRC_32_ISCSI);
const MAGIC: u32 = u32::from_le_bytes(*b"OPMR");
/// Layout of [`Record`]; a record of another layout is discarded.
const LAYOUT: u32 = 1;

const FAULT_NONE: u32 = 0;
const FAULT_HANG: u32 = 1;
const FAULT_PANIC: u32 = 2;

const PANIC_FILE_BYTES: usize = 48;
const PANIC_MESSAGE_BYTES: usize = 96;

#[derive(Clone, Copy)]
struct RawCheckpoint {
    name: [u8; CHECKPOINT_NAME_BYTES],
    arg: u32,
    uptime_ms: u32,
    hart: u32,
    crc: u32,
}

impl RawCheckpoint {
    const EMPTY: Self = Self {
        name: [0; CHECKPOINT_NAME_BYTES],
        arg: 0,
        uptime_ms: 0,
        hart: 0,
        crc: 0,
    };

    fn digest(&self) -> u32 {
        let mut digest = CRC.digest();
        digest.update(&self.name);
        for word in [self.arg, self.uptime_ms, self.hart] {
            digest.update(&word.to_le_bytes());
        }
        digest.finalize()
    }

    fn decode(&self) -> Option<Checkpoint> {
        if self.crc != self.digest() {
            return None;
        }
        Some(Checkpoint {
            name: text(&self.name)?,
            arg: self.arg,
            uptime_ms: self.uptime_ms,
            hart: u8::try_from(self.hart).ok()?,
        })
    }
}

#[derive(Clone, Copy)]
struct RawFault {
    kind: u32,
    detected_uptime_ms: u32,
    stalled_executors: u32,
    /// Per hart: responded, mepc, ra, sp, mcause, mstatus.
    harts: [[u32; 6]; 2],
    samples: [u32; 16],
    panic_line: u32,
    panic_file: [u8; PANIC_FILE_BYTES],
    panic_message: [u8; PANIC_MESSAGE_BYTES],
    crc: u32,
}

impl RawFault {
    const NONE: Self = Self {
        kind: FAULT_NONE,
        detected_uptime_ms: 0,
        stalled_executors: 0,
        harts: [[0; 6]; 2],
        samples: [0; 16],
        panic_line: 0,
        panic_file: [0; PANIC_FILE_BYTES],
        panic_message: [0; PANIC_MESSAGE_BYTES],
        crc: 0,
    };

    fn digest(&self) -> u32 {
        let mut digest = CRC.digest();
        for word in [self.kind, self.detected_uptime_ms, self.stalled_executors] {
            digest.update(&word.to_le_bytes());
        }
        for word in self.harts.iter().flatten().chain(&self.samples) {
            digest.update(&word.to_le_bytes());
        }
        digest.update(&self.panic_line.to_le_bytes());
        digest.update(&self.panic_file);
        digest.update(&self.panic_message);
        digest.finalize()
    }

    fn seal(mut self) -> Self {
        self.crc = self.digest();
        self
    }

    fn decode(&self) -> Option<Fault> {
        if self.crc != self.digest() {
            return None;
        }
        match self.kind {
            FAULT_HANG => Some(Fault::Hang(HangFault {
                detected_uptime_ms: self.detected_uptime_ms,
                stalled_executors: u8::try_from(self.stalled_executors).ok()?,
                harts: self
                    .harts
                    .map(|[responded, mepc, ra, sp, mcause, mstatus]| HartState {
                        responded: responded != 0,
                        mepc,
                        ra,
                        sp,
                        mcause,
                        mstatus,
                    }),
                samples: self.samples,
            })),
            FAULT_PANIC => Some(Fault::Panic(PanicFault {
                file: text(&self.panic_file)?,
                line: self.panic_line,
                message: text(&self.panic_message)?,
            })),
            _ => None,
        }
    }
}

/// The record in reset-retained memory. Any bit pattern is a record; an
/// invalid one reports nothing.
#[derive(Clone, Copy)]
pub struct Record {
    magic: u32,
    layout: u32,
    boot_count: u32,
    /// Index the next checkpoint is written at.
    head: u32,
    /// Checkpoints written, at most the ring's size.
    len: u32,
    header_crc: u32,
    checkpoints: [RawCheckpoint; POST_MORTEM_CHECKPOINTS],
    fault: RawFault,
}

/// What the previous boot left behind.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Previous {
    pub boot_count: u32,
    /// Oldest first; a checkpoint whose CRC failed is dropped.
    pub checkpoints: heapless::Vec<Checkpoint, POST_MORTEM_CHECKPOINTS>,
    pub fault: Option<Fault>,
}

impl Previous {
    pub fn summary(&self) -> PostMortemSummary {
        PostMortemSummary {
            boot_count: self.boot_count,
            checkpoints: self.checkpoints.len() as u8,
            fault: self.fault.clone(),
        }
    }

    /// Checkpoints `first..`, at most one page.
    pub fn page(&self, first: u8) -> PostMortemCheckpoints {
        PostMortemCheckpoints {
            first,
            checkpoints: self
                .checkpoints
                .iter()
                .skip(usize::from(first))
                .take(POST_MORTEM_CHECKPOINT_PAGE)
                .cloned()
                .collect(),
        }
    }
}

impl Record {
    /// A record holding nothing, for memory a power-on left undefined.
    pub const EMPTY: Self = Self {
        magic: 0,
        layout: 0,
        boot_count: 0,
        head: 0,
        len: 0,
        header_crc: 0,
        checkpoints: [RawCheckpoint::EMPTY; POST_MORTEM_CHECKPOINTS],
        fault: RawFault::NONE,
    };

    fn header_digest(&self) -> u32 {
        let mut digest = CRC.digest();
        for word in [
            self.magic,
            self.layout,
            self.boot_count,
            self.head,
            self.len,
        ] {
            digest.update(&word.to_le_bytes());
        }
        digest.finalize()
    }

    fn header_valid(&self) -> bool {
        self.magic == MAGIC
            && self.layout == LAYOUT
            && self.header_crc == self.header_digest()
            && (self.head as usize) < POST_MORTEM_CHECKPOINTS
            && (self.len as usize) <= POST_MORTEM_CHECKPOINTS
    }

    fn seal_header(&mut self) {
        self.header_crc = self.header_digest();
    }

    /// Start this boot's record: return what the previous boot left, when a
    /// valid record survived, and clear the ring and the fault.
    pub fn begin_boot(&mut self) -> Option<Previous> {
        let previous = self.header_valid().then(|| {
            let len = self.len as usize;
            let first =
                (self.head as usize + POST_MORTEM_CHECKPOINTS - len) % POST_MORTEM_CHECKPOINTS;
            Previous {
                boot_count: self.boot_count,
                checkpoints: (0..len)
                    .filter_map(|offset| {
                        self.checkpoints[(first + offset) % POST_MORTEM_CHECKPOINTS].decode()
                    })
                    .collect(),
                fault: self.fault.decode(),
            }
        });
        let boot_count = previous
            .as_ref()
            .map_or(1, |previous| previous.boot_count.wrapping_add(1));
        *self = Self::EMPTY;
        self.magic = MAGIC;
        self.layout = LAYOUT;
        self.boot_count = boot_count;
        self.fault = RawFault::NONE.seal();
        self.seal_header();
        previous
    }

    /// Append a checkpoint, replacing the oldest once the ring is full. A
    /// name longer than a checkpoint holds is cut at a character boundary.
    pub fn checkpoint(&mut self, name: &str, arg: u32, uptime_ms: u32, hart: u8) {
        let mut raw = RawCheckpoint {
            name: bytes(name),
            arg,
            uptime_ms,
            hart: u32::from(hart),
            crc: 0,
        };
        raw.crc = raw.digest();
        let head = self.head as usize % POST_MORTEM_CHECKPOINTS;
        self.checkpoints[head] = raw;
        self.head = ((head + 1) % POST_MORTEM_CHECKPOINTS) as u32;
        self.len = (self.len + 1).min(POST_MORTEM_CHECKPOINTS as u32);
        self.seal_header();
    }

    /// Record that a watchdog found this boot hung; a later fault replaces it.
    pub fn record_hang(&mut self, hang: &HangFault) {
        self.fault = RawFault {
            kind: FAULT_HANG,
            detected_uptime_ms: hang.detected_uptime_ms,
            stalled_executors: u32::from(hang.stalled_executors),
            harts: hang.harts.map(|hart| {
                [
                    u32::from(hart.responded),
                    hart.mepc,
                    hart.ra,
                    hart.sp,
                    hart.mcause,
                    hart.mstatus,
                ]
            }),
            samples: hang.samples,
            ..RawFault::NONE
        }
        .seal();
    }

    /// Record that this boot panicked, cutting long texts.
    pub fn record_panic(&mut self, file: &str, line: u32, message: &str) {
        self.fault = RawFault {
            kind: FAULT_PANIC,
            panic_line: line,
            panic_file: bytes(file),
            panic_message: bytes(message),
            ..RawFault::NONE
        }
        .seal();
    }
}

/// `text` as a zero-padded array, cut at a character boundary.
fn bytes<const N: usize>(text: &str) -> [u8; N] {
    let mut end = text.len().min(N);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut raw = [0; N];
    raw[..end].copy_from_slice(&text.as_bytes()[..end]);
    raw
}

/// The zero-padded text in `raw`; `None` when it is not UTF-8.
fn text<const N: usize>(raw: &[u8]) -> Option<heapless::String<N>> {
    let end = raw.iter().position(|&byte| byte == 0).unwrap_or(raw.len());
    heapless::String::try_from(core::str::from_utf8(&raw[..end]).ok()?).ok()
}

/// Checkpoints per second above which a caller is misusing checkpoints.
pub const CHECKPOINT_RATE_LIMIT: u32 = 100;

/// Counts checkpoints per second of uptime and reports a second that exceeded
/// [`CHECKPOINT_RATE_LIMIT`] once it ends.
#[derive(Clone, Copy, Debug, Default)]
pub struct RateMonitor {
    second: u32,
    count: u32,
}

impl RateMonitor {
    pub const fn new() -> Self {
        Self {
            second: 0,
            count: 0,
        }
    }

    /// Count a checkpoint at `uptime_ms`; returns the previous second's count
    /// when that second exceeded the limit.
    pub fn record(&mut self, uptime_ms: u32) -> Option<u32> {
        let second = uptime_ms / 1000;
        if second == self.second {
            self.count += 1;
            return None;
        }
        let previous = core::mem::replace(&mut self.count, 1);
        self.second = second;
        (previous > CHECKPOINT_RATE_LIMIT).then_some(previous)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hang() -> HangFault {
        let hart = |mepc| HartState {
            responded: true,
            mepc,
            ra: mepc + 4,
            sp: 0x4080_0000,
            mcause: 0x8000_0007,
            mstatus: 0x1880,
        };
        HangFault {
            detected_uptime_ms: 1_500,
            stalled_executors: 0b01,
            harts: [hart(0x4200_1000), HartState::default()],
            samples: [0x4200_1000; 16],
        }
    }

    #[test]
    fn a_power_on_leaves_nothing_to_report() {
        let mut record = Record::EMPTY;
        assert_eq!(record.begin_boot(), None);
        // Whatever memory a power-on leaves also fails the header.
        let mut random = Record::EMPTY;
        random.magic = MAGIC;
        random.layout = LAYOUT;
        random.header_crc = 0x1234_5678;
        assert_eq!(random.begin_boot(), None);
    }

    #[test]
    fn the_next_boot_reports_checkpoints_and_the_fault_once() {
        let mut record = Record::EMPTY;
        record.begin_boot();
        record.checkpoint("wifi.start", 1, 10, 0);
        record.checkpoint("coex.grant", 2, 20, 1);
        record.record_hang(&hang());
        let previous = record.begin_boot().unwrap();
        assert_eq!(previous.boot_count, 1);
        let names = previous
            .checkpoints
            .iter()
            .map(|checkpoint| checkpoint.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, ["wifi.start", "coex.grant"]);
        assert_eq!(previous.checkpoints[1].hart, 1);
        assert_eq!(previous.fault, Some(Fault::Hang(hang())));
        // The boot after reports its own, empty history, not the hang again.
        let next = record.begin_boot().unwrap();
        assert_eq!(next.boot_count, 2);
        assert!(next.checkpoints.is_empty());
        assert_eq!(next.fault, None);
    }

    #[test]
    fn the_ring_keeps_the_newest_checkpoints_in_order() {
        let mut record = Record::EMPTY;
        record.begin_boot();
        for index in 0..(POST_MORTEM_CHECKPOINTS as u32 + 5) {
            record.checkpoint("step", index, index, 0);
        }
        let previous = record.begin_boot().unwrap();
        let args = previous
            .checkpoints
            .iter()
            .map(|checkpoint| checkpoint.arg)
            .collect::<Vec<_>>();
        assert_eq!(
            args,
            (5..POST_MORTEM_CHECKPOINTS as u32 + 5).collect::<Vec<_>>()
        );
        let page = previous.page(30);
        assert_eq!(page.first, 30);
        assert_eq!(page.checkpoints.len(), 2);
        assert_eq!(
            previous.page(0).checkpoints.len(),
            POST_MORTEM_CHECKPOINT_PAGE
        );
        assert_eq!(
            previous.summary().checkpoints,
            POST_MORTEM_CHECKPOINTS as u8
        );
    }

    #[test]
    fn a_corrupted_part_is_dropped_and_the_rest_reported() {
        let mut record = Record::EMPTY;
        record.begin_boot();
        record.checkpoint("kept", 1, 1, 0);
        record.checkpoint("damaged", 2, 2, 0);
        record.record_panic("src/main.rs", 42, "boom");
        record.checkpoints[1].arg ^= 1;
        let previous = record.begin_boot().unwrap();
        assert_eq!(previous.checkpoints.len(), 1);
        assert_eq!(previous.checkpoints[0].name.as_str(), "kept");
        assert!(matches!(previous.fault, Some(Fault::Panic(ref panic)) if panic.line == 42));
        record.checkpoint("x", 0, 0, 0);
        record.record_panic("f", 1, "m");
        record.fault.panic_line ^= 1;
        assert_eq!(record.begin_boot().unwrap().fault, None);
    }

    #[test]
    fn long_texts_are_cut_at_a_character_boundary() {
        let mut record = Record::EMPTY;
        record.begin_boot();
        record.checkpoint("ieee802154.receive.frame", 0, 0, 0);
        record.record_panic(&"é".repeat(40), 7, &"x".repeat(200));
        let previous = record.begin_boot().unwrap();
        assert_eq!(previous.checkpoints[0].name.as_str(), "ieee802154.recei");
        let Some(Fault::Panic(panic)) = previous.fault else {
            panic!("the panic is reported");
        };
        assert_eq!(panic.file.as_str(), "é".repeat(24));
        assert_eq!(panic.message.len(), 96);
    }

    #[test]
    fn a_second_with_too_many_checkpoints_is_reported_once_it_ends() {
        let mut monitor = RateMonitor::default();
        for index in 0..=CHECKPOINT_RATE_LIMIT {
            assert_eq!(monitor.record(index % 1000), None);
        }
        assert_eq!(monitor.record(1_000), Some(CHECKPOINT_RATE_LIMIT + 1));
        assert_eq!(monitor.record(2_000), None);
    }
}
