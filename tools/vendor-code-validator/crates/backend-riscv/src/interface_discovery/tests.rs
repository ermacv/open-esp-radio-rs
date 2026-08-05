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
        memory_regions: Vec::new(),
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
fn control_flow_join_drops_conflicting_pointer_provenance() {
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
    assert!(calls.is_empty());
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
