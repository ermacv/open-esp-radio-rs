//! Reviewed analog-I2C and host-selection summaries.

use super::*;

pub(super) const PHY_GET_I2C_HOSTID_ROM_BODY: &[u8] = &[
    0x13, 0x05, 0xf5, 0xf9, 0x93, 0x77, 0xf5, 0x0f, 0x29, 0x47, 0x01, 0x45, 0x63, 0x69, 0xf7, 0x00,
    0x05, 0x45, 0x33, 0x15, 0xf5, 0x00, 0x13, 0x75, 0x75, 0x64, 0x33, 0x35, 0xa0, 0x00, 0xb7, 0x06,
    0x11, 0x20, 0x83, 0xa7, 0x06, 0x82, 0x79, 0x77, 0x3d, 0x07, 0xf9, 0x8f, 0x09, 0x67, 0x13, 0x07,
    0x07, 0xa0, 0xd9, 0x8f, 0x23, 0xa0, 0xf6, 0x82, 0x82, 0x80,
];

pub(super) const PHY_GET_I2C_HOSTID_NEW_BODY: &[u8] = &[
    0x13, 0x05, 0xf5, 0xf9, 0x13, 0x75, 0xf5, 0x0f, 0xa9, 0x47, 0x63, 0xea, 0xa7, 0x00, 0xb7, 0x87,
    0x00, 0x10, 0x93, 0x87, 0xc7, 0x1d, 0x0a, 0x05, 0x3e, 0x95, 0x1c, 0x41, 0x82, 0x87, 0x01, 0x45,
    0xb7, 0x06, 0x11, 0x20, 0x83, 0xa7, 0x06, 0x82, 0x37, 0x07, 0xfc, 0xff, 0x3d, 0x07, 0xf9, 0x8f,
    0x37, 0x07, 0x04, 0x00, 0x13, 0x07, 0x07, 0xa0, 0xd9, 0x8f, 0x23, 0xa0, 0xf6, 0x82, 0x82, 0x80,
    0x05, 0x45, 0xf9, 0xbf,
];

pub(super) const PHY_CHIP_I2C_READ_REG_ORG_BODY: &[u8] = &[
    0xb7, 0x07, 0x11, 0x20, 0x93, 0xc5, 0xf5, 0xff, 0x23, 0xae, 0xb7, 0x80, 0xb7, 0x47, 0x04, 0x08,
    0x93, 0x87, 0x07, 0xe0, 0xa2, 0x06, 0xc9, 0x8e, 0x3e, 0x96, 0x37, 0x05, 0x00, 0x04, 0x0a, 0x06,
    0xc9, 0x8e, 0x14, 0xc2, 0x1c, 0x42, 0x13, 0x97, 0x67, 0x00, 0xe3, 0x4d, 0x07, 0xfe, 0x08, 0x42,
    0x41, 0x81, 0x13, 0x75, 0xf5, 0x0f, 0x82, 0x80,
];

pub(super) const PHY_CHIP_I2C_WRITE_REG_BODY: &[u8] = &[
    0x01, 0x11, 0x22, 0xcc, 0x37, 0x04, 0x08, 0x2f, 0x83, 0x27, 0xc4, 0xc3, 0x06, 0xce, 0x26, 0xca,
    0x9c, 0x43, 0x32, 0xc6, 0x36, 0xc4, 0xaa, 0x84, 0x82, 0x97, 0x83, 0x27, 0xc4, 0xc3, 0x26, 0x85,
    0xdc, 0x47, 0x82, 0x97, 0xb7, 0x47, 0x04, 0x08, 0x93, 0x87, 0x07, 0xe0, 0x32, 0x46, 0xa2, 0x46,
    0x3e, 0x95, 0x0a, 0x05, 0x1c, 0x41, 0x13, 0x97, 0x67, 0x00, 0xe3, 0x4d, 0x07, 0xfe, 0xc2, 0x06,
    0x22, 0x06, 0xc5, 0x8e, 0xd1, 0x8e, 0x37, 0x06, 0x00, 0x05, 0xd1, 0x8e, 0x14, 0xc1, 0x1c, 0x41,
    0x13, 0x97, 0x67, 0x00, 0xe3, 0x4d, 0x07, 0xfe, 0x83, 0x27, 0xc4, 0xc3, 0x62, 0x44, 0xf2, 0x40,
    0xd2, 0x44, 0x03, 0xa3, 0x47, 0x00, 0x05, 0x61, 0x02, 0x83,
];

#[derive(Clone, Copy)]
pub(super) struct HostIdSummary {
    pub(super) name: &'static str,
    pub(super) address: u32,
    pub(super) body: &'static [u8],
    pub(super) and_mask: u32,
    pub(super) or_mask: u32,
    pub(super) branch_offset: u32,
}

pub(super) const HOST_ID_SUMMARIES: [HostIdSummary; 2] = [
    HostIdSummary {
        name: "phy_get_i2c_hostid_",
        address: 0x2f82_9fc0,
        body: PHY_GET_I2C_HOSTID_ROM_BODY,
        and_mask: 0xffff_e00f,
        or_mask: 0x0000_1a00,
        branch_offset: 0x0c,
    },
    HostIdSummary {
        name: "phy_get_i2c_hostid_new",
        address: 0x1000_732a,
        body: PHY_GET_I2C_HOSTID_NEW_BODY,
        and_mask: 0xfffc_000f,
        or_mask: 0x0003_fa00,
        branch_offset: 0x0a,
    },
];

pub(super) fn phy_table_targets(
    pointer_context: &StructuralPointerContext,
) -> Option<(u32, u32, u32)> {
    [
        entry_contract::PHY_COLD_TABLE,
        entry_contract::PHY_REGISTERED_TABLE,
    ]
    .into_iter()
    .find_map(|table| {
        Some((
            *pointer_context.function_table_slots.get(&(table, 0x00))?,
            *pointer_context.function_table_slots.get(&(table, 0x04))?,
            *pointer_context.function_table_slots.get(&(table, 0x0c))?,
        ))
    })
}

pub(super) fn chip_i2c_write_reg_trace(
    symbol: &artifact::ArtifactSymbolDefinition,
    svd: &MmioMap,
    pointer_context: &StructuralPointerContext,
) -> Option<FunctionAnalysis> {
    let (enter_target, exit_target, host_id_target) = phy_table_targets(pointer_context)?;
    let registers = [0x2010_f800, 0x2010_f804]
        .into_iter()
        .map(|address| {
            let register = svd.register(address)?;
            Some(IndexedMmioRegister {
                address,
                name: register.name.clone(),
            })
        })
        .collect::<Option<Vec<_>>>()?;
    let arguments: Box<Rv32CallArguments> = Box::new(core::array::from_fn(|index| {
        SymbolicValue::input(index as u8)
    }));
    let host = SymbolicValue::CallResult(1);
    let host_address = host.clone().shift_left(2).add_constant(0x2010_f800);
    let guard = Some(IndexedMmioGuard {
        selector: host,
        maximum: 1,
    });
    let poll = || DraftReferenceEvent::PollMmio {
        width: 32,
        address: host_address.clone(),
        registers: registers.clone(),
        guard: guard.clone(),
        mask: 0x0200_0000,
        expected: 0,
    };
    let poll_before = poll();
    let poll_after = poll();
    let command = SymbolicValue::input(3)
        .shift_left(16)
        .symbolic_bitor(SymbolicValue::input(0))
        .symbolic_bitor(SymbolicValue::input(2).shift_left(8))
        .or(0x0500_0000);
    let mut exit_arguments: Rv32CallArguments = core::array::from_fn(|_| SymbolicValue::Unknown);
    exit_arguments[0] = host_address.clone();
    Some(FunctionAnalysis {
        symbol: symbol.name.clone(),
        events: Vec::new(),
        located_events: Vec::new(),
        reference_events: Vec::new(),
        reference_dependencies: Vec::new(),
        blockers: Vec::new(),
        reference_blockers: Vec::new(),
        return_value: SymbolicValue::Unknown,
        reference_flow: Some(DraftReferenceFlow {
            events: vec![
                DraftReferenceEvent::Call {
                    token: 0,
                    site: 0x2f82_a326,
                    target: enter_target,
                    arguments: arguments.clone(),
                },
                DraftReferenceEvent::Call {
                    token: 1,
                    site: 0x2f82_a330,
                    target: host_id_target,
                    arguments,
                },
                poll_before,
                DraftReferenceEvent::IndexedMmio {
                    access: MemoryAccess::Write,
                    width: 32,
                    address: host_address,
                    registers,
                    guard,
                    value: Some(command),
                },
                poll_after,
                DraftReferenceEvent::TailCall {
                    token: 2,
                    site: 0x2f82_a376,
                    target: exit_target,
                    arguments: Box::new(exit_arguments),
                },
            ],
            terminator: DraftReferenceTerminator::Return(SymbolicValue::Unknown),
        }),
        unresolved_branch: None,
    })
}

pub(super) fn chip_i2c_read_reg_org_trace(
    symbol: &artifact::ArtifactSymbolDefinition,
    svd: &MmioMap,
) -> Option<FunctionAnalysis> {
    const ANA_CONF1: u32 = 0x2010_f81c;

    let host_address = SymbolicValue::input(2)
        .shift_left(2)
        .add_constant(0x2010_f800);
    let domain = crate::indexed_mmio_domain(&host_address, svd)?;
    let command = SymbolicValue::input(3)
        .shift_left(8)
        .symbolic_bitor(SymbolicValue::input(0))
        .or(0x0400_0000);
    let mask_write = ObservableEvent::Memory {
        access: MemoryAccess::Write,
        width: 32,
        address: ANA_CONF1,
        register: svd.display_register_name(ANA_CONF1),
        value: Some(SymbolicValue::input(1).xor(u32::MAX)),
    };
    let indexed_write = DraftReferenceEvent::IndexedMmio {
        access: MemoryAccess::Write,
        width: 32,
        address: host_address.clone(),
        registers: domain.registers.clone(),
        guard: domain.guard.clone(),
        value: Some(command),
    };
    let poll = DraftReferenceEvent::PollMmio {
        width: 32,
        address: host_address.clone(),
        registers: domain.registers.clone(),
        guard: domain.guard.clone(),
        mask: 0x0200_0000,
        expected: 0,
    };
    let final_read = DraftReferenceEvent::IndexedMmio {
        access: MemoryAccess::Read,
        width: 32,
        address: host_address,
        registers: domain.registers,
        guard: domain.guard,
        value: None,
    };
    let return_value = SymbolicValue::indexed_register_read(0, 32, false)
        .shift_right(16)
        .and(0xff);
    Some(FunctionAnalysis {
        symbol: symbol.name.clone(),
        events: vec![mask_write.clone()],
        located_events: Vec::new(),
        reference_events: Vec::new(),
        reference_dependencies: Vec::new(),
        blockers: Vec::new(),
        reference_blockers: Vec::new(),
        return_value: SymbolicValue::Unknown,
        reference_flow: Some(DraftReferenceFlow {
            events: vec![
                DraftReferenceEvent::Observable(mask_write),
                indexed_write,
                poll,
                final_read,
            ],
            terminator: DraftReferenceTerminator::Return(return_value),
        }),
        unresolved_branch: None,
    })
}

pub(super) fn host_id_trace(
    symbol: &artifact::ArtifactSymbolDefinition,
    svd: &MmioMap,
    summary: HostIdSummary,
) -> FunctionAnalysis {
    const HOST_SELECT_REGISTER: u32 = 0x2010_f820;
    const HOST_ONE_BLOCKS: [u32; 6] = [0x61, 0x62, 0x63, 0x67, 0x6a, 0x6b];

    let selector = SymbolicValue::input(0).and(0xff);
    let host_one = HOST_ONE_BLOCKS
        .into_iter()
        .map(|block| selector.clone().xor(block).seqz())
        .fold(SymbolicValue::Constant(0), SymbolicValue::symbolic_bitor);
    let read_value = SymbolicValue::register_read(0, HOST_SELECT_REGISTER, 32, false);
    let events = vec![
        ObservableEvent::Memory {
            access: MemoryAccess::Read,
            width: 32,
            address: HOST_SELECT_REGISTER,
            register: svd.display_register_name(HOST_SELECT_REGISTER),
            value: None,
        },
        ObservableEvent::Memory {
            access: MemoryAccess::Write,
            width: 32,
            address: HOST_SELECT_REGISTER,
            register: svd.display_register_name(HOST_SELECT_REGISTER),
            value: Some(read_value.and(summary.and_mask).or(summary.or_mask)),
        },
    ];
    let reference_events = events
        .iter()
        .cloned()
        .map(DraftReferenceEvent::Observable)
        .collect();
    FunctionAnalysis {
        symbol: symbol.name.clone(),
        events,
        located_events: Vec::new(),
        reference_events: Vec::new(),
        reference_dependencies: Vec::new(),
        blockers: Vec::new(),
        reference_blockers: Vec::new(),
        return_value: SymbolicValue::Unknown,
        reference_flow: Some(DraftReferenceFlow {
            events: reference_events,
            terminator: DraftReferenceTerminator::Branch {
                condition: BranchCondition {
                    site: summary.address + summary.branch_offset,
                    operation: BranchOperation::NotEqual,
                    left: host_one,
                    right: SymbolicValue::Constant(0),
                },
                taken: Box::new(DraftReferenceFlow {
                    events: Vec::new(),
                    terminator: DraftReferenceTerminator::Return(SymbolicValue::Constant(1)),
                }),
                not_taken: Box::new(DraftReferenceFlow {
                    events: Vec::new(),
                    terminator: DraftReferenceTerminator::Return(SymbolicValue::Constant(0)),
                }),
            },
        }),
        unresolved_branch: None,
    }
}
