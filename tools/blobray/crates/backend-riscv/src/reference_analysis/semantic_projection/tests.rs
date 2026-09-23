use super::*;
use crate::{ExternalReturnModel, ExternalSemanticSpec, SemanticFunctionBodyPolicy};

const RETURN: [u8; 4] = [0x67, 0x80, 0, 0];
static SEMANTIC: DirectSemanticFunctionSpec = DirectSemanticFunctionSpec {
    id: "test-reviewed-body",
    source: "test-provider",
    c_name: "reviewed",
    argument_count: 0,
    body_policy: SemanticFunctionBodyPolicy::AnalyzeBody,
    return_model: ExternalReturnModel::Unmodeled,
    semantic: ExternalSemanticSpec {
        operation: "test.return",
        arguments: &[],
        return_type: "void",
        replacement: None,
        event_dispatch: None,
    },
    evidence: "synthetic exact return body",
};
static OTHER: DirectSemanticFunctionSpec = DirectSemanticFunctionSpec {
    id: "conflicting-reviewed-body",
    body_policy: SemanticFunctionBodyPolicy::OpaqueBoundary,
    ..SEMANTIC
};
static HOOKS: RiscvSummaryHooks = RiscvSummaryHooks {
    secondary_return_target: |_| false,
    direct_semantic: |symbol| {
        (!symbol.addresses_resolved && symbol.name == "reviewed" && symbol.bytes == RETURN)
            .then_some(&SEMANTIC)
    },
    direct_external_semantic: |_| None,
    direct_external_intrinsic: |_, _| None,
    reference_intrinsic: |_, _, _| None,
    caller_memory_input_domain: |_, _, _| None,
    standard_memory_function: |_| None,
    wide_signed_divide: |_, _| None,
};

fn pair(
    bytes: &[u8],
) -> (
    artifact::ArtifactSymbolDefinition,
    artifact::ArtifactSymbolDefinition,
) {
    let origin = artifact::ArtifactSymbolDefinition {
        identity: artifact::ArtifactSymbolDefinition::synthetic_identity(
            module_path!(),
            &(Some("member.o".into())),
            "reviewed",
            0,
        ),
        member: Some("member.o".into()),
        name: "reviewed".into(),
        address: 0,
        bytes: bytes.to_vec(),
        addresses_resolved: false,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let linked = artifact::ArtifactSymbolDefinition {
        identity: artifact::ArtifactSymbolDefinition::synthetic_identity(
            module_path!(),
            &None,
            "reviewed",
            0x1000,
        ),
        member: None,
        address: 0x1000,
        addresses_resolved: true,
        ..origin.clone()
    };
    (origin, linked)
}

#[test]
fn same_name_different_body_remains_a_queryable_candidate() {
    let (origin, mut linked) = pair(&RETURN);
    linked.bytes.splice(..0, [0x13, 0x05, 0x10, 0x00]); // li a0, 1
    let mut catalog = SemanticProjectionCatalog::default();
    assert_eq!(
        catalog.observe(&origin, &linked, &HOOKS),
        Some(SemanticProjectionStatus::Unverified {
            gap: SemanticProjectionGap::DifferentBody
        })
    );
    assert!(catalog.semantic(&linked).is_none());
    assert_eq!(
        catalog.gaps(&linked),
        [SemanticProjectionGap::DifferentBody]
    );
}

#[test]
fn proof_is_bound_to_body_relocations_and_address_domain() {
    let (origin, linked) = pair(&RETURN);
    let mut catalog = SemanticProjectionCatalog::default();
    assert_eq!(
        catalog.observe(&origin, &linked, &HOOKS),
        Some(SemanticProjectionStatus::VerifiedPositionIndependentBody)
    );
    assert_eq!(catalog.semantic(&linked), Some(&SEMANTIC));
    let mut changed = linked.clone();
    changed.bytes[0] ^= 1;
    assert!(catalog.semantic(&changed).is_none());
    changed = linked.clone();
    changed.addresses_resolved = false;
    assert!(catalog.semantic(&changed).is_none());
    changed = linked.clone();
    changed.relocations.push(artifact::SymbolRelocation {
        reference: open_radio_vendor_analysis_model::SymbolReference::Unknown {
            reason: "synthetic fixture".to_owned(),
        },
        address: linked.address as u32,
        kind: artifact::RelocationKind::Lo12I,
        symbol: "other".into(),
        addend: 0,
    });
    assert!(catalog.semantic(&changed).is_none());
    assert_eq!(
        catalog.observe(&origin, &changed, &HOOKS),
        Some(SemanticProjectionStatus::Unverified {
            gap: SemanticProjectionGap::RelocationProofRequired
        })
    );
}

#[test]
fn raw_authentication_cannot_be_replaced_with_a_matching_name() {
    let (mut origin, linked) = pair(&RETURN);
    origin.bytes[0] ^= 1;
    let mut catalog = SemanticProjectionCatalog::default();
    assert!(catalog.observe(&origin, &linked, &HOOKS).is_none());
    assert!(catalog.semantic(&linked).is_none());
}

#[test]
fn conflicting_reviewed_contracts_do_not_overwrite_each_other() {
    let (origin, linked) = pair(&RETURN);
    let mut catalog = SemanticProjectionCatalog::default();
    catalog.observe(&origin, &linked, &HOOKS);
    let other_hooks = RiscvSummaryHooks {
        direct_semantic: |_| Some(&OTHER),
        ..HOOKS
    };
    catalog.observe(&origin, &linked, &other_hooks);
    assert!(catalog.semantic(&linked).is_none());
    assert_eq!(
        catalog.gaps(&linked),
        [SemanticProjectionGap::ConflictingSemantics]
    );
    catalog.observe(&origin, &linked, &HOOKS);
    assert_eq!(catalog.candidates.values().map(Vec::len).sum::<usize>(), 2);
}

#[test]
fn identical_bytes_do_not_prove_pc_dependent_behavior() {
    for (word, expected) in [
        (
            0x00000517_u32,
            SemanticProjectionGap::PositionDependent { offset: 0 },
        ), // auipc
        (
            0x00000073,
            SemanticProjectionGap::PositionDependent { offset: 0 },
        ), // ecall
        (
            0x000000ef,
            SemanticProjectionGap::CallProofRequired { offset: 0 },
        ), // jal ra
        (
            0x000080e7,
            SemanticProjectionGap::CallProofRequired { offset: 0 },
        ), // jalr ra
        (
            0x1000006f,
            SemanticProjectionGap::InvalidControlFlow { offset: 0 },
        ), // j outside
    ] {
        let bytes = [word.to_le_bytes(), RETURN].concat();
        let (origin, linked) = pair(&bytes);
        assert_eq!(
            verify_position_independent_body(&origin, &linked),
            Err(expected)
        );
    }
}

#[test]
fn closed_control_flow_and_decode_coverage_are_required() {
    let (origin, linked) = pair(&0x00000013_u32.to_le_bytes()); // nop, then unknown bytes
    assert_eq!(
        verify_position_independent_body(&origin, &linked),
        Err(SemanticProjectionGap::OpenBodyBoundary)
    );
    let (origin, linked) = pair(&[0x01]); // truncated instruction
    assert!(matches!(
        verify_position_independent_body(&origin, &linked),
        Err(SemanticProjectionGap::UnsupportedInstruction { .. })
    ));
    let (origin, linked) = pair(&0x0000006f_u32.to_le_bytes()); // closed self-loop
    assert_eq!(verify_position_independent_body(&origin, &linked), Ok(()));
    let (origin, linked) = pair(&[0x82, 0x80]); // compressed return
    assert_eq!(verify_position_independent_body(&origin, &linked), Ok(()));
}

#[test]
fn proof_cannot_be_reused_for_another_occurrence_with_identical_metadata() {
    let (origin, mut linked) = pair(&RETURN);
    linked.identity = artifact::CodeIdentity::Symbol {
        artifact_sha256: "11".repeat(32),
        location: crate::SymbolLocation {
            object: crate::ObjectLocation::ArchiveMember { ordinal: 0 },
            table: crate::ArtifactSymbolTable::Static,
            index: 1,
        },
    };
    let mut catalog = SemanticProjectionCatalog::default();
    catalog.observe(&origin, &linked, &HOOKS).unwrap();
    assert!(catalog.semantic(&linked).is_some());
    let mut other = linked.clone();
    if let artifact::CodeIdentity::Symbol { location, .. } = &mut other.identity {
        location.object = crate::ObjectLocation::ArchiveMember { ordinal: 1 };
    }
    assert!(catalog.semantic(&other).is_none());
    assert!(catalog.gaps(&other).is_empty());
}
