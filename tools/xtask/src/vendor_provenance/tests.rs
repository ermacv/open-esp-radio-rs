use super::*;

#[test]
fn identifiers_skip_short_words_and_numbers() {
    let mut words = BTreeSet::new();
    identifiers(
        "// SOURCE: complete `libpp.a[trc.o]::rcGetRate`, size 0xd0; in 2 of",
        &mut words,
    );
    assert!(words.contains("rcGetRate"));
    assert!(words.contains("SOURCE"));
    identifiers("the ROM main loop may abort ru2str", &mut words);
    assert!(!words.contains("main") && !words.contains("abort"));
    assert!(words.contains("ru2str"));
    assert!(!words.contains("of"));
    assert!(!words.contains("0xd0"));
}

#[test]
fn only_comment_blocks_opened_by_the_marker_are_cited() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("a.rs"),
        "// Unrelated rcGetRate prose.\n\
         fn x() {}\n\
         /// SOURCE: complete\n\
         /// `hal_mac_tx_set_ppdu` body.\n\
         fn y() {}\n\
         // rcUpdateRate after the block\n",
    )
    .unwrap();
    let mut words = BTreeSet::new();
    production_words(directory.path(), &mut words).unwrap();
    assert!(words.contains("hal_mac_tx_set_ppdu"));
    assert!(!words.contains("rcGetRate"));
    assert!(!words.contains("rcUpdateRate"));
}

#[test]
fn the_registry_round_trips() {
    let entries = vec![Entry {
        artifact: "libbtbb".into(),
        member: "bt_bb_v2.o".into(),
        symbol: "bt_bb_v2_init_cmplx".into(),
        code: "ab".into(),
    }];
    assert_eq!(parse_registry(&render_registry(&entries)).unwrap(), entries);
}

#[test]
fn register_descriptions_are_cited_at_any_depth() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("model")).unwrap();
    std::fs::write(
        directory.path().join("model/a.toml"),
        "[[sources]]\ndescription = \"complete bt_bb_v2_init_cmplx\"\n\
         [[peripherals.registers]]\n\
         [peripherals.registers.register]\nname = \"rcGetRate\"\n\
         [[peripherals.registers.register.fields]]\n\
         description = \"bt_bb_rx_dpo_set replaces this field\"\n",
    )
    .unwrap();
    let mut words = BTreeSet::new();
    register_words(directory.path(), &mut words).unwrap();
    assert!(words.contains("bt_bb_v2_init_cmplx"));
    assert!(words.contains("bt_bb_rx_dpo_set"));
    assert!(!words.contains("rcGetRate"));
}
