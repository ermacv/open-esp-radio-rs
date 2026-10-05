use super::*;
use object::write::{
    Object as Writer, Relocation as WriteRelocation, StandardSection, Symbol as WriteSymbol,
    SymbolSection,
};
use object::{Architecture, BinaryFormat, Endianness, SymbolFlags, SymbolScope};

/// A relocatable RV32 object whose `.text` holds `f` and its alias `g` at
/// offset 0 (eight bytes: `lui a5, 0; lw a4, 0(a5)`, both relocated against
/// the external `data`) and `h` at offset 8 (`ret`, calling nothing).
fn object() -> Vec<u8> {
    let mut writer = Writer::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = writer.section_id(StandardSection::Text);
    let code = [0x0000_07b7_u32, 0x0007_a703, 0x0000_8067]
        .iter()
        .flat_map(|w| w.to_le_bytes())
        .collect::<Vec<_>>();
    writer.append_section_data(text, &code, 4);
    let function = |name: &str, value: u64, size: u64| WriteSymbol {
        name: name.as_bytes().to_vec(),
        value,
        size,
        kind: object::SymbolKind::Text,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Section(text),
        flags: SymbolFlags::None,
    };
    writer.add_symbol(function("f", 0, 8));
    writer.add_symbol(function("g", 0, 8));
    writer.add_symbol(function("h", 8, 4));
    let data = writer.add_symbol(WriteSymbol {
        name: b"data".to_vec(),
        value: 0,
        size: 0,
        kind: object::SymbolKind::Data,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Undefined,
        flags: SymbolFlags::None,
    });
    for (offset, r_type) in [(0, rv32::R_RISCV_HI20), (4, rv32::R_RISCV_LO12_I)] {
        writer
            .add_relocation(
                text,
                WriteRelocation {
                    offset,
                    symbol: data,
                    addend: 0,
                    flags: object::RelocationFlags::Elf { r_type },
                },
            )
            .unwrap();
    }
    writer.write().unwrap()
}

#[test]
fn symbols_at_one_address_are_one_function_with_every_name() {
    let bytes = object();
    let elf = Elf::parse(&bytes).unwrap();
    let functions = elf.functions().unwrap();
    assert_eq!(functions.len(), 2);
    assert_eq!(functions[0].names, ["f", "g"]);
    assert_eq!((functions[0].address, functions[0].size), (0, 8));
    assert_eq!(functions[1].names, ["h"]);
    assert_eq!(functions[1].label(), "h");
}

#[test]
fn relocatable_code_carries_its_relocation_sites() {
    let bytes = object();
    let elf = Elf::parse(&bytes).unwrap();
    let code = elf.code("a.o").unwrap();
    let f = code.iter().find(|c| c.name == "f").unwrap();
    assert_eq!(f.bytes.len(), 8);
    assert_eq!(
        f.sites,
        [
            Site {
                offset: 0,
                r_type: rv32::R_RISCV_HI20,
                reference: Reference::Symbol("data".into()),
            },
            Site {
                offset: 4,
                r_type: rv32::R_RISCV_LO12_I,
                reference: Reference::Symbol("data".into()),
            },
        ]
    );
    let h = code.iter().find(|c| c.name == "h").unwrap();
    assert!(h.sites.is_empty());
}

#[test]
fn a_single_object_is_its_own_member_and_text_is_not_binary() {
    let bytes = object();
    assert!(is_binary(&bytes));
    assert!(!is_binary(b"#include <stdint.h>\n"));
    let objects = objects(&bytes).unwrap();
    assert_eq!(objects.len(), 1);
    assert_eq!(objects[0].0, "");
    assert!(
        Elf::executable(&bytes).is_err(),
        "a relocatable object is no image"
    );
}

#[test]
fn rust_names_demangle_without_their_hash() {
    assert_eq!(
        demangle("_ZN4core3fmt5write17h0123456789abcdefE"),
        "core::fmt::write"
    );
    assert_eq!(demangle("memcpy"), "memcpy");
}

/// The table, pinned: a changed row changes every vendor fingerprint and
/// must be a deliberate change of this test and of the registered
/// fingerprints together.
#[test]
fn the_relocation_table_is_pinned() {
    use rv32::{Field as F, Role as R};
    let rows: Vec<(u32, &str, F, R)> = (0..=66)
        .map(|r_type| {
            let kind = rv32::kind(r_type);
            (r_type, kind.name, kind.field, kind.role)
        })
        .filter(|row| row.1 != "unknown")
        .collect();
    let expected = [
        (0, "NONE", F::None, R::Hint),
        (1, "32", F::Data(4), R::Word),
        (2, "64", F::Data(8), R::Other),
        (3, "RELATIVE", F::Data(4), R::Other),
        (4, "COPY", F::None, R::Other),
        (5, "JUMP_SLOT", F::Data(4), R::Other),
        (6, "TLS_DTPMOD32", F::Data(4), R::Other),
        (7, "TLS_DTPMOD64", F::Data(8), R::Other),
        (8, "TLS_DTPREL32", F::Data(4), R::Other),
        (9, "TLS_DTPREL64", F::Data(8), R::Other),
        (10, "TLS_TPREL32", F::Data(4), R::Other),
        (11, "TLS_TPREL64", F::Data(8), R::Other),
        (12, "TLSDESC", F::Data(4), R::Other),
        (16, "BRANCH", F::Branch, R::Branch),
        (17, "JAL", F::Jump, R::Jump),
        (18, "CALL", F::Call, R::Call),
        (19, "CALL_PLT", F::Call, R::Call),
        (20, "GOT_HI20", F::Upper, R::Other),
        (21, "TLS_GOT_HI20", F::Upper, R::Other),
        (22, "TLS_GD_HI20", F::Upper, R::Other),
        (23, "PCREL_HI20", F::Upper, R::PcRelativeHigh),
        (24, "PCREL_LO12_I", F::Immediate, R::PcRelativeLow),
        (25, "PCREL_LO12_S", F::Store, R::PcRelativeLow),
        (26, "HI20", F::Upper, R::AbsoluteHigh),
        (27, "LO12_I", F::Immediate, R::AbsoluteLow),
        (28, "LO12_S", F::Store, R::AbsoluteLow),
        (29, "TPREL_HI20", F::Upper, R::Other),
        (30, "TPREL_LO12_I", F::Immediate, R::Other),
        (31, "TPREL_LO12_S", F::Store, R::Other),
        (32, "TPREL_ADD", F::None, R::Other),
        (33, "ADD8", F::Data(1), R::Other),
        (34, "ADD16", F::Data(2), R::Other),
        (35, "ADD32", F::Data(4), R::Other),
        (36, "ADD64", F::Data(8), R::Other),
        (37, "SUB8", F::Data(1), R::Other),
        (38, "SUB16", F::Data(2), R::Other),
        (39, "SUB32", F::Data(4), R::Other),
        (40, "SUB64", F::Data(8), R::Other),
        (41, "GOT32_PCREL", F::Data(4), R::Other),
        (43, "ALIGN", F::None, R::Hint),
        (44, "RVC_BRANCH", F::CompressedBranch, R::Branch),
        (45, "RVC_JUMP", F::CompressedJump, R::Branch),
        (46, "RVC_LUI", F::CompressedUpper, R::Other),
        (47, "GPREL_I", F::Immediate, R::Other),
        (48, "GPREL_S", F::Store, R::Other),
        (49, "TPREL_I", F::Immediate, R::Other),
        (50, "TPREL_S", F::Store, R::Other),
        (51, "RELAX", F::None, R::Hint),
        (52, "SUB6", F::Low6, R::Other),
        (53, "SET6", F::Low6, R::Other),
        (54, "SET8", F::Data(1), R::Other),
        (55, "SET16", F::Data(2), R::Other),
        (56, "SET32", F::Data(4), R::Other),
        (57, "32_PCREL", F::Data(4), R::Other),
        (58, "IRELATIVE", F::Data(4), R::Other),
        (59, "PLT32", F::Data(4), R::Other),
        (60, "SET_ULEB128", F::Uleb128, R::Other),
        (61, "SUB_ULEB128", F::Uleb128, R::Other),
        (62, "TLSDESC_HI20", F::Upper, R::Other),
        (63, "TLSDESC_LOAD_LO12", F::Immediate, R::Other),
        (64, "TLSDESC_ADD_LO12", F::Immediate, R::Other),
        (65, "TLSDESC_CALL", F::None, R::Other),
    ];
    assert_eq!(rows, expected);
    assert_eq!(rv32::kind(42).field, F::Unknown);
    assert_eq!(rv32::kind(255).role, R::Other);
}

/// The masks, pinned per field on an all-ones location.
#[test]
fn each_field_masks_exactly_its_immediate_bits() {
    let masked = |r_type: u32, length: usize| {
        let mut bytes = vec![0xff; length];
        rv32::mask(&mut bytes, 0, r_type);
        bytes
    };
    let word = |bytes: &[u8]| u32::from_le_bytes(bytes[..4].try_into().unwrap());
    let half = |bytes: &[u8]| u16::from_le_bytes(bytes[..2].try_into().unwrap());
    assert_eq!(word(&masked(rv32::R_RISCV_HI20, 4)), 0x0000_0fff);
    assert_eq!(word(&masked(rv32::R_RISCV_JAL, 4)), 0x0000_0fff);
    assert_eq!(word(&masked(rv32::R_RISCV_LO12_I, 4)), 0x000f_ffff);
    assert_eq!(word(&masked(rv32::R_RISCV_LO12_S, 4)), 0x01ff_f07f);
    assert_eq!(word(&masked(rv32::R_RISCV_BRANCH, 4)), 0x01ff_f07f);
    let call = masked(rv32::R_RISCV_CALL, 8);
    assert_eq!((word(&call), word(&call[4..])), (0x0000_0fff, 0x000f_ffff));
    assert_eq!(half(&masked(rv32::R_RISCV_RVC_BRANCH, 2)), 0xe383);
    assert_eq!(half(&masked(rv32::R_RISCV_RVC_JUMP, 2)), 0xe003);
    assert_eq!(half(&masked(rv32::R_RISCV_RVC_LUI, 2)), 0xef83);
    assert_eq!(masked(rv32::R_RISCV_SET6, 2), [0xc0, 0xff]);
    assert_eq!(masked(rv32::R_RISCV_ADD16, 4), [0, 0, 0xff, 0xff]);
    assert_eq!(masked(rv32::R_RISCV_32, 4), [0; 4]);
    assert_eq!(masked(rv32::R_RISCV_RELAX, 4), [0xff; 4]);
    assert_eq!(masked(42, 4), [0; 4], "an unknown type masks its word");
    let mut leb = vec![0xff, 0xff, 0x7f, 0xff];
    rv32::mask(&mut leb, 0, rv32::R_RISCV_SET_ULEB128);
    assert_eq!(leb, [0x80, 0x80, 0x00, 0xff]);
    let mut short = vec![0xff; 3];
    rv32::mask(&mut short, 0, rv32::R_RISCV_HI20);
    assert_eq!(short, [0xff; 3], "a part past the end is left alone");
}

/// This test's own executable symbolizes to this file.
#[cfg(target_os = "linux")]
#[test]
fn an_address_symbolizes_to_its_function_and_source_file() {
    let path = std::env::current_exe().unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let elf = Elf::parse(&bytes).unwrap();
    let marker = elf
        .symbols()
        .find(|s| s.kind == SymbolKind::Text && s.demangled().ends_with("tests::symbolized_marker"))
        .expect("the marker function is linked");
    let symbolizer = dwarf::Symbolizer::new(&bytes).unwrap();
    let frames = symbolizer.frames(marker.address).unwrap();
    let outer = frames.last().expect("line tables describe the marker");
    assert!(
        outer
            .file
            .as_deref()
            .is_some_and(|file| file.ends_with("tests.rs")),
        "{frames:?}"
    );
    assert!(outer.line.is_some(), "{frames:?}");
    assert!(std::hint::black_box(symbolized_marker as fn() -> u32)() == 7);
}

#[inline(never)]
fn symbolized_marker() -> u32 {
    std::hint::black_box(7)
}
