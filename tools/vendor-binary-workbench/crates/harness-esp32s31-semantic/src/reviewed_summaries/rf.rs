//! Reviewed RFPLL, frequency-offset, and IQ-estimator summaries.

use super::body_identity::*;
use super::*;

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

pub(super) fn exact_rfpll_calibration_poll(symbol: &artifact::ArtifactSymbolDefinition) -> bool {
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

pub(super) fn exact_rfpll_cap_calibration_search(
    symbol: &artifact::ArtifactSymbolDefinition,
) -> bool {
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

pub(super) fn exact_rf_frequency_offset_scratch_wrapper(
    symbol: &artifact::ArtifactSymbolDefinition,
) -> bool {
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

pub(super) fn exact_iq_estimator_poll(symbol: &artifact::ArtifactSymbolDefinition) -> bool {
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

pub(super) fn rf_frequency_offset_scratch_trace(
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

pub(super) fn iq_estimator_poll_trace(
    symbol: &artifact::ArtifactSymbolDefinition,
    svd: &MmioMap,
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
            register: svd.display_register_name(address),
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

pub(super) fn one_call_flow(
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

pub(super) fn rfpll_cap_calibration_search_trace(
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

pub(super) fn rfpll_calibration_poll_trace(
    symbol: &artifact::ArtifactSymbolDefinition,
) -> FunctionAnalysis {
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
