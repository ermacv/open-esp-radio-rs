use super::*;

const SP_DOWN_16: u32 = 0xff01_0113; // addi sp, sp, -16
const SAVE_RA: u32 = 0x0011_2623; // sw ra, 12(sp)
const LOAD_RA: u32 = 0x00c1_2083; // lw ra, 12(sp)
const SP_UP_16: u32 = 0x0101_0113; // addi sp, sp, 16
const SP_DOWN_32: u32 = 0xfe01_0113; // addi sp, sp, -32
const SP_UP_32: u32 = 0x0201_0113; // addi sp, sp, 32
const RET: u32 = 0x0000_8067; // jalr zero, 0(ra)
const LOAD_SLOT: u32 = 0x0001_2783; // lw a5, 0(sp)
const CALL_A5: u32 = 0x0007_80e7; // jalr ra, 0(a5)
const TEXT: u32 = 0x1000;

/// `jal ra, target` at `site`.
fn call(site: u32, target: u32) -> u32 {
    let offset = target.wrapping_sub(site);
    ((offset >> 20 & 1) << 31)
        | ((offset >> 1 & 0x3ff) << 21)
        | ((offset >> 11 & 1) << 20)
        | ((offset >> 12 & 0xff) << 12)
        | (1 << 7)
        | 0x6f
}

struct Symbol {
    name: &'static str,
    words: Vec<u32>,
    /// `.stack_sizes` record, if any.
    frame: Option<u8>,
}

/// A static RV32 executable with one loaded `.text` holding the symbols in
/// order from `TEXT`, a symbol table and `.stack_sizes`.
fn executable(symbols: &[Symbol]) -> Vec<u8> {
    let mut text = Vec::new();
    let mut entries = Vec::new();
    let mut sizes = Vec::new();
    for symbol in symbols {
        let address = TEXT + text.len() as u32;
        for word in &symbol.words {
            text.extend_from_slice(&word.to_le_bytes());
        }
        entries.push((symbol.name, address, 4 * symbol.words.len() as u32));
        if let Some(frame) = symbol.frame {
            sizes.extend_from_slice(&address.to_le_bytes());
            sizes.push(frame);
        }
    }
    let mut strtab = vec![0u8];
    let mut symtab = vec![0u8; 16];
    for (name, address, size) in &entries {
        let offset = strtab.len() as u32;
        strtab.extend_from_slice(name.as_bytes());
        strtab.push(0);
        symtab.extend_from_slice(&offset.to_le_bytes());
        symtab.extend_from_slice(&address.to_le_bytes());
        symtab.extend_from_slice(&size.to_le_bytes());
        symtab.push(0x12); // STB_GLOBAL, STT_FUNC
        symtab.push(0);
        symtab.extend_from_slice(&1u16.to_le_bytes()); // .text
    }
    let shstrtab = b"\0.text\0.stack_sizes\0.symtab\0.strtab\0.shstrtab\0";
    // Layout: ELF header, one program header, then the section contents.
    let mut out = vec![0u8; 52 + 32];
    let place = |out: &mut Vec<u8>, bytes: &[u8]| {
        while !out.len().is_multiple_of(4) {
            out.push(0);
        }
        let offset = out.len() as u32;
        out.extend_from_slice(bytes);
        offset
    };
    let text_offset = place(&mut out, &text);
    let sizes_offset = place(&mut out, &sizes);
    let symtab_offset = place(&mut out, &symtab);
    let strtab_offset = place(&mut out, &strtab);
    let shstrtab_offset = place(&mut out, shstrtab);
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
    let sections = out.len() as u32;
    let header = |name: u32,
                  kind: u32,
                  flags: u32,
                  address: u32,
                  offset: u32,
                  size: u32,
                  link: u32,
                  info: u32,
                  align: u32,
                  entry: u32| {
        [
            name, kind, flags, address, offset, size, link, info, align, entry,
        ]
    };
    let headers = [
        [0; 10],
        header(1, 1, 6, TEXT, text_offset, text.len() as u32, 0, 0, 4, 0),
        header(7, 1, 0, 0, sizes_offset, sizes.len() as u32, 0, 0, 1, 0),
        header(20, 2, 0, 0, symtab_offset, symtab.len() as u32, 4, 1, 4, 16),
        header(28, 3, 0, 0, strtab_offset, strtab.len() as u32, 0, 0, 1, 0),
        header(
            36,
            3,
            0,
            0,
            shstrtab_offset,
            shstrtab.len() as u32,
            0,
            0,
            1,
            0,
        ),
    ];
    for fields in headers {
        for field in fields {
            out.extend_from_slice(&field.to_le_bytes());
        }
    }
    // ELF header: ELFCLASS32, little endian, ET_EXEC, EM_RISCV, soft float.
    let mut elf = Vec::new();
    elf.extend_from_slice(b"\x7fELF\x01\x01\x01\0\0\0\0\0\0\0\0\0");
    elf.extend_from_slice(&2u16.to_le_bytes());
    elf.extend_from_slice(&243u16.to_le_bytes());
    for word in [1u32, TEXT, 52, sections, 0] {
        elf.extend_from_slice(&word.to_le_bytes());
    }
    for half in [52u16, 32, 1, 40, 6, 5] {
        elf.extend_from_slice(&half.to_le_bytes());
    }
    // PT_LOAD of .text, readable and executable.
    for word in [
        1u32,
        text_offset,
        TEXT,
        TEXT,
        text.len() as u32,
        text.len() as u32,
        5,
        4,
    ] {
        elf.extend_from_slice(&word.to_le_bytes());
    }
    out[..84].copy_from_slice(&elf);
    out
}

fn leaf(frame: Option<u8>) -> Symbol {
    Symbol {
        name: "leaf",
        words: vec![SP_DOWN_32, SP_UP_32, RET],
        frame,
    }
}

#[test]
fn a_callee_runs_below_its_call_site_depth() {
    // root: 16 bytes, calls leaf (32 bytes) at depth 16.
    let leaf_address = TEXT + 24;
    let image = executable(&[
        Symbol {
            name: "root",
            words: vec![
                SP_DOWN_16,
                SAVE_RA,
                call(TEXT + 8, leaf_address),
                LOAD_RA,
                SP_UP_16,
                RET,
            ],
            frame: Some(16),
        },
        leaf(Some(32)),
    ]);
    let analysis = analyze(&image).unwrap();
    let root = &analysis.functions[&TEXT];
    assert_eq!(root.frame, Some(16));
    assert_eq!(root.source, Some(FrameSource::StackSizes));
    assert_eq!(root.observed, Some(16));
    let bound = analysis.bound(TEXT).unwrap();
    assert_eq!(bound.bytes, Some(48));
    assert!(bound.unresolved.is_empty());
    assert_eq!(bound.path, [(TEXT, 16), (leaf_address, 32)]);
}

#[test]
fn an_unresolved_call_leaves_the_bound_unknown_with_its_reason() {
    // root loads a pointer from its stack slot and calls through it.
    let image = executable(&[Symbol {
        name: "root",
        words: vec![
            SP_DOWN_16, SAVE_RA, LOAD_SLOT, CALL_A5, LOAD_RA, SP_UP_16, RET,
        ],
        frame: Some(16),
    }]);
    let bound = analyze(&image).unwrap().bound(TEXT).unwrap();
    assert_eq!(bound.bytes, None);
    assert_eq!(bound.unresolved, [(TEXT + 12, Reason::StackSlotCall)]);
    assert_eq!(
        bound.reasons(),
        BTreeMap::from([(Reason::StackSlotCall, 1)])
    );
}

#[test]
fn recursion_leaves_the_bound_unknown() {
    let image = executable(&[Symbol {
        name: "root",
        words: vec![
            SP_DOWN_16,
            SAVE_RA,
            call(TEXT + 8, TEXT),
            LOAD_RA,
            SP_UP_16,
            RET,
        ],
        frame: Some(16),
    }]);
    let bound = analyze(&image).unwrap().bound(TEXT).unwrap();
    assert_eq!(bound.bytes, None);
    assert_eq!(bound.reasons(), BTreeMap::from([(Reason::Recursion, 1)]));
}

#[test]
fn an_observed_depth_beyond_the_frame_record_fails() {
    let image = executable(&[leaf(Some(16))]);
    let error = analyze(&image).unwrap_err();
    assert_eq!(error.code, ErrorCode::Integrity);
}

#[test]
fn a_complete_graph_without_a_record_gives_the_observed_frame() {
    let analysis = analyze(&executable(&[leaf(None)])).unwrap();
    let facts = &analysis.functions[&TEXT];
    assert_eq!(
        (facts.frame, facts.source),
        (Some(32), Some(FrameSource::Observed))
    );
    assert_eq!(analysis.bound(TEXT).unwrap().bytes, Some(32));
}

#[test]
fn frame_records_decode_with_uleb128_sizes() {
    let image = executable(&[leaf(Some(32))]);
    assert_eq!(stack_sizes(&image).unwrap(), BTreeMap::from([(TEXT, 32)]));
}
