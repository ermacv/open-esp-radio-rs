use super::*;

/// Little-endian code of RISC-V instruction words written as the
/// disassembler prints them: eight hex digits for a full instruction, four
/// for a compressed one.
fn code(words: &[&str]) -> Vec<u8> {
    words
        .iter()
        .flat_map(|word| {
            let value = u32::from_str_radix(word, 16).unwrap();
            let bytes = value.to_le_bytes();
            bytes[..word.len() / 2].to_vec()
        })
        .collect()
}

fn corpus(name: &str, code: Vec<u8>) -> Corpus {
    Corpus {
        functions: vec![Function {
            origin: "test".into(),
            name: name.into(),
            entry: 0,
            code,
            references: BTreeMap::new(),
        }],
        names: BTreeMap::new(),
        registers: Registers::default(),
        missing: vec![],
    }
}

fn accesses(corpus: &Corpus) -> Vec<Access> {
    corpus
        .walk(&corpus.functions[0])
        .into_iter()
        .filter_map(|step| step.access)
        .collect()
}

#[test]
fn a_read_modify_write_that_ors_a_constant_sets_those_bits() {
    // libpp pwr_hal_set_lpclk_sync_enable: lui/addi the word, lw, lui the
    // bit, or, sw, ret.
    let corpus = corpus(
        "sync_enable",
        code(&[
            "2010e7b7", "83078793", "4398", "080006b7", "8f55", "c398", "8082",
        ]),
    );
    assert_eq!(
        accesses(&corpus),
        [
            Access {
                address: 0x2010_d830,
                width: 4,
                effect: Effect::Read,
            },
            Access {
                address: 0x2010_d830,
                width: 4,
                effect: Effect::Modify {
                    clear: 0,
                    set: 0x0800_0000,
                    value: 0,
                },
            },
        ]
    );
}

#[test]
fn a_masked_argument_or_into_a_cleared_field_is_a_value_field() {
    // libcoexist hal_set_extern_pti: clear bits 15:12 through an and-mask,
    // then insert (argument << 4) & 0xff into bits 7:4.
    let corpus = corpus(
        "set_pti",
        code(&[
            "2010f7b7", "49c78793", "4398", "76c5", "16fd", "8f75", "c398", "4398", "0592",
            "0ff5f593", "f0f77713", "8dd9", "c38c", "8082",
        ]),
    );
    let writes: Vec<Effect> = accesses(&corpus)
        .into_iter()
        .filter(|access| access.effect != Effect::Read)
        .map(|access| access.effect)
        .collect();
    assert_eq!(
        writes,
        [
            Effect::Modify {
                clear: 0xf000,
                set: 0,
                value: 0,
            },
            Effect::Modify {
                clear: 0,
                set: 0,
                value: 0xf0,
            },
        ]
    );
}

#[test]
fn xref_lists_only_accesses_inside_the_range() {
    let corpus = corpus(
        "sync_enable",
        code(&[
            "2010e7b7", "83078793", "4398", "080006b7", "8f55", "c398", "8082",
        ]),
    );
    let inside = xref(&corpus, 0x2010_d830, 0x2010_d834);
    assert_eq!(inside.lines().count(), 2);
    assert!(inside.contains("W test::sync_enable+0x10  sets"));
    assert!(xref(&corpus, 0x2010_d834, 0x2010_d840).is_empty());
}

#[test]
fn show_prints_every_definition_with_folded_constants() {
    let corpus = corpus(
        "sync_enable",
        code(&[
            "2010e7b7", "83078793", "4398", "080006b7", "8f55", "c398", "8082",
        ]),
    );
    let text = show(&corpus, "sync_enable").unwrap();
    assert!(text.starts_with("== test::sync_enable at 0x0, 0x14 bytes"));
    assert!(text.contains("= 0x2010d830"));
    assert!(show(&corpus, "absent").is_none());
}
