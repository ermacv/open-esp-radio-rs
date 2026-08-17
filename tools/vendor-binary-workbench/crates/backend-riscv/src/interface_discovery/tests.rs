use super::*;

fn symbol(
    bytes: Vec<u8>,
    relocations: Vec<artifact::SymbolRelocation>,
) -> artifact::ArtifactSymbolDefinition {
    artifact::ArtifactSymbolDefinition {
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
                address: 0,
                kind: artifact::RelocationKind::Hi20,
                symbol: "g_services".to_owned(),
                addend: 0,
            },
            artifact::SymbolRelocation {
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
                address: 2,
                kind: artifact::RelocationKind::Hi20,
                symbol: "g_services".to_owned(),
                addend: 0,
            },
            artifact::SymbolRelocation {
                address: 6,
                kind: artifact::RelocationKind::Lo12I,
                symbol: "g_services".to_owned(),
                addend: 0,
            },
            artifact::SymbolRelocation {
                address: 12,
                kind: artifact::RelocationKind::Hi20,
                symbol: "g_services".to_owned(),
                addend: 0,
            },
            artifact::SymbolRelocation {
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
                address: 0,
                kind: artifact::RelocationKind::Hi20,
                symbol: "g_services".to_owned(),
                addend: 0,
            },
            artifact::SymbolRelocation {
                address: 4,
                kind: artifact::RelocationKind::Lo12I,
                symbol: "g_services".to_owned(),
                addend: 0,
            },
            artifact::SymbolRelocation {
                address: 8,
                kind: artifact::RelocationKind::Hi20,
                symbol: "service_fn".to_owned(),
                addend: 0,
            },
            artifact::SymbolRelocation {
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
        InterfaceRoot::FunctionArgument { index: 1 }
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
                address: 4,
                kind: artifact::RelocationKind::Hi20,
                symbol: "g_services".to_owned(),
                addend: 0,
            },
            artifact::SymbolRelocation {
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
        InterfaceRoot::FunctionArgument { index: 0 }
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
        InterfaceRoot::FunctionArgument { index: 0 }
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
                address: 0,
                kind: artifact::RelocationKind::Hi20,
                symbol: "g_services".to_owned(),
                addend: 0,
            },
            artifact::SymbolRelocation {
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
            InterfaceRoot::FunctionArgument { index: 0 },
            InterfaceRoot::FunctionArgument { index: 5 },
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
        }
    );
    assert_eq!(calls[0].target.slot().unwrap().offset, 16);
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
                address: 0,
                kind: artifact::RelocationKind::PcRelHi20,
                symbol: "g_services".to_owned(),
                addend: 0,
            },
            artifact::SymbolRelocation {
                address: 4,
                kind: artifact::RelocationKind::PcRelLo12I,
                symbol: "g_services".to_owned(),
                addend: 0,
            },
        ],
    );

    assert!(discover_interface_calls(&symbol).unwrap().is_empty());
}
