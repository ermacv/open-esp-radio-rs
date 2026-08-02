//! Reviewed semantic summaries for effects that cannot yet be reconstructed
//! from the structural instruction trace alone.
//!
//! Vendor summaries are selected by the platform harness from an exact symbol
//! name, load address and size. Artifact authentication is a caller-owned
//! precondition; the validator does not embed expected body digests.

use crate::*;

const MAX_REVIEWED_MEMORY_INTRINSIC_BYTES: u32 = 256;
const ROM_DIVDI3_SIZE: usize = 926;
const ROM_DIVDI3_ADDRESS: u32 = crate::wide_signed_divide_target_address();
const ROM_PHY_WAIT_RFPLL_CAL_END_ADDRESS: u64 = 0x2f82_5874;
const ROM_PHY_WAIT_RFPLL_CAL_END_SIZE: usize = 86;
const ROM_PHY_I2C_READ_REG_MASK_ADDRESS: u32 = 0x2f82_a37c;
const ROM_RFPLL_TIMEOUT_MESSAGE_ADDRESS: u32 = 0x2f84_d9cc;
const ROM_PHY_RFPLL_CAP_INIT_CAL_ADDRESS: u64 = 0x2f82_5ada;
const ROM_PHY_RFPLL_CAP_INIT_CAL_SIZE: usize = 192;
const ROM_PHY_READ_PLL_CAP_ADDRESS: u32 = 0x2f82_5a32;
const ROM_PHY_WRITE_PLL_CAP_ADDRESS: u32 = 0x2f82_59f2;
const ROM_PHY_I2C_READ_REG_ADDRESS: u32 = 0x2f82_a30a;
const ROM_PHY_I2C_WRITE_REG_MASK_ADDRESS: u32 = 0x2f82_a3a8;
const ROM_PHY_SET_RF_FREQ_OFFSET_ADDRESS: u64 = 0x2f82_5c10;
const ROM_PHY_SET_RF_FREQ_OFFSET_SIZE: usize = 16;
const ROM_PHY_SET_RFPLL_FREQ_ADDRESS: u32 = 0x2f82_5b9a;
const ROM_PHY_IQ_EST_ENABLE_ADDRESS: u64 = 0x2f82_89d4;
const ROM_PHY_IQ_EST_ENABLE_SIZE: usize = 180;

const PHY_GET_I2C_HOSTID_ROM_BODY: &[u8] = &[
    0x13, 0x05, 0xf5, 0xf9, 0x93, 0x77, 0xf5, 0x0f, 0x29, 0x47, 0x01, 0x45, 0x63, 0x69, 0xf7, 0x00,
    0x05, 0x45, 0x33, 0x15, 0xf5, 0x00, 0x13, 0x75, 0x75, 0x64, 0x33, 0x35, 0xa0, 0x00, 0xb7, 0x06,
    0x11, 0x20, 0x83, 0xa7, 0x06, 0x82, 0x79, 0x77, 0x3d, 0x07, 0xf9, 0x8f, 0x09, 0x67, 0x13, 0x07,
    0x07, 0xa0, 0xd9, 0x8f, 0x23, 0xa0, 0xf6, 0x82, 0x82, 0x80,
];

const PHY_GET_I2C_HOSTID_NEW_BODY: &[u8] = &[
    0x13, 0x05, 0xf5, 0xf9, 0x13, 0x75, 0xf5, 0x0f, 0xa9, 0x47, 0x63, 0xea, 0xa7, 0x00, 0xb7, 0x87,
    0x00, 0x10, 0x93, 0x87, 0xc7, 0x1d, 0x0a, 0x05, 0x3e, 0x95, 0x1c, 0x41, 0x82, 0x87, 0x01, 0x45,
    0xb7, 0x06, 0x11, 0x20, 0x83, 0xa7, 0x06, 0x82, 0x37, 0x07, 0xfc, 0xff, 0x3d, 0x07, 0xf9, 0x8f,
    0x37, 0x07, 0x04, 0x00, 0x13, 0x07, 0x07, 0xa0, 0xd9, 0x8f, 0x23, 0xa0, 0xf6, 0x82, 0x82, 0x80,
    0x05, 0x45, 0xf9, 0xbf,
];

const PHY_CHIP_I2C_READ_REG_ORG_BODY: &[u8] = &[
    0xb7, 0x07, 0x11, 0x20, 0x93, 0xc5, 0xf5, 0xff, 0x23, 0xae, 0xb7, 0x80, 0xb7, 0x47, 0x04, 0x08,
    0x93, 0x87, 0x07, 0xe0, 0xa2, 0x06, 0xc9, 0x8e, 0x3e, 0x96, 0x37, 0x05, 0x00, 0x04, 0x0a, 0x06,
    0xc9, 0x8e, 0x14, 0xc2, 0x1c, 0x42, 0x13, 0x97, 0x67, 0x00, 0xe3, 0x4d, 0x07, 0xfe, 0x08, 0x42,
    0x41, 0x81, 0x13, 0x75, 0xf5, 0x0f, 0x82, 0x80,
];

const PHY_CHIP_I2C_WRITE_REG_BODY: &[u8] = &[
    0x01, 0x11, 0x22, 0xcc, 0x37, 0x04, 0x08, 0x2f, 0x83, 0x27, 0xc4, 0xc3, 0x06, 0xce, 0x26, 0xca,
    0x9c, 0x43, 0x32, 0xc6, 0x36, 0xc4, 0xaa, 0x84, 0x82, 0x97, 0x83, 0x27, 0xc4, 0xc3, 0x26, 0x85,
    0xdc, 0x47, 0x82, 0x97, 0xb7, 0x47, 0x04, 0x08, 0x93, 0x87, 0x07, 0xe0, 0x32, 0x46, 0xa2, 0x46,
    0x3e, 0x95, 0x0a, 0x05, 0x1c, 0x41, 0x13, 0x97, 0x67, 0x00, 0xe3, 0x4d, 0x07, 0xfe, 0xc2, 0x06,
    0x22, 0x06, 0xc5, 0x8e, 0xd1, 0x8e, 0x37, 0x06, 0x00, 0x05, 0xd1, 0x8e, 0x14, 0xc1, 0x1c, 0x41,
    0x13, 0x97, 0x67, 0x00, 0xe3, 0x4d, 0x07, 0xfe, 0x83, 0x27, 0xc4, 0xc3, 0x62, 0x44, 0xf2, 0x40,
    0xd2, 0x44, 0x03, 0xa3, 0x47, 0x00, 0x05, 0x61, 0x02, 0x83,
];

#[derive(Clone, Copy)]
struct HostIdSummary {
    name: &'static str,
    address: u32,
    body: &'static [u8],
    and_mask: u32,
    or_mask: u32,
    branch_offset: u32,
}

const HOST_ID_SUMMARIES: [HostIdSummary; 2] = [
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

fn exact_standard_memory_intrinsic(symbol: &artifact::ArtifactSymbolDefinition) -> bool {
    matches!(
        (symbol.name.as_str(), symbol.address, symbol.bytes.len()),
        ("memcpy", 0x2f80_d260, 224) | ("memset", 0x2f82_20c6, 168)
    )
}

struct ReviewedBodyIdentity<'a> {
    name: &'a str,
    address: u64,
    size: usize,
}

fn reviewed_identity_matches(
    actual: ReviewedBodyIdentity<'_>,
    expected: ReviewedBodyIdentity<'_>,
) -> bool {
    actual.name == expected.name
        && actual.address == expected.address
        && actual.size == expected.size
}

fn exact_wide_signed_divide(symbol: &artifact::ArtifactSymbolDefinition) -> bool {
    reviewed_identity_matches(
        ReviewedBodyIdentity {
            name: &symbol.name,
            address: symbol.address,
            size: symbol.bytes.len(),
        },
        ReviewedBodyIdentity {
            name: "__divdi3",
            address: u64::from(ROM_DIVDI3_ADDRESS),
            size: ROM_DIVDI3_SIZE,
        },
    )
}

fn exact_rfpll_calibration_poll(symbol: &artifact::ArtifactSymbolDefinition) -> bool {
    reviewed_identity_matches(
        ReviewedBodyIdentity {
            name: &symbol.name,
            address: symbol.address,
            size: symbol.bytes.len(),
        },
        ReviewedBodyIdentity {
            name: "phy_wait_rfpll_cal_end",
            address: ROM_PHY_WAIT_RFPLL_CAL_END_ADDRESS,
            size: ROM_PHY_WAIT_RFPLL_CAL_END_SIZE,
        },
    )
}

fn exact_rfpll_cap_calibration_search(symbol: &artifact::ArtifactSymbolDefinition) -> bool {
    reviewed_identity_matches(
        ReviewedBodyIdentity {
            name: &symbol.name,
            address: symbol.address,
            size: symbol.bytes.len(),
        },
        ReviewedBodyIdentity {
            name: "phy_rfpll_cap_init_cal",
            address: ROM_PHY_RFPLL_CAP_INIT_CAL_ADDRESS,
            size: ROM_PHY_RFPLL_CAP_INIT_CAL_SIZE,
        },
    )
}

fn exact_rf_frequency_offset_scratch_wrapper(symbol: &artifact::ArtifactSymbolDefinition) -> bool {
    reviewed_identity_matches(
        ReviewedBodyIdentity {
            name: &symbol.name,
            address: symbol.address,
            size: symbol.bytes.len(),
        },
        ReviewedBodyIdentity {
            name: "phy_set_rf_freq_offset",
            address: ROM_PHY_SET_RF_FREQ_OFFSET_ADDRESS,
            size: ROM_PHY_SET_RF_FREQ_OFFSET_SIZE,
        },
    )
}

fn exact_iq_estimator_poll(symbol: &artifact::ArtifactSymbolDefinition) -> bool {
    reviewed_identity_matches(
        ReviewedBodyIdentity {
            name: &symbol.name,
            address: symbol.address,
            size: symbol.bytes.len(),
        },
        ReviewedBodyIdentity {
            name: "phy_iq_est_enable",
            address: ROM_PHY_IQ_EST_ENABLE_ADDRESS,
            size: ROM_PHY_IQ_EST_ENABLE_SIZE,
        },
    )
}

fn rf_frequency_offset_scratch_trace(
    symbol: &artifact::ArtifactSymbolDefinition,
) -> FunctionAnalysis {
    let mut arguments: Rv32CallArguments = core::array::from_fn(|_| SymbolicValue::Unknown);
    for (index, argument) in arguments.iter_mut().enumerate().take(3) {
        *argument = SymbolicValue::input(index as u8);
    }
    FunctionAnalysis {
        symbol: symbol.name.clone(),
        events: Vec::new(),
        reference_events: Vec::new(),
        reference_dependencies: Vec::new(),
        blockers: Vec::new(),
        reference_blockers: Vec::new(),
        return_value: SymbolicValue::Unknown,
        reference_flow: Some(DraftReferenceFlow {
            events: vec![DraftReferenceEvent::ScratchCall {
                token: 0,
                site: 0x2f82_5c16,
                target: ROM_PHY_SET_RFPLL_FREQ_ADDRESS,
                arguments: Box::new(arguments),
                scratch_argument: 3,
                scratch_size: 5,
            }],
            terminator: DraftReferenceTerminator::Return(SymbolicValue::CallResult(0)),
        }),
        unresolved_branch: None,
    }
}

fn iq_estimator_poll_trace(
    symbol: &artifact::ArtifactSymbolDefinition,
    svd: &MmioRegisterMap,
    pointer_context: &StructuralPointerContext,
) -> Option<FunctionAnalysis> {
    const CONFIG0: u32 = 0x2010_044c;
    const CONFIG1: u32 = 0x2010_0450;
    const DONE: u32 = 0x2010_047c;
    const STATUS: u32 = 0x2010_08d0;

    for address in [CONFIG0, CONFIG1, DONE, STATUS] {
        svd.register(address)?;
    }
    let parameter = pointer_context
        .data_pointer_cells
        .values()
        .find(|value| {
            matches!(
                value,
                SymbolicValue::SymbolAddress { symbol, .. }
                    if symbol == entry_contract::LINKED_PHY_PARAM_SYMBOL
            )
        })?
        .clone();
    let counter_address = parameter.add_constant(0x1ac);
    let mmio = |access, address, value| {
        DraftReferenceEvent::Observable(ObservableEvent::Memory {
            access,
            width: 32,
            address,
            register: svd.register_name(address),
            value,
        })
    };

    let increment = DraftReferenceFlow {
        events: vec![
            DraftReferenceEvent::Memory {
                access: MemoryAccess::Read,
                width: 16,
                address: counter_address.clone(),
                region: "registered phy_param IQ-estimator counter".to_owned(),
                value: None,
            },
            DraftReferenceEvent::Memory {
                access: MemoryAccess::Write,
                width: 16,
                address: counter_address.clone(),
                region: "registered phy_param IQ-estimator counter".to_owned(),
                value: Some(SymbolicValue::memory_read(0, 16, false).add_constant(1)),
            },
        ],
        terminator: DraftReferenceTerminator::Return(SymbolicValue::Constant(0)),
    };
    let keep_polling = DraftReferenceFlow {
        events: vec![mmio(MemoryAccess::Read, STATUS, None)],
        terminator: DraftReferenceTerminator::Branch {
            condition: BranchCondition {
                site: 0x2f82_8a7a,
                operation: BranchOperation::NotEqual,
                left: SymbolicValue::register_read(1, STATUS, 32, false)
                    .shift_right(20)
                    .and(3),
                right: SymbolicValue::Constant(0),
            },
            taken: Box::new(increment),
            not_taken: Box::new(DraftReferenceFlow {
                events: Vec::new(),
                terminator: DraftReferenceTerminator::Return(SymbolicValue::Constant(0)),
            }),
        },
    };
    let poll_body = DraftReferenceFlow {
        events: vec![mmio(MemoryAccess::Read, DONE, None)],
        terminator: DraftReferenceTerminator::Branch {
            condition: BranchCondition {
                site: 0x2f82_8a64,
                operation: BranchOperation::NotEqual,
                left: SymbolicValue::register_read(0, DONE, 32, false).and(0x0001_0000),
                right: SymbolicValue::Constant(0),
            },
            taken: Box::new(DraftReferenceFlow {
                events: Vec::new(),
                terminator: DraftReferenceTerminator::Return(SymbolicValue::Constant(1)),
            }),
            not_taken: Box::new(keep_polling),
        },
    };

    let events = vec![
        DraftReferenceEvent::Memory {
            access: MemoryAccess::Write,
            width: 16,
            address: counter_address,
            region: "registered phy_param IQ-estimator counter".to_owned(),
            value: Some(SymbolicValue::Constant(0)),
        },
        mmio(MemoryAccess::Read, CONFIG0, None),
        mmio(
            MemoryAccess::Write,
            CONFIG0,
            Some(
                SymbolicValue::register_read(0, CONFIG0, 32, false)
                    .and(0xf3ff_ffff)
                    .or(0x0400_0000),
            ),
        ),
        mmio(MemoryAccess::Read, CONFIG1, None),
        mmio(
            MemoryAccess::Write,
            CONFIG1,
            Some(
                SymbolicValue::register_read(1, CONFIG1, 32, false)
                    .and(0xffe7_ffff)
                    .or(0x0010_0000),
            ),
        ),
        mmio(MemoryAccess::Read, CONFIG1, None),
        mmio(
            MemoryAccess::Write,
            CONFIG1,
            Some(
                SymbolicValue::register_read(2, CONFIG1, 32, false)
                    .and(0xfffe_0003)
                    .symbolic_bitor(SymbolicValue::input(1).shift_left(2).and(0x0001_fffc)),
            ),
        ),
        mmio(MemoryAccess::Read, CONFIG1, None),
        mmio(
            MemoryAccess::Write,
            CONFIG1,
            Some(SymbolicValue::register_read(3, CONFIG1, 32, false).or(1)),
        ),
        DraftReferenceEvent::DelayMicros {
            micros: SymbolicValue::Constant(1),
        },
        mmio(MemoryAccess::Read, CONFIG1, None),
        mmio(
            MemoryAccess::Write,
            CONFIG1,
            Some(SymbolicValue::register_read(4, CONFIG1, 32, false).or(2)),
        ),
        DraftReferenceEvent::PollFlow {
            body: Box::new(poll_body),
            exit_when_mask: 1,
            exit_when_expected: 1,
        },
    ];

    Some(FunctionAnalysis {
        symbol: symbol.name.clone(),
        events: Vec::new(),
        reference_events: Vec::new(),
        reference_dependencies: vec!["ets_delay_us".to_owned()],
        blockers: Vec::new(),
        reference_blockers: Vec::new(),
        return_value: SymbolicValue::Unknown,
        reference_flow: Some(DraftReferenceFlow {
            events,
            terminator: DraftReferenceTerminator::Return(SymbolicValue::Unknown),
        }),
        unresolved_branch: None,
    })
}

fn one_call_flow(
    site: u32,
    target: u32,
    arguments: Rv32CallArguments,
    return_modeled: bool,
) -> DraftReferenceFlow {
    DraftReferenceFlow {
        events: vec![DraftReferenceEvent::Call {
            token: 0,
            site,
            target,
            arguments: Box::new(arguments),
        }],
        terminator: DraftReferenceTerminator::Return(if return_modeled {
            SymbolicValue::CallResult(0)
        } else {
            SymbolicValue::Unknown
        }),
    }
}

fn rfpll_cap_calibration_search_trace(
    symbol: &artifact::ArtifactSymbolDefinition,
) -> FunctionAnalysis {
    let no_arguments: Rv32CallArguments = core::array::from_fn(|_| SymbolicValue::Unknown);

    let mut setup_arguments = no_arguments.clone();
    for (index, value) in [0x62, 1, 0x0b, 6, 6, 1].into_iter().enumerate() {
        setup_arguments[index] = SymbolicValue::Constant(value);
    }

    let mut writer_arguments = no_arguments.clone();
    writer_arguments[0] = SymbolicValue::input(0);

    let mut sample_arguments = no_arguments.clone();
    for (index, value) in [0x62, 1, 0x0c].into_iter().enumerate() {
        sample_arguments[index] = SymbolicValue::Constant(value);
    }

    FunctionAnalysis {
        symbol: symbol.name.clone(),
        events: Vec::new(),
        reference_events: Vec::new(),
        reference_dependencies: vec!["ets_delay_us".to_owned()],
        blockers: Vec::new(),
        reference_blockers: Vec::new(),
        return_value: SymbolicValue::Unknown,
        reference_flow: Some(DraftReferenceFlow {
            events: vec![DraftReferenceEvent::SymmetricCalibrationSearch {
                token: 0,
                attempts_per_direction: 10,
                settle_micros: 5,
                sample_shift: 2,
                sample_mask: 3,
                accepted_sample: 0,
                initial_read: Box::new(one_call_flow(
                    0x2f82_5aee,
                    ROM_PHY_READ_PLL_CAP_ADDRESS,
                    no_arguments,
                    true,
                )),
                setup: Box::new(one_call_flow(
                    0x2f82_5b02,
                    ROM_PHY_I2C_WRITE_REG_MASK_ADDRESS,
                    setup_arguments,
                    false,
                )),
                write_candidate: Box::new(one_call_flow(
                    0x2f82_5b2a,
                    ROM_PHY_WRITE_PLL_CAP_ADDRESS,
                    writer_arguments,
                    false,
                )),
                sample: Box::new(one_call_flow(
                    0x2f82_5b3c,
                    ROM_PHY_I2C_READ_REG_ADDRESS,
                    sample_arguments,
                    true,
                )),
            }],
            terminator: DraftReferenceTerminator::Return(SymbolicValue::CallResult(0)),
        }),
        unresolved_branch: None,
    }
}

fn rfpll_calibration_poll_trace(symbol: &artifact::ArtifactSymbolDefinition) -> FunctionAnalysis {
    let mut read_arguments: Rv32CallArguments = core::array::from_fn(|_| SymbolicValue::Unknown);
    for (index, value) in [0x62, 1, 7, 1, 1].into_iter().enumerate() {
        read_arguments[index] = SymbolicValue::Constant(value);
    }
    let mut diagnostic_arguments: [SymbolicValue; 8] =
        core::array::from_fn(|_| SymbolicValue::Unknown);
    diagnostic_arguments[0] = SymbolicValue::Constant(ROM_RFPLL_TIMEOUT_MESSAGE_ADDRESS);
    let body = DraftReferenceFlow {
        events: vec![
            DraftReferenceEvent::DelayMicros {
                micros: SymbolicValue::Constant(20),
            },
            DraftReferenceEvent::Call {
                token: 0,
                site: 0x2f82_58a0,
                target: ROM_PHY_I2C_READ_REG_MASK_ADDRESS,
                arguments: Box::new(read_arguments),
            },
        ],
        terminator: DraftReferenceTerminator::Return(SymbolicValue::CallResult(0)),
    };
    FunctionAnalysis {
        symbol: symbol.name.clone(),
        events: Vec::new(),
        reference_events: Vec::new(),
        reference_dependencies: vec!["ets_delay_us".to_owned(), "ets_printf".to_owned()],
        blockers: Vec::new(),
        reference_blockers: Vec::new(),
        return_value: SymbolicValue::Unknown,
        reference_flow: Some(DraftReferenceFlow {
            events: vec![DraftReferenceEvent::BoundedPoll {
                maximum_attempts: 100,
                body: Box::new(body),
                repeat_while_mask: u32::MAX,
                repeat_while_expected: 0,
                on_exhausted: Some(Box::new(DraftReferenceEvent::DiagnosticCall {
                    function: "ets_printf".to_owned(),
                    argument_count: 1,
                    arguments: Box::new(diagnostic_arguments),
                })),
            }],
            terminator: DraftReferenceTerminator::Return(SymbolicValue::Unknown),
        }),
        unresolved_branch: None,
    }
}

pub(super) fn wide_signed_divide_intrinsic(
    symbol: &artifact::ArtifactSymbolDefinition,
    arguments: &Rv32CallArguments,
) -> Option<(SymbolicValue, SymbolicValue)> {
    exact_wide_signed_divide(symbol).then(|| {
        SymbolicValue::wide_signed_divide_words(
            arguments[0].clone(),
            arguments[1].clone(),
            arguments[2].clone(),
            arguments[3].clone(),
        )
    })
}

pub(super) fn standard_memory_intrinsic_trace(
    symbol: &artifact::ArtifactSymbolDefinition,
    arguments: &Rv32CallArguments,
) -> Option<std::result::Result<FunctionAnalysis, String>> {
    if !exact_standard_memory_intrinsic(symbol) {
        return None;
    }
    Some((|| {
        let length = arguments[2]
            .as_constant()
            .ok_or_else(|| format!("{} length is not constant", symbol.name))?;
        if length > MAX_REVIEWED_MEMORY_INTRINSIC_BYTES {
            return Err(format!(
                "{} length {length} exceeds the reviewed summary limit of {MAX_REVIEWED_MEMORY_INTRINSIC_BYTES} bytes",
                symbol.name
            ));
        }

        let mut reference_events = Vec::new();
        if symbol.name == "memcpy" {
            for offset in 0..length {
                reference_events.push(DraftReferenceEvent::Memory {
                    access: MemoryAccess::Read,
                    width: 8,
                    address: SymbolicValue::input(1).add_constant(offset),
                    region: "standard memcpy source".to_owned(),
                    value: None,
                });
            }
            for offset in 0..length {
                reference_events.push(DraftReferenceEvent::Memory {
                    access: MemoryAccess::Write,
                    width: 8,
                    address: SymbolicValue::input(0).add_constant(offset),
                    region: "standard memcpy destination".to_owned(),
                    value: Some(SymbolicValue::memory_read(offset, 8, false)),
                });
            }
        } else {
            let byte = SymbolicValue::input(1).and(0xff);
            for offset in 0..length {
                reference_events.push(DraftReferenceEvent::Memory {
                    access: MemoryAccess::Write,
                    width: 8,
                    address: SymbolicValue::input(0).add_constant(offset),
                    region: "standard memset destination".to_owned(),
                    value: Some(byte.clone()),
                });
            }
        }
        Ok(FunctionAnalysis {
            symbol: symbol.name.clone(),
            events: Vec::new(),
            reference_events,
            reference_dependencies: Vec::new(),
            blockers: Vec::new(),
            reference_blockers: Vec::new(),
            return_value: SymbolicValue::input(0),
            reference_flow: None,
            unresolved_branch: None,
        })
    })())
}

pub(super) fn reference_intrinsic_trace(
    symbol: &artifact::ArtifactSymbolDefinition,
    svd: &MmioRegisterMap,
    pointer_context: &StructuralPointerContext,
) -> Option<FunctionAnalysis> {
    let intrinsic_arguments: Rv32CallArguments =
        core::array::from_fn(|index| SymbolicValue::input(index as u8));
    if let Some((return_value, _)) = wide_signed_divide_intrinsic(symbol, &intrinsic_arguments) {
        return Some(FunctionAnalysis {
            symbol: symbol.name.clone(),
            events: Vec::new(),
            reference_events: Vec::new(),
            reference_dependencies: Vec::new(),
            blockers: Vec::new(),
            reference_blockers: Vec::new(),
            return_value,
            reference_flow: None,
            unresolved_branch: None,
        });
    }

    if exact_rfpll_calibration_poll(symbol) {
        return Some(rfpll_calibration_poll_trace(symbol));
    }

    if exact_rfpll_cap_calibration_search(symbol) {
        return Some(rfpll_cap_calibration_search_trace(symbol));
    }

    if exact_rf_frequency_offset_scratch_wrapper(symbol) {
        return Some(rf_frequency_offset_scratch_trace(symbol));
    }

    if exact_iq_estimator_poll(symbol) {
        return iq_estimator_poll_trace(symbol, svd, pointer_context);
    }

    if symbol.name == "ets_delay_us" {
        return Some(FunctionAnalysis {
            symbol: symbol.name.clone(),
            events: Vec::new(),
            reference_events: vec![DraftReferenceEvent::DelayMicros {
                micros: SymbolicValue::input(0),
            }],
            reference_dependencies: Vec::new(),
            blockers: Vec::new(),
            reference_blockers: Vec::new(),
            return_value: SymbolicValue::Unknown,
            reference_flow: None,
            unresolved_branch: None,
        });
    }

    if symbol.name == "phy_chip_i2c_readReg_org"
        && symbol.address == 0x2f82_9ffa
        && symbol.bytes == PHY_CHIP_I2C_READ_REG_ORG_BODY
    {
        return chip_i2c_read_reg_org_trace(symbol, svd);
    }

    if symbol.name == "phy_chip_i2c_writeReg"
        && symbol.address == 0x2f82_a30e
        && symbol.bytes == PHY_CHIP_I2C_WRITE_REG_BODY
    {
        return chip_i2c_write_reg_trace(symbol, svd, pointer_context);
    }

    HOST_ID_SUMMARIES
        .iter()
        .find(|summary| {
            symbol.name == summary.name
                && symbol.address == u64::from(summary.address)
                && symbol.bytes == summary.body
        })
        .map(|summary| host_id_trace(symbol, svd, *summary))
}

fn phy_table_targets(pointer_context: &StructuralPointerContext) -> Option<(u32, u32, u32)> {
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

fn chip_i2c_write_reg_trace(
    symbol: &artifact::ArtifactSymbolDefinition,
    svd: &MmioRegisterMap,
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

fn chip_i2c_read_reg_org_trace(
    symbol: &artifact::ArtifactSymbolDefinition,
    svd: &MmioRegisterMap,
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
        register: svd.register_name(ANA_CONF1),
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

fn host_id_trace(
    symbol: &artifact::ArtifactSymbolDefinition,
    svd: &MmioRegisterMap,
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
            register: svd.register_name(HOST_SELECT_REGISTER),
            value: None,
        },
        ObservableEvent::Memory {
            access: MemoryAccess::Write,
            width: 32,
            address: HOST_SELECT_REGISTER,
            register: svd.register_name(HOST_SELECT_REGISTER),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn symbol(bytes: Vec<u8>) -> artifact::ArtifactSymbolDefinition {
        artifact::ArtifactSymbolDefinition {
            member: None,
            name: "phy_get_i2c_hostid_new".to_owned(),
            address: 0x1000_732a,
            bytes,
            addresses_resolved: true,
            memory_regions: Vec::new(),
            relocations: Vec::new(),
        }
    }

    fn map() -> MmioRegisterMap {
        MmioRegisterMap {
            registers: vec![
                Register {
                    address: 0x2010_f800,
                    name: "I2C_ANA_MST.I2C0_CTRL".to_owned(),
                },
                Register {
                    address: 0x2010_f804,
                    name: "I2C_ANA_MST.I2C1_CTRL".to_owned(),
                },
                Register {
                    address: 0x2010_f81c,
                    name: "I2C_ANA_MST.ANA_CONF1".to_owned(),
                },
                Register {
                    address: 0x2010_f820,
                    name: "I2C_ANA_MST.ANA_CONF2".to_owned(),
                },
            ],
            windows: vec![Window {
                start: 0x2010_f800,
                end: 0x2010_f900,
            }],
        }
    }

    #[test]
    fn exact_host_id_body_has_a_modeled_return_and_mmio_effect() {
        let trace = reference_intrinsic_trace(
            &symbol(PHY_GET_I2C_HOSTID_NEW_BODY.to_vec()),
            &map(),
            &StructuralPointerContext::default(),
        )
        .expect("the pinned body must have a summary");

        assert!(trace.is_reference_eligible(), "{trace:#?}");
        assert!(trace.reference_exit_return_modeled());
        assert_eq!(trace.events.len(), 2);
    }

    #[test]
    fn changed_host_id_body_does_not_receive_the_reviewed_summary() {
        let mut bytes = PHY_GET_I2C_HOSTID_NEW_BODY.to_vec();
        bytes[0] ^= 1;

        assert!(
            reference_intrinsic_trace(&symbol(bytes), &map(), &StructuralPointerContext::default())
                .is_none()
        );
    }

    #[test]
    fn exact_i2c_poll_body_generates_an_explicit_busy_loop() {
        let symbol = artifact::ArtifactSymbolDefinition {
            member: None,
            name: "phy_chip_i2c_readReg_org".to_owned(),
            address: 0x2f82_9ffa,
            bytes: PHY_CHIP_I2C_READ_REG_ORG_BODY.to_vec(),
            addresses_resolved: true,
            memory_regions: Vec::new(),
            relocations: Vec::new(),
        };
        let trace =
            reference_intrinsic_trace(&symbol, &map(), &StructuralPointerContext::default())
                .expect("the pinned polling body must have a summary");

        assert!(trace.is_reference_eligible(), "{trace:#?}");
        assert!(trace.reference_exit_return_modeled());
        assert_eq!(trace.reference_indexed_mmio_count(), 3);
        let program = crate::ResolvedReferenceProgram::try_from(&trace).unwrap();
        let generated = crate::codegen::generate(&program, "rom.elf", "sha256", None, &[])
            .expect("polling summary must be code-generatable");
        assert!(generated.source.contains("// Poll until"));
        assert!(generated.source.contains("loop {"));
        assert!(
            generated
                .source
                .contains("if value & 0x02000000_u32 == 0x00000000_u32")
        );
    }

    #[test]
    fn changed_i2c_poll_body_does_not_receive_the_reviewed_summary() {
        let mut bytes = PHY_CHIP_I2C_READ_REG_ORG_BODY.to_vec();
        bytes[0] ^= 1;
        let symbol = artifact::ArtifactSymbolDefinition {
            member: None,
            name: "phy_chip_i2c_readReg_org".to_owned(),
            address: 0x2f82_9ffa,
            bytes,
            addresses_resolved: true,
            memory_regions: Vec::new(),
            relocations: Vec::new(),
        };

        assert!(
            reference_intrinsic_trace(&symbol, &map(), &StructuralPointerContext::default())
                .is_none()
        );
    }

    #[test]
    fn i2c_write_summary_requires_exact_body_and_phy_entry_contract() {
        let make_symbol = |bytes| artifact::ArtifactSymbolDefinition {
            member: None,
            name: "phy_chip_i2c_writeReg".to_owned(),
            address: 0x2f82_a30e,
            bytes,
            addresses_resolved: true,
            memory_regions: Vec::new(),
            relocations: Vec::new(),
        };
        let exact = make_symbol(PHY_CHIP_I2C_WRITE_REG_BODY.to_vec());
        assert!(
            reference_intrinsic_trace(&exact, &map(), &StructuralPointerContext::default())
                .is_none(),
            "a mutable function table must never be inferred without an entry contract"
        );

        let mut context = StructuralPointerContext::default();
        for (offset, target) in [(0x00, 1), (0x04, 2), (0x0c, 3)] {
            context
                .function_table_slots
                .insert((entry_contract::PHY_REGISTERED_TABLE, offset), target);
        }
        assert!(reference_intrinsic_trace(&exact, &map(), &context).is_some());

        let mut changed_bytes = PHY_CHIP_I2C_WRITE_REG_BODY.to_vec();
        changed_bytes[0] ^= 1;
        assert!(reference_intrinsic_trace(&make_symbol(changed_bytes), &map(), &context).is_none());
    }

    #[test]
    fn wide_divide_identity_requires_exact_name_address_and_size() {
        assert!(reviewed_identity_matches(
            ReviewedBodyIdentity {
                name: "__divdi3",
                address: u64::from(ROM_DIVDI3_ADDRESS),
                size: ROM_DIVDI3_SIZE,
            },
            ReviewedBodyIdentity {
                name: "__divdi3",
                address: u64::from(ROM_DIVDI3_ADDRESS),
                size: ROM_DIVDI3_SIZE,
            },
        ));
        assert!(!reviewed_identity_matches(
            ReviewedBodyIdentity {
                name: "__divdi3",
                address: u64::from(ROM_DIVDI3_ADDRESS),
                size: ROM_DIVDI3_SIZE - 1,
            },
            ReviewedBodyIdentity {
                name: "__divdi3",
                address: u64::from(ROM_DIVDI3_ADDRESS),
                size: ROM_DIVDI3_SIZE,
            },
        ));
    }
}
