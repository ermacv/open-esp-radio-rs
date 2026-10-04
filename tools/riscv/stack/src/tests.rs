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
const BRANCH_NEXT: u32 = 0x0000_0263; // beq zero, zero, 4

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

const RODATA: u32 = 0x2000;

/// A static RV32 executable with one loaded `.text` holding the symbols in
/// order from `TEXT`, a symbol table and `.stack_sizes`.
fn executable(symbols: &[Symbol]) -> Vec<u8> {
    image(symbols, &[], false)
}

/// [`executable`] with `rodata` words in a `.rodata` at `RODATA`, covered by
/// a sized `table` data symbol when `sized`.
fn image(symbols: &[Symbol], rodata: &[u32], sized: bool) -> Vec<u8> {
    placed(TEXT, symbols, rodata, sized, false)
}

/// [`image`] whose `.rodata` is a compiler jump table: a `.LJTI0_0` label and
/// an `R_RISCV_32` relocation for each word, as `--emit-relocs` keeps them.
fn labelled(symbols: &[Symbol], rodata: &[u32]) -> Vec<u8> {
    placed(TEXT, symbols, rodata, false, true)
}

/// [`image`] with its `.text` at `text` instead of `TEXT`, and a labelled
/// jump table when `labelled`.
fn placed(
    text_base: u32,
    symbols: &[Symbol],
    rodata_words: &[u32],
    sized: bool,
    labelled: bool,
) -> Vec<u8> {
    let mut objects = Vec::new();
    if labelled {
        objects.push((".LJTI0_0", 0, 0));
    }
    if sized {
        objects.push(("table", 0, 4 * rodata_words.len() as u32));
    }
    let relocated: Vec<usize> = if labelled {
        (0..rodata_words.len()).collect()
    } else {
        Vec::new()
    };
    built(
        text_base,
        symbols,
        &Rodata {
            words: rodata_words,
            relocated: &relocated,
            objects: &objects,
        },
    )
}

/// `.rodata` words at `RODATA`: which of them carry an `R_RISCV_32`
/// relocation (by index), and its symbols as (name, byte offset, size); a
/// size of zero is a label.
struct Rodata<'a> {
    words: &'a [u32],
    relocated: &'a [usize],
    objects: &'a [(&'static str, u32, u32)],
}

/// An executable of `symbols` at `text_base` and `rodata`.
fn built(text_base: u32, symbols: &[Symbol], rodata: &Rodata<'_>) -> Vec<u8> {
    let rodata_words = rodata.words;
    let objects = rodata.objects;
    let relocated = rodata.relocated;
    let rodata: Vec<u8> = rodata_words
        .iter()
        .flat_map(|word| word.to_le_bytes())
        .collect();
    let mut text = Vec::new();
    let mut entries = Vec::new();
    let mut sizes = Vec::new();
    for symbol in symbols {
        let address = text_base + text.len() as u32;
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
    let code_symbols = entries.len();
    for &(name, offset, size) in objects {
        entries.push((name, RODATA + offset, size));
    }
    // Relocations with no symbol: the addend is the address.
    let mut rela = Vec::new();
    for &i in relocated {
        rela.extend_from_slice(&(RODATA + 4 * i as u32).to_le_bytes());
        rela.extend_from_slice(&1u32.to_le_bytes()); // R_RISCV_32
        rela.extend_from_slice(&rodata_words[i].to_le_bytes());
    }
    for (index, (name, address, size)) in entries.iter().enumerate() {
        let offset = strtab.len() as u32;
        strtab.extend_from_slice(name.as_bytes());
        strtab.push(0);
        symtab.extend_from_slice(&offset.to_le_bytes());
        symtab.extend_from_slice(&address.to_le_bytes());
        symtab.extend_from_slice(&size.to_le_bytes());
        let data = index >= code_symbols;
        // STB_GLOBAL with STT_NOTYPE for a label, STT_OBJECT or STT_FUNC.
        symtab.push(match (*size, data) {
            (0, true) => 0x10,
            (_, true) => 0x11,
            _ => 0x12,
        });
        symtab.push(0);
        symtab.extend_from_slice(&(if data { 6u16 } else { 1 }).to_le_bytes()); // .rodata or .text
    }
    let shstrtab = b"\0.text\0.stack_sizes\0.symtab\0.strtab\0.shstrtab\0.rodata\0.rela.rodata\0";
    // Layout: ELF header, one program header, then the section contents.
    let mut out = vec![0u8; 52 + 2 * 32];
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
    let rodata_offset = place(&mut out, &rodata);
    let rela_offset = place(&mut out, &rela);
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
        header(
            1,
            1,
            6,
            text_base,
            text_offset,
            text.len() as u32,
            0,
            0,
            4,
            0,
        ),
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
        header(
            46,
            1,
            2,
            RODATA,
            rodata_offset,
            rodata.len() as u32,
            0,
            0,
            4,
            0,
        ),
        // SHT_RELA with SHF_INFO_LINK for .rodata (6), symbols in .symtab (3).
        header(54, 4, 0x40, 0, rela_offset, rela.len() as u32, 3, 6, 4, 12),
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
    for word in [1u32, text_base, 52, sections, 0] {
        elf.extend_from_slice(&word.to_le_bytes());
    }
    for half in [52u16, 32, 2, 40, 8, 5] {
        elf.extend_from_slice(&half.to_le_bytes());
    }
    // PT_LOAD of .text, readable and executable.
    for word in [
        1u32,
        text_offset,
        text_base,
        text_base,
        text.len() as u32,
        text.len() as u32,
        5,
        4,
    ] {
        elf.extend_from_slice(&word.to_le_bytes());
    }
    // PT_LOAD of .rodata, readable.
    for word in [
        1u32,
        rodata_offset,
        RODATA,
        RODATA,
        rodata.len() as u32,
        rodata.len() as u32,
        4,
        4,
    ] {
        elf.extend_from_slice(&word.to_le_bytes());
    }
    out[..116].copy_from_slice(&elf);
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
    let analysis = analyze(&image, &[], &[]).unwrap();
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
    let bound = analyze(&image, &[], &[]).unwrap().bound(TEXT).unwrap();
    assert_eq!(bound.bytes, None);
    assert_eq!(bound.unresolved, [(TEXT + 12, Reason::StackSlotCall)]);
    assert_eq!(
        bound.reasons(),
        BTreeMap::from([(Reason::StackSlotCall, 1)])
    );
}

#[test]
fn a_resolution_gives_an_unresolved_site_each_of_its_targets() {
    // root: frame 16, an indirect call through a5 at offset 8.
    let root = Symbol {
        name: "root",
        words: vec![
            SP_DOWN_16, SAVE_RA, LOAD_SLOT, CALL_A5, LOAD_RA, SP_UP_16, RET,
        ],
        frame: Some(16),
    };
    let small = Symbol {
        name: "small",
        words: vec![SP_DOWN_16, SP_UP_16, RET],
        frame: Some(16),
    };
    let large = Symbol {
        name: "large",
        words: vec![SP_DOWN_32, SP_UP_32, RET],
        frame: Some(32),
    };
    let elf = executable(&[root, small, large]);
    let analysis = analyze(&elf, &[], &[]).unwrap();
    let site = TEXT + 12;
    let small = TEXT + 7 * 4;
    let large = small + 3 * 4;
    assert_eq!(analysis.bound(TEXT).unwrap().bytes, None);
    let mut resolutions = Resolutions::new();
    resolutions.add(site, Fact::FieldType, [small, large]);
    let bound = analysis.bound_with(TEXT, &resolutions).unwrap();
    assert_eq!(bound.bytes, Some(16 + 32));
    assert_eq!(bound.path.last(), Some(&(large, 32)));
    // A site the analysis resolved keeps its own target.
    let mut only_small = Resolutions::new();
    only_small.add(site, Fact::FieldType, [small]);
    assert_eq!(
        analysis.bound_with(TEXT, &only_small).unwrap().bytes,
        Some(32)
    );
}

/// An empty set resolves a site only where its fact proves the site reaches
/// nothing; otherwise the site is a hole, and the bound keeps the resolved
/// part as `partial + ?`.
#[test]
fn an_empty_candidate_set_is_a_hole_unless_its_fact_proves_it() {
    let root = Symbol {
        name: "root",
        words: vec![
            SP_DOWN_16, SAVE_RA, LOAD_SLOT, CALL_A5, LOAD_RA, SP_UP_16, RET,
        ],
        frame: Some(16),
    };
    let elf = executable(&[root]);
    let analysis = analyze(&elf, &[], &[]).unwrap();
    let site = TEXT + 12;
    let mut found = Resolutions::new();
    found.add(site, Fact::WakerVtables, []);
    let hole = analysis.bound_with(TEXT, &found).unwrap();
    assert_eq!(hole.bytes, None);
    assert_eq!(hole.partial, 16);
    assert_eq!(hole.unresolved, [(site, Reason::NoCandidate)]);
    let mut proven = Resolutions::new();
    proven.add(site, Fact::IpcPosts, []);
    assert_eq!(analysis.bound_with(TEXT, &proven).unwrap().bytes, Some(16));
    // Facts unite: a field type's candidate joins a found-empty waker set.
    found.add(site, Fact::FieldType, [TEXT]);
    assert_eq!(found.get(site).unwrap().targets, BTreeSet::from([TEXT]));
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
    let bound = analyze(&image, &[], &[]).unwrap().bound(TEXT).unwrap();
    assert_eq!(bound.bytes, None);
    assert_eq!(bound.reasons(), BTreeMap::from([(Reason::Recursion, 1)]));
}

#[test]
fn an_observed_depth_beyond_the_frame_record_fails() {
    let image = executable(&[leaf(Some(16))]);
    let error = analyze(&image, &[], &[]).unwrap_err();
    assert_eq!(error.code, ErrorCode::Integrity);
}

#[test]
fn a_complete_graph_without_a_record_gives_the_observed_frame() {
    let analysis = analyze(&executable(&[leaf(None)]), &[], &[]).unwrap();
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

/// `slli a0, a0, 2; lui a2, %hi(RODATA); addi a2, a2, 0; add a0, a0, a2;
/// lw a0, 0(a0); jr a0`, then two returns as the table's arms; `bounded`
/// first checks the index: `li a1, 1; bltu a1, a0, <first arm>`.
fn dispatch(bounded: bool) -> Symbol {
    let check: &[u32] = if bounded {
        &[0x0010_0593, 0x00a5_ee63]
    } else {
        &[]
    };
    let mut words = check.to_vec();
    words.extend([
        0x0025_1513,
        0x0000_2637,
        0x0006_0613,
        0x00c5_0533,
        0x0005_2503,
        0x0005_0067,
        RET,
        RET,
    ]);
    Symbol {
        name: "dispatch",
        words,
        frame: Some(0),
    }
}

#[test]
fn a_bounded_jump_table_reads_exactly_its_entries() {
    // Two arms; the word after the table is not an entry.
    let image = image(&[dispatch(true)], &[TEXT + 32, TEXT + 36, 0x5000], false);
    let function = &functions(&image).unwrap()[0];
    assert_eq!(
        sweep::transfers(
            &image,
            function,
            &relocations::Relocations::read(&image).unwrap()
        )
        .unwrap()
        .transfers,
        []
    );
    assert_eq!(
        analyze(&image, &[], &[])
            .unwrap()
            .bound(TEXT)
            .unwrap()
            .bytes,
        Some(0)
    );
}

#[test]
fn a_bounded_table_entry_out_of_the_function_is_a_jump() {
    let image = image(&[dispatch(true)], &[TEXT + 32, 0x5000], false);
    let function = &functions(&image).unwrap()[0];
    let transfers: Vec<_> = sweep::transfers(
        &image,
        function,
        &relocations::Relocations::read(&image).unwrap(),
    )
    .unwrap()
    .transfers
    .iter()
    .map(|t| (t.site, t.target, t.kind))
    .collect();
    assert_eq!(transfers, [(TEXT + 28, Some(0x5000), TransferKind::Tail)]);
    let bound = analyze(&image, &[], &[]).unwrap().bound(TEXT).unwrap();
    assert_eq!(bound.reasons(), BTreeMap::from([(Reason::OutsideImage, 1)]));
}

#[test]
fn an_unbounded_jump_table_stays_unknown() {
    // Without a bound or a sized table object, entries past the readable
    // ones could leave the function.
    let image = image(&[dispatch(false)], &[TEXT + 24, TEXT + 28], false);
    let bound = analyze(&image, &[], &[]).unwrap().bound(TEXT).unwrap();
    assert_eq!(bound.bytes, None);
    assert_eq!(bound.unresolved, [(TEXT + 20, Reason::IndirectJump)]);
}

#[test]
fn a_jump_through_a_sized_table_object_keeps_only_entries_out_of_the_function() {
    let image = image(&[dispatch(false)], &[TEXT + 24, 0x5000], true);
    let analysis = analyze(&image, &[], &[]).unwrap();
    let jumps: Vec<_> = analysis.functions[&TEXT]
        .transfers
        .iter()
        .map(|t| (t.site, t.target))
        .collect();
    assert_eq!(jumps, [(TEXT + 20, Some(0x5000))]);
}

#[test]
fn a_call_through_a_constant_table_object_reaches_every_entry() {
    // root: lui s11, %hi(RODATA); addi s11, s11, 0; then, past a merge
    // where the sweep forgets the constant but the value analysis keeps it,
    // slli a0, a0, 2; add a0, a0, s11; lw a0, 0(a0); jalr a0, at depth 16.
    let (small, large) = (TEXT + 52, TEXT + 64);
    let image = image(
        &[
            Symbol {
                name: "root",
                words: vec![
                    SP_DOWN_16,
                    SAVE_RA,
                    0x0000_2db7,
                    0x000d_8d93,
                    BRANCH_NEXT,
                    0x0025_1513,
                    0x01b5_0533,
                    0x0005_2503,
                    0x0005_00e7,
                    LOAD_RA,
                    SP_UP_16,
                    RET,
                    RET,
                ],
                frame: Some(16),
            },
            Symbol {
                name: "small",
                words: vec![SP_DOWN_16, SP_UP_16, RET],
                frame: Some(16),
            },
            Symbol {
                name: "large",
                words: vec![SP_DOWN_32, SP_UP_32, RET],
                frame: Some(32),
            },
        ],
        &[small, large, small],
        true,
    );
    let analysis = analyze(&image, &[], &[]).unwrap();
    let calls: Vec<_> = analysis.functions[&TEXT]
        .transfers
        .iter()
        .map(|t| (t.site, t.target))
        .collect();
    assert_eq!(calls, [(TEXT + 32, Some(small)), (TEXT + 32, Some(large))]);
    let bound = analysis.bound(TEXT).unwrap();
    assert_eq!(bound.bytes, Some(48));
    assert_eq!(bound.path, [(TEXT, 16), (large, 32)]);
}

#[test]
fn a_table_base_rewritten_before_the_call_stays_unresolved() {
    // As above, but `addi s11, s11, 4` after the indexing `add`.
    let image = image(
        &[Symbol {
            name: "root",
            words: vec![
                SP_DOWN_16,
                SAVE_RA,
                0x0000_2db7,
                0x000d_8d93,
                BRANCH_NEXT,
                0x0025_1513,
                0x01b5_0533,
                0x004d_8d93,
                0x0005_2503,
                0x0005_00e7,
                LOAD_RA,
                SP_UP_16,
                RET,
            ],
            frame: Some(16),
        }],
        &[TEXT, TEXT],
        true,
    );
    let bound = analyze(&image, &[], &[]).unwrap().bound(TEXT).unwrap();
    assert_eq!(bound.bytes, None);
    assert_eq!(bound.reasons(), BTreeMap::from([(Reason::LoadedCall, 1)]));
}

#[test]
fn analysed_jump_targets_inside_the_function_are_not_transfers() {
    // li a0, 0; beqz a1, 1f; li a0, 1; 1: slli a0, a0, 2; lui a2,
    // %hi(RODATA); addi a2, a2, 0; add a0, a0, a2; lw a0, 0(a0); jr a0;
    // ret. The value analysis knows the index is 0 or 1: entry 0 is the
    // local `ret`, entry 1 the function `leaf`.
    let leaf_address = TEXT + 40;
    let image = image(
        &[
            Symbol {
                name: "dispatch",
                words: vec![
                    0x0000_0513,
                    0x0005_8463,
                    0x0010_0513,
                    0x0025_1513,
                    0x0000_2637,
                    0x0006_0613,
                    0x00c5_0533,
                    0x0005_2503,
                    0x0005_0067,
                    RET,
                ],
                frame: Some(0),
            },
            leaf(Some(32)),
        ],
        &[TEXT + 36, leaf_address],
        false,
    );
    let analysis = analyze(&image, &[], &[]).unwrap();
    let jumps: Vec<_> = analysis.functions[&TEXT]
        .transfers
        .iter()
        .map(|t| (t.site, t.target, t.kind))
        .collect();
    assert_eq!(jumps, [(TEXT + 32, Some(leaf_address), TransferKind::Tail)]);
    assert_eq!(analysis.bound(TEXT).unwrap().bytes, Some(32));
}

#[test]
fn a_call_into_a_companion_runs_below_its_site() {
    // root calls a function of a ROM-like companion ELF at 0x9000, which
    // has no .stack_sizes: its frame is the observed one.
    let rom = 0x9000;
    let image = image(
        &[Symbol {
            name: "root",
            words: vec![
                SP_DOWN_16,
                SAVE_RA,
                call(TEXT + 8, rom),
                LOAD_RA,
                SP_UP_16,
                RET,
            ],
            frame: Some(16),
        }],
        &[],
        false,
    );
    let companion = placed(rom, &[leaf(None)], &[], false, false);
    let alone = analyze(&image, &[], &[]).unwrap().bound(TEXT).unwrap();
    assert_eq!(alone.reasons(), BTreeMap::from([(Reason::OutsideImage, 1)]));
    let with_rom = analyze(&image, &[&companion], &[]).unwrap();
    assert_eq!(with_rom.functions[&rom].source, Some(FrameSource::Observed));
    let bound = with_rom.bound(TEXT).unwrap();
    assert_eq!(bound.bytes, Some(48));
    assert_eq!(bound.path, [(TEXT, 16), (rom, 32)]);
}

#[test]
fn companions_may_not_overlap() {
    let image = executable(&[leaf(Some(32))]);
    let error = analyze(&image, &[&image], &[]).unwrap_err();
    assert_eq!(error.code, ErrorCode::Integrity);
}

#[test]
fn a_zero_table_entry_is_an_empty_slot_not_a_call() {
    // The sized table holds `leaf` and an empty `Option<fn>` slot.
    let leaf_address = TEXT + 48 + 4;
    let image = image(
        &[
            Symbol {
                name: "root",
                words: vec![
                    SP_DOWN_16,
                    SAVE_RA,
                    0x0000_2db7,
                    0x000d_8d93,
                    BRANCH_NEXT,
                    0x0025_1513,
                    0x01b5_0533,
                    0x0005_2503,
                    0x0005_00e7,
                    LOAD_RA,
                    SP_UP_16,
                    RET,
                    RET,
                ],
                frame: Some(16),
            },
            leaf(Some(32)),
        ],
        &[leaf_address, 0],
        true,
    );
    let analysis = analyze(&image, &[], &[]).unwrap();
    let calls: Vec<_> = analysis.functions[&TEXT]
        .transfers
        .iter()
        .map(|t| (t.site, t.target))
        .collect();
    assert_eq!(calls, [(TEXT + 32, Some(leaf_address))]);
    assert_eq!(analysis.bound(TEXT).unwrap().bytes, Some(48));
}

#[test]
fn a_global_untyped_code_label_is_a_function() {
    // A companion whose only symbol is an untyped global label, as the ROM's
    // `__call_*` trampolines are, reached by `j` to the real function.
    let rom = 0x9000;
    let mut companion = placed(rom, &[leaf(None)], &[], false, false);
    // Turn the symbol `leaf` into STB_GLOBAL, STT_NOTYPE with no size.
    let mut record = rom.to_le_bytes().to_vec();
    record.extend_from_slice(&12u32.to_le_bytes());
    record.push(0x12);
    let symtab = companion
        .windows(record.len())
        .position(|w| w == record)
        .expect("leaf's symbol entry");
    companion[symtab + 4..symtab + 8].copy_from_slice(&0u32.to_le_bytes());
    companion[symtab + 8] = 0x10;
    let functions = functions(&companion).unwrap();
    assert_eq!(functions.len(), 1);
    assert_eq!((functions[0].address, functions[0].size), (rom, 12));
}

#[test]
fn a_labelled_jump_table_is_read_from_its_relocations() {
    // No bounds check: the relocations alone name the table's entries.
    let image = labelled(&[dispatch(false)], &[TEXT + 24, TEXT + 28]);
    let function = &functions(&image).unwrap()[0];
    let swept = sweep::transfers(
        &image,
        function,
        &relocations::Relocations::read(&image).unwrap(),
    )
    .unwrap();
    assert_eq!(swept.transfers, []);
    assert_eq!(swept.jumps, [(TEXT + 20, vec![TEXT + 24, TEXT + 28])]);
    let bound = analyze(&image, &[], &[]).unwrap().bound(TEXT).unwrap();
    assert_eq!(bound.bytes, Some(0));
}

#[test]
fn a_relocation_that_disagrees_with_its_word_fails() {
    let mut image = labelled(&[dispatch(false)], &[TEXT + 24, TEXT + 28]);
    let mut entry = RODATA.to_le_bytes().to_vec();
    entry.extend_from_slice(&1u32.to_le_bytes());
    entry.extend_from_slice(&(TEXT + 24).to_le_bytes());
    let at = image
        .windows(entry.len())
        .position(|w| w == entry)
        .expect("the first entry's relocation");
    image[at + 8..at + 12].copy_from_slice(&(TEXT + 20).to_le_bytes());
    let error = analyze(&image, &[], &[]).unwrap_err();
    assert_eq!(error.code, ErrorCode::Integrity);
}

#[test]
fn a_labelled_table_completes_the_control_flow_graph() {
    // The table's entries become edges of the value analysis's graph, so its
    // arms are analysed; without them the graph stops at the jump.
    let table = [TEXT + 24, TEXT + 28];
    let known = analyze(&labelled(&[dispatch(false)], &table), &[], &[]).unwrap();
    assert!(known.functions[&TEXT].complete);
    let unknown = analyze(&image(&[dispatch(false)], &table, false), &[], &[]).unwrap();
    assert!(!unknown.functions[&TEXT].complete);
}

/// An image whose `root` calls the companion's function at `ROM`, which
/// jumps through an unlabelled table it cannot bound.
const ROM: u32 = 0x9000;

fn calls_rom() -> Vec<u8> {
    image(
        &[Symbol {
            name: "root",
            words: vec![
                SP_DOWN_16,
                SAVE_RA,
                call(TEXT + 8, ROM),
                LOAD_RA,
                SP_UP_16,
                RET,
            ],
            frame: Some(16),
        }],
        &[],
        false,
    )
}

fn summary(name: &str, address: u32, frame: u64) -> Summary {
    Summary {
        name: name.into(),
        address,
        frame,
        calls: Vec::new(),
    }
}

#[test]
fn a_summary_stands_for_a_companion_function_the_code_does_not_bound() {
    let rom = placed(ROM, &[dispatch(false)], &[], false, false);
    let image = calls_rom();
    let alone = analyze(&image, &[&rom], &[]).unwrap().bound(TEXT).unwrap();
    assert_eq!(alone.bytes, None);
    let analysis = analyze(&image, &[&rom], &[summary("dispatch", ROM, 0)]).unwrap();
    assert_eq!(analysis.functions[&ROM].source, Some(FrameSource::Summary));
    assert_eq!(analysis.summaries, BTreeSet::from(["dispatch".to_owned()]));
    assert_eq!(analysis.bound(TEXT).unwrap().bytes, Some(16));
}

#[test]
fn a_summary_must_agree_with_code_the_analysis_bounds() {
    let rom = placed(ROM, &[leaf(None)], &[], false, false);
    let image = calls_rom();
    assert!(analyze(&image, &[&rom], &[summary("leaf", ROM, 32)]).is_ok());
    let error = analyze(&image, &[&rom], &[summary("leaf", ROM, 16)]).unwrap_err();
    assert_eq!(error.code, ErrorCode::Integrity);
}

#[test]
fn a_summary_must_name_its_function_s_address() {
    let rom = placed(ROM, &[leaf(None)], &[], false, false);
    let image = calls_rom();
    for wrong in [summary("leaf", ROM + 4, 32), summary("other", ROM, 32)] {
        let error = analyze(&image, &[&rom], &[wrong]).unwrap_err();
        assert_eq!(error.code, ErrorCode::Integrity);
    }
}

#[test]
fn a_summary_the_image_does_not_reach_is_unused() {
    // Another image of the platform may reach it; the platform's audit
    // fails on a summary no image uses.
    let rom = placed(ROM, &[leaf(None)], &[], false, false);
    let image = executable(&[leaf(Some(32))]);
    let analysis = analyze(&image, &[&rom], &[summary("leaf", ROM, 32)]).unwrap();
    assert!(analysis.summaries.is_empty());
    assert_eq!(analysis.functions[&ROM].source, Some(FrameSource::Observed));
}

#[test]
fn the_platform_rom_summaries_parse() {
    let text = include_str!("../../../../platform/esp32s31/linker/rom/functions.toml");
    let summaries = parse_summaries(text).unwrap();
    assert!(summaries.iter().any(|s| s.name == "memset"));
}

#[test]
fn only_data_neither_writable_nor_executable_reads_as_constant() {
    let elf = image(
        &[Symbol {
            name: "f",
            words: vec![RET],
            frame: Some(0),
        }],
        &[0x1234_5678],
        true,
    );
    let file = object::File::parse(elf.as_slice()).unwrap();
    assert_eq!(image::read_only_word(&file, RODATA), Some(0x1234_5678));
    // Code loaded into RAM may hold a table the program rewrites.
    assert_eq!(image::read_only_word(&file, TEXT), None);
}

/// The platform's interrupt entry up to its handler call
/// (`platform/esp32s31/runtime/src/stacks.rs`, `PSRAM_TRAP_ENTER`).
const PLATFORM_ENTRY: [u32; 35] = [
    0x3401_1173,
    0x3402_92f3,
    0x0251_6063,
    0x0051_4133,
    0x0051_42b3,
    0x0051_4133,
    0xfb01_0113,
    0x0061_2423,
    0x0401_2023,
    0x0140_006f,
    0xfb01_0113,
    0x0061_2423,
    0x0010_0313,
    0x0461_2023,
    0x0011_2023,
    0x3400_2373,
    0x0061_2223,
    0x0071_2623,
    0x01c1_2823,
    0x01d1_2a23,
    0x01e1_2c23,
    0x01f1_2e23,
    0x02a1_2023,
    0x02b1_2223,
    0x02c1_2423,
    0x02d1_2623,
    0x02e1_2823,
    0x02f1_2a23,
    0x0301_2c23,
    0x0311_2e23,
    0x3000_2373,
    0x0461_2223,
    0x0000_6337,
    0x3003_3073,
    0x3402_9073,
];

/// `entry` words, then `la a0, handler; j dispatch`, a `dispatch` that
/// calls `a0` and a `handler` that returns.
fn trap_image(entry: &[u32]) -> (Vec<u8>, u32) {
    let auipc = TEXT + 4 * entry.len() as u32;
    let dispatch = auipc + 12;
    let handler = dispatch + 4;
    let mut words = entry.to_vec();
    words.push(0x0000_0517); // auipc a0, 0
    words.push(((handler - auipc) << 20) | (10 << 15) | (10 << 7) | 0x13); // addi a0, a0
    words.push(call(auipc + 8, dispatch) - (1 << 7)); // jal zero, dispatch
    let elf = executable(&[
        Symbol {
            name: "entry",
            words,
            frame: None,
        },
        Symbol {
            name: "dispatch",
            words: vec![0x0005_00e7], // jalr ra, 0(a0)
            frame: None,
        },
        Symbol {
            name: "handler",
            words: vec![RET],
            frame: Some(0),
        },
    ]);
    (elf, handler)
}

#[test]
fn the_platform_trap_entry_keeps_its_frame_on_the_interrupt_stack() {
    let (elf, handler) = trap_image(&PLATFORM_ENTRY);
    let functions = functions(&elf).unwrap();
    assert_eq!(
        trap_entry(&elf, &functions, TEXT).unwrap(),
        TrapEntry {
            entry: TEXT,
            frame: 80,
            handler,
        }
    );
}

#[test]
fn a_trap_entry_must_move_sp_before_its_first_memory_access() {
    let mut entry = vec![0x0011_2023]; // sw ra, 0(sp)
    entry.extend(PLATFORM_ENTRY);
    let (elf, _) = trap_image(&entry);
    let functions = functions(&elf).unwrap();
    let error = trap_entry(&elf, &functions, TEXT).unwrap_err();
    assert!(error.message.contains("outside its frame"), "{error:?}");
}

#[test]
fn a_trap_entry_learns_the_interrupt_stack_only_from_comparing_both_stacks() {
    // csrrw sp, mscratch, sp; addi sp, sp, -16; sw ra, 0(sp): mscratch may
    // hold the interrupted task's sp in a nested trap.
    let (elf, _) = trap_image(&[0x3401_1173, SP_DOWN_16, 0x0011_2023]);
    let functions = functions(&elf).unwrap();
    assert!(trap_entry(&elf, &functions, TEXT).is_err());
}

#[test]
fn a_vector_table_names_its_entries_by_relocations() {
    let elf = placed(
        TEXT,
        &[Symbol {
            name: "f",
            words: vec![RET],
            frame: Some(0),
        }],
        &[TEXT, TEXT],
        true,
        true,
    );
    assert_eq!(
        vector_table(&elf, "table").unwrap(),
        [Some(TEXT), Some(TEXT)]
    );
    assert!(vector_table(&elf, "missing").is_err());
}

fn binding(source: u16, level: u8, core: u32) -> [u32; 2] {
    [u32::from(source) | u32::from(level) << 16, core]
}

/// An image whose `__OER_INTERRUPT_TABLE` lists `entries` (source, level,
/// core, handler symbol index or none) in the ESP32-S31 binding layout.
fn table_image(entries: &[(u16, u8, u32, Option<u32>)], relocate_handlers: bool) -> Vec<u8> {
    let handler = Symbol {
        name: "handler",
        words: vec![SP_DOWN_16, SP_UP_16, RET],
        frame: Some(16),
    };
    let mut words = vec![RODATA + 8, entries.len() as u32];
    let mut relocated = vec![0];
    for &(source, level, core, target) in entries {
        words.extend(binding(source, level, core));
        if target.is_some() && relocate_handlers {
            relocated.push(words.len());
        }
        words.push(target.unwrap_or(0));
    }
    built(
        TEXT,
        &[handler],
        &Rodata {
            words: &words,
            relocated: &relocated,
            objects: &[
                ("__OER_INTERRUPT_TABLE", 0, 8),
                ("entries", 8, 12 * entries.len() as u32),
            ],
        },
    )
}

const S31_TABLE: TableLayout = TableLayout {
    entry: 12,
    source: Field { offset: 0, size: 2 },
    level: Field { offset: 2, size: 1 },
    core: Field { offset: 4, size: 4 },
    handler: 8,
};

#[test]
fn the_interrupt_table_reads_each_binding_and_its_relocated_handler() {
    let elf = table_image(&[(25, 1, 0, Some(TEXT)), (67, 8, 1, None)], true);
    let entries = interrupt_table(&elf, "__OER_INTERRUPT_TABLE", &S31_TABLE).unwrap();
    assert_eq!(
        entries,
        [
            TableEntry {
                source: 25,
                level: 1,
                core: 0,
                handler: Some(TEXT)
            },
            TableEntry {
                source: 67,
                level: 8,
                core: 1,
                handler: None
            },
        ]
    );
}

#[test]
fn a_handler_word_without_its_relocation_fails() {
    let elf = table_image(&[(25, 1, 0, Some(TEXT))], false);
    assert!(interrupt_table(&elf, "__OER_INTERRUPT_TABLE", &S31_TABLE).is_err());
    assert!(interrupt_table(&elf, "__MISSING", &S31_TABLE).is_err());
}

/// `lui a1, upper` then `addi a1, a1, lower`: `a1 = value`.
fn load_a1(value: u32) -> [u32; 2] {
    let lower = value & 0xfff;
    let upper = value.wrapping_add(0x800) & 0xffff_f000;
    [
        upper | (11 << 7) | 0x37,
        (lower << 20) | (11 << 15) | (11 << 7) | 0x13,
    ]
}

#[test]
fn constant_arguments_collect_a_register_at_every_direct_call() {
    let callee = TEXT + 4 * 12;
    let [lui, addi] = load_a1(0x1234);
    let [lui2, addi2] = load_a1(0x5678);
    let caller = Symbol {
        name: "caller",
        words: vec![
            SP_DOWN_16,
            SAVE_RA,
            lui,
            addi,
            call(TEXT + 16, callee),
            lui2,
            addi2,
            call(TEXT + 28, callee),
            LOAD_RA,
            SP_UP_16,
            RET,
            BRANCH_NEXT,
        ],
        frame: Some(16),
    };
    let elf = executable(&[
        caller,
        Symbol {
            name: "callee",
            words: vec![RET],
            frame: Some(0),
        },
    ]);
    let analysis = analyze(&elf, &[], &[]).unwrap();
    assert_eq!(
        analysis.constant_arguments(callee, 1).unwrap(),
        BTreeSet::from([0x1234, 0x5678])
    );
    // a2 is never set: not exact.
    assert!(analysis.constant_arguments(callee, 2).is_err());
    assert!(!address_taken(&elf, callee).unwrap());
}

#[test]
fn a_relocated_data_word_takes_a_function_s_address() {
    let elf = labelled(&[leaf(Some(32))], &[TEXT]);
    assert!(address_taken(&elf, TEXT).unwrap());
    assert!(!address_taken(&elf, TEXT + 4).unwrap());
}

/// `source`, a `no_std` program with an `_start`, compiled and linked for
/// RV32 by the repository's `rustc` with debug information and the link's
/// relocations kept, as images are.
fn compiled(source: &str) -> Vec<u8> {
    // One directory per call: tests compile the same program in parallel.
    static CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let directory = std::env::temp_dir().join(format!(
        "oer-riscv-stack-{}-{}",
        std::process::id(),
        CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let input = directory.join("main.rs");
    let output = directory.join("main.elf");
    std::fs::write(&input, source).unwrap();
    let status = std::process::Command::new("rustc")
        // Frames come from `.stack_sizes`, as for images.
        .env("RUSTC_BOOTSTRAP", "1")
        .args(["-Z", "emit-stack-sizes"])
        .args(["--edition", "2024", "--crate-type", "bin"])
        .args(["--target", "riscv32imafc-unknown-none-elf"])
        .args([
            "-C",
            "opt-level=s",
            "-C",
            "debuginfo=2",
            "-C",
            "panic=abort",
        ])
        .args(["-C", "link-arg=--emit-relocs", "-C", "link-arg=-e_start"])
        // `keep`, which a program defines to hold what `_start` does not
        // reach, survives the link's garbage collection.
        .args(["-C", "link-arg=--undefined=keep"])
        .arg("-o")
        .arg(&output)
        .arg(&input)
        .status()
        .unwrap();
    assert!(status.success());
    let elf = std::fs::read(&output).unwrap();
    let _ = std::fs::remove_dir_all(&directory);
    elf
}

/// `program` linked against the library crate `shared` built from `library`,
/// with several codegen units each, as images are.
fn compiled_with_library(library: &str, program: &str) -> Vec<u8> {
    let directory = std::env::temp_dir().join(format!(
        "oer-riscv-stack-{}-{}-library",
        std::process::id(),
        program.len()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let rustc = |arguments: &[&std::ffi::OsStr]| {
        let status = std::process::Command::new("rustc")
            .env("RUSTC_BOOTSTRAP", "1")
            .args(["-Z", "emit-stack-sizes"])
            .args(["--edition", "2024"])
            .args(["--target", "riscv32imafc-unknown-none-elf"])
            .args([
                "-C",
                "opt-level=s",
                "-C",
                "debuginfo=2",
                "-C",
                "panic=abort",
            ])
            .args(["-C", "codegen-units=4"])
            .args(arguments)
            .status()
            .unwrap();
        assert!(status.success());
    };
    let library_source = directory.join("shared.rs");
    let program_source = directory.join("main.rs");
    let output = directory.join("main.elf");
    std::fs::write(&library_source, library).unwrap();
    std::fs::write(&program_source, program).unwrap();
    rustc(&[
        "--crate-type".as_ref(),
        "rlib".as_ref(),
        "--crate-name".as_ref(),
        "shared".as_ref(),
        "--out-dir".as_ref(),
        directory.as_os_str(),
        library_source.as_os_str(),
    ]);
    let rlib = directory.join("libshared.rlib");
    let mut extern_shared = std::ffi::OsString::from("shared=");
    extern_shared.push(&rlib);
    rustc(&[
        "--crate-type".as_ref(),
        "bin".as_ref(),
        "--extern".as_ref(),
        extern_shared.as_os_str(),
        "-C".as_ref(),
        "link-arg=--emit-relocs".as_ref(),
        "-C".as_ref(),
        "link-arg=-e_start".as_ref(),
        "-C".as_ref(),
        "link-arg=--undefined=keep".as_ref(),
        "-o".as_ref(),
        output.as_os_str(),
        program_source.as_os_str(),
    ]);
    let elf = std::fs::read(&output).unwrap();
    let _ = std::fs::remove_dir_all(&directory);
    elf
}

const SHARED_LIBRARY: &str = r#"
#![no_std]
pub struct Payload<T> {
    pub value: T,
    pub tag: u16,
}

#[inline(never)]
pub fn consume(payload: &mut Payload<u32>, extra: Option<&u8>) -> u32 {
    payload.tag = payload.tag.wrapping_add(1);
    payload.value + extra.map_or(0, |extra| u32::from(*extra))
}
"#;

const SHARED_PROGRAM: &str = r#"
#![no_std]
#![no_main]
use shared::Payload;

static mut HOOK: Option<fn(&mut Payload<u32>, Option<&u8>) -> u32> = None;

mod local {
    #[inline(never)]
    pub fn replace(payload: &mut shared::Payload<u32>, _: Option<&u8>) -> u32 {
        payload.value = 0;
        1
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn keep(on: bool) {
    unsafe { HOOK = Some(if on { shared::consume } else { local::replace }) };
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    let mut payload = Payload { value: 7, tag: 0 };
    if let Some(hook) = unsafe { HOOK } {
        core::hint::black_box(hook(&mut payload, None));
    }
    loop {}
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
"#;

/// The soundness of matching Rust-ABI fields by type name rests on rustc
/// naming one type identically in every crate and codegen unit: a field
/// typed in one crate reaches a function of another crate and one of
/// another unit.
#[test]
fn a_type_named_in_another_crate_or_unit_still_matches() {
    let elf = compiled_with_library(SHARED_LIBRARY, SHARED_PROGRAM);
    let analysis = analyze(&elf, &[], &[]).unwrap();
    let types = TypeFacts::read(&elf).unwrap();
    let taken = taken_addresses(&elf).unwrap();
    let named = |name: &str| {
        functions(&elf)
            .unwrap()
            .into_iter()
            .find(|function| {
                function
                    .names
                    .iter()
                    .any(|candidate| candidate.contains(name))
            })
            .unwrap()
            .address
    };
    let (start, consume, replace) = (named("_start"), named("consume"), named("replace"));
    let resolutions = function_pointer_resolutions(&analysis, &types, &taken);
    let site = analysis.functions[&start]
        .transfers
        .iter()
        .find(|transfer| transfer.target.is_none())
        .unwrap()
        .site;
    let candidates = resolutions
        .get(site)
        .map(|resolution| &resolution.targets)
        .expect("resolved");
    assert!(
        candidates.is_superset(&BTreeSet::from([consume, replace])),
        "{resolutions:x?} consume={consume:#x} replace={replace:#x}"
    );
}

const MERGED_PROGRAM: &str = r#"
#![no_std]
#![no_main]

static mut UNSIGNED: Option<fn(u32) -> u32> = None;
static mut SIGNED: Option<fn(i32) -> i32> = None;

#[inline(never)]
#[unsafe(no_mangle)]
fn step_unsigned(value: u32) -> u32 {
    value.wrapping_mul(3).wrapping_add(1)
}

#[inline(never)]
#[unsafe(no_mangle)]
fn step_signed(value: i32) -> i32 {
    value.wrapping_mul(3).wrapping_add(1)
}

#[inline(never)]
fn halve_signed(value: i32) -> i32 {
    value / 2
}

#[inline(never)]
fn halve_unsigned(value: u32) -> u32 {
    value >> 1
}

#[unsafe(no_mangle)]
pub extern "C" fn keep(on: bool) {
    unsafe {
        UNSIGNED = Some(if on { step_unsigned } else { halve_unsigned });
        SIGNED = Some(if on { step_signed } else { halve_signed });
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    if let Some(signed) = unsafe { SIGNED } {
        core::hint::black_box(signed(1));
    }
    if let Some(unsigned) = unsafe { UNSIGNED } {
        core::hint::black_box(unsigned(1));
    }
    loop {}
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
"#;

/// Function merging puts `step_signed` and `step_unsigned` at one address,
/// whose own subprogram is one of theirs; a field of either type still
/// reaches it, by the merged symbol's subprogram.
#[test]
fn a_merged_function_of_another_type_stays_a_candidate() {
    let elf = compiled(MERGED_PROGRAM);
    let analysis = analyze(&elf, &[], &[]).unwrap();
    let types = TypeFacts::read(&elf).unwrap();
    let taken = taken_addresses(&elf).unwrap();
    let named = |name: &str| {
        functions(&elf)
            .unwrap()
            .into_iter()
            .find(|function| function.names.iter().any(|candidate| candidate == name))
            .unwrap()
    };
    let (start, signed) = (named("_start").address, named("step_signed"));
    // The precondition: the two bodies were merged into one address.
    assert!(
        signed.names.iter().any(|name| name == "step_unsigned"),
        "not merged: {:?}",
        signed.names
    );
    let resolutions = function_pointer_resolutions(&analysis, &types, &taken);
    let sites: Vec<u32> = analysis.functions[&start]
        .transfers
        .iter()
        .filter(|transfer| transfer.target.is_none())
        .map(|transfer| transfer.site)
        .collect();
    assert_eq!(sites.len(), 2, "{sites:x?}");
    for site in sites {
        assert!(
            resolutions
                .get(site)
                .map(|resolution| &resolution.targets)
                .is_some_and(|targets| targets.contains(&signed.address)),
            "{site:#x}: {resolutions:x?}"
        );
    }
}

/// The MIR facts of the hook program's crate: `_start` calls through a
/// `fn(u32) -> u32`, and the crate makes `shallow` and `twice` such pointers,
/// `negate` a `fn(i32) -> i32` and `deep` a `fn(u8, u8)` (a stand-in arity);
/// `extra` adds leak facts.
fn hook_facts(elf: &[u8], call: &str, extra: &str) -> MirFacts {
    let symbol = |part: &str| {
        functions(elf)
            .unwrap()
            .into_iter()
            .flat_map(|function| function.names)
            .find(|name| name.contains(part))
            .map(|name| crate::mir::function_key(&name))
            .unwrap()
    };
    let extra = extra
        .replace("NEGATE", &symbol("negate"))
        .replace("DEEP", &symbol("deep"));
    let text = format!(
        r#"{{"schema": 2, "krate": "main",
            "calls": {{"_start": [{call}]}},
            "fn_pointers": {{"fn(u32) -> u32": ["{}", "{}"],
                             "fn(i32) -> i32": ["{}"],
                             "fn(u8, u8)": ["{}"]}},
            "vtables": {{}},
            "leaked_types": [], "leaked_functions": {{}}, "edges": {{}},
            "leaked_traits": [], "trait_contents": {{}}, "unknown_leak": false}}"#,
        symbol("shallow"),
        symbol("twice"),
        symbol("negate"),
        symbol("deep"),
    );
    // The extra facts replace the base's fields of their names.
    let mut facts: serde_json::Value = serde_json::from_str(&text).unwrap();
    if !extra.is_empty() {
        let extra: serde_json::Value = serde_json::from_str(&extra).unwrap();
        for (field, value) in extra.as_object().unwrap() {
            facts[field] = value.clone();
        }
    }
    MirFacts::from_json(&[&facts.to_string()]).unwrap()
}

#[test]
fn the_mir_facts_resolve_a_site_by_its_instance_s_calls() {
    let elf = compiled(HOOK_PROGRAM);
    let analysis = analyze(&elf, &[], &[]).unwrap();
    let dwarf = Dwarf::read(&elf).unwrap();
    let named = |name: &str| {
        functions(&elf)
            .unwrap()
            .into_iter()
            .find(|function| {
                function
                    .names
                    .iter()
                    .any(|candidate| candidate.contains(name))
            })
            .unwrap()
            .address
    };
    let (start, shallow, twice) = (named("_start"), named("shallow"), named("twice"));
    let (negate, deep) = (named("negate"), named("deep"));
    let site = analysis.functions[&start]
        .transfers
        .iter()
        .find(|transfer| transfer.target.is_none())
        .unwrap()
        .site;
    let pointer = r#"{"fn_pointer": "fn(u32) -> u32"}"#;
    let resolved =
        mir_resolutions(&elf, &analysis, &dwarf, &hook_facts(&elf, pointer, "")).unwrap();
    let resolution = resolved.get(site).expect("resolved");
    assert_eq!(resolution.targets, BTreeSet::from([shallow, twice]));
    assert_eq!(resolution.facts, BTreeSet::from([Fact::Mir]));
    // A call the MIR names no type for leaves the site a hole.
    let unknown = format!(r#"{pointer}, "unknown""#);
    let facts = hook_facts(&elf, &unknown, "");
    assert!(
        mir_resolutions(&elf, &analysis, &dwarf, &facts)
            .unwrap()
            .get(site)
            .is_none()
    );
    let targets = |extra: &str| {
        let facts = hook_facts(&elf, pointer, extra);
        mir_resolutions(&elf, &analysis, &dwarf, &facts)
            .unwrap()
            .get(site)
            .expect("resolved")
            .targets
            .clone()
    };
    // A leaked type's functions reach every site their ABI fits, and no
    // site of another argument count.
    assert_eq!(
        targets(r#"{"leaked_types": ["fn(i32) -> i32", "fn(u8, u8)"]}"#),
        BTreeSet::from([shallow, twice, negate])
    );
    // A type transmuted into the site's type reaches it.
    assert_eq!(
        targets(r#"{"edges": {"fn(u32) -> u32": ["fn(i32) -> i32"]}}"#),
        BTreeSet::from([shallow, twice, negate])
    );
    // A function a constant holds untyped leaks as its own signature.
    assert_eq!(
        targets(r#"{"leaked_functions": {"NEGATE": "fn(i32) -> i32", "DEEP": "fn(u8, u8)"}}"#),
        BTreeSet::from([shallow, twice, negate])
    );
    // A leaked trait leaks its vtable functions, of unknown signature, and
    // what its implementors carry.
    assert_eq!(
        targets(
            r#"{"vtables": {"main::Job": {"3": ["DEEP"]}},
                "leaked_traits": ["main::Job"],
                "trait_contents": {"main::Job": {"keys": ["fn(i32) -> i32"], "traits": [], "unknown": false}}}"#
        ),
        BTreeSet::from([shallow, twice, negate, deep])
    );
    // A leak of unknown contents leaks every function made a pointer.
    assert_eq!(
        targets(r#"{"unknown_leak": true}"#),
        BTreeSet::from([shallow, twice, negate])
    );
}

const WAKER_PROGRAM: &str = r#"
#![no_std]
#![no_main]
use core::task::{RawWaker, RawWakerVTable, Waker};

static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, wake, by_ref, drop);

fn mark(value: u32) {
    unsafe { core::ptr::write_volatile(0x1000 as *mut u32, value) }
}
unsafe fn clone(data: *const ()) -> RawWaker {
    mark(1);
    RawWaker::new(data, &VTABLE)
}
unsafe fn wake(_: *const ()) {
    let mut buffer = [0_u32; 32];
    for (i, word) in buffer.iter_mut().enumerate() {
        unsafe { core::ptr::write_volatile(word, i as u32) };
    }
    mark(buffer[3]);
}
unsafe fn by_ref(_: *const ()) {
    mark(3);
}
unsafe fn drop(_: *const ()) {
    mark(4);
}

#[unsafe(no_mangle)]
pub extern "C" fn _start(waker: &Waker) {
    waker.wake_by_ref();
}

#[unsafe(no_mangle)]
pub extern "C" fn keep() -> *const RawWakerVTable {
    &VTABLE
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    loop {}
}
"#;

#[test]
fn a_waker_call_reaches_its_slot_of_every_waker_vtable() {
    let elf = compiled(WAKER_PROGRAM);
    let analysis = analyze(&elf, &[], &[]).unwrap();
    let dwarf = Dwarf::read(&elf).unwrap();
    let vtables = waker_vtables(&elf, &dwarf, &analysis).unwrap();
    assert_eq!(vtables.len(), 1, "{vtables:x?}");
    let start = functions(&elf)
        .unwrap()
        .into_iter()
        .find(|function| function.names.iter().any(|name| name == "_start"))
        .unwrap()
        .address;
    assert_eq!(analysis.bound(start).unwrap().bytes, None);
    let resolutions = waker_resolutions(&analysis, &dwarf, &vtables).unwrap();
    // `_start`'s `wake_by_ref` reaches `by_ref` (slot 2), not `wake`.
    let site = analysis.functions[&start]
        .transfers
        .iter()
        .find(|transfer| transfer.target.is_none())
        .unwrap()
        .site;
    assert_eq!(
        resolutions.get(site).unwrap().targets,
        BTreeSet::from([vtables[0][2]])
    );
    let bound = analysis.bound_with(start, &resolutions).unwrap();
    assert!(bound.unresolved.is_empty(), "{bound:?}");
    assert!(bound.bytes.is_some());
}

#[test]
fn a_hart_stacks_one_interrupt_per_level_and_an_exception() {
    // lui a5, 3 (the source table at 0x3000); add a5, a5, a0; lw a5, 0(a5).
    const LUI_SOURCES: u32 = (3 << 12) | (15 << 7) | 0x37;
    const ADD_INDEX: u32 = (10 << 20) | (15 << 15) | (15 << 7) | 0x33;
    const LOAD_ENTRY: u32 = (15 << 15) | (2 << 12) | (15 << 7) | 0x03;
    // slli a0, a0, 2: the source number scaled to a word index.
    const SCALE_INDEX: u32 = (2 << 20) | (10 << 15) | (1 << 12) | (10 << 7) | 0x13;
    let dispatcher = Symbol {
        name: "dispatcher",
        words: vec![
            SP_DOWN_16,
            SAVE_RA,
            LUI_SOURCES,
            SCALE_INDEX,
            ADD_INDEX,
            LOAD_ENTRY,
            CALL_A5,
            LOAD_RA,
            SP_UP_16,
            RET,
        ],
        frame: Some(16),
    };
    let small = Symbol {
        name: "small",
        words: vec![SP_DOWN_16, SP_UP_16, RET],
        frame: Some(16),
    };
    let large = Symbol {
        name: "large",
        words: vec![SP_DOWN_32, SP_UP_32, RET],
        frame: Some(32),
    };
    let elf = executable(&[dispatcher, small, large]);
    let analysis = analyze(&elf, &[], &[]).unwrap();
    let (dispatcher, small, large) = (TEXT, TEXT + 40, TEXT + 52);
    let entry = |level, core, handler| TableEntry {
        source: 0,
        level,
        core,
        handler: Some(handler),
    };
    let table = [entry(1, 0, small), entry(8, 0, large), entry(1, 1, small)];
    let vector = TrapEntry {
        entry: 0x100,
        frame: 80,
        handler: dispatcher,
    };
    let exception = TrapEntry {
        entry: 0x200,
        frame: 80,
        handler: small,
    };
    let harts = interrupt_stacks(
        &analysis,
        &Stacks {
            table: &table,
            cores: &[0, 1],
            always: &[1],
            vectors: &[vector],
            exception,
            sources: 0x3000,
            resolutions: &Resolutions::new(),
        },
    )
    .unwrap();
    // Level 1: 80 + 16 + 16; level 8: 80 + 16 + 32; the exception 80 + 16.
    let levels = |hart: &HartStack| {
        hart.levels
            .iter()
            .map(|level| (level.level, level.bytes))
            .collect::<Vec<_>>()
    };
    assert_eq!(levels(&harts[0]), [(1, Some(112)), (8, Some(128))]);
    assert_eq!(harts[0].exception.bytes, Some(96));
    assert_eq!(harts[0].bytes, Some(112 + 128 + 96));
    // Hart 1 has no level-8 entry; its level 1 reaches the small handler only.
    assert_eq!(levels(&harts[1]), [(1, Some(112))]);
    assert_eq!(harts[1].bytes, Some(112 + 96));
}

const HOOK_PROGRAM: &str = r#"
#![no_std]
#![no_main]

static mut HOOK: Option<fn(u32) -> u32> = None;
static mut OTHER: Option<fn(u8)> = None;
static mut NEGATE: Option<fn(i32) -> i32> = None;

#[inline(never)]
fn shallow(value: u32) -> u32 {
    value + 1
}

#[inline(never)]
fn twice(value: u32) -> u32 {
    value * 2
}

#[inline(never)]
fn negate(value: i32) -> i32 {
    -value
}

#[inline(never)]
fn halve(value: i32) -> i32 {
    value / 2
}

#[inline(never)]
fn deep(value: u8) {
    let mut buffer = [0u32; 64];
    for (i, word) in buffer.iter_mut().enumerate() {
        unsafe { core::ptr::write_volatile(word, i as u32 + u32::from(value)) };
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn keep(on: bool) {
    unsafe {
        HOOK = Some(if on { shallow } else { twice });
        if on {
            OTHER = Some(deep);
        }
        if let Some(other) = OTHER {
            other(1);
        }
        NEGATE = Some(if on { negate } else { halve });
        if let Some(negate) = NEGATE {
            core::hint::black_box(negate(1));
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _start(value: u32) -> u32 {
    match unsafe { HOOK } {
        Some(hook) => hook(value),
        None => 0,
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    loop {}
}
"#;

#[test]
fn a_call_through_a_static_s_function_pointer_reaches_the_taken_functions_of_its_type() {
    let elf = compiled(HOOK_PROGRAM);
    let analysis = analyze(&elf, &[], &[]).unwrap();
    let types = TypeFacts::read(&elf).unwrap();
    let taken = taken_addresses(&elf).unwrap();
    let named = |name: &str| {
        functions(&elf)
            .unwrap()
            .into_iter()
            .find(|function| {
                function
                    .names
                    .iter()
                    .any(|candidate| candidate.contains(name))
            })
            .unwrap()
            .address
    };
    let (start, shallow, twice, negate, deep) = (
        named("_start"),
        named("shallow"),
        named("twice"),
        named("negate"),
        named("deep"),
    );
    assert!(taken.contains(&shallow) && taken.contains(&deep));
    assert_eq!(analysis.bound(start).unwrap().bytes, None);
    let resolutions = function_pointer_resolutions(&analysis, &types, &taken);
    let site = analysis.functions[&start]
        .transfers
        .iter()
        .find(|transfer| transfer.target.is_none())
        .unwrap()
        .site;
    // `negate` and `halve` have the sizes of `fn(u32) -> u32` but `i32`, and
    // `deep`'s parameter count differs: none can be the field's.
    assert_eq!(
        resolutions.get(site).map(|resolution| &resolution.targets),
        Some(&BTreeSet::from([shallow, twice])),
        "{resolutions:x?} negate={negate:#x}"
    );
    let bound = analysis.bound_with(start, &resolutions).unwrap();
    assert!(bound.unresolved.is_empty(), "{bound:?}");
}
