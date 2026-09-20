use std::{
    fs,
    sync::atomic::{AtomicUsize, Ordering},
};

use object::{
    Architecture, BinaryFormat, Endianness, SectionKind, SymbolFlags, SymbolKind, SymbolScope,
    write::{Object, Symbol, SymbolSection},
};

use super::{
    ReviewedStackFrame, StackBudget, StackFrame, StackSourceLocation, analyze_stack, audit_stack,
    decode_stack_sizes, reviewed_frame_matches,
};

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

#[test]
fn symbol_coverage_distinguishes_aliases_missing_rust_other_text_and_rom() {
    use super::{StackCoverageOrigin as Origin, StackCoverageStatus as Status};
    for include_missing in [false, true] {
        let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
        let text = object.add_section(Vec::new(), b".text".to_vec(), SectionKind::Text);
        object.append_section_data(text, &[0; 0x3100], 4);
        for (name, value, section, kind) in [
            (
                "_stack_start",
                0x9000,
                SymbolSection::Absolute,
                SymbolKind::Data,
            ),
            (
                "_stack_end",
                0x8000,
                SymbolSection::Absolute,
                SymbolKind::Data,
            ),
            (
                "measured",
                0x3000,
                SymbolSection::Section(text),
                SymbolKind::Text,
            ),
            (
                "measured_alias",
                0x3000,
                SymbolSection::Section(text),
                SymbolKind::Text,
            ),
            (
                "_ZN7fixture4lost17h1234567890abcdefE",
                0x3010,
                SymbolSection::Section(text),
                SymbolKind::Text,
            ),
            (
                "asm_or_c",
                0x3020,
                SymbolSection::Section(text),
                SymbolKind::Text,
            ),
            (
                "rom_call",
                0x4000,
                SymbolSection::Absolute,
                SymbolKind::Text,
            ),
        ] {
            object.add_symbol(Symbol {
                name: name.as_bytes().to_vec(),
                value,
                size: 4,
                kind,
                scope: SymbolScope::Linkage,
                weak: false,
                section,
                flags: SymbolFlags::None,
            });
        }
        let section = object.add_section(
            Vec::new(),
            b".stack_sizes".to_vec(),
            SectionKind::ReadOnlyData,
        );
        let mut data = Vec::new();
        for address in [0x3000_u32, 0x3050].into_iter().chain(
            include_missing
                .then_some([0x3010, 0x3020])
                .into_iter()
                .flatten(),
        ) {
            data.extend(address.to_le_bytes());
            data.push(0); // A zero-sized frame is measured, never missing.
        }
        object.append_section_data(section, &data, 1);
        let path = std::env::temp_dir().join(format!(
            "oer-coverage-{}-{}.elf",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&path, object.write().unwrap()).unwrap();
        let report = analyze_stack(&path, &budget()).unwrap();
        let coverage_path = path.with_extension("toml");
        fs::write(&coverage_path, "schema = 1\nreviewed = []\n").unwrap();
        let mut strict_budget = budget();
        strict_budget.coverage_policy = Some(coverage_path.clone());
        let strict = analyze_stack(&path, &strict_budget).unwrap();
        assert!(strict.coverage_reviewed);
        assert_eq!(audit_stack(&strict).is_ok(), include_missing);
        assert_eq!(
            strict.audit.errors.len(),
            if include_missing { 0 } else { 2 }
        );
        fs::remove_file(coverage_path).unwrap();
        fs::remove_file(path).unwrap();
        assert_eq!(report.coverage.linked_text_addresses, 3);
        assert_eq!(
            report.coverage.measured_linked_text_addresses,
            if include_missing { 3 } else { 1 }
        );
        assert_eq!(
            report.coverage.linked_text_status,
            if include_missing {
                Status::Complete
            } else {
                Status::Incomplete
            }
        );
        assert_eq!(report.coverage.metadata_without_text_symbol, [0x3050]);
        let missing = &report.coverage.unmeasured_functions;
        if include_missing {
            assert_eq!(missing.len(), 1);
        } else {
            assert_eq!(missing[0].origin, Origin::LinkedRustSymbol);
            assert_eq!(missing[1].origin, Origin::LinkedOtherText);
            assert_eq!(missing.len(), 3);
        }
        assert_eq!(missing.last().unwrap().origin, Origin::AbsoluteText);
        // Frame-size policy success does not overwrite the independent coverage result.
        audit_stack(&report).unwrap();
        let human = super::render_stack_report(&report);
        assert!(human.contains("Measured-frame budget audit only"));
        assert!(human.contains("rom_call"));
        assert!(human.contains("metadata without text symbol: 0x00003050"));
        if !include_missing {
            assert!(human.contains("Incomplete"));
        }
    }
}

#[test]
fn absolute_declarations_do_not_establish_linked_coverage() {
    let path = test_elf(true, 0, 0x2000, 0x1000);
    let report = analyze_stack(&path, &budget()).unwrap();
    fs::remove_file(path).unwrap();
    assert_eq!(report.coverage.linked_text_addresses, 0);
    assert_eq!(report.coverage.measured_linked_text_addresses, 0);
    assert_eq!(
        report.coverage.linked_text_status,
        super::StackCoverageStatus::Unavailable
    );
}

#[test]
fn reviewed_match_inventory_includes_small_frames_and_unused_rules() {
    let path = test_elf(true, 0, 0x2000, 0x1000);
    let mut policy = budget();
    let mut matched = policy.reviewed_frames[0].clone();
    matched.function_contains = "<unknown function at".into();
    policy.reviewed_frames[0].function_contains = "unused_rule".into();
    policy.reviewed_frames.push(matched);
    let report = analyze_stack(&path, &policy).unwrap();
    fs::remove_file(path).unwrap();
    assert!(report.reviewed_rule_matches[0].addresses.is_empty());
    assert_eq!(report.reviewed_rule_matches[1].addresses, [0x3000]);
    assert_eq!(report.schema, 2);
}

fn budget() -> StackBudget {
    StackBudget {
        schema: 4,
        coverage_policy: None,
        stack_start_symbol: "_stack_start".into(),
        stack_end_symbol: "_stack_end".into(),
        warn_frame_bytes: 8 * 1024,
        max_frame_bytes: 32 * 1024,
        max_move_bytes: 4 * 1024,
        runtime_cpu0_minimum_free_bytes: 32 * 1024,
        runtime_cpu1_minimum_free_bytes: 4 * 1024,
        runtime_irq_minimum_free_bytes: 4 * 1024,
        reported_frame_count: 30,
        reviewed_frames: vec![ReviewedStackFrame {
            function_contains: "<unknown function at".into(),
            source_ends_with: Vec::new(),
            max_bytes: 32 * 1024,
            reason: "synthetic fixture".into(),
            execution_stack: None,
        }],
    }
}

#[test]
fn stack_policy_separates_review_and_hard_limits() {
    assert!(budget().validate().is_ok());
    let mut invalid = budget();
    invalid.warn_frame_bytes = invalid.max_frame_bytes + 1;
    assert!(invalid.validate().is_err());
}

#[test]
fn irq_stack_policy_requires_a_nonzero_reserve() {
    let mut policy = budget();
    policy.runtime_irq_minimum_free_bytes = 0;
    assert!(policy.validate().is_err());
    policy.runtime_irq_minimum_free_bytes = 1;
    assert!(policy.validate().is_ok());
}

#[test]
fn decodes_little_endian_32_bit_stack_entries() {
    let data = [
        0x88, 0x08, 0x00, 0x2f, 0x00, // 0x2f000888, 0 bytes
        0xf0, 0x08, 0x00, 0x2f, 0xd0, 0x01, // 0x2f0008f0, 208 bytes
    ];
    assert_eq!(
        decode_stack_sizes(&data, 4, true).unwrap(),
        vec![(0x2f00_0888, 0), (0x2f00_08f0, 208)]
    );
}

#[test]
fn rejects_truncated_stack_metadata() {
    assert!(decode_stack_sizes(&[1, 2, 3], 4, true).is_err());
    assert!(decode_stack_sizes(&[1, 0, 0, 0, 0x80], 4, true).is_err());
}

#[test]
fn frame_json_retains_complete_function_identity() {
    let frame = StackFrame {
        address: 0x5000_0000,
        size: 123,
        functions: vec!["crate::complete::function".into()],
        source: None,
    };
    assert_eq!(frame.functions[0], "crate::complete::function");
}

#[test]
fn exact_frame_limit_passes_and_one_byte_over_fails() {
    let at_limit = test_elf(true, 32 * 1024, 0x2000, 0x1000);
    let report = analyze_stack(&at_limit, &budget()).unwrap();
    assert!(report.violations.is_empty());
    assert!(audit_stack(&report).is_ok());
    fs::remove_file(at_limit).unwrap();

    let over_limit = test_elf(true, 32 * 1024 + 1, 0x2000, 0x1000);
    let report = analyze_stack(&over_limit, &budget()).unwrap();
    assert_eq!(report.violations.len(), 1);
    assert!(audit_stack(&report).is_err());
    fs::remove_file(over_limit).unwrap();
}

#[test]
fn unreviewed_large_frame_and_reviewed_growth_fail() {
    let path = test_elf(true, 9 * 1024, 0x2000, 0x1000);
    let mut policy = budget();
    policy.reviewed_frames.clear();
    let report = analyze_stack(&path, &policy).unwrap();
    assert!(report.audit.errors[0].contains("unreviewed frame"));
    assert!(audit_stack(&report).is_err());

    policy.reviewed_frames.push(ReviewedStackFrame {
        function_contains: "<unknown function at".into(),
        source_ends_with: Vec::new(),
        max_bytes: 8 * 1024 + 512,
        reason: "synthetic fixture".into(),
        execution_stack: None,
    });
    let report = analyze_stack(&path, &policy).unwrap();
    assert!(report.audit.errors[0].contains("reviewed frame"));
    assert!(audit_stack(&report).is_err());
    fs::remove_file(path).unwrap();
}

#[test]
fn reviewed_source_accepts_repository_and_cargo_trimmed_identities() {
    let reviewed = ReviewedStackFrame {
        function_contains: "::supervisor::run".into(),
        source_ends_with: vec![
            "crates/composition/esp32s31/embassy/ieee80211/src/supervisor.rs".into(),
            "open-esp-radio-esp32s31-embassy-wifi-0.1.0/src/supervisor.rs".into(),
        ],
        max_bytes: 16 * 1024,
        reason: "fixture".into(),
        execution_stack: None,
    };
    let frame = |file: &str| StackFrame {
        address: 0,
        size: 9 * 1024,
        functions: vec!["crate::supervisor::run::{closure#0}".into()],
        source: Some(StackSourceLocation {
            file: file.into(),
            line: Some(1),
            column: Some(1),
        }),
    };

    assert!(reviewed_frame_matches(
        &frame("/checkout/crates/composition/esp32s31/embassy/ieee80211/src/supervisor.rs"),
        &reviewed
    ));
    assert!(reviewed_frame_matches(
        &frame("./open-esp-radio-esp32s31-embassy-wifi-0.1.0/src/supervisor.rs"),
        &reviewed
    ));
    assert!(!reviewed_frame_matches(
        &frame("./another-package-0.1.0/src/supervisor.rs"),
        &reviewed
    ));
}

#[test]
fn overlapping_reviews_fail_independently_of_order_and_frame_size() {
    // The broad allowance used to hide the narrower limit when listed first.
    // Ambiguity also matters for small frames: a rule may select a different
    // execution stack, whose headroom must not depend on policy order.
    for size in [1024, 12 * 1024] {
        let path = test_elf(true, size, 0x20000, 0x1000);
        let mut policy = budget();
        let mut narrower = policy.reviewed_frames[0].clone();
        narrower.function_contains = "unknown function".into();
        narrower.max_bytes = 9 * 1024;
        policy.reviewed_frames.push(narrower);
        for _ in 0..2 {
            let report = analyze_stack(&path, &policy).unwrap();
            assert!(audit_stack(&report).is_err());
            assert_eq!(report.violations.len(), 1);
            let error = &report.audit.errors[0];
            assert!(error.contains("matches multiple reviewed rules"));
            assert!(error.contains("reviewed_frames[0]"));
            assert!(error.contains("reviewed_frames[1]"));
            for rule in &report.reviewed_rule_matches {
                assert_eq!(rule.addresses, [0x3000]);
            }
            policy.reviewed_frames.reverse();
        }
        fs::remove_file(path).unwrap();
    }
}

#[test]
fn execution_stack_rejects_reviewed_frames_that_exceed_storage_or_headroom() {
    let mut policy = budget();
    policy.reviewed_frames[0].execution_stack = Some(super::ExecutionStack {
        storage_symbol: "secondary_stack".into(),
        minimum_free_bytes: 4 * 1024,
    });
    for (size, passes) in [(12 * 1024, true), (12 * 1024 + 1, false), (17_424, false)] {
        let path = test_elf(true, size, 0x20000, 0x1000);
        let report = analyze_stack(&path, &policy).unwrap();
        assert_eq!(audit_stack(&report).is_ok(), passes);
        if !passes {
            assert!(report.audit.errors[0].contains("secondary_stack"));
        }
        fs::remove_file(path).unwrap();
    }
}

#[test]
fn execution_stack_is_checked_below_the_global_warning_threshold() {
    let mut policy = budget();
    policy.reviewed_frames[0].execution_stack = Some(super::ExecutionStack {
        storage_symbol: "secondary_stack".into(),
        minimum_free_bytes: 12 * 1024,
    });
    let path = test_elf(true, 6 * 1024, 0x20000, 0x1000);
    assert!(audit_stack(&analyze_stack(&path, &policy).unwrap()).is_err());
    policy.reviewed_frames[0]
        .execution_stack
        .as_mut()
        .unwrap()
        .storage_symbol = "missing_stack".into();
    assert!(
        analyze_stack(&path, &policy)
            .unwrap_err()
            .to_string()
            .contains("missing execution stack")
    );
    fs::remove_file(path).unwrap();
}

#[test]
fn empty_stack_section_is_not_evidence_of_small_frames() {
    let path = test_elf_metadata(true, None, 0x2000, 0x1000);
    let error = analyze_stack(&path, &budget()).unwrap_err().to_string();
    assert!(error.contains("missing or empty .stack_sizes"));
    fs::remove_file(path).unwrap();
}

#[test]
fn missing_metadata_and_invalid_stack_symbols_fail_closed() {
    let missing = test_elf(false, 0, 0x2000, 0x1000);
    let error = analyze_stack(&missing, &budget()).unwrap_err().to_string();
    assert!(error.contains("missing or empty .stack_sizes"));
    fs::remove_file(missing).unwrap();

    let invalid = test_elf(true, 64, 0x1000, 0x2000);
    let error = analyze_stack(&invalid, &budget()).unwrap_err().to_string();
    assert!(error.contains("empty or upward-growing range"));
    fs::remove_file(invalid).unwrap();
}

fn test_elf(
    include_stack_sizes: bool,
    frame_size: u64,
    stack_start: u64,
    stack_end: u64,
) -> std::path::PathBuf {
    test_elf_metadata(
        include_stack_sizes,
        Some(frame_size),
        stack_start,
        stack_end,
    )
}

fn test_elf_metadata(
    include_stack_sizes: bool,
    frame_size: Option<u64>,
    stack_start: u64,
    stack_end: u64,
) -> std::path::PathBuf {
    let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    for (name, value, kind) in [
        ("_stack_start", stack_start, SymbolKind::Data),
        ("_stack_end", stack_end, SymbolKind::Data),
        ("fixture_poll", 0x3000, SymbolKind::Text),
    ] {
        object.add_symbol(Symbol {
            name: name.as_bytes().to_vec(),
            value,
            size: 0,
            kind,
            scope: SymbolScope::Linkage,
            weak: false,
            section: SymbolSection::Absolute,
            flags: SymbolFlags::None,
        });
    }
    let stack_section =
        object.add_section(Vec::new(), b".bss".to_vec(), SectionKind::UninitializedData);
    object.append_section_bss(stack_section, 16 * 1024, 16);
    object.add_symbol(Symbol {
        name: b"secondary_stack".to_vec(),
        value: 0,
        size: 16 * 1024,
        kind: SymbolKind::Data,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Section(stack_section),
        flags: SymbolFlags::None,
    });
    if include_stack_sizes {
        let section = object.add_section(
            Vec::new(),
            b".stack_sizes".to_vec(),
            SectionKind::ReadOnlyData,
        );
        let mut entry = Vec::new();
        if let Some(frame_size) = frame_size {
            entry.extend(0x3000_u32.to_le_bytes());
            encode_uleb128(frame_size, &mut entry);
        }
        object.append_section_data(section, &entry, 1);
    }
    let path = std::env::temp_dir().join(format!(
        "open-radio-stack-{}-{}.elf",
        std::process::id(),
        NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(&path, object.write().unwrap()).unwrap();
    path
}

fn encode_uleb128(mut value: u64, output: &mut Vec<u8>) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        output.push(byte);
        if value == 0 {
            return;
        }
    }
}
