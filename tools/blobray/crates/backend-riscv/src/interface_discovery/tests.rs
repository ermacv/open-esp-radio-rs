use super::*;

fn symbol(
    bytes: Vec<u8>,
    relocations: Vec<artifact::SymbolRelocation>,
) -> artifact::ArtifactSymbolDefinition {
    artifact::ArtifactSymbolDefinition {
        identity: artifact::ArtifactSymbolDefinition::synthetic_identity(
            module_path!(),
            &(Some("vendor.o".to_owned())),
            "vendor_fn",
            0,
        ),
        member: Some("vendor.o".to_owned()),
        name: "vendor_fn".to_owned(),
        address: 0,
        bytes,
        addresses_resolved: false,
        memory_regions: Default::default(),
        relocations,
    }
}

#[test]
fn discovers_pointer_cell_table_slot_and_call_arguments() {
    let symbol = symbol(
        vec![
            0xb7, 0x07, 0x00, 0x00, // lui a5, 0
            0x83, 0xa7, 0x07, 0x00, // lw a5, 0(a5)
            0x83, 0xa2, 0x07, 0x01, // lw t0, 16(a5)
            0xe7, 0x80, 0x02, 0x00, // jalr ra, 0(t0)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        vec![
            artifact::SymbolRelocation {
                reference: open_radio_vendor_analysis_model::SymbolReference::Unknown {
                    reason: "synthetic fixture".to_owned(),
                },
                address: 0,
                kind: artifact::RelocationKind::Hi20,
                symbol: "g_services".to_owned(),
                addend: 0,
            },
            artifact::SymbolRelocation {
                reference: open_radio_vendor_analysis_model::SymbolReference::Unknown {
                    reason: "synthetic fixture".to_owned(),
                },
                address: 4,
                kind: artifact::RelocationKind::Lo12I,
                symbol: "g_services".to_owned(),
                addend: 0,
            },
        ],
    );

    let calls = discover_interface_calls(&symbol).unwrap();
    assert_eq!(calls.len(), 1);
    let call = &calls[0];
    assert_eq!(call.site, 12);
    assert_eq!(call.kind, InterfaceCallKind::Call);
    assert_eq!(call.target.loads.len(), 2);
    assert_eq!(call.target.loads[0].offset, 0);
    assert_eq!(call.target.loads[1].offset, 16);
    assert_eq!(call.target.container_loads().len(), 1);
    assert_eq!(call.target.slot().unwrap().offset, 16);
    assert_eq!(call.arguments[0].canonical(), "arg0");
    assert!(matches!(
        call.target.root,
        InterfaceRoot::RelocatedSymbol { ref symbol, .. } if symbol == "g_services"
    ));
}

#[test]
fn preserves_bounded_pointer_alternatives_at_a_shared_tail_epilogue() {
    let symbol = symbol(
        vec![
            0x11, 0xc5, // beqz a0, +12
            0xb7, 0x07, 0x00, 0x00, // lui a5, %hi(g_services)
            0x9c, 0x43, // lw a5, %lo(g_services)(a5)
            0xfc, 0x4b, // lw a5, 84(a5)
            0x29, 0xa0, // j +10
            0xb7, 0x07, 0x00, 0x00, // lui a5, %hi(g_services)
            0x9c, 0x43, // lw a5, %lo(g_services)(a5)
            0xbc, 0x4f, // lw a5, 88(a5)
            0x82, 0x87, // jr a5
        ],
        vec![
            artifact::SymbolRelocation {
                reference: open_radio_vendor_analysis_model::SymbolReference::Unknown {
                    reason: "synthetic fixture".to_owned(),
                },
                address: 2,
                kind: artifact::RelocationKind::Hi20,
                symbol: "g_services".to_owned(),
                addend: 0,
            },
            artifact::SymbolRelocation {
                reference: open_radio_vendor_analysis_model::SymbolReference::Unknown {
                    reason: "synthetic fixture".to_owned(),
                },
                address: 6,
                kind: artifact::RelocationKind::Lo12I,
                symbol: "g_services".to_owned(),
                addend: 0,
            },
            artifact::SymbolRelocation {
                reference: open_radio_vendor_analysis_model::SymbolReference::Unknown {
                    reason: "synthetic fixture".to_owned(),
                },
                address: 12,
                kind: artifact::RelocationKind::Hi20,
                symbol: "g_services".to_owned(),
                addend: 0,
            },
            artifact::SymbolRelocation {
                reference: open_radio_vendor_analysis_model::SymbolReference::Unknown {
                    reason: "synthetic fixture".to_owned(),
                },
                address: 16,
                kind: artifact::RelocationKind::Lo12I,
                symbol: "g_services".to_owned(),
                addend: 0,
            },
        ],
    );

    let calls = discover_interface_calls(&symbol).unwrap();

    assert_eq!(calls.len(), 2, "{calls:#?}");
    assert!(
        calls
            .iter()
            .all(|call| { call.site == 20 && call.kind == InterfaceCallKind::TailJump })
    );
    assert_eq!(
        calls
            .iter()
            .map(|call| call.target.slot().unwrap().offset)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([84, 88])
    );
}

#[test]
fn local_wifi_ap_link_unit_preserves_esf_recycle_tail_slot_alternatives() {
    let Some(artifact) = std::env::var_os("OPEN_RADIO_VENDOR_WIFI_AP_ELF") else {
        return;
    };
    let artifact = std::path::Path::new(&artifact);
    if !artifact.is_file() {
        return;
    }
    let symbols =
        artifact::load_code_symbols(artifact, "", artifact::CodeSymbolSelection::All).unwrap();
    let function = symbols
        .iter()
        .find(|symbol| symbol.name == "esf_buf_recycle")
        .unwrap();

    let calls = discover_interface_calls(function).unwrap();
    let tail_slots = calls
        .iter()
        .filter(|call| call.site == 0x1006_8f5e)
        .filter_map(|call| call.target.slot().map(|slot| slot.offset))
        .collect::<BTreeSet<_>>();

    assert_eq!(tail_slots, BTreeSet::from([0x58]), "{calls:#?}");

    let allocate = symbols
        .iter()
        .find(|symbol| symbol.name == "esf_buf_alloc_dynamic")
        .unwrap();
    let allocate_calls = discover_interface_calls(allocate).unwrap();
    let recovered = allocate_calls
        .iter()
        .filter_map(|call| call.target.slot().map(|slot| (call.site, slot.offset)))
        .collect::<BTreeSet<_>>();
    assert!(
        recovered.contains(&(0x1005_e724, 0x58)),
        "{allocate_calls:#?}"
    );
    assert!(
        recovered.contains(&(0x1005_e770, 0x158)),
        "{allocate_calls:#?}"
    );
}

#[test]
fn discovers_relocated_function_assignment_through_pointer_cell() {
    let symbol = symbol(
        vec![
            0xb7, 0x07, 0x00, 0x00, // lui a5, %hi(g_services)
            0x83, 0xa7, 0x07, 0x00, // lw a5, %lo(g_services)(a5)
            0x37, 0x07, 0x00, 0x00, // lui a4, %hi(service_fn)
            0x13, 0x07, 0x07, 0x00, // addi a4, a4, %lo(service_fn)
            0x93, 0x87, 0x07, 0x08, // addi a5, a5, 128
            0xd8, 0xc3, // sw a4, 4(a5)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        vec![
            artifact::SymbolRelocation {
                reference: open_radio_vendor_analysis_model::SymbolReference::Unknown {
                    reason: "synthetic fixture".to_owned(),
                },
                address: 0,
                kind: artifact::RelocationKind::Hi20,
                symbol: "g_services".to_owned(),
                addend: 0,
            },
            artifact::SymbolRelocation {
                reference: open_radio_vendor_analysis_model::SymbolReference::Unknown {
                    reason: "synthetic fixture".to_owned(),
                },
                address: 4,
                kind: artifact::RelocationKind::Lo12I,
                symbol: "g_services".to_owned(),
                addend: 0,
            },
            artifact::SymbolRelocation {
                reference: open_radio_vendor_analysis_model::SymbolReference::Unknown {
                    reason: "synthetic fixture".to_owned(),
                },
                address: 8,
                kind: artifact::RelocationKind::Hi20,
                symbol: "service_fn".to_owned(),
                addend: 0,
            },
            artifact::SymbolRelocation {
                reference: open_radio_vendor_analysis_model::SymbolReference::Unknown {
                    reason: "synthetic fixture".to_owned(),
                },
                address: 12,
                kind: artifact::RelocationKind::Lo12I,
                symbol: "service_fn".to_owned(),
                addend: 0,
            },
        ],
    );

    let discovery = discover_interface_calls(&symbol).unwrap();
    assert_eq!(discovery.assignments.len(), 1);
    let assignment = &discovery.assignments[0];
    assert_eq!(assignment.site, 20);
    assert_eq!(assignment.offset, 0x84);
    assert_eq!(assignment.width, 32);
    assert_eq!(assignment.container_loads.len(), 1);
    assert_eq!(assignment.container_loads[0].offset, 0);
    assert!(matches!(
        assignment.root,
        InterfaceRoot::RelocatedSymbol { ref symbol, .. } if symbol == "g_services"
    ));
    assert!(matches!(
        assignment.target,
        InterfaceRoot::RelocatedSymbol { ref symbol, .. } if symbol == "service_fn"
    ));
}

#[test]
fn runtime_registration_preserves_the_callback_argument_without_typing_it() {
    let symbol = symbol(
        vec![
            0xb7, 0x07, 0x00, 0x00, // lui a5, 0
            0x23, 0xa0, 0xb7, 0x00, // sw a1, 0(a5)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        vec![artifact::SymbolRelocation {
            reference: open_radio_vendor_analysis_model::SymbolReference::Unknown {
                reason: "synthetic fixture".to_owned(),
            },
            address: 0,
            kind: artifact::RelocationKind::Hi20,
            symbol: "callback_cell".to_owned(),
            addend: 0,
        }],
    );

    let discovery = discover_interface_calls(&symbol).unwrap();
    assert_eq!(discovery.assignments.len(), 1);
    assert!(matches!(
        discovery.assignments[0].target,
        InterfaceRoot::FunctionArgument { index: 1, .. }
    ));
}

#[test]
fn floating_point_blocker_preserves_later_interface_call_evidence() {
    let symbol = symbol(
        vec![
            0x07, 0x20, 0x05, 0x00, // flw f0, 0(a0)
            0xb7, 0x07, 0x00, 0x00, // lui a5, 0
            0x83, 0xa7, 0x07, 0x00, // lw a5, 0(a5)
            0x83, 0xa2, 0x07, 0x01, // lw t0, 16(a5)
            0xe7, 0x80, 0x02, 0x00, // jalr ra, 0(t0)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        vec![
            artifact::SymbolRelocation {
                reference: open_radio_vendor_analysis_model::SymbolReference::Unknown {
                    reason: "synthetic fixture".to_owned(),
                },
                address: 4,
                kind: artifact::RelocationKind::Hi20,
                symbol: "g_services".to_owned(),
                addend: 0,
            },
            artifact::SymbolRelocation {
                reference: open_radio_vendor_analysis_model::SymbolReference::Unknown {
                    reason: "synthetic fixture".to_owned(),
                },
                address: 8,
                kind: artifact::RelocationKind::Lo12I,
                symbol: "g_services".to_owned(),
                addend: 0,
            },
        ],
    );

    let discovery = discover_interface_calls(&symbol).unwrap();
    assert_eq!(discovery.calls.len(), 1);
    assert_eq!(discovery.calls[0].site, 16);
    assert_eq!(discovery.decode_blockers.len(), 1);
    assert_eq!(
        discovery.decode_blockers[0].class,
        artifact::UnsupportedInstructionClass::FloatingPoint
    );
    assert_eq!(discovery.decode_blockers[0].address, 0);
}

#[test]
fn discovers_context_relative_nested_callback_without_platform_knowledge() {
    let symbol = symbol(
        vec![
            0x83, 0x27, 0x85, 0x00, // lw a5, 8(a0)
            0x83, 0xa2, 0xc7, 0x00, // lw t0, 12(a5)
            0xe7, 0x80, 0x02, 0x00, // jalr ra, 0(t0)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        Vec::new(),
    );

    let calls = discover_interface_calls(&symbol).unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].target.root,
        InterfaceRoot::FunctionArgument {
            index: 0,
            owner: symbol.identity.clone()
        }
    );
    assert_eq!(
        calls[0]
            .target
            .loads
            .iter()
            .map(|load| load.offset)
            .collect::<Vec<_>>(),
        [8, 12]
    );
}

#[test]
fn compressed_register_move_preserves_argument_root_for_object_method() {
    let symbol = symbol(
        vec![
            0x2a, 0x84, // c.mv s0, a0 (decoded as add s0, zero, a0)
            0x3c, 0x5c, // c.lw a5, 120(s0)
            0x82, 0x97, // c.jalr a5
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        Vec::new(),
    );

    let calls = discover_interface_calls(&symbol).unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].target.root,
        InterfaceRoot::FunctionArgument {
            index: 0,
            owner: symbol.identity.clone()
        }
    );
    assert_eq!(calls[0].target.loads[0].offset, 120);
}

#[test]
fn preserves_affine_argument_indexed_slots_without_inventing_a_fixed_offset() {
    let symbol = symbol(
        vec![
            0xb7, 0x07, 0x00, 0x00, // lui a5, 0
            0x83, 0xa7, 0x07, 0x00, // lw a5, 0(a5)
            0x13, 0x13, 0x25, 0x00, // slli t1, a0, 2
            0xb3, 0x87, 0x67, 0x00, // add a5, a5, t1
            0x83, 0xa2, 0x07, 0x00, // lw t0, 0(a5)
            0xe7, 0x80, 0x02, 0x00, // jalr ra, 0(t0)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        vec![
            artifact::SymbolRelocation {
                reference: open_radio_vendor_analysis_model::SymbolReference::Unknown {
                    reason: "synthetic fixture".to_owned(),
                },
                address: 0,
                kind: artifact::RelocationKind::Hi20,
                symbol: "g_services".to_owned(),
                addend: 0,
            },
            artifact::SymbolRelocation {
                reference: open_radio_vendor_analysis_model::SymbolReference::Unknown {
                    reason: "synthetic fixture".to_owned(),
                },
                address: 4,
                kind: artifact::RelocationKind::Lo12I,
                symbol: "g_services".to_owned(),
                addend: 0,
            },
        ],
    );

    let calls = discover_interface_calls(&symbol).unwrap();
    assert_eq!(calls.len(), 1);
    let slot = calls[0].target.slot().unwrap();
    assert_eq!(slot.offset, 0);
    assert_eq!(slot.width, 32);
    assert_eq!(calls[0].target.fixed_slot(), None);
    assert_eq!(
        slot.selector,
        Some(InterfaceSlotSelector {
            argument: 0,
            scale: 4,
            addend: 0,
        })
    );
    assert!(calls[0].target.canonical().contains("arg0*4"));
}

#[test]
fn control_flow_join_preserves_bounded_pointer_provenance_alternatives() {
    let symbol = symbol(
        vec![
            0x63, 0x04, 0xb5, 0x00, // beq a0, a1, +8
            0x93, 0x07, 0x05, 0x00, // mv a5, a0
            0x83, 0xa2, 0x07, 0x00, // lw t0, 0(a5)
            0xe7, 0x80, 0x02, 0x00, // jalr ra, 0(t0)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        Vec::new(),
    );

    let calls = discover_interface_calls(&symbol).unwrap();
    assert_eq!(calls.len(), 2);
    let roots = calls
        .iter()
        .map(|call| call.target.root.clone())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        roots,
        BTreeSet::from([
            InterfaceRoot::FunctionArgument {
                owner: symbol.identity.clone(),
                index: 0
            },
            InterfaceRoot::FunctionArgument {
                owner: symbol.identity.clone(),
                index: 5
            },
        ])
    );
}

#[test]
fn return_is_not_reported_as_an_interface_call() {
    let calls =
        discover_interface_calls(&symbol(vec![0x67, 0x80, 0x00, 0x00], Vec::new())).unwrap();
    assert!(calls.is_empty());
}

#[test]
fn linked_absolute_address_is_retained_as_a_root() {
    let mut symbol = symbol(
        vec![
            0xb7, 0x27, 0x34, 0x12, // lui a5, 0x12342
            0x83, 0xa2, 0x07, 0x01, // lw t0, 16(a5)
            0xe7, 0x80, 0x02, 0x00, // jalr ra, 0(t0)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        Vec::new(),
    );
    symbol.addresses_resolved = true;

    let calls = discover_interface_calls(&symbol).unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].target.root,
        InterfaceRoot::AbsoluteAddress {
            address: 0x1234_2000,
            data_address: crate::DataAddressResolution::Unknown {
                reason: crate::DataAddressGap::NoContainingDefinition
            },
        }
    );
    assert_eq!(calls[0].target.slot().unwrap().offset, 16);

    let bounded = discover_interface_calls_with_data_symbols(
        &symbol,
        &[artifact::ArtifactDataSymbolDefinition {
            identity: open_radio_vendor_analysis_model::DataIdentity::Synthetic {
                namespace: module_path!().to_owned(),
                key: "function_pointer_cell".to_owned(),
            },
            member: None,
            name: "function_pointer_cell".to_owned(),
            address: 0x1234_2010,
            size: 4,
            exported: true,
        }],
    )
    .unwrap();
    assert_eq!(bounded.len(), 1);
    let InterfaceRoot::AbsoluteAddress {
        address,
        data_address,
    } = &bounded[0].target.root
    else {
        panic!("numeric root");
    };
    assert_eq!(*address, 0x1234_2000);
    assert_eq!(data_address.candidates()[0].symbol, "function_pointer_cell");
    assert_eq!(data_address.candidates()[0].symbol_address, 0x1234_2010);
    assert_eq!(bounded[0].target.slot().unwrap().offset, 16);
}

#[test]
fn linked_static_table_registration_retains_bounded_absolute_addresses() {
    let mut symbol = symbol(
        vec![
            0xb7, 0x17, 0x06, 0x10, // lui a5, 0x10061
            0x37, 0x17, 0x06, 0x10, // lui a4, 0x10061
            0x93, 0x87, 0x07, 0x0c, // addi a5, a5, 0xc0
            0x23, 0x26, 0xf7, 0x32, // sw a5, 0x32c(a4)
            0x01, 0x45, // li a0, 0
            0x82, 0x80, // ret
        ],
        Vec::new(),
    );
    symbol.address = 0x1004_e5ea;
    symbol.addresses_resolved = true;
    let data_symbols = [
        artifact::ArtifactDataSymbolDefinition {
            identity: open_radio_vendor_analysis_model::DataIdentity::Synthetic {
                namespace: module_path!().to_owned(),
                key: "registered_table".to_owned(),
            },
            member: None,
            name: "registered_table".to_owned(),
            address: 0x1006_10c0,
            size: 0x70,
            exported: true,
        },
        artifact::ArtifactDataSymbolDefinition {
            identity: open_radio_vendor_analysis_model::DataIdentity::Synthetic {
                namespace: module_path!().to_owned(),
                key: "table_cell".to_owned(),
            },
            member: None,
            name: "table_cell".to_owned(),
            address: 0x1006_132c,
            size: 4,
            exported: true,
        },
    ];

    let unknown = discover_interface_calls(&symbol).unwrap();
    assert_eq!(unknown.assignments.len(), 1);
    assert!(matches!(
        unknown.assignments[0].target,
        InterfaceRoot::AbsoluteAddress {
            data_address: crate::DataAddressResolution::Unknown {
                reason: crate::DataAddressGap::NoContainingDefinition
            },
            ..
        }
    ));
    let discovery = discover_interface_calls_with_data_symbols(&symbol, &data_symbols).unwrap();
    assert_eq!(discovery.assignments.len(), 1);
    let assignment = &discovery.assignments[0];
    assert_eq!(assignment.site, 0x1004_e5f6);
    assert_eq!(assignment.offset, 0x32c);
    let InterfaceRoot::AbsoluteAddress {
        address,
        data_address,
    } = &assignment.root
    else {
        panic!("numeric root");
    };
    assert_eq!(*address, 0x1006_1000);
    assert_eq!(data_address.candidates()[0].symbol, "table_cell");
    let InterfaceRoot::AbsoluteAddress {
        address,
        data_address,
    } = &assignment.target
    else {
        panic!("numeric target");
    };
    assert_eq!(*address, 0x1006_10c0);
    assert_eq!(data_address.candidates()[0].symbol, "registered_table");
}

#[test]
fn pcrel_pair_with_the_wrong_base_register_is_not_accepted() {
    let symbol = symbol(
        vec![
            0x97, 0x07, 0x00, 0x00, // auipc a5, 0
            0x83, 0x27, 0x07, 0x00, // lw a5, 0(a4), not 0(a5)
            0x83, 0xa2, 0x07, 0x01, // lw t0, 16(a5)
            0xe7, 0x80, 0x02, 0x00, // jalr ra, 0(t0)
        ],
        vec![
            artifact::SymbolRelocation {
                reference: open_radio_vendor_analysis_model::SymbolReference::Unknown {
                    reason: "synthetic fixture".to_owned(),
                },
                address: 0,
                kind: artifact::RelocationKind::PcRelHi20,
                symbol: "g_services".to_owned(),
                addend: 0,
            },
            artifact::SymbolRelocation {
                reference: open_radio_vendor_analysis_model::SymbolReference::Unknown {
                    reason: "synthetic fixture".to_owned(),
                },
                address: 4,
                kind: artifact::RelocationKind::PcRelLo12I,
                symbol: "g_services".to_owned(),
                addend: 0,
            },
        ],
    );

    assert!(discover_interface_calls(&symbol).unwrap().is_empty());
}

#[test]
fn overlapping_data_candidates_keep_all_physical_owners_and_observed_offsets() {
    let mut owner = symbol(
        [0x123427b7u32, 0x0107a283, 0x000280e7, 0x00008067]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect(),
        Vec::new(),
    );
    owner.addresses_resolved = true;
    // More than the pointer-flow alternative limit: ranges are evidence on a
    // single numeric expression, not eight guessed pointer values.
    let mut definitions = (0..12)
        .map(|index| artifact::ArtifactDataSymbolDefinition {
            identity: crate::DataIdentity::Symbol {
                artifact_sha256: format!("{index:064x}"),
                location: crate::SymbolLocation {
                    object: crate::ObjectLocation::Standalone,
                    table: crate::ArtifactSymbolTable::Static,
                    index: 1,
                },
            },
            member: None,
            name: "same_name".to_owned(),
            address: 0x1234_2000,
            size: 20 + index as u32,
            exported: index % 2 == 0,
        })
        .collect::<Vec<_>>();
    let result = discover_interface_calls_with_data_symbols(&owner, &definitions).unwrap();
    assert_eq!(result.calls.len(), 1);
    let target = &result.calls[0].target;
    assert_eq!(target.loads[0].offset, 16);
    let InterfaceRoot::AbsoluteAddress {
        address,
        data_address,
    } = &target.root
    else {
        panic!("numeric root");
    };
    assert_eq!(*address, 0x1234_2000);
    assert_eq!(data_address.candidates().len(), 12);
    assert!(matches!(
        data_address,
        crate::DataAddressResolution::Ambiguous { .. }
    ));
    assert_eq!(
        data_address
            .candidates()
            .iter()
            .map(|candidate| &candidate.identity)
            .collect::<BTreeSet<_>>()
            .len(),
        12
    );
    data_address.validate().unwrap();
    definitions.reverse();
    assert_eq!(
        result,
        discover_interface_calls_with_data_symbols(&owner, &definitions).unwrap()
    );
}

fn flow_fixture(words: &[u32]) -> artifact::ArtifactSymbolDefinition {
    symbol(
        words.iter().flat_map(|word| word.to_le_bytes()).collect(),
        vec![],
    )
}

fn branch_nonzero(register: u32, displacement: i32) -> u32 {
    let imm = displacement as u32;
    ((imm >> 12 & 1) << 31)
        | ((imm >> 5 & 0x3f) << 25)
        | (register << 15)
        | (1 << 12)
        | ((imm >> 1 & 0xf) << 8)
        | ((imm >> 11 & 1) << 7)
        | 0x63
}

fn jump(displacement: i32) -> u32 {
    let imm = displacement as u32;
    ((imm >> 20 & 1) << 31)
        | ((imm >> 1 & 0x3ff) << 21)
        | ((imm >> 11 & 1) << 20)
        | ((imm >> 12 & 0xff) << 12)
        | 0x6f
}

#[test]
fn retains_twelve_branch_targets_and_unknown_at_one_call() {
    let count = 12;
    let mut words = Vec::new();
    for candidate in 0..count {
        words.push(branch_nonzero(10, 12));
        // lw t0, candidate*4(a1)
        words.push(((candidate * 4) << 20) | (11 << 15) | (2 << 12) | (5 << 7) | 3);
        words.push(jump((count * 12 - (candidate * 12 + 8)) as i32));
    }
    words.push(0x00028067); // jr t0
    let result = discover_interface_calls(&flow_fixture(&words)).unwrap();
    assert_eq!(result.calls.len(), count as usize);
    assert_eq!(
        result
            .calls
            .iter()
            .map(|call| call.target.loads[0].offset)
            .collect::<BTreeSet<_>>(),
        (0..count).map(|i| (i * 4) as i32).collect()
    );
    let gap = result
        .gaps
        .iter()
        .find(|gap| gap.reason == InterfaceGapReason::UnresolvedCallTarget)
        .unwrap();
    let InterfaceArgumentValue::Alternatives(values) = &gap.registers[5].value else {
        panic!("missing retained alternatives");
    };
    assert_eq!(values.len(), count as usize + 1);
    assert!(values.contains(&InterfaceArgumentValue::Unknown));
}

#[test]
fn propagation_budget_retains_frontier_and_already_discovered_calls() {
    let function = flow_fixture(&[
        0x00052283, // lw t0, 0(a0)
        0x00450513, // addi a0, a0, 4
        branch_nonzero(11, -8),
        0x00028067, // jr t0
    ]);
    let limits = InterfaceDiscoveryLimits {
        max_state_updates: 24,
        max_value_alternatives: 128,
    };
    let result = discover_interface_calls_with_limits(&function, &[], limits).unwrap();
    assert_eq!(result.limits, limits);
    assert!(!result.calls.is_empty());
    let frontier = result
        .gaps
        .iter()
        .filter(|gap| {
            matches!(
                gap.reason,
                InterfaceGapReason::StateUpdateLimit {
                    limit: 24,
                    processed: 24
                }
            )
        })
        .collect::<Vec<_>>();
    assert!(!frontier.is_empty());
    assert!(
        frontier
            .iter()
            .all(|gap| gap.owner == function.identity && gap.registers.len() == 32)
    );
    assert!(frontier.iter().any(|gap| matches!(&gap.registers[10].value, InterfaceArgumentValue::Alternatives(values) if values.len() > 1)));
    let result = discover_interface_calls_with_limits(
        &function,
        &[],
        InterfaceDiscoveryLimits {
            max_state_updates: 4096,
            max_value_alternatives: 3,
        },
    )
    .unwrap();
    assert!(result.gaps.iter().any(|gap| matches!(gap.reason, InterfaceGapReason::ValueAlternativeLimit {limit:3, observed} if observed > 3)));
    assert!(!result.calls.is_empty());
}

#[test]
fn loaded_store_target_keeps_load_site_and_post_offset() {
    let result = discover_interface_calls(&flow_fixture(&[
        0x0045a283, // lw t0, 4(a1)
        0x00828293, // addi t0, t0, 8
        0x00552623, // sw t0, 12(a0)
        0x00008067, // ret
    ]))
    .unwrap();
    assert_eq!(result.assignments.len(), 1);
    let assignment = &result.assignments[0];
    assert_eq!(
        assignment.target,
        InterfaceRoot::FunctionArgument {
            index: 1,
            owner: result.assignments[0].owner.clone()
        }
    );
    assert_eq!(
        assignment.target_loads,
        vec![InterfaceLoad {
            site: 0,
            offset: 4,
            width: 32,
            selector: None
        }]
    );
    assert_eq!(assignment.target_offset, 8);
    assert_eq!(assignment.offset, 12);
}

#[test]
fn low_relocation_rejects_equal_names_with_different_physical_targets() {
    let relocation = artifact::SymbolRelocation {
        reference: crate::SymbolReference::Captured {
            artifact_sha256: "a".repeat(64),
            location: crate::SymbolLocation {
                object: crate::ObjectLocation::ArchiveMember { ordinal: 0 },
                table: artifact::ArtifactSymbolTable::Static,
                index: 1,
            },
            binding: crate::SymbolBinding::LocalDefinition,
        },
        address: 4,
        kind: artifact::RelocationKind::PcRelLo12I,
        symbol: "cell".into(),
        addend: 0,
    };
    let owner = symbol(vec![], vec![relocation.clone()]);
    let matching = relocated_root(&owner, &relocation);
    assert!(
        low_relocation_value(&owner, 4, &matching)
            .unwrap()
            .is_some()
    );
    let mut wrong = relocation;
    if let crate::SymbolReference::Captured { location, .. } = &mut wrong.reference {
        location.index = 2;
    }
    let mismatched = relocated_root(&owner, &wrong);
    assert!(
        low_relocation_value(&owner, 4, &mismatched)
            .unwrap()
            .is_none()
    );
}

#[test]
fn low_relocation_keeps_matching_pointer_when_another_branch_is_unknown() {
    let relocation = artifact::SymbolRelocation {
        reference: crate::SymbolReference::Unknown {
            reason: "fixture".into(),
        },
        address: 4,
        kind: artifact::RelocationKind::PcRelLo12I,
        symbol: "cell".into(),
        addend: 0,
    };
    let owner = symbol(vec![], vec![relocation]);
    let known = relocated_root(&owner, &owner.relocations[0]);
    let joined = Value::Alternatives(vec![Value::Unknown, known.clone()]);
    let (_, retained) = low_relocation_value(&owner, 4, &joined).unwrap().unwrap();
    assert_eq!(retained.atoms(), &[Value::Unknown, known]);
}
