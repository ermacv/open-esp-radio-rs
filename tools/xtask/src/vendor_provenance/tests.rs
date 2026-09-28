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
    identifiers("phy_init.o (sha256 …) phy_rf_init", &mut words);
    assert!(
        !words.contains("phy_init"),
        "an archive member name is not a function"
    );
    assert!(words.contains("phy_rf_init"));
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
    let chip_dir = directory.path().join("esp32s31");
    std::fs::create_dir(&chip_dir).unwrap();
    std::fs::rename(directory.path().join("a.rs"), chip_dir.join("a.rs")).unwrap();
    let words = scan(directory.path(), "esp32s31").0;
    assert!(words.contains("hal_mac_tx_set_ppdu"));
    assert!(!words.contains("rcGetRate"));
    assert!(!words.contains("rcUpdateRate"));
}

fn supported() -> Vec<String> {
    vec!["esp32c5".into(), "esp32s31".into()]
}

/// Another chip's facts cite its own pins, so they are not this chip's
/// references.
#[test]
fn another_chips_directory_is_not_scanned() {
    let directory = tempfile::tempdir().unwrap();
    for (chip, symbol) in [
        ("esp32s31", "bt_bb_v2_init_cmplx"),
        ("esp32c5", "ieee802154_txon_delay_set"),
    ] {
        let crate_dir = directory.path().join("hardware").join(chip);
        std::fs::create_dir_all(&crate_dir).unwrap();
        std::fs::write(
            crate_dir.join("a.rs"),
            format!("// SOURCE: `{symbol}` body.\nfn x() {{}}\n"),
        )
        .unwrap();
    }
    let (words, problems) = scan(directory.path(), "esp32s31");
    assert!(words.contains("bt_bb_v2_init_cmplx"));
    assert!(!words.contains("ieee802154_txon_delay_set"));
    assert!(problems.is_empty(), "{problems:?}");
}

/// Words and violations of a scan of `chip` under `directory`.
fn scan(directory: &Path, chip: &str) -> (BTreeSet<String>, Vec<String>) {
    let supported = supported();
    let mut found = Found::default();
    production_words(
        &Scan {
            base: directory,
            chip,
            supported: &supported,
            citable: &supported,
        },
        directory,
        &mut found,
    )
    .unwrap();
    let mut problems = found.problems;
    for (location, _) in found.uncharted {
        problems.push(format!("{location}: names no chip"));
    }
    (found.words, problems)
}

/// A chip-neutral block cites only the chips it names, and names at least one.
#[test]
fn neutral_blocks_cite_the_chips_they_name() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("a.rs"),
        "// SOURCE(esp32s31): `bt_bb_v2_init_cmplx`\nfn x() {}\n\
         // SOURCE(esp32s31,esp32c5)[TAG]: `phy_get_data_sat`\nfn y() {}\n\
         // SOURCE: `phy_i2c_init1`\nfn z() {}\n",
    )
    .unwrap();
    let (s31, s31_problems) = scan(directory.path(), "esp32s31");
    let (c5, c5_problems) = scan(directory.path(), "esp32c5");
    assert!(s31.contains("bt_bb_v2_init_cmplx") && s31.contains("phy_get_data_sat"));
    assert!(!c5.contains("bt_bb_v2_init_cmplx") && c5.contains("phy_get_data_sat"));
    assert!(!s31.contains("phy_i2c_init1") && !c5.contains("phy_i2c_init1"));
    assert!(
        !s31.contains("esp32c5"),
        "chip names are not function candidates"
    );
    for problems in [s31_problems, c5_problems] {
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("a.rs:5") && problems[0].contains("names no chip"));
    }
}

#[test]
fn the_registry_round_trips() {
    let entries = vec![Entry {
        artifact: "libbtbb".into(),
        member: "bt_bb_v2.o".into(),
        symbol: "bt_bb_v2_init_cmplx".into(),
        code: "ab".into(),
        decisions: vec![],
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

/// An untagged neutral block is a violation of exactly the chips whose pins
/// define a function it names; a block citing only a standard passes.
#[test]
fn an_untagged_neutral_block_fails_only_the_chips_it_cites() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("a.rs"),
        "// SOURCE: `phy_i2c_init1` body\nfn x() {}\n\
         // SOURCE: IEEE Std 802.11-2020 12.7.1.6.2 (KDF)\nfn y() {}\n",
    )
    .unwrap();
    let supported = supported();
    let mut found = Found::default();
    production_words(
        &Scan {
            base: directory.path(),
            chip: "esp32c5",
            supported: &supported,
            citable: &supported,
        },
        directory.path(),
        &mut found,
    )
    .unwrap();
    assert_eq!(found.uncharted.len(), 2);
    let c5: BTreeSet<&str> = ["phy_i2c_init1"].into();
    let s31: BTreeSet<&str> = ["bt_bb_v2_init_cmplx"].into();
    let c5_problems = uncharted_citations(&found.uncharted, &c5);
    assert_eq!(c5_problems.len(), 1, "{c5_problems:?}");
    assert!(c5_problems[0].starts_with("a.rs:1") && c5_problems[0].contains("phy_i2c_init1"));
    assert!(uncharted_citations(&found.uncharted, &s31).is_empty());
}

#[test]
fn decisions_cite_the_code_shaped_names_their_places_quote() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("coverage.toml"),
        "[[decision]]\nreason = \"\"\"\nthe TXOP holding path, \\\nnot used\"\"\"\n\
         [[decision.place]]\nfunction = \"mac_tx_set_txop_q\"\nstart = 0x30\nend = 0x34\n\
         [[decision.place]]\nfunction = \"hal_mac_fill_hwtxop\"\n",
    )
    .unwrap();
    std::fs::write(
        directory.path().join("state.rs"),
        r#"("phy/src/target_port.rs", "self.registers,"), Place::Function("mac_tx_set_txop_q")"#,
    )
    .unwrap();
    std::fs::write(directory.path().join("notes.md"), r#""ignored_file_name""#).unwrap();
    let words = decision_words(directory.path()).unwrap();
    assert_eq!(
        words.keys().collect::<Vec<_>>(),
        ["hal_mac_fill_hwtxop", "mac_tx_set_txop_q"]
    );
    assert_eq!(
        words["mac_tx_set_txop_q"].iter().collect::<Vec<_>>(),
        ["coverage.toml", "state.rs"]
    );
}

#[test]
fn a_changed_decision_function_names_the_exclusions_to_review() {
    let files: BTreeSet<String> = ["coverage.toml".to_owned()].into();
    assert_eq!(
        review_hint(Some(&files)),
        "; re-review the exclusions in decisions/coverage.toml"
    );
    assert_eq!(review_hint(None), "");
}

#[test]
fn the_registry_round_trips_decision_citations() {
    let entries = vec![Entry {
        artifact: "libpp".into(),
        member: "hal_mac_tx.o".into(),
        symbol: "mac_tx_set_txop_q".into(),
        code: "abc".into(),
        decisions: vec!["coverage.rs".into()],
    }];
    assert_eq!(parse_registry(&render_registry(&entries)).unwrap(), entries);
}

#[test]
fn accepted_functions_take_the_printed_form() {
    assert_eq!(
        accepted_function("phy_write_pll_cap"),
        (None, "phy_write_pll_cap")
    );
    assert_eq!(
        accepted_function("libnet80211[wl_cnx.o]::sta_reset_beacon_timeout"),
        (
            Some(("libnet80211", "wl_cnx.o")),
            "sta_reset_beacon_timeout"
        )
    );
    assert_eq!(
        accepted_function("rom[]::sta_reset_beacon_timeout"),
        (Some(("rom", "")), "sta_reset_beacon_timeout")
    );
}
