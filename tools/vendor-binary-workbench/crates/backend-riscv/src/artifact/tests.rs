use super::*;
use object::{SectionKind, SymbolKind};
use rv_asm::{Inst, IsCompressed, Xlen};

use super::model::riscv_relocation_kind;

fn write_visibility_fixture() -> std::path::PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use object::{
        Architecture, BinaryFormat, Endianness, SymbolFlags, SymbolScope,
        write::{Object, Symbol, SymbolSection},
    };

    let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let section = object.add_section(Vec::new(), b".text".to_vec(), SectionKind::Text);
    let global_offset = object.append_section_data(section, &[0xef, 0x00, 0x80, 0x00], 4);
    let local_offset = object.append_section_data(section, &[0x67, 0x80, 0x00, 0x00], 4);
    let gap_offset = object.append_section_data(section, &[0, 0, 0, 0], 4);
    for (name, value, scope) in [
        ("exported_function", global_offset, SymbolScope::Dynamic),
        ("private_function", local_offset, SymbolScope::Compilation),
    ] {
        object.add_symbol(Symbol {
            name: name.as_bytes().to_vec(),
            value,
            size: 4,
            kind: SymbolKind::Text,
            scope,
            weak: false,
            section: SymbolSection::Section(section),
            flags: SymbolFlags::None,
        });
    }
    object.add_symbol(Symbol {
        name: b"hidden_entry".to_vec(),
        value: gap_offset,
        size: 0,
        kind: SymbolKind::Text,
        scope: SymbolScope::Compilation,
        weak: false,
        section: SymbolSection::Section(section),
        flags: SymbolFlags::None,
    });
    object.add_symbol(Symbol {
        name: b"external_service".to_vec(),
        value: 0,
        size: 0,
        kind: SymbolKind::Text,
        scope: SymbolScope::Dynamic,
        weak: false,
        section: SymbolSection::Undefined,
        flags: SymbolFlags::None,
    });
    static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "vendor-workbench-symbol-visibility-{}-{}.o",
        std::process::id(),
        NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, object.write().unwrap()).unwrap();
    path
}

fn write_pcrel_fixture(got: bool, include_high: bool) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use object::{
        Architecture, BinaryFormat, Endianness, RelocationFlags, SymbolFlags, SymbolScope,
        write::{Object, Relocation, Symbol, SymbolSection},
    };

    let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let section = object.add_section(Vec::new(), b".text".to_vec(), SectionKind::Text);
    let bytes = if got {
        vec![
            0x97, 0x07, 0x00, 0x00, // auipc a5, 0 (GOT address)
            0x83, 0xa7, 0x07, 0x00, // lw a5, 0(a5) (symbol address from GOT)
            0x83, 0xa7, 0x07, 0x00, // lw a5, 0(a5) (pointer cell)
            0x83, 0xa2, 0x07, 0x01, // lw t0, 16(a5) (slot)
            0xe7, 0x80, 0x02, 0x00, // jalr ra, 0(t0)
        ]
    } else {
        vec![
            0x97, 0x07, 0x00, 0x00, // auipc a5, 0
            0x83, 0xa7, 0x07, 0x00, // lw a5, 0(a5) (pointer cell)
            0x83, 0xa2, 0x07, 0x01, // lw t0, 16(a5) (slot)
            0xe7, 0x80, 0x02, 0x00, // jalr ra, 0(t0)
        ]
    };
    let function_offset = object.append_section_data(section, &bytes, 4);
    object.add_symbol(Symbol {
        name: b"vendor_callback".to_vec(),
        value: function_offset,
        size: bytes.len() as u64,
        kind: SymbolKind::Text,
        scope: SymbolScope::Dynamic,
        weak: false,
        section: SymbolSection::Section(section),
        flags: SymbolFlags::None,
    });
    let label = object.add_symbol(Symbol {
        name: b".Lpcrel0".to_vec(),
        value: function_offset,
        size: 0,
        kind: SymbolKind::Label,
        scope: SymbolScope::Compilation,
        weak: false,
        section: SymbolSection::Section(section),
        flags: SymbolFlags::None,
    });
    let external = object.add_symbol(Symbol {
        name: b"g_services".to_vec(),
        value: 0,
        size: 0,
        kind: SymbolKind::Data,
        scope: SymbolScope::Dynamic,
        weak: false,
        section: SymbolSection::Undefined,
        flags: SymbolFlags::None,
    });
    if include_high {
        object
            .add_relocation(
                section,
                Relocation {
                    offset: function_offset,
                    symbol: external,
                    addend: 0,
                    flags: RelocationFlags::Elf {
                        r_type: if got {
                            object::elf::R_RISCV_GOT_HI20
                        } else {
                            object::elf::R_RISCV_PCREL_HI20
                        },
                    },
                },
            )
            .unwrap();
    }
    object
        .add_relocation(
            section,
            Relocation {
                offset: function_offset + 4,
                symbol: label,
                addend: 0,
                flags: RelocationFlags::Elf {
                    r_type: object::elf::R_RISCV_PCREL_LO12_I,
                },
            },
        )
        .unwrap();

    static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "vendor-workbench-pcrel-{}-{}.o",
        std::process::id(),
        NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, object.write().unwrap()).unwrap();
    path
}

#[test]
fn decoder_reads_mixed_width_code_without_objdump() {
    let symbol = ArtifactSymbolDefinition {
        member: None,
        name: "synthetic_mixed_width".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x13, 0x00, 0x00, 0x00, // addi zero, zero, 0
            0x01, 0x00, // c.nop
        ],
        addresses_resolved: true,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let decoded = decode_symbol(&symbol).unwrap();
    assert_eq!(decoded.len(), 2);
    assert_eq!(decoded.first().unwrap().width, 4);
    assert_eq!(decoded.last().unwrap().width, 2);
}

#[test]
fn compressed_andi_immediate_is_sign_extended() {
    // c.andi a5, -2
    let (instruction, width) = Inst::decode(0x9b_f9, Xlen::Rv32).unwrap();
    let Inst::Andi { imm, .. } = instruction else {
        panic!("expected C.ANDI, got {instruction}");
    };
    assert_eq!(width, IsCompressed::Yes);
    assert_eq!(imm.as_u32(), 0x3e); // rv-asm 0.2.1 decoder behavior
    assert_eq!(andi_immediate(imm, 2), 0xffff_fffe);
}

#[test]
fn memory_region_requires_a_complete_supported_width() {
    let region = MemoryRegion {
        start: 0x1000,
        length: 4,
        writable: true,
        name: ".data".to_owned(),
    };
    assert!(region.contains(0x1000, 32));
    assert!(region.contains(0x1003, 8));
    assert!(!region.contains(0x1003, 16));
    assert!(!region.contains(0x1000, 24));
}

#[test]
fn recognizes_both_riscv_call_relocation_kinds() {
    assert_eq!(
        riscv_relocation_kind(object::elf::R_RISCV_CALL),
        Some(RelocationKind::Call)
    );
    assert_eq!(
        riscv_relocation_kind(object::elf::R_RISCV_CALL_PLT),
        Some(RelocationKind::CallPlt)
    );
}

#[test]
fn recognizes_absolute_pc_relative_and_got_relocation_kinds() {
    assert_eq!(
        riscv_relocation_kind(object::elf::R_RISCV_PCREL_HI20),
        Some(RelocationKind::PcRelHi20)
    );
    assert_eq!(
        riscv_relocation_kind(object::elf::R_RISCV_PCREL_LO12_I),
        Some(RelocationKind::PcRelLo12I)
    );
    assert_eq!(
        riscv_relocation_kind(object::elf::R_RISCV_GOT_HI20),
        Some(RelocationKind::GotHi20)
    );
}

#[test]
fn pcrel_low_is_normalized_from_local_label_to_actual_symbol() {
    let path = write_pcrel_fixture(false, true);
    let symbols = load_code_symbols(&path, "vendor_callback", CodeSymbolSelection::All).unwrap();
    std::fs::remove_file(path).unwrap();

    assert_eq!(symbols.len(), 1);
    assert_eq!(
        symbols[0]
            .relocations
            .iter()
            .map(|relocation| (
                relocation.address,
                relocation.kind,
                relocation.symbol.as_str()
            ))
            .collect::<Vec<_>>(),
        [
            (0, RelocationKind::PcRelHi20, "g_services"),
            (4, RelocationKind::PcRelLo12I, "g_services"),
        ]
    );
    let calls = crate::discover_interface_calls(&symbols[0]).unwrap();
    assert_eq!(calls.len(), 1);
    assert!(matches!(
        calls[0].target.root,
        crate::InterfaceRoot::RelocatedSymbol {
            addressing: crate::InterfaceSymbolAddressing::PcRelative,
            ..
        }
    ));
    assert_eq!(
        calls[0]
            .target
            .loads
            .iter()
            .map(|load| load.offset)
            .collect::<Vec<_>>(),
        [0, 16]
    );
}

#[test]
fn got_pair_resolves_symbol_address_without_inventing_a_table_load() {
    let path = write_pcrel_fixture(true, true);
    let symbols = load_code_symbols(&path, "vendor_callback", CodeSymbolSelection::All).unwrap();
    std::fs::remove_file(path).unwrap();

    assert_eq!(
        symbols[0]
            .relocations
            .iter()
            .map(|relocation| (
                relocation.address,
                relocation.kind,
                relocation.symbol.as_str()
            ))
            .collect::<Vec<_>>(),
        [
            (0, RelocationKind::GotHi20, "g_services"),
            (4, RelocationKind::GotPcRelLo12I, "g_services"),
        ]
    );
    let calls = crate::discover_interface_calls(&symbols[0]).unwrap();
    assert_eq!(calls.len(), 1);
    assert!(matches!(
        calls[0].target.root,
        crate::InterfaceRoot::RelocatedSymbol {
            addressing: crate::InterfaceSymbolAddressing::Got,
            ..
        }
    ));
    assert_eq!(
        calls[0]
            .target
            .loads
            .iter()
            .map(|load| load.offset)
            .collect::<Vec<_>>(),
        [0, 16]
    );
}

#[test]
fn unpaired_pcrel_low_is_rejected_as_malformed_evidence() {
    let path = write_pcrel_fixture(false, false);
    let error = load_code_symbols(&path, "vendor_callback", CodeSymbolSelection::All).unwrap_err();
    std::fs::remove_file(path).unwrap();
    assert!(error.to_string().contains("has no HI20 relocation"));
}

#[test]
fn relocated_call_link_register_distinguishes_call_and_tail_call() {
    let mut symbol = ArtifactSymbolDefinition {
        member: None,
        name: "relocated_call".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x97, 0x00, 0x00, 0x00, // auipc ra, 0
            0xe7, 0x80, 0x00, 0x00, // jalr ra, 0(ra)
        ],
        addresses_resolved: true,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    assert_eq!(relocated_call_is_tail(&symbol, 0x1000), Some(false));

    symbol.bytes[4..].copy_from_slice(&[0x67, 0x80, 0x00, 0x00]); // jalr zero, 0(ra)
    assert_eq!(relocated_call_is_tail(&symbol, 0x1000), Some(true));
    assert_eq!(relocated_call_is_tail(&symbol, 0x1004), None);
}

#[test]
fn code_symbol_selection_explicitly_controls_local_functions() {
    let path = write_visibility_fixture();

    let exported = load_code_symbols(&path, "", CodeSymbolSelection::Exported).unwrap();
    let all = load_code_symbols(&path, "", CodeSymbolSelection::All).unwrap();

    std::fs::remove_file(path).unwrap();
    assert_eq!(
        exported
            .iter()
            .map(|symbol| symbol.name.as_str())
            .collect::<Vec<_>>(),
        ["exported_function"]
    );
    assert_eq!(
        all.iter()
            .map(|symbol| symbol.name.as_str())
            .collect::<Vec<_>>(),
        ["exported_function", "private_function"]
    );
}

#[test]
fn artifact_inventory_preserves_definition_binding_and_section_facts() {
    let path = write_visibility_fixture();
    let inventory = inspect_artifact(&path).unwrap();
    std::fs::remove_file(path).unwrap();

    assert_eq!(inventory.container, ArtifactContainerKind::Elf32);
    assert_eq!(inventory.objects.len(), 1);
    assert_eq!(inventory.objects[0].kind, ArtifactObjectKind::Relocatable);
    let symbols = &inventory.objects[0].symbols;
    let exported = symbols
        .iter()
        .find(|symbol| symbol.name == "exported_function")
        .unwrap();
    assert_eq!(exported.binding, ArtifactSymbolBinding::Global);
    assert_eq!(exported.visibility, ArtifactSymbolVisibility::Default);
    assert_eq!(exported.definition, ArtifactSymbolDefinitionState::Section);
    assert_eq!(exported.section.as_deref(), Some(".text"));
    assert_eq!(exported.scope, ArtifactSymbolScope::Dynamic);
    assert!(exported.is_exported_definition());

    let private = symbols
        .iter()
        .find(|symbol| symbol.name == "private_function")
        .unwrap();
    assert_eq!(private.binding, ArtifactSymbolBinding::Local);
    assert_eq!(private.scope, ArtifactSymbolScope::Compilation);
    assert!(!private.is_exported_definition());

    let external = symbols
        .iter()
        .find(|symbol| symbol.name == "external_service")
        .unwrap();
    assert_eq!(external.binding, ArtifactSymbolBinding::Global);
    assert_eq!(
        external.definition,
        ArtifactSymbolDefinitionState::Undefined
    );
    assert!(!external.is_exported_definition());
}

#[test]
fn artifact_inventory_reports_executable_bytes_without_sized_symbol_coverage() {
    let path = write_visibility_fixture();
    let inventory = inspect_artifact(&path).unwrap();
    std::fs::remove_file(path).unwrap();

    let sections = &inventory.objects[0].code_sections;
    assert_eq!(sections.len(), 1);
    assert_eq!(sections[0].name, ".text");
    assert_eq!(sections[0].size, 12);
    assert_eq!(sections[0].named_sized_symbols, 2);
    assert_eq!(sections[0].named_zero_sized_symbols, 1);
    assert_eq!(sections[0].symbol_covered_bytes, 8);
    assert_eq!(
        sections[0].uncovered_ranges,
        [ArtifactCodeRange {
            start_offset: 8,
            end_offset: 12,
        }]
    );
    assert_eq!(sections[0].function_candidates.len(), 1);
    let candidate = &sections[0].function_candidates[0];
    assert_eq!(candidate.entry_offset, 8);
    assert_eq!(candidate.end_limit_offset, 12);
    assert_eq!(candidate.symbol_names, ["hidden_entry"]);
    assert_eq!(candidate.direct_control_flow.len(), 1);
    assert_eq!(candidate.direct_control_flow[0].caller, "exported_function");
    assert_eq!(candidate.direct_control_flow[0].site_offset, 0);
    assert_eq!(
        candidate.direct_control_flow[0].kind,
        ArtifactDirectControlFlowKind::Call
    );
}
