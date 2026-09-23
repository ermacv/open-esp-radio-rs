//! Pointer-provenance state and conservative control-flow merging.

use std::collections::{BTreeMap, VecDeque};

use rv_asm::{Inst, Reg};

use super::*;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) enum Value {
    Unknown,
    Constant(u32),
    Argument {
        owner: artifact::CodeIdentity,
        index: u8,
        offset: i32,
    },
    Selector(InterfaceSlotSelector),
    Pointer(InterfacePointer),
    Alternatives(Vec<Value>),
    IndexedPointer(InterfacePointer, InterfaceSlotSelector),
    GotAddress(InterfacePointer),
}

impl Value {
    fn absolute_root(
        address: u32,
        accessed_address: u32,
        width: Option<u8>,
        data_symbols: &[artifact::ArtifactDataSymbolDefinition],
    ) -> InterfaceRoot {
        InterfaceRoot::AbsoluteAddress {
            address,
            data_address: crate::ReferenceResolver::resolve_data_address(
                data_symbols,
                accessed_address,
                width,
            ),
        }
    }

    pub(super) fn atoms(&self) -> &[Self] {
        match self {
            Self::Alternatives(values) => values,
            _ => std::slice::from_ref(self),
        }
    }

    pub(super) fn add(self, right: Self) -> Self {
        let mut output = Vec::new();
        for left in self.atoms() {
            for right in right.atoms() {
                output.push(match (left.clone(), right.clone()) {
                    (value, Self::Constant(offset)) | (Self::Constant(offset), value) => {
                        value.add_constant(offset as i32)
                    }
                    (Self::Pointer(pointer), Self::Selector(selector))
                    | (Self::Selector(selector), Self::Pointer(pointer)) => {
                        Self::IndexedPointer(pointer, selector)
                    }
                    (
                        Self::Pointer(pointer),
                        Self::Argument {
                            index, offset: 0, ..
                        },
                    )
                    | (
                        Self::Argument {
                            index, offset: 0, ..
                        },
                        Self::Pointer(pointer),
                    ) => Self::IndexedPointer(
                        pointer,
                        InterfaceSlotSelector {
                            argument: index,
                            scale: 1,
                            addend: 0,
                        },
                    ),
                    _ => Self::Unknown,
                });
            }
        }
        alternatives(output)
    }

    pub(super) fn add_constant(self, offset: i32) -> Self {
        match self {
            Self::Constant(value) => Self::Constant(value.wrapping_add(offset as u32)),
            Self::Argument {
                owner,
                index,
                offset: current,
            } => Self::Argument {
                owner,
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
            Self::Alternatives(values) => alternatives(
                values
                    .into_iter()
                    .map(|value| value.add_constant(offset))
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
            Self::Argument {
                owner,
                index,
                offset,
            } => InterfaceArgumentValue::Pointer(InterfacePointer {
                root: InterfaceRoot::FunctionArgument {
                    owner: owner.clone(),
                    index: *index,
                },
                loads: Vec::new(),
                post_offset: *offset,
            }),
            Self::Pointer(pointer) => InterfaceArgumentValue::Pointer(pointer.clone()),
            Self::Alternatives(values) => {
                InterfaceArgumentValue::Alternatives(values.iter().map(Self::as_argument).collect())
            }
            Self::Selector(selector) => InterfaceArgumentValue::Selector(selector.clone()),
            Self::IndexedPointer(pointer, selector) => InterfaceArgumentValue::IndexedPointer {
                pointer: pointer.clone(),
                selector: selector.clone(),
            },
            Self::GotAddress(pointer) => InterfaceArgumentValue::GotAddress(pointer.clone()),
        }
    }

    pub(super) fn as_pointer(&self) -> Option<InterfacePointer> {
        match self {
            Self::Pointer(pointer) => Some(pointer.clone()),
            Self::Argument {
                owner,
                index,
                offset,
            } => Some(InterfacePointer {
                root: InterfaceRoot::FunctionArgument {
                    owner: owner.clone(),
                    index: *index,
                },
                loads: Vec::new(),
                post_offset: *offset,
            }),
            _ => None,
        }
    }

    pub(super) fn as_data_pointer(
        &self,
        data_symbols: &[artifact::ArtifactDataSymbolDefinition],
    ) -> Option<InterfacePointer> {
        self.as_pointer().or_else(|| {
            let Self::Constant(address) = self else {
                return None;
            };
            Some(InterfacePointer {
                root: Self::absolute_root(*address, *address, None, data_symbols),
                loads: Vec::new(),
                post_offset: 0,
            })
        })
    }

    pub(super) fn as_data_store_pointer(
        &self,
        offset: i32,
        width: u8,
        data_symbols: &[artifact::ArtifactDataSymbolDefinition],
    ) -> Option<InterfacePointer> {
        if let Some(mut pointer) = self.as_pointer() {
            pointer.post_offset = pointer.post_offset.wrapping_add(offset);
            return Some(pointer);
        }
        (|| {
            let Self::Constant(base) = self else {
                return None;
            };
            let data_address = base.checked_add_signed(offset).map_or(
                crate::DataAddressResolution::Unknown {
                    reason: crate::DataAddressGap::AddressOverflow,
                },
                |address| {
                    crate::ReferenceResolver::resolve_data_address(
                        data_symbols,
                        address,
                        Some(width),
                    )
                },
            );
            Some(InterfacePointer {
                root: InterfaceRoot::AbsoluteAddress {
                    address: *base,
                    data_address,
                },
                loads: Vec::new(),
                post_offset: offset,
            })
        })()
    }

    pub(super) fn as_pointers(&self) -> Vec<InterfacePointer> {
        match self {
            Self::Pointer(pointer) => vec![pointer.clone()],
            Self::Alternatives(values) => values.iter().flat_map(Self::as_pointers).collect(),
            Self::Argument {
                owner,
                index,
                offset,
            } => vec![InterfacePointer {
                root: InterfaceRoot::FunctionArgument {
                    owner: owner.clone(),
                    index: *index,
                },
                loads: Vec::new(),
                post_offset: *offset,
            }],
            _ => Vec::new(),
        }
    }

    pub(super) fn shift_left(self, amount: u32) -> Self {
        let scale = 1_u32.wrapping_shl(amount & 31);
        match self {
            Self::Alternatives(values) => alternatives(
                values
                    .into_iter()
                    .map(|value| value.shift_left(amount))
                    .collect(),
            ),
            Self::Argument {
                index, offset: 0, ..
            } => Self::Selector(InterfaceSlotSelector {
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

fn alternatives(values: Vec<Value>) -> Value {
    let mut atoms = values
        .into_iter()
        .flat_map(|value| match value {
            Value::Alternatives(values) => values,
            value => vec![value],
        })
        .collect::<Vec<_>>();
    atoms.sort();
    atoms.dedup();
    match atoms.len() {
        0 => Value::Unknown,
        1 => atoms.pop().expect("one alternative"),
        _ => Value::Alternatives(atoms),
    }
}

pub(super) type RegisterState = [Value; 32];

pub(super) fn initial_state(owner: &artifact::CodeIdentity) -> RegisterState {
    let mut values = core::array::from_fn(|_| Value::Unknown);
    values[0] = Value::Constant(0);
    for index in 0..RV32_REGISTER_ARGUMENT_COUNT {
        values[10 + index] = Value::Argument {
            owner: owner.clone(),
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
            reference: relocation.reference.clone(),
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
    let matched = base.atoms().iter().any(|base| base == &expected_base);
    Some(matched.then(|| {
        (
            relocation,
            alternatives(
                base.atoms()
                    .iter()
                    .map(|base| {
                        if base == &expected_base {
                            value.clone()
                        } else {
                            Value::Unknown
                        }
                    })
                    .collect(),
            ),
        )
    }))
}

pub(super) fn append_load(
    value: Value,
    site: u32,
    offset: i32,
    width: u8,
    data_symbols: &[artifact::ArtifactDataSymbolDefinition],
) -> Value {
    if let Value::Alternatives(values) = value {
        return alternatives(
            values
                .into_iter()
                .map(|value| append_load(value, site, offset, width, data_symbols))
                .collect(),
        );
    }
    if let Value::Constant(base) = value {
        let data_address = base.checked_add_signed(offset).map_or(
            crate::DataAddressResolution::Unknown {
                reason: crate::DataAddressGap::AddressOverflow,
            },
            |address| {
                crate::ReferenceResolver::resolve_data_address(data_symbols, address, Some(width))
            },
        );
        let mut pointer = InterfacePointer {
            root: InterfaceRoot::AbsoluteAddress {
                address: base,
                data_address,
            },
            loads: Vec::new(),
            post_offset: 0,
        };
        pointer.loads.push(InterfaceLoad {
            site,
            offset,
            width,
            selector: None,
        });
        return Value::Pointer(pointer);
    }
    let (mut pointer, selector) = match value {
        Value::Pointer(pointer) => (pointer, None),
        Value::IndexedPointer(pointer, selector) => (pointer, Some(selector)),
        Value::Argument {
            owner,
            index,
            offset,
        } => (
            InterfacePointer {
                root: InterfaceRoot::FunctionArgument { owner, index },
                loads: Vec::new(),
                post_offset: offset,
            },
            None,
        ),
        Value::Constant(_) => unreachable!("constant load handled above"),
        Value::Selector(_) | Value::Alternatives(_) | Value::GotAddress(_) | Value::Unknown => {
            return Value::Unknown;
        }
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

pub(super) fn clear_destination(instruction: Inst, values: &mut RegisterState) -> Option<Reg> {
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
    destination.filter(|register| *register != Reg::ZERO)
}

fn merge_state(target: &mut RegisterState, incoming: &RegisterState) -> bool {
    let mut changed = false;
    for (target, incoming) in target.iter_mut().zip(incoming) {
        let merged = alternatives(vec![target.clone(), incoming.clone()]);
        if *target != merged {
            *target = merged;
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
            if merge_state(existing, state) && !queue.contains(&index) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_retains_unknown_and_values_independently_of_arrival_order() {
        let mut known = initial_state(&artifact::CodeIdentity::Synthetic {
            namespace: module_path!().into(),
            key: "state-fixture".into(),
        });
        known[5] = Value::Argument {
            owner: artifact::CodeIdentity::Synthetic {
                namespace: module_path!().into(),
                key: "state-fixture".into(),
            },
            index: 0,
            offset: 4,
        };
        let unknown = initial_state(&artifact::CodeIdentity::Synthetic {
            namespace: module_path!().into(),
            key: "state-fixture".into(),
        });
        let mut forward = known.clone();
        let mut reverse = unknown.clone();
        assert!(merge_state(&mut forward, &unknown));
        assert!(merge_state(&mut reverse, &known));
        assert_eq!(forward, reverse);
        assert!(!merge_state(&mut forward, &unknown));
        assert!(!merge_state(&mut forward, &known));
        let loaded = append_load(forward[5].clone(), 20, 8, 32, &[]);
        assert_eq!(loaded.as_pointers()[0].loads[0].offset, 12);
        assert!(loaded.atoms().contains(&Value::Unknown));
    }

    #[test]
    fn scalar_selector_indexed_and_got_alternatives_survive_join_and_serialization() {
        let pointer = InterfacePointer {
            root: InterfaceRoot::FunctionArgument {
                owner: artifact::CodeIdentity::Synthetic {
                    namespace: module_path!().into(),
                    key: "state-fixture".into(),
                },
                index: 0,
            },
            loads: vec![],
            post_offset: 0,
        };
        let selector = InterfaceSlotSelector {
            argument: 1,
            scale: 4,
            addend: 0,
        };
        let values = vec![
            Value::Constant(8),
            Value::Unknown,
            Value::Selector(selector.clone()),
            Value::IndexedPointer(pointer.clone(), selector),
            Value::GotAddress(pointer),
        ];
        let mut joined = initial_state(&artifact::CodeIdentity::Synthetic {
            namespace: module_path!().into(),
            key: "state-fixture".into(),
        });
        for value in &values {
            let mut incoming = initial_state(&artifact::CodeIdentity::Synthetic {
                namespace: module_path!().into(),
                key: "state-fixture".into(),
            });
            incoming[5] = value.clone();
            merge_state(&mut joined, &incoming);
        }
        assert_eq!(joined[5].atoms().len(), values.len());
        let result = joined[5].clone().add_constant(4).as_argument();
        let json = serde_json::to_string(&result).unwrap();
        assert_eq!(
            serde_json::from_str::<InterfaceArgumentValue>(&json).unwrap(),
            result
        );
        assert!(json.contains("indexed-pointer"));
        assert!(json.contains("got-address"));
        assert!(json.contains("selector"));
    }
}
