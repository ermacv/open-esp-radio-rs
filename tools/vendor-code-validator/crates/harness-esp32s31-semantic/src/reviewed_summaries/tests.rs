//! Reviewed summary identity and trace regression tests.

use super::{
    body_identity::*, direct_semantic::*, i2c::*, intrinsics::*, reference_intrinsic_trace,
};
use crate::*;

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

fn pp_post_symbol() -> artifact::ArtifactSymbolDefinition {
    artifact::ArtifactSymbolDefinition {
        member: Some("pp.o".to_owned()),
        name: "pp_post".to_owned(),
        address: 0,
        bytes: PP_POST_BODY.to_vec(),
        addresses_resolved: false,
        memory_regions: Vec::new(),
        relocations: PP_POST_RELOCATIONS
            .iter()
            .map(|&(address, kind, symbol)| artifact::SymbolRelocation {
                address,
                kind,
                symbol: symbol.to_owned(),
                addend: 0,
            })
            .collect(),
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
fn pp_post_semantics_require_the_exact_body_and_relocation_schema() {
    let exact = pp_post_symbol();
    let spec = direct_semantic_function(&exact).expect("pinned pp_post must be recognized");
    assert_eq!(spec.id, "esp32s31-libpp-pp-post-v1");
    assert_eq!(spec.semantic.operation, "wifi.internal-signal.post");
    assert_eq!(spec.semantic.arguments.len(), 1);
    let dispatch = spec
        .semantic
        .event_dispatch
        .expect("reviewed pp_post must declare its dispatch projection");
    assert_eq!(dispatch.mechanism, "internal-signal");
    assert_eq!(dispatch.execution_context, "unspecified");
    assert_eq!(dispatch.receiver, None);
    assert_eq!(dispatch.argument_roles, PP_POST_EVENT_ROLES);

    let mut changed_body = pp_post_symbol();
    changed_body.bytes[0] ^= 1;
    assert!(direct_semantic_function(&changed_body).is_none());

    let mut changed_relocation = pp_post_symbol();
    changed_relocation.relocations[0].symbol = "different_table".to_owned();
    assert!(direct_semantic_function(&changed_relocation).is_none());
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
    let trace = reference_intrinsic_trace(&symbol, &map(), &StructuralPointerContext::default())
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
        reference_intrinsic_trace(&symbol, &map(), &StructuralPointerContext::default()).is_none()
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
        reference_intrinsic_trace(&exact, &map(), &StructuralPointerContext::default()).is_none(),
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
