//! Observable and draft reference events.

use open_radio_vendor_validator_core::{ExternalFunctionRef, ExternalTableRef};

use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryAccess {
    Read,
    Write,
}

pub fn parse_fence_set(value: &str) -> Option<u8> {
    let mut encoded = 0_u8;
    for character in value.chars() {
        encoded |= match character.to_ascii_lowercase() {
            'i' => 1 << 3,
            'o' => 1 << 2,
            'r' => 1 << 1,
            'w' => 1,
            _ => return None,
        };
    }
    Some(encoded)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ObservableEvent {
    Memory {
        access: MemoryAccess,
        width: u8,
        address: u32,
        register: String,
        value: Option<SymbolicValue>,
    },
    Fence {
        fm: u8,
        predecessor: u8,
        successor: u8,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DraftReferenceEvent {
    Observable(ObservableEvent),
    IndexedMmio {
        access: MemoryAccess,
        width: u8,
        address: SymbolicValue,
        registers: Vec<IndexedMmioRegister>,
        guard: Option<IndexedMmioGuard>,
        value: Option<SymbolicValue>,
    },
    PollMmio {
        width: u8,
        address: SymbolicValue,
        registers: Vec<IndexedMmioRegister>,
        guard: Option<IndexedMmioGuard>,
        mask: u32,
        expected: u32,
    },
    BoundedPoll {
        maximum_attempts: u16,
        body: Box<DraftReferenceFlow>,
        repeat_while_mask: u32,
        repeat_while_expected: u32,
        on_exhausted: Option<Box<DraftReferenceEvent>>,
    },
    PollFlow {
        body: Box<DraftReferenceFlow>,
        exit_when_mask: u32,
        exit_when_expected: u32,
    },
    SymmetricCalibrationSearch {
        token: u32,
        attempts_per_direction: u16,
        settle_micros: u32,
        sample_shift: u8,
        sample_mask: u32,
        accepted_sample: u32,
        initial_read: Box<DraftReferenceFlow>,
        setup: Box<DraftReferenceFlow>,
        write_candidate: Box<DraftReferenceFlow>,
        sample: Box<DraftReferenceFlow>,
    },
    DelayMicros {
        micros: SymbolicValue,
    },
    Memory {
        access: MemoryAccess,
        width: u8,
        address: SymbolicValue,
        region: String,
        value: Option<SymbolicValue>,
    },
    PrivateStackLoad {
        token: u32,
        offset: i32,
        width: u8,
        signed: bool,
    },
    PrivateStackStore {
        offset: i32,
        width: u8,
        value: SymbolicValue,
    },
    ExternalCall {
        token: u32,
        site: u32,
        table: ExternalTableRef,
        function: ExternalFunctionRef,
        arguments: Box<[SymbolicValue]>,
    },
    DiagnosticCall {
        function: String,
        argument_count: u8,
        arguments: Box<[SymbolicValue]>,
    },
    TailCall {
        token: u32,
        site: u32,
        target: u32,
        arguments: Box<[SymbolicValue]>,
    },
    Call {
        token: u32,
        site: u32,
        target: u32,
        arguments: Box<[SymbolicValue]>,
    },
    ComposedCall {
        token: u32,
        symbol: String,
        arguments: Box<[SymbolicValue]>,
        flow: Box<DraftReferenceFlow>,
        result_modeled: bool,
    },
    ScratchCall {
        token: u32,
        site: u32,
        target: u32,
        arguments: Box<[SymbolicValue]>,
        scratch_argument: u8,
        scratch_size: u16,
    },
    ComposedCallWithScratch {
        token: u32,
        symbol: String,
        arguments: Box<[SymbolicValue]>,
        flow: Box<DraftReferenceFlow>,
        result_modeled: bool,
        scratch_argument: u8,
        scratch_size: u16,
    },
    WideSignedDivide {
        token: u32,
        dividend_low: SymbolicValue,
        dividend_high: SymbolicValue,
        divisor_low: SymbolicValue,
        divisor_high: SymbolicValue,
    },
    BranchDecision {
        condition: BranchCondition,
        taken: bool,
    },
}

pub fn reference_event_is_mmio_read(event: &DraftReferenceEvent) -> bool {
    matches!(
        event,
        DraftReferenceEvent::Observable(ObservableEvent::Memory {
            access: MemoryAccess::Read,
            ..
        }) | DraftReferenceEvent::IndexedMmio {
            access: MemoryAccess::Read,
            ..
        }
    )
}

impl ObservableEvent {
    pub fn canonical(&self) -> String {
        match self {
            Self::Memory {
                access,
                width,
                address,
                register,
                value,
            } => {
                let access = match access {
                    MemoryAccess::Read => "R",
                    MemoryAccess::Write => "W",
                };
                let value = value
                    .as_ref()
                    .map_or_else(|| "-".to_owned(), SymbolicValue::canonical);
                format!("{access}\t{width}\t{address:#010x}\t{register}\t{value}")
            }
            Self::Fence {
                fm,
                predecessor,
                successor,
            } => format!("FENCE\tfm={fm:#x}\tpred={predecessor:#x}\tsucc={successor:#x}"),
        }
    }

    pub fn equivalent(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Memory {
                    access: left_access,
                    width: left_width,
                    address: left_address,
                    value: left_value,
                    ..
                },
                Self::Memory {
                    access: right_access,
                    width: right_width,
                    address: right_address,
                    value: right_value,
                    ..
                },
            ) => {
                left_access == right_access
                    && left_width == right_width
                    && left_address == right_address
                    && left_value == right_value
            }
            (
                Self::Fence {
                    fm: left_fm,
                    predecessor: left_predecessor,
                    successor: left_successor,
                },
                Self::Fence {
                    fm: right_fm,
                    predecessor: right_predecessor,
                    successor: right_successor,
                },
            ) => {
                left_fm == right_fm
                    && left_predecessor == right_predecessor
                    && left_successor == right_successor
            }
            _ => false,
        }
    }

    pub fn unmapped_address(&self) -> Option<u32> {
        match self {
            Self::Memory {
                address, register, ..
            } if register == "UNMAPPED" => Some(*address),
            _ => None,
        }
    }

    pub fn memory_value(&self) -> Option<String> {
        match self {
            Self::Memory { value, .. } => value.as_ref().map(SymbolicValue::canonical),
            Self::Fence { .. } => None,
        }
    }
}
