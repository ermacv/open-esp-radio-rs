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
    let rodata = rodata_words;
    let rodata: Vec<u8> = rodata.iter().flat_map(|word| word.to_le_bytes()).collect();
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
    if labelled {
        entries.push((".LJTI0_0", RODATA, 0));
    }
    // Relocations with no symbol: the addend is the address.
    let mut rela = Vec::new();
    if labelled {
        for (i, word) in rodata_words.iter().enumerate() {
            rela.extend_from_slice(&(RODATA + 4 * i as u32).to_le_bytes());
            rela.extend_from_slice(&1u32.to_le_bytes()); // R_RISCV_32
            rela.extend_from_slice(&word.to_le_bytes());
        }
    }
    if sized {
        entries.push(("table", RODATA, rodata.len() as u32));
    }
    for (name, address, size) in &entries {
        let offset = strtab.len() as u32;
        strtab.extend_from_slice(name.as_bytes());
        strtab.push(0);
        symtab.extend_from_slice(&offset.to_le_bytes());
        symtab.extend_from_slice(&address.to_le_bytes());
        symtab.extend_from_slice(&size.to_le_bytes());
        let data = *address == RODATA;
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
