use super::*;
use crate::FloatingRoundingMode;
use object::{SectionKind, SymbolKind};
use rv_asm::{Inst, IsCompressed, Reg, Xlen};

use super::model::riscv_relocation_kind;

#[test]
fn captured_linked_data_and_execution_survive_input_replacement() {
    use crate::execution::ExecutableImage;
    use object::{
        Architecture, BinaryFormat, Endianness, SymbolFlags, SymbolScope,
        write::{Object, Symbol, SymbolSection},
    };

    let linked_data = |name: &str, address: u32, value: u8| {
        let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
        let section = object.add_section(Vec::new(), b".data".to_vec(), SectionKind::Data);
        object.append_section_data(section, &[value, 0, 0, 0], 4);
        object.add_symbol(Symbol {
            name: name.as_bytes().to_vec(),
            value: u64::from(address),
            size: 4,
            kind: SymbolKind::Data,
            scope: SymbolScope::Dynamic,
            weak: false,
            section: SymbolSection::Section(section),
            flags: SymbolFlags::None,
        });
        let mut bytes = object.write().unwrap();
        // Give this synthetic ELF an addressed data section, as in ROM images
        // without a corresponding PT_LOAD segment.
        bytes[16..18].copy_from_slice(&object::elf::ET_EXEC.to_le_bytes());
        let headers = u32::from_le_bytes(bytes[32..36].try_into().unwrap()) as usize;
        let data_address = headers + 40 + 12;
        bytes[data_address..data_address + 4].copy_from_slice(&address.to_le_bytes());
        bytes
    };
    let directory = tempfile::tempdir().unwrap();
    let primary_path = directory.path().join("primary.elf");
    let companion_path = directory.path().join("companion.elf");
    std::fs::write(
        &primary_path,
        with_duplicate_dynamic_table(linked_data("primary_state", 0x2000, 17)),
    )
    .unwrap();
    std::fs::write(&companion_path, linked_data("companion_state", 0x3000, 29)).unwrap();
    let primary = CapturedArtifact::open(&primary_path).unwrap();
    let companion = CapturedArtifact::open(&companion_path).unwrap();
    // No catalog has been queried yet: lazy parsing must use frozen bytes too.
    std::fs::write(&primary_path, linked_data("replacement", 0x4000, 99)).unwrap();
    std::fs::remove_file(companion_path).unwrap();
    let mut image = ExecutableImage::from_captured(&primary).unwrap();
    image.add_captured_companion(&companion).unwrap();
    for (capture, name, address, value) in [
        (&primary, "primary_state", 0x2000, 17),
        (&companion, "companion_state", 0x3000, 29),
    ] {
        assert_eq!(image.symbol_address(name), Some(address));
        assert_eq!(image.loaded_byte(address), Some(value));
        let symbols = capture.data_symbols().unwrap();
        assert_eq!(symbols.len(), if name == "primary_state" { 2 } else { 1 });
        assert_eq!(
            symbols
                .iter()
                .map(|symbol| &symbol.identity)
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            symbols.len()
        );
        assert!(
            symbols
                .iter()
                .all(|symbol| symbol.identity.artifact_sha256() == Some(capture.sha256()))
        );
        assert_eq!(symbols[0].name, name);
        assert_eq!(symbols[0].address, address);
        assert!(std::ptr::eq(symbols, capture.data_symbols().unwrap()));
        let objects = capture.data_objects().unwrap();
        assert!(
            objects
                .iter()
                .any(|object| object.initializer == [value, 0, 0, 0])
        );
    }
    assert_eq!(image.symbol_address("replacement"), None);
}

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
        "blobray-symbol-visibility-{}-{}.o",
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
        "blobray-pcrel-{}-{}.o",
        std::process::id(),
        NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, object.write().unwrap()).unwrap();
    path
}

#[test]
fn decoder_reads_mixed_width_code_without_objdump() {
    let symbol = ArtifactSymbolDefinition {
        identity: ArtifactSymbolDefinition::synthetic_identity(
            module_path!(),
            &(None),
            "synthetic_mixed_width",
            0x1000,
        ),
        member: None,
        name: "synthetic_mixed_width".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x13, 0x00, 0x00, 0x00, // addi zero, zero, 0
            0x01, 0x00, // c.nop
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let decoded = decode_symbol(&symbol).unwrap();
    assert_eq!(decoded.len(), 2);
    assert_eq!(decoded.first().unwrap().width, 4);
    assert_eq!(decoded.last().unwrap().width, 2);
}

#[test]
fn analysis_decoder_preserves_explicit_extension_blockers() {
    let flw = 0x0005_2007_u32;
    let csrrw = (0x300_u32 << 20) | (10 << 15) | (1 << 12) | (11 << 7) | 0x73;
    let custom = 0x0000_000b_u32;
    let addi = 0x0015_0513_u32;
    let symbol = ArtifactSymbolDefinition {
        identity: ArtifactSymbolDefinition::synthetic_identity(
            module_path!(),
            &(None),
            "extension_mix",
            0x2000,
        ),
        member: None,
        name: "extension_mix".to_owned(),
        address: 0x2000,
        bytes: [flw, addi, csrrw, addi, custom, addi]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect(),
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };

    assert!(decode_symbol(&symbol).is_err());
    let decoded = decode_symbol_for_analysis(&symbol).unwrap();
    assert_eq!(decoded.len(), 6);
    assert!(matches!(decoded[1], AnalysisInstruction::Supported(_)));
    assert_eq!(decoded[0].address(), 0x2000);
    assert!(matches!(
        decoded[0],
        AnalysisInstruction::Unsupported(UnsupportedInstruction {
            class: UnsupportedInstructionClass::FloatingPoint,
            linear_control_flow: true,
            ..
        })
    ));
    assert!(matches!(
        decoded[2],
        AnalysisInstruction::Unsupported(UnsupportedInstruction {
            class: UnsupportedInstructionClass::Csr,
            integer_destination: Some(11),
            linear_control_flow: true,
            ..
        })
    ));
    assert!(matches!(
        decoded[4],
        AnalysisInstruction::Unsupported(UnsupportedInstruction {
            class: UnsupportedInstructionClass::VendorCustom,
            linear_control_flow: false,
            ..
        })
    ));
}

#[test]
fn analysis_decoder_classifies_compressed_float_memory_operations() {
    // Quadrant 0, funct3 011 is C.FLW on RV32 with the F extension.
    let symbol = ArtifactSymbolDefinition {
        identity: ArtifactSymbolDefinition::synthetic_identity(
            module_path!(),
            &(None),
            "compressed_float",
            0x3000,
        ),
        member: None,
        name: "compressed_float".to_owned(),
        address: 0x3000,
        bytes: 0x6000_u16.to_le_bytes().to_vec(),
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let decoded = decode_symbol_for_analysis(&symbol).unwrap();
    assert!(matches!(
        decoded.as_slice(),
        [AnalysisInstruction::Unsupported(UnsupportedInstruction {
            class: UnsupportedInstructionClass::FloatingPoint,
            width: 2,
            linear_control_flow: true,
            ..
        })]
    ));
}

#[test]
fn analysis_decoder_preserves_zero_fill_as_ambiguous_trap_evidence() {
    let symbol = ArtifactSymbolDefinition {
        identity: ArtifactSymbolDefinition::synthetic_identity(
            module_path!(),
            &(None),
            "zero_fill",
            0x3800,
        ),
        member: None,
        name: "zero_fill".to_owned(),
        address: 0x3800,
        bytes: 0_u16.to_le_bytes().to_vec(),
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let decoded = decode_symbol_for_analysis(&symbol).unwrap();
    assert!(matches!(
        decoded.as_slice(),
        [AnalysisInstruction::Unsupported(UnsupportedInstruction {
            class: UnsupportedInstructionClass::ZeroFillOrIllegalTrap,
            width: 2,
            raw: 0,
            linear_control_flow: false,
            ..
        })]
    ));
}

#[test]
fn reachable_blockers_exclude_padding_after_return() {
    let symbol = ArtifactSymbolDefinition {
        identity: ArtifactSymbolDefinition::synthetic_identity(
            module_path!(),
            &(None),
            "entry",
            0x3900,
        ),
        member: None,
        name: "entry".to_owned(),
        address: 0x3900,
        bytes: [
            0x0020_29f3_u32.to_le_bytes().as_slice(),
            [0x67, 0x80, 0x00, 0x00].as_slice(),
            [0x00, 0x00, 0x00, 0x00].as_slice(),
        ]
        .concat(),
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };

    let blockers = reachable_unsupported_instructions(&symbol).unwrap();

    assert_eq!(blockers.len(), 1);
    assert_eq!(blockers[0].address, symbol.address);
    assert_eq!(
        blockers[0].class,
        UnsupportedInstructionClass::FloatingPointCsr
    );
}

#[test]
fn unsupported_mnemonics_make_review_evidence_actionable() {
    assert_eq!(unsupported_instruction_mnemonic(4, 0x0005_2787), "flw");
    assert_eq!(unsupported_instruction_mnemonic(4, 0x00f5_2027), "fsw");
    assert_eq!(unsupported_instruction_mnemonic(4, 0xa0b5_27d3), "feq.s");
    assert_eq!(unsupported_instruction_mnemonic(4, 0x0020_29f3), "csrrs");
    assert_eq!(unsupported_instruction_mnemonic(2, 0), "illegal-zero");
}

#[test]
fn floating_memory_decoder_handles_standard_and_compressed_forms() {
    let blocker = |width, raw| UnsupportedInstruction {
        address: 0x2000,
        width,
        raw,
        class: UnsupportedInstructionClass::FloatingPoint,
        integer_destination: None,
        linear_control_flow: true,
    };
    let flw = (4_u32 << 20) | (10 << 15) | (2 << 12) | (15 << 7) | 0x07;
    let fsw = (15_u32 << 20) | (10 << 15) | (2 << 12) | (8 << 7) | 0x27;

    assert_eq!(
        decode_floating_memory_instruction(blocker(4, flw)),
        Some(FloatingMemoryInstruction {
            address: 0x2000,
            instruction_width: 4,
            access: FloatingMemoryAccess::Load,
            floating_register: 15,
            base: Reg::A0,
            offset: 4,
        })
    );
    assert_eq!(
        decode_floating_memory_instruction(blocker(4, fsw)),
        Some(FloatingMemoryInstruction {
            address: 0x2000,
            instruction_width: 4,
            access: FloatingMemoryAccess::Store,
            floating_register: 15,
            base: Reg::A0,
            offset: 8,
        })
    );
    assert_eq!(
        decode_floating_memory_instruction(blocker(2, 0xec26)),
        Some(FloatingMemoryInstruction {
            address: 0x2000,
            instruction_width: 2,
            access: FloatingMemoryAccess::Store,
            floating_register: 9,
            base: Reg::SP,
            offset: 24,
        })
    );
    assert_eq!(
        decode_floating_memory_instruction(blocker(2, 0x7d00)),
        Some(FloatingMemoryInstruction {
            address: 0x2000,
            instruction_width: 2,
            access: FloatingMemoryAccess::Load,
            floating_register: 8,
            base: Reg::A0,
            offset: 56,
        })
    );
}

#[test]
fn floating_decoder_only_invalidates_real_integer_destinations() {
    let instruction = |funct7: u32| {
        let raw = (funct7 << 25) | (11 << 20) | (10 << 15) | (2 << 12) | (15 << 7) | 0x53;
        let symbol = ArtifactSymbolDefinition {
            identity: ArtifactSymbolDefinition::synthetic_identity(
                module_path!(),
                &(None),
                "floating_destination",
                0,
            ),
            member: None,
            name: "floating_destination".to_owned(),
            address: 0,
            bytes: raw.to_le_bytes().to_vec(),
            addresses_resolved: true,
            memory_regions: Default::default(),
            relocations: Vec::new(),
        };
        let decoded = decode_symbol_for_analysis(&symbol).unwrap();
        let [AnalysisInstruction::Unsupported(blocker)] = decoded.as_slice() else {
            panic!("expected one floating blocker");
        };
        *blocker
    };

    assert_eq!(instruction(0x00).integer_destination, None); // fadd.s f15, f10, f11
    assert_eq!(instruction(0x50).integer_destination, Some(15)); // feq.s x15, f10, f11
}

#[test]
fn floating_data_decoder_accepts_exact_structural_operations() {
    let blocker = |raw| UnsupportedInstruction {
        address: 0,
        width: 4,
        raw,
        class: UnsupportedInstructionClass::FloatingPoint,
        integer_destination: None,
        linear_control_flow: true,
    };
    let encode = |funct7: u32, funct3: u32, destination: u32, source1: u32, source2: u32| {
        (funct7 << 25)
            | (source2 << 20)
            | (source1 << 15)
            | (funct3 << 12)
            | (destination << 7)
            | 0x53
    };

    assert_eq!(
        decode_floating_data_instruction(blocker(encode(0x78, 0, 10, 11, 0))),
        Some(FloatingDataInstruction {
            operation: FloatingDataOperation::MoveFromInteger,
            destination: 10,
            source1: 11,
            source2: 0,
            source3: None,
            rounding: None,
        })
    );
    assert_eq!(
        decode_floating_data_instruction(blocker(encode(0x10, 2, 10, 11, 12))),
        Some(FloatingDataInstruction {
            operation: FloatingDataOperation::SignXor,
            destination: 10,
            source1: 11,
            source2: 12,
            source3: None,
            rounding: None,
        })
    );
    assert_eq!(
        decode_floating_data_instruction(blocker(encode(0x50, 2, 10, 11, 12))),
        Some(FloatingDataInstruction {
            operation: FloatingDataOperation::CompareEqual,
            destination: 10,
            source1: 11,
            source2: 12,
            source3: None,
            rounding: None,
        })
    );
    assert_eq!(
        decode_floating_data_instruction(blocker(encode(0x00, 0, 10, 11, 12))),
        None,
        "fadd.s is arithmetic, not a bit-preserving operation"
    );
}

#[test]
fn floating_data_decoder_accepts_rx11ax_ampdu_limit_sequence() {
    let blocker = |raw| UnsupportedInstruction {
        address: 0x1000,
        width: 4,
        raw,
        class: UnsupportedInstructionClass::FloatingPoint,
        integer_destination: None,
        linear_control_flow: true,
    };
    let cases = [
        (
            0xd005_77d3,
            FloatingDataInstruction {
                operation: FloatingDataOperation::SignedWordToSingle,
                destination: 15,
                source1: 10,
                source2: 0,
                source3: None,
                rounding: Some(FloatingRoundingMode::Dynamic),
            },
        ),
        (
            0x08d7_f7d3,
            FloatingDataInstruction {
                operation: FloatingDataOperation::SubtractSingle,
                destination: 15,
                source1: 15,
                source2: 13,
                source3: None,
                rounding: Some(FloatingRoundingMode::Dynamic),
            },
        ),
        (
            0x18d7_f7d3,
            FloatingDataInstruction {
                operation: FloatingDataOperation::DivideSingle,
                destination: 15,
                source1: 15,
                source2: 13,
                source3: None,
                rounding: Some(FloatingRoundingMode::Dynamic),
            },
        ),
        (
            0x40f7_7743,
            FloatingDataInstruction {
                operation: FloatingDataOperation::FusedMultiplyAddSingle,
                destination: 14,
                source1: 14,
                source2: 15,
                source3: Some(8),
                rounding: Some(FloatingRoundingMode::Dynamic),
            },
        ),
        (
            0xc007_17d3,
            FloatingDataInstruction {
                operation: FloatingDataOperation::SingleToSignedWord,
                destination: 15,
                source1: 14,
                source2: 0,
                source3: None,
                rounding: Some(FloatingRoundingMode::TowardZero),
            },
        ),
    ];

    for (raw, expected) in cases {
        assert_eq!(
            decode_floating_data_instruction(blocker(raw)),
            Some(expected)
        );
    }

    assert_eq!(
        decode_floating_data_instruction(blocker(0x08d7_d7d3)),
        None,
        "reserved rounding encodings must remain blockers"
    );
}

#[test]
fn analysis_decoder_distinguishes_standard_float_and_vendor_csrs() {
    let csr = |address: u32| (address << 20) | (2 << 12) | (10 << 7) | 0x73;
    let symbol = ArtifactSymbolDefinition {
        identity: ArtifactSymbolDefinition::synthetic_identity(
            module_path!(),
            &(None),
            "csr_classes",
            0x4000,
        ),
        member: None,
        name: "csr_classes".to_owned(),
        address: 0x4000,
        bytes: [csr(0x300), csr(0x001), csr(0x7c1), csr(0xbcc)]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect(),
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let classes = decode_symbol_for_analysis(&symbol)
        .unwrap()
        .into_iter()
        .map(|instruction| match instruction {
            AnalysisInstruction::Unsupported(blocker) => blocker.class,
            AnalysisInstruction::Supported(instruction) => {
                panic!("unexpected supported CSR: {instruction:?}")
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(
        classes,
        [
            UnsupportedInstructionClass::Csr,
            UnsupportedInstructionClass::FloatingPointCsr,
            UnsupportedInstructionClass::VendorCsr,
            UnsupportedInstructionClass::VendorCsr,
        ]
    );
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
fn reviewed_section_range_becomes_a_code_symbol_without_changing_elf_symbols() {
    let path = write_visibility_fixture();
    let reviewed = load_reviewed_code_ranges(
        &path,
        &[ReviewedCodeRange {
            member: None,
            section: ".text".to_owned(),
            name: "recovered_gap".to_owned(),
            start_offset: 8,
            end_offset: 12,
        }],
    )
    .unwrap();
    let ordinary = load_code_symbols(&path, "", CodeSymbolSelection::All).unwrap();
    std::fs::remove_file(path).unwrap();

    assert_eq!(reviewed.len(), 1);
    assert_eq!(reviewed[0].name, "recovered_gap");
    assert_eq!(reviewed[0].address, 8);
    assert_eq!(reviewed[0].bytes, [0, 0, 0, 0]);
    assert!(ordinary.iter().all(|symbol| symbol.name != "recovered_gap"));
}

#[test]
fn reviewed_section_range_fails_closed_outside_section() {
    let path = write_visibility_fixture();
    let error = load_reviewed_code_ranges(
        &path,
        &[ReviewedCodeRange {
            member: None,
            section: ".text".to_owned(),
            name: "oversized".to_owned(),
            start_offset: 8,
            end_offset: 16,
        }],
    )
    .unwrap_err();
    std::fs::remove_file(path).unwrap();
    assert!(error.to_string().contains("invalid section offsets"));
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
        identity: ArtifactSymbolDefinition::synthetic_identity(
            module_path!(),
            &(None),
            "relocated_call",
            0x1000,
        ),
        member: None,
        name: "relocated_call".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x97, 0x00, 0x00, 0x00, // auipc ra, 0
            0xe7, 0x80, 0x00, 0x00, // jalr ra, 0(ra)
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
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
fn artifact_container_probe_reads_an_elf_without_inventory_projection() {
    let path = write_visibility_fixture();
    let container = inspect_artifact_container(&path).unwrap();
    std::fs::remove_file(path).unwrap();

    assert_eq!(container, ArtifactContainerKind::Elf32);
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

#[test]
fn archive_inventory_distinguishes_non_code_payloads_from_unknown_members() {
    use object::{Architecture, BinaryFormat, Endianness, write::Object};
    use std::io::Write as _;
    let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let section = object.add_section(Vec::new(), b".data".to_vec(), SectionKind::Data);
    object.append_section_data(section, &[1, 2, 3, 4], 4);
    let data = object.write().unwrap();
    let mut archive = b"!<arch>\n".to_vec();
    for (name, bytes) in [
        ("data.o/", data.as_slice()),
        ("unknown.bin/", b"unclassified".as_slice()),
    ] {
        writeln!(
            archive,
            "{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`",
            name,
            0,
            0,
            0,
            "100644",
            bytes.len()
        )
        .unwrap();
        archive.extend_from_slice(bytes);
        if !bytes.len().is_multiple_of(2) {
            archive.push(b'\n');
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("inventory.a");
    std::fs::write(&path, archive).unwrap();
    let inventory = inspect_artifact(&path).unwrap();
    assert_eq!(inventory.members[0].name, "data.o");
    assert_eq!(inventory.members[0].status, "non-code");
    assert!(inventory.members[0].reason.is_none());
    assert_eq!(inventory.members[1].name, "unknown.bin");
    assert_eq!(inventory.members[1].status, "unrecognized");
    assert!(inventory.members[1].reason.is_some());
    assert_eq!(inventory.skipped_members, 1);
}

#[test]
fn captured_archive_preserves_repeated_member_names_and_immutable_bytes() {
    use std::io::Write as _;
    let object_path = write_visibility_fixture();
    let bytes = std::fs::read(&object_path).unwrap();
    std::fs::remove_file(object_path).unwrap();
    let mut archive = b"!<arch>\n".to_vec();
    for _ in 0..2 {
        writeln!(
            archive,
            "{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`",
            "same.o/",
            0,
            0,
            0,
            "100644",
            bytes.len()
        )
        .unwrap();
        archive.extend_from_slice(&bytes);
        if !bytes.len().is_multiple_of(2) {
            archive.push(b'\n');
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("repeated.a");
    std::fs::write(&path, archive).unwrap();
    let capture = CapturedArtifact::open(&path).unwrap();
    let digest = capture.sha256().to_owned();
    let inventory = inspect_artifact(&path).unwrap();
    assert_eq!(inventory.objects.len(), 2);
    assert_eq!(inventory.objects[0].member, inventory.objects[1].member);
    assert_ne!(inventory.objects[0].location, inventory.objects[1].location);
    std::fs::write(&path, b"replaced after capture").unwrap();
    let symbols = capture
        .code_symbols("exported_function", CodeSymbolSelection::Exported)
        .unwrap();
    assert_eq!(symbols.len(), 2);
    assert_ne!(symbols[0].location, symbols[1].location);
    assert_ne!(
        symbols[0].definition.identity,
        symbols[1].definition.identity
    );
    for symbol in symbols {
        let body = inspect_captured_function(&path, &capture, symbol.location).unwrap();
        assert_eq!(body.code_identity, symbol.definition.identity);
        assert!(
            body.labels
                .iter()
                .any(|label| label.name == "exported_function")
        );
        let selected = capture.code_symbol(symbol.location).unwrap().unwrap();
        assert!(
            std::ptr::eq(selected, symbol),
            "queries share the same parsed object"
        );
        assert_eq!(selected.definition.bytes, [0xef, 0x00, 0x80, 0x00]);
    }
    assert_eq!(capture.sha256(), digest);
}

#[test]
fn unnamed_symbol_definitions_retain_their_table_identity() {
    use object::{
        Architecture, BinaryFormat, Endianness, SymbolFlags, SymbolScope,
        write::{Object, Symbol, SymbolSection},
    };
    let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let section = object.add_section(Vec::new(), b".text".to_vec(), SectionKind::Text);
    object.append_section_data(section, &[0x67, 0x80, 0, 0], 4);
    object.add_symbol(Symbol {
        name: Vec::new(),
        value: 0,
        size: 4,
        kind: SymbolKind::Text,
        scope: SymbolScope::Compilation,
        weak: false,
        section: SymbolSection::Section(section),
        flags: SymbolFlags::None,
    });
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("unnamed.o");
    std::fs::write(&path, object.write().unwrap()).unwrap();
    let inventory = inspect_artifact(&path).unwrap();
    let fact = inventory.objects[0]
        .symbols
        .iter()
        .find(|symbol| symbol.name.is_empty() && symbol.kind == ArtifactSymbolKind::Text)
        .unwrap();
    let capture = CapturedArtifact::open(&path).unwrap();
    let symbol = capture
        .code_symbol(open_radio_vendor_analysis_model::SymbolLocation {
            object: inventory.objects[0].location,
            table: fact.table,
            index: fact.index,
        })
        .unwrap()
        .unwrap();
    assert!(symbol.definition.name.is_empty());
    assert_eq!(symbol.definition.bytes, [0x67, 0x80, 0, 0]);
}

#[test]
fn entirely_unknown_archive_payloads_remain_inspectable() {
    use std::io::Write as _;
    let mut bytes = b"!<arch>\n".to_vec();
    let opaque = b"opaque";
    writeln!(
        bytes,
        "{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`",
        "unknown.bin/",
        0,
        0,
        0,
        "100644",
        opaque.len()
    )
    .unwrap();
    bytes.extend_from_slice(opaque);
    let capture = CapturedArtifact::from_data(&bytes).unwrap();
    let inventory = capture.inventory().unwrap();
    assert_eq!(inventory.objects.len(), 0);
    assert_eq!(inventory.skipped_members, 1);
    assert_eq!(inventory.members[0].status, "unrecognized");
    assert!(inventory.members[0].reason.is_some());
    assert_eq!(
        capture
            .object_bytes(capture.objects()[0].location())
            .unwrap(),
        Some(opaque.as_slice())
    );
    assert!(std::ptr::eq(capture.inventory().unwrap(), inventory));
}

#[test]
fn thin_archive_without_external_capture_has_an_explicit_gap() {
    use std::io::Write as _;
    let mut bytes = b"!<thin>\n".to_vec();
    writeln!(
        bytes,
        "{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`",
        "missing.o/", 0, 0, 0, "100644", 128
    )
    .unwrap();
    let capture = CapturedArtifact::from_data(&bytes).unwrap();
    let inventory = capture.inventory().unwrap();
    assert_eq!(inventory.members.len(), 1);
    assert_eq!(inventory.members[0].status, "unavailable");
    assert!(
        capture
            .object_bytes(capture.objects()[0].location())
            .is_err()
    );
    assert!(capture.code_symbols("", CodeSymbolSelection::All).is_err());
}

fn with_duplicate_dynamic_table(mut bytes: Vec<u8>) -> Vec<u8> {
    let table_offset = u32::from_le_bytes(bytes[32..36].try_into().unwrap()) as usize;
    let stride = u16::from_le_bytes(bytes[46..48].try_into().unwrap()) as usize;
    let count = u16::from_le_bytes(bytes[48..50].try_into().unwrap());
    let headers = bytes[table_offset..table_offset + stride * usize::from(count)].to_vec();
    let mut dynamic = headers
        .chunks_exact(stride)
        .find(|header| {
            u32::from_le_bytes(header[4..8].try_into().unwrap()) == object::elf::SHT_SYMTAB
        })
        .unwrap()
        .to_vec();
    // Both tables deliberately describe the same symbols. Table ownership still
    // distinguishes the occurrences; names and addresses cannot be their key.
    dynamic[4..8].copy_from_slice(&object::elf::SHT_DYNSYM.to_le_bytes());
    while !bytes.len().is_multiple_of(4) {
        bytes.push(0);
    }
    let offset = u32::try_from(bytes.len()).unwrap();
    bytes.extend_from_slice(&headers);
    bytes.extend_from_slice(&dynamic);
    bytes[32..36].copy_from_slice(&offset.to_le_bytes());
    bytes[48..50].copy_from_slice(&(count + 1).to_le_bytes());
    bytes
}

#[test]
fn data_catalog_retains_unnamed_symbols_colocated_anchors_and_both_tables() {
    use object::{
        Architecture, BinaryFormat, Endianness, SymbolFlags, SymbolScope,
        write::{Object, Symbol, SymbolSection},
    };
    let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let section = object.add_section(Vec::new(), b".data".to_vec(), SectionKind::Data);
    object.append_section_data(section, &[1, 2, 3, 4, 5, 6, 7, 8], 4);
    for (name, value, size, kind) in [
        ("anchor", 0, 0, SymbolKind::Label),
        ("alias", 0, 0, SymbolKind::Label),
        ("sized", 0, 4, SymbolKind::Data),
        ("", 4, 4, SymbolKind::Data),
        ("tail", 0, 0, SymbolKind::Data),
    ] {
        object.add_symbol(Symbol {
            name: name.as_bytes().to_vec(),
            value,
            size,
            kind,
            scope: SymbolScope::Compilation,
            weak: false,
            section: SymbolSection::Section(section),
            flags: SymbolFlags::None,
        });
    }
    let bytes = with_duplicate_dynamic_table(object.write().unwrap());
    let capture = CapturedArtifact::from_data(&bytes).unwrap();
    let objects = capture.data_objects().unwrap();
    assert_eq!(objects.len(), 10);
    let identities = objects
        .iter()
        .map(|object| &object.identity)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(identities.len(), objects.len());
    for name in ["anchor", "alias", "sized", "", "tail"] {
        let definitions = objects
            .iter()
            .filter(|object| object.name == name)
            .collect::<Vec<_>>();
        assert_eq!(definitions.len(), 2);
        for definition in definitions {
            assert_eq!(
                definition.synthetic_from_anchor,
                matches!(name, "anchor" | "alias" | "tail")
            );
            assert_eq!(
                definition.initializer,
                if name.is_empty() {
                    vec![5, 6, 7, 8]
                } else {
                    vec![1, 2, 3, 4]
                }
            );
        }
    }
}

#[test]
fn static_and_dynamic_tables_are_both_queryable_without_merging_occurrences() {
    let path = write_visibility_fixture();
    let bytes = with_duplicate_dynamic_table(std::fs::read(&path).unwrap());
    std::fs::remove_file(path).unwrap();
    let capture = CapturedArtifact::from_data(&bytes).unwrap();
    let definitions = capture
        .code_symbols("exported_function", CodeSymbolSelection::Exported)
        .unwrap();
    assert_eq!(definitions.len(), 2);
    assert_ne!(definitions[0].location.table, definitions[1].location.table);
    assert_ne!(
        definitions[0].definition.identity,
        definitions[1].definition.identity
    );
    for symbol in definitions {
        let body = inspect_captured_function(
            std::path::Path::new("<captured>"),
            &capture,
            symbol.location,
        )
        .unwrap();
        assert_eq!(body.code_identity, symbol.definition.identity);
        assert!(
            body.labels
                .iter()
                .any(|label| label.name == "exported_function")
        );
    }
}
