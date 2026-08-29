//! Reviewed non-returning controller assertion paths.

use super::*;

pub(super) const BTDM_ASSERT_BODY: &[u8] = &[
    0x8d, 0x47, // li a5, 3
    0x63, 0x03, 0xf7, 0x00, // beq a4, a5, +6
    0x82, 0x80, // ret
    0x23, 0x20, 0x00, 0x00, // sw zero, 0(zero)
    0x02, 0x90, // ebreak
];

pub(super) const BLE_BTDM_ASSERT_ADDRESS: u64 = 0x1005_5030;
pub(super) const BREDR_BTDM_ASSERT_ADDRESS: u64 = 0x1003_d6aa;

pub(super) fn exact_btdm_assert(symbol: &artifact::ArtifactSymbolDefinition) -> bool {
    symbol.member.is_none()
        && symbol.name == "wr_btdm_assert"
        && symbol.addresses_resolved
        && matches!(
            symbol.address,
            BLE_BTDM_ASSERT_ADDRESS | BREDR_BTDM_ASSERT_ADDRESS
        )
        && symbol.bytes == BTDM_ASSERT_BODY
        && symbol.relocations.is_empty()
}

pub(super) fn btdm_assert_trace(symbol: &artifact::ArtifactSymbolDefinition) -> FunctionAnalysis {
    let function_address = symbol.address as u32;
    let fail_stop = DraftReferenceFlow {
        events: Vec::new(),
        terminator: DraftReferenceTerminator::FailStop {
            // The reviewed non-returning sequence starts at the deliberate
            // null store and ends with `ebreak` four bytes later.
            site: function_address + 8,
            function: symbol.name.clone(),
            argument_count: 0,
            arguments: Box::new([]),
        },
    };
    let normal_return = DraftReferenceFlow {
        events: Vec::new(),
        terminator: DraftReferenceTerminator::Return(SymbolicValue::Unknown),
    };

    FunctionAnalysis {
        symbol: symbol.name.clone(),
        events: Vec::new(),
        located_events: Vec::new(),
        located_reference_events: Vec::new(),
        reference_events: Vec::new(),
        reference_dependencies: Vec::new(),
        blockers: Vec::new(),
        reference_blockers: Vec::new(),
        return_value: SymbolicValue::Unknown,
        reference_flow: Some(DraftReferenceFlow {
            events: Vec::new(),
            terminator: DraftReferenceTerminator::Branch {
                condition: BranchCondition {
                    site: function_address + 2,
                    operation: BranchOperation::Equal,
                    left: SymbolicValue::input(4),
                    right: SymbolicValue::Constant(3),
                },
                taken: Box::new(fail_stop),
                not_taken: Box::new(normal_return),
            },
        }),
        unresolved_branch: None,
    }
}
