//! Reviewed summary identity and trace regression tests.

use super::{
    body_identity::*, direct_semantic::*, fail_stop::*, i2c::*, intrinsics::*,
    reference_intrinsic_trace, rf::*,
};
use crate::*;

fn symbol(bytes: Vec<u8>) -> artifact::ArtifactSymbolDefinition {
    artifact::ArtifactSymbolDefinition {
        member: None,
        name: "phy_get_i2c_hostid_new".to_owned(),
        address: 0x1000_732a,
        bytes,
        addresses_resolved: true,
        memory_regions: Default::default(),
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
        memory_regions: Default::default(),
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

fn btdm_assert_symbol(address: u64) -> artifact::ArtifactSymbolDefinition {
    artifact::ArtifactSymbolDefinition {
        member: None,
        name: "wr_btdm_assert".to_owned(),
        address,
        bytes: BTDM_ASSERT_BODY.to_vec(),
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    }
}

fn map() -> MmioMap {
    MmioMap {
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
        regions: vec![MmioRegion {
            name: "i2c-analog".to_owned(),
            start: 0x2010_f800,
            end: 0x2010_f900,
            readable: true,
            writable: true,
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
    assert_eq!(dispatch.receiver, Some("esp32s31::pp-task"));
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
fn btdm_assert_preserves_its_level_gated_fail_stop() {
    for address in [BLE_BTDM_ASSERT_ADDRESS, BREDR_BTDM_ASSERT_ADDRESS] {
        let symbol = btdm_assert_symbol(address);
        let trace =
            reference_intrinsic_trace(&symbol, &map(), &StructuralPointerContext::default())
                .expect("the exact controller assertion body must have a summary");

        assert!(trace.is_reference_eligible(), "{trace:#?}");
        let flow = trace
            .reference_flow
            .expect("the assertion level check must remain structured");
        let DraftReferenceTerminator::Branch {
            condition,
            taken,
            not_taken,
        } = flow.terminator
        else {
            panic!("the assertion summary must branch on its level argument");
        };
        assert_eq!(condition.left, SymbolicValue::input(4));
        assert_eq!(condition.right, SymbolicValue::Constant(3));
        assert!(matches!(
            taken.terminator,
            DraftReferenceTerminator::FailStop { .. }
        ));
        assert!(matches!(
            not_taken.terminator,
            DraftReferenceTerminator::Return(SymbolicValue::Unknown)
        ));
    }
}

#[test]
fn changed_btdm_assert_body_is_not_treated_as_a_fail_stop() {
    let mut symbol = btdm_assert_symbol(BLE_BTDM_ASSERT_ADDRESS);
    symbol.bytes[0] ^= 1;

    assert!(
        reference_intrinsic_trace(&symbol, &map(), &StructuralPointerContext::default()).is_none()
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
        memory_regions: Default::default(),
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
        memory_regions: Default::default(),
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
        memory_regions: Default::default(),
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
fn linked_body_binding_rejects_same_size_mutations_and_unresolved_context() {
    let binding = ReviewedLinkedBody {
        name: "synthetic",
        address: 0x1000,
        size: 4,
        // SHA-256 of the synthetic fixture [1, 2, 3, 4], not vendor bytes.
        sha256: "9f64a747e1b97f131fabb6b447296c9b6f0201e79fb3c5356e6c77e89b6a806a",
    };
    let mut exact = symbol(vec![1, 2, 3, 4]);
    exact.name = "synthetic".to_owned();
    exact.address = 0x1000;
    assert!(binding.matches(&exact));
    for index in 0..exact.bytes.len() {
        let mut changed = exact.clone();
        changed.bytes[index] ^= 1;
        assert!(!binding.matches(&changed));
    }
    let mut unresolved = exact.clone();
    unresolved.addresses_resolved = false;
    assert!(!binding.matches(&unresolved));
    let mut member = exact.clone();
    member.member = Some("other.o".to_owned());
    assert!(!binding.matches(&member));
    let mut relocated = exact.clone();
    relocated.relocations.push(artifact::SymbolRelocation {
        address: 0x1000,
        kind: artifact::RelocationKind::Call,
        symbol: "other".to_owned(),
        addend: 0,
    });
    assert!(!binding.matches(&relocated));
    let mut renamed = exact.clone();
    renamed.name = "other".to_owned();
    assert!(!binding.matches(&renamed));
    let mut moved = exact.clone();
    moved.address += 4;
    assert!(!binding.matches(&moved));
    exact.bytes.push(0);
    assert!(!binding.matches(&exact));
}

const ROM_SUMMARY_BODIES: &[(&str, u64, usize)] = &[
    ("phy_wait_rfpll_cal_end", 0x2f82_5874, 86),
    ("phy_rfpll_cap_init_cal", 0x2f82_5ada, 192),
    ("phy_set_rf_freq_offset", 0x2f82_5c10, 16),
    ("phy_iq_est_enable", 0x2f82_89d4, 180),
    ("__divdi3", ROM_DIVDI3_ADDRESS as u64, ROM_DIVDI3_SIZE),
];

fn accepts_reviewed_rom_body(symbol: &artifact::ArtifactSymbolDefinition) -> bool {
    exact_rfpll_calibration_poll(symbol)
        || exact_rfpll_cap_calibration_search(symbol)
        || exact_rf_frequency_offset_scratch_wrapper(symbol)
        || exact_iq_estimator_poll(symbol)
        || exact_wide_signed_divide(symbol)
}

#[test]
fn rom_hooks_reject_synthetic_bodies_with_matching_symbol_metadata() {
    for &(name, address, size) in ROM_SUMMARY_BODIES {
        for fill in [0, 0xff] {
            let mut fake = symbol(vec![fill; size]);
            fake.name = name.to_owned();
            fake.address = address;
            assert!(!accepts_reviewed_rom_body(&fake), "{name}");
            assert!(
                reference_intrinsic_trace(&fake, &map(), &StructuralPointerContext::default())
                    .is_none()
            );
            let arguments = core::array::from_fn(|index| SymbolicValue::input(index as u8));
            assert!(wide_signed_divide_intrinsic(&fake, &arguments).is_none());
        }
    }
}

#[test]
fn dtm_callee_metadata_cannot_prove_a_caller_owned_channel_domain() {
    let channel = MemoryObjectLocation {
        root: MemoryObjectRoot::Argument { index: 0 },
        offset: 0x0e,
    };
    for fill in [0, 0xff] {
        let mut fake = symbol(vec![fill; 550]);
        fake.name = "r_sym_ble_G4zC4UNjJYmyjOsZ3vNq".to_owned();
        fake.address = 0x1003_4b1c;
        assert!((RISCV_HARNESS.summaries.caller_memory_input_domain)(&fake, &channel, 8).is_none());
    }
}

#[test]
#[ignore = "requires caller-owned BLOBRAY_REVIEWED_ROM path; authenticates before loading"]
fn authenticated_rom_bindings_accept_reviewed_bodies_and_reject_every_byte_mutation() {
    use sha2::{Digest, Sha256};
    let path = std::path::PathBuf::from(
        std::env::var_os("BLOBRAY_REVIEWED_ROM").expect("set BLOBRAY_REVIEWED_ROM"),
    );
    let image = std::fs::read(&path).unwrap();
    assert_eq!(
        format!("{:x}", Sha256::digest(&image)),
        "a52ad7513deb656a910a5740125f1cce2c7941f11ce57213b7b43aea93d5ab87"
    );
    for &(name, address, size) in ROM_SUMMARY_BODIES {
        let exact = artifact::load_code_symbol_exact(&path, None, name, address)
            .unwrap()
            .unwrap();
        assert_eq!(exact.bytes.len(), size);
        assert!(accepts_reviewed_rom_body(&exact), "{name}");
        for index in 0..exact.bytes.len() {
            let mut changed = exact.clone();
            changed.bytes[index] ^= 1;
            assert!(!accepts_reviewed_rom_body(&changed), "{name} byte {index}");
        }
    }
}
