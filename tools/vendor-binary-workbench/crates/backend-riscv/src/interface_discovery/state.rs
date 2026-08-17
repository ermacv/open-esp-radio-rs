//! Pointer-provenance state and conservative control-flow merging.

use std::collections::{BTreeMap, VecDeque};

use rv_asm::{Inst, Reg};

use super::*;

const MAX_POINTER_ALTERNATIVES: usize = 8;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum Value {
    Unknown,
    Constant(u32),
    Argument { index: u8, offset: i32 },
    Selector(InterfaceSlotSelector),
    Pointer(InterfacePointer),
    PointerAlternatives(Vec<InterfacePointer>),
    IndexedPointer(InterfacePointer, InterfaceSlotSelector),
    GotAddress(InterfacePointer),
}

impl Value {
    pub(super) fn add_constant(self, offset: i32) -> Self {
        match self {
            Self::Constant(value) => Self::Constant(value.wrapping_add(offset as u32)),
            Self::Argument {
                index,
                offset: current,
            } => Self::Argument {
                index,
                offset: current.wrapping_add(offset),
            },
            Self::Selector(mut selector) => {
                selector.addend = selector.addend.wrapping_add(offset);
                Self::Selector(selector)
            }
            Self::Pointer(mut pointer) => {
                pointer.post_offset = pointer.post_offset.wrapping_add(offset);
                Self::Pointer(pointer)
            }
            Self::PointerAlternatives(pointers) => pointer_alternatives(
                pointers
                    .into_iter()
                    .map(|mut pointer| {
                        pointer.post_offset = pointer.post_offset.wrapping_add(offset);
                        pointer
                    })
                    .collect(),
            ),
            Self::IndexedPointer(mut pointer, selector) => {
                pointer.post_offset = pointer.post_offset.wrapping_add(offset);
                Self::IndexedPointer(pointer, selector)
            }
            Self::GotAddress(mut pointer) => {
                pointer.post_offset = pointer.post_offset.wrapping_add(offset);
                Self::GotAddress(pointer)
            }
            Self::Unknown => Self::Unknown,
        }
    }

    pub(super) fn as_argument(&self) -> InterfaceArgumentValue {
        match self {
            Self::Unknown => InterfaceArgumentValue::Unknown,
            Self::Constant(value) => InterfaceArgumentValue::Constant(*value),
            Self::Argument { index, offset } => InterfaceArgumentValue::Pointer(InterfacePointer {
                root: InterfaceRoot::FunctionArgument { index: *index },
                loads: Vec::new(),
                post_offset: *offset,
            }),
            Self::Pointer(pointer) => InterfaceArgumentValue::Pointer(pointer.clone()),
            Self::Selector(_)
            | Self::PointerAlternatives(_)
            | Self::IndexedPointer(_, _)
            | Self::GotAddress(_) => InterfaceArgumentValue::Unknown,
        }
    }

    pub(super) fn as_pointer(&self) -> Option<InterfacePointer> {
        match self {
            Self::Pointer(pointer) => Some(pointer.clone()),
            Self::Argument { index, offset } => Some(InterfacePointer {
                root: InterfaceRoot::FunctionArgument { index: *index },
                loads: Vec::new(),
                post_offset: *offset,
            }),
            _ => None,
        }
    }

    pub(super) fn as_pointers(&self) -> Vec<InterfacePointer> {
        match self {
            Self::Pointer(pointer) => vec![pointer.clone()],
            Self::PointerAlternatives(pointers) => pointers.clone(),
            Self::Argument { index, offset } => vec![InterfacePointer {
                root: InterfaceRoot::FunctionArgument { index: *index },
                loads: Vec::new(),
                post_offset: *offset,
            }],
            _ => Vec::new(),
        }
    }

    pub(super) fn shift_left(self, amount: u32) -> Self {
        let scale = 1_u32.wrapping_shl(amount & 31);
        match self {
            Self::Argument { index, offset: 0 } => Self::Selector(InterfaceSlotSelector {
                argument: index,
                scale,
                addend: 0,
            }),
            Self::Selector(mut selector) => {
                selector.scale = selector.scale.wrapping_mul(scale);
                selector.addend = selector.addend.wrapping_shl(amount & 31);
                Self::Selector(selector)
            }
            Self::Constant(value) => Self::Constant(value.wrapping_shl(amount & 31)),
            _ => Self::Unknown,
        }
    }
}

fn pointer_alternatives(mut pointers: Vec<InterfacePointer>) -> Value {
    pointers.sort();
    pointers.dedup();
    match pointers.len() {
        0 => Value::Unknown,
        1 => Value::Pointer(pointers.pop().expect("one pointer alternative")),
        2..=MAX_POINTER_ALTERNATIVES => Value::PointerAlternatives(pointers),
        _ => Value::Unknown,
    }
}

pub(super) type RegisterState = [Value; 32];

pub(super) fn initial_state() -> RegisterState {
    let mut values = core::array::from_fn(|_| Value::Unknown);
    values[0] = Value::Constant(0);
    for index in 0..RV32_REGISTER_ARGUMENT_COUNT {
        values[10 + index] = Value::Argument {
            index: index as u8,
            offset: 0,
        };
    }
    values
}

pub(super) fn set(values: &mut RegisterState, register: Reg, value: Value) {
    if register != Reg::ZERO {
        values[usize::from(register.0)] = value;
    }
}

pub(super) fn relocated_root(
    owner: &artifact::ArtifactSymbolDefinition,
    relocation: &artifact::SymbolRelocation,
) -> Value {
    let addressing = match relocation.kind {
        artifact::RelocationKind::Hi20
        | artifact::RelocationKind::Lo12I
        | artifact::RelocationKind::Lo12S => InterfaceSymbolAddressing::Absolute,
        artifact::RelocationKind::PcRelHi20
        | artifact::RelocationKind::PcRelLo12I
        | artifact::RelocationKind::PcRelLo12S => InterfaceSymbolAddressing::PcRelative,
        artifact::RelocationKind::GotHi20 | artifact::RelocationKind::GotPcRelLo12I => {
            InterfaceSymbolAddressing::Got
        }
        artifact::RelocationKind::Call | artifact::RelocationKind::CallPlt => {
            return Value::Unknown;
        }
    };
    let pointer = InterfacePointer {
        root: InterfaceRoot::RelocatedSymbol {
            member: owner.member.clone(),
            symbol: relocation.symbol.clone(),
            addend: relocation.addend,
            addressing,
        },
        loads: Vec::new(),
        post_offset: 0,
    };
    if addressing == InterfaceSymbolAddressing::Got
        && relocation.kind == artifact::RelocationKind::GotHi20
    {
        Value::GotAddress(pointer)
    } else {
        Value::Pointer(pointer)
    }
}

fn low_relocation_root(
    owner: &artifact::ArtifactSymbolDefinition,
    pc: u32,
) -> Option<(&artifact::SymbolRelocation, Value)> {
    if owner.addresses_resolved {
        return None;
    }
    [
        artifact::RelocationKind::Lo12I,
        artifact::RelocationKind::PcRelLo12I,
        artifact::RelocationKind::GotPcRelLo12I,
    ]
    .into_iter()
    .find_map(|kind| {
        owner
            .relocation(pc, kind)
            .map(|relocation| (relocation, relocated_root(owner, relocation)))
    })
}

pub(super) fn low_relocation_value<'a>(
    owner: &'a artifact::ArtifactSymbolDefinition,
    pc: u32,
    base: &Value,
) -> Option<Option<(&'a artifact::SymbolRelocation, Value)>> {
    let (relocation, value) = low_relocation_root(owner, pc)?;
    let expected_base = match (&value, relocation.kind) {
        (Value::Pointer(pointer), artifact::RelocationKind::GotPcRelLo12I) => {
            Value::GotAddress(pointer.clone())
        }
        _ => value.clone(),
    };
    Some((base == &expected_base).then_some((relocation, value)))
}

pub(super) fn append_load(value: Value, site: u32, offset: i32, width: u8) -> Value {
    if let Value::PointerAlternatives(pointers) = value {
        return pointer_alternatives(
            pointers
                .into_iter()
                .filter_map(|pointer| {
                    match append_load(Value::Pointer(pointer), site, offset, width) {
                        Value::Pointer(pointer) => Some(pointer),
                        _ => None,
                    }
                })
                .collect(),
        );
    }
    let (mut pointer, selector) = match value {
        Value::Pointer(pointer) => (pointer, None),
        Value::IndexedPointer(pointer, selector) => (pointer, Some(selector)),
        Value::Argument { index, offset } => (
            InterfacePointer {
                root: InterfaceRoot::FunctionArgument { index },
                loads: Vec::new(),
                post_offset: offset,
            },
            None,
        ),
        Value::Constant(address) => (
            InterfacePointer {
                root: InterfaceRoot::AbsoluteAddress { address },
                loads: Vec::new(),
                post_offset: 0,
            },
            None,
        ),
        Value::Selector(_)
        | Value::PointerAlternatives(_)
        | Value::GotAddress(_)
        | Value::Unknown => return Value::Unknown,
    };
    pointer.loads.push(InterfaceLoad {
        site,
        offset: pointer.post_offset.wrapping_add(offset),
        width,
        selector,
    });
    pointer.post_offset = 0;
    Value::Pointer(pointer)
}

pub(super) fn clear_call_clobbers(values: &mut RegisterState) {
    for register in [
        Reg::RA,
        Reg::T0,
        Reg::T1,
        Reg::T2,
        Reg::A0,
        Reg::A1,
        Reg::A2,
        Reg::A3,
        Reg::A4,
        Reg::A5,
        Reg::A6,
        Reg::A7,
        Reg::T3,
        Reg::T4,
        Reg::T5,
        Reg::T6,
    ] {
        set(values, register, Value::Unknown);
    }
}

pub(super) fn clear_destination(instruction: Inst, values: &mut RegisterState) {
    let destination = match instruction {
        Inst::Slti { dest, .. }
        | Inst::Sltiu { dest, .. }
        | Inst::Xori { dest, .. }
        | Inst::Ori { dest, .. }
        | Inst::Andi { dest, .. }
        | Inst::Slli { dest, .. }
        | Inst::SlliW { dest, .. }
        | Inst::Srli { dest, .. }
        | Inst::SrliW { dest, .. }
        | Inst::Srai { dest, .. }
        | Inst::SraiW { dest, .. }
        | Inst::AddW { dest, .. }
        | Inst::Sub { dest, .. }
        | Inst::SubW { dest, .. }
        | Inst::Sll { dest, .. }
        | Inst::SllW { dest, .. }
        | Inst::Slt { dest, .. }
        | Inst::Sltu { dest, .. }
        | Inst::Xor { dest, .. }
        | Inst::Srl { dest, .. }
        | Inst::SrlW { dest, .. }
        | Inst::Sra { dest, .. }
        | Inst::SraW { dest, .. }
        | Inst::Or { dest, .. }
        | Inst::And { dest, .. }
        | Inst::Mul { dest, .. }
        | Inst::MulW { dest, .. }
        | Inst::Mulh { dest, .. }
        | Inst::Mulhsu { dest, .. }
        | Inst::Mulhu { dest, .. }
        | Inst::Div { dest, .. }
        | Inst::DivW { dest, .. }
        | Inst::Divu { dest, .. }
        | Inst::DivuW { dest, .. }
        | Inst::Rem { dest, .. }
        | Inst::RemW { dest, .. }
        | Inst::Remu { dest, .. }
        | Inst::RemuW { dest, .. }
        | Inst::LrW { dest, .. }
        | Inst::ScW { dest, .. }
        | Inst::AmoW { dest, .. } => Some(dest),
        _ => None,
    };
    if let Some(destination) = destination {
        set(values, destination, Value::Unknown);
    }
}

fn merge_state(target: &mut RegisterState, incoming: &RegisterState) -> bool {
    let mut changed = false;
    for (target, incoming) in target.iter_mut().zip(incoming) {
        if target != incoming && *target != Value::Unknown {
            let target_pointers = target.as_pointers();
            let incoming_pointers = incoming.as_pointers();
            *target = if target_pointers.is_empty() || incoming_pointers.is_empty() {
                Value::Unknown
            } else {
                pointer_alternatives(
                    target_pointers
                        .into_iter()
                        .chain(incoming_pointers)
                        .collect(),
                )
            };
            changed = true;
        }
    }
    changed
}

pub(super) fn enqueue_state(
    index: usize,
    state: &RegisterState,
    states: &mut BTreeMap<usize, RegisterState>,
    queue: &mut VecDeque<usize>,
) {
    match states.get_mut(&index) {
        Some(existing) => {
            if merge_state(existing, state) {
                queue.push_back(index);
            }
        }
        None => {
            states.insert(index, state.clone());
            queue.push_back(index);
        }
    }
}

pub(super) fn branch_target(
    instruction_indices: &BTreeMap<u32, usize>,
    pc: u32,
    offset: i32,
) -> Option<usize> {
    instruction_indices
        .get(&pc.wrapping_add(offset as u32))
        .copied()
}
