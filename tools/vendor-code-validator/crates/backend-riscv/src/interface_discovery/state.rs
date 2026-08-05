//! Pointer-provenance state and conservative control-flow merging.

use std::collections::{BTreeMap, VecDeque};

use rv_asm::{Inst, Reg};

use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum Value {
    Unknown,
    Constant(u32),
    Pointer(InterfacePointer),
    GotAddress(InterfacePointer),
}

impl Value {
    pub(super) fn add_constant(self, offset: i32) -> Self {
        match self {
            Self::Constant(value) => Self::Constant(value.wrapping_add(offset as u32)),
            Self::Pointer(mut pointer) => {
                pointer.post_offset = pointer.post_offset.wrapping_add(offset);
                Self::Pointer(pointer)
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
            Self::Pointer(pointer) => InterfaceArgumentValue::Pointer(pointer.clone()),
            Self::GotAddress(_) => InterfaceArgumentValue::Unknown,
        }
    }
}

pub(super) type RegisterState = [Value; 32];

pub(super) fn initial_state() -> RegisterState {
    let mut values = core::array::from_fn(|_| Value::Unknown);
    values[0] = Value::Constant(0);
    for index in 0..RV32_REGISTER_ARGUMENT_COUNT {
        values[10 + index] = Value::Pointer(InterfacePointer {
            root: InterfaceRoot::FunctionArgument { index: index as u8 },
            loads: Vec::new(),
            post_offset: 0,
        });
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
    let mut pointer = match value {
        Value::Pointer(pointer) => pointer,
        Value::Constant(address) => InterfacePointer {
            root: InterfaceRoot::AbsoluteAddress { address },
            loads: Vec::new(),
            post_offset: 0,
        },
        Value::GotAddress(_) | Value::Unknown => return Value::Unknown,
    };
    pointer.loads.push(InterfaceLoad {
        site,
        offset: pointer.post_offset.wrapping_add(offset),
        width,
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
            *target = Value::Unknown;
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
