use super::super::*;

const TEST_CONTROLLER_SRAM_ENCODING: ReviewedCompressedPointerEncoding =
    ReviewedCompressedPointerEncoding::new(
        "test-controller-sram-low20-word-address-v1",
        0x2f00_0000,
        20,
        2,
    );

fn compressed_pointer_context() -> StructuralPointerContext {
    let mut context = synthetic_delay_pointer_context();
    context
        .reviewed_compressed_pointer_encodings
        .push(TEST_CONTROLLER_SRAM_ENCODING);
    context
}

fn compressed_pointer_trace(name: &str, instructions: &[u32]) -> FunctionAnalysis {
    compressed_pointer_trace_with_context(name, instructions, &compressed_pointer_context())
}

fn compressed_pointer_trace_with_context(
    name: &str,
    instructions: &[u32],
    pointer_context: &StructuralPointerContext,
) -> FunctionAnalysis {
    const CALL_PREFIX: [u32; 3] = [
        0x0040_0513, // li a0, 4
        0x0000_0317, // auipc t1, 0
        0x0003_00e7, // jalr ra, 0(t1)
    ];
    let symbol = artifact::ArtifactSymbolDefinition {
        member: Some("compressed_pointer.o".to_owned()),
        name: name.to_owned(),
        address: 0x1000_0000,
        bytes: CALL_PREFIX
            .iter()
            .chain(instructions)
            .copied()
            .flat_map(u32::to_le_bytes)
            .collect(),
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let relocated_calls = direct::StructuralRelocatedCalls::from([(
        StructuralCallSite::new(&symbol, symbol.address as u32 + 4),
        ("test_opaque_pointer".to_owned(), None),
    )]);
    trace_binary_symbol(&symbol, &map(), &relocated_calls, pointer_context, None).unwrap()
}

const LW_A5_4_A0: u32 = 0x0045_2783;
const LUI_A4_LOW20_CEILING: u32 = 0x0010_0737;
const ADDI_A4_A4_NEGATIVE_ONE: u32 = 0xfff7_0713;
const AND_A5_A5_A4: u32 = 0x00e7_f7b3;
const SLLI_A5_A5_2: u32 = 0x0027_9793;
const LUI_A4_CONTROLLER_SRAM: u32 = 0x2f00_0737;
const OR_A5_A5_A4: u32 = 0x00e7_e7b3;
const LW_A0_12_A5: u32 = 0x00c7_a503;
const RET: u32 = 0x0000_8067;

#[test]
fn reviewed_low20_word_address_recovers_dynamic_ram_provenance() {
    let trace = compressed_pointer_trace(
        "reviewed_low20_word_address",
        &[
            LW_A5_4_A0,
            LUI_A4_LOW20_CEILING,
            ADDI_A4_A4_NEGATIVE_ONE,
            AND_A5_A5_A4,
            SLLI_A5_A5_2,
            LUI_A4_CONTROLLER_SRAM,
            OR_A5_A5_A4,
            LW_A0_12_A5,
            RET,
        ],
    );

    assert!(trace.reference_blockers.is_empty(), "{trace:#?}");
    let memory_reads = trace
        .reference_events
        .iter()
        .filter_map(|event| match event {
            DraftReferenceEvent::Memory {
                access: MemoryAccess::Read,
                width,
                region,
                ..
            } => Some((*width, region.as_str())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        memory_reads,
        vec![(32, "dynamic RAM address"), (32, "dynamic RAM address")]
    );
}

#[test]
fn equivalent_shift_pair_recovers_the_same_reviewed_pointer_provenance() {
    let trace = compressed_pointer_trace(
        "reviewed_low20_word_address_shift_pair",
        &[
            LW_A5_4_A0,
            0x00c7_9793, // slli a5, a5, 12
            0x00a7_d793, // srli a5, a5, 10
            LUI_A4_CONTROLLER_SRAM,
            OR_A5_A5_A4,
            LW_A0_12_A5,
            RET,
        ],
    );

    assert!(trace.reference_blockers.is_empty(), "{trace:#?}");
    assert_eq!(
        trace
            .reference_events
            .iter()
            .filter(|event| matches!(
                event,
                DraftReferenceEvent::Memory { region, .. } if region == "dynamic RAM address"
            ))
            .count(),
        2
    );
}

#[test]
fn reviewed_compressed_pointer_rejects_wrong_base_shift_mask_and_source() {
    let cases = [
        (
            "wrong_base",
            vec![
                LW_A5_4_A0,
                LUI_A4_LOW20_CEILING,
                ADDI_A4_A4_NEGATIVE_ONE,
                AND_A5_A5_A4,
                SLLI_A5_A5_2,
                0x2e00_0737, // lui a4, 0x2e000
                OR_A5_A5_A4,
                LW_A0_12_A5,
                RET,
            ],
            1,
        ),
        (
            "wrong_shift",
            vec![
                LW_A5_4_A0,
                LUI_A4_LOW20_CEILING,
                ADDI_A4_A4_NEGATIVE_ONE,
                AND_A5_A5_A4,
                0x0037_9793, // slli a5, a5, 3
                LUI_A4_CONTROLLER_SRAM,
                OR_A5_A5_A4,
                LW_A0_12_A5,
                RET,
            ],
            1,
        ),
        (
            "wrong_mask",
            vec![
                LW_A5_4_A0,
                0x0008_0737, // lui a4, 0x80
                ADDI_A4_A4_NEGATIVE_ONE,
                AND_A5_A5_A4,
                SLLI_A5_A5_2,
                LUI_A4_CONTROLLER_SRAM,
                OR_A5_A5_A4,
                LW_A0_12_A5,
                RET,
            ],
            1,
        ),
        (
            "mixed_memory_read_tokens",
            vec![
                LW_A5_4_A0,
                0x0085_2683, // lw a3, 8(a0)
                0x3ff0_0713, // li a4, 0x3ff
                AND_A5_A5_A4,
                0x0010_0737, // lui a4, 0x100
                0xc007_0713, // addi a4, a4, -0x400
                0x00e6_f6b3, // and a3, a3, a4
                0x00d7_e7b3, // or a5, a5, a3
                SLLI_A5_A5_2,
                LUI_A4_CONTROLLER_SRAM,
                OR_A5_A5_A4,
                LW_A0_12_A5,
                RET,
            ],
            2,
        ),
        (
            "missing_memory_provenance",
            vec![
                0x0005_0793, // mv a5, a0
                LUI_A4_LOW20_CEILING,
                ADDI_A4_A4_NEGATIVE_ONE,
                AND_A5_A5_A4,
                SLLI_A5_A5_2,
                LUI_A4_CONTROLLER_SRAM,
                OR_A5_A5_A4,
                LW_A0_12_A5,
                RET,
            ],
            0,
        ),
    ];

    for (name, instructions, source_reads) in cases {
        let trace = compressed_pointer_trace(name, &instructions);
        assert!(
            trace
                .reference_blockers
                .iter()
                .any(|blocker| blocker.contains("unmodeled-memory-load")),
            "{name} unexpectedly gained pointer provenance: {trace:#?}"
        );
        let dynamic_reads = trace
            .reference_events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    DraftReferenceEvent::Memory { region, .. } if region == "dynamic RAM address"
                )
            })
            .count();
        assert!(
            dynamic_reads <= source_reads,
            "{name} unexpectedly emitted the reconstructed-pointer access: {trace:#?}"
        );
    }
}

#[test]
fn invalid_reviewed_compressed_pointer_encodings_fail_closed() {
    let instructions = [
        LW_A5_4_A0,
        LUI_A4_LOW20_CEILING,
        ADDI_A4_A4_NEGATIVE_ONE,
        AND_A5_A5_A4,
        SLLI_A5_A5_2,
        LUI_A4_CONTROLLER_SRAM,
        OR_A5_A5_A4,
        LW_A0_12_A5,
        RET,
    ];
    let cases = [
        ReviewedCompressedPointerEncoding::new("", 0x2f00_0000, 20, 2),
        ReviewedCompressedPointerEncoding::new("overlapping-base", 0x2f00_0004, 20, 2),
        ReviewedCompressedPointerEncoding::new("overflowing-field", 0x2f00_0000, 31, 2),
        ReviewedCompressedPointerEncoding::new("empty-field", 0x2f00_0000, 0, 2),
    ];

    for encoding in cases {
        let mut context = synthetic_delay_pointer_context();
        context.reviewed_compressed_pointer_encodings.push(encoding);
        let trace = compressed_pointer_trace_with_context(
            "invalid_reviewed_compressed_pointer_encoding",
            &instructions,
            &context,
        );
        assert!(
            trace
                .reference_blockers
                .iter()
                .any(|blocker| blocker.contains("unmodeled-memory-load")),
            "invalid encoding unexpectedly granted pointer provenance: {trace:#?}"
        );
    }
}

#[test]
fn indexed_absolute_ram_preserves_argument_stride_and_field_offset() {
    let symbol = artifact::ArtifactSymbolDefinition {
        member: Some("diagnostic.o".to_owned()),
        name: "record_slot".to_owned(),
        address: 0x1000_0000,
        bytes: vec![
            0x13, 0x07, 0xc0, 0x02, // li a4, 44
            0xb3, 0x07, 0xe5, 0x02, // mul a5, a0, a4
            0x37, 0xf7, 0x02, 0x10, // lui a4, 0x1002f
            0x13, 0x07, 0x07, 0x56, // addi a4, a4, 0x560
            0xb3, 0x87, 0xe7, 0x00, // add a5, a5, a4
            0x23, 0xaa, 0xb7, 0x00, // sw a1, 20(a5)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let trace = trace_binary_symbol(
        &symbol,
        &map(),
        &direct::StructuralRelocatedCalls::new(),
        &StructuralPointerContext::default(),
        None,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert!(trace.reference_blockers.is_empty(), "{trace:#?}");
    let DraftReferenceEvent::Memory {
        access: MemoryAccess::Write,
        address,
        region,
        ..
    } = &trace.reference_events[0]
    else {
        panic!(
            "expected indexed RAM write, got {:#?}",
            trace.reference_events
        );
    };
    assert_eq!(region, "indexed RAM object");
    assert_eq!(
        address.memory_object_location_with_reads(&BTreeMap::new()),
        Some(MemoryObjectLocation {
            root: MemoryObjectRoot::Indexed {
                root: std::sync::Arc::new(MemoryObjectRoot::Absolute {
                    address: 0x1002_f560,
                }),
                argument: 0,
                stride: 0x2c,
            },
            offset: 0x14,
        })
    );
}

#[test]
fn shifted_argument_index_preserves_absolute_table_read() {
    let slli_a1 = (1_u32 << 20) | (10 << 15) | (1 << 12) | (11 << 7) | 0x13;
    let lui_a2 = (0x1002f_u32 << 12) | (12 << 7) | 0x37;
    let addi_a2 = (0xec8_u32 << 20) | (12 << 15) | (12 << 7) | 0x13;
    let add_a1_a2 = (12_u32 << 20) | (11 << 15) | (11 << 7) | 0x33;
    let lhu_a0 = (11_u32 << 15) | (5 << 12) | (10 << 7) | 0x03;
    let symbol = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "shifted_table_read".to_owned(),
        address: 0x1000_0000,
        bytes: [slli_a1, lui_a2, addi_a2, add_a1_a2, lhu_a0, 0x0000_8067]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect(),
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };

    let trace = trace_binary_symbol(
        &symbol,
        &map(),
        &direct::StructuralRelocatedCalls::new(),
        &StructuralPointerContext::default(),
        None,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    let DraftReferenceEvent::Memory {
        access: MemoryAccess::Read,
        address,
        width: 16,
        region,
        ..
    } = &trace.reference_events[0]
    else {
        panic!("expected indexed halfword table read: {trace:#?}");
    };
    assert_eq!(region, "indexed RAM object");
    assert_eq!(
        address.memory_object_location_with_reads(&BTreeMap::new()),
        Some(MemoryObjectLocation {
            root: MemoryObjectRoot::Indexed {
                root: std::sync::Arc::new(MemoryObjectRoot::Absolute {
                    address: 0x1002_eec8,
                }),
                argument: 0,
                stride: 2,
            },
            offset: 0,
        })
    );
}

#[test]
fn indexed_absolute_mmio_prefers_a_typed_register_domain() {
    let slli_a2_a0_3 = (3_u32 << 20) | (10 << 15) | (1 << 12) | (12 << 7) | 0x13;
    let lui_a3 = (0x50002_u32 << 12) | (13 << 7) | 0x37;
    let add_a2_a2_a3 = (13_u32 << 20) | (12 << 15) | (12 << 7) | 0x33;
    let sw_a1_0_a2 = (11_u32 << 20) | (12 << 15) | (2 << 12) | 0x23;
    let symbol = artifact::ArtifactSymbolDefinition {
        member: Some("table.o".to_owned()),
        name: "write_table_entry".to_owned(),
        address: 0x1000_0000,
        bytes: [slli_a2_a0_3, lui_a3, add_a2_a2_a3, sw_a1_0_a2, 0x0000_8067]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect(),
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let typed_domain = indexed_map(0x5000_2000, 8, 6, "SYNTHETIC.TABLE_ENTRY");
    let trace = trace_binary_symbol(
        &symbol,
        &typed_domain,
        &direct::StructuralRelocatedCalls::new(),
        &StructuralPointerContext::default(),
        None,
    )
    .unwrap();

    let [
        DraftReferenceEvent::IndexedMmio {
            access: MemoryAccess::Write,
            width: 32,
            address,
            registers,
            guard: Some(guard),
            value: Some(value),
        },
    ] = trace.reference_events.as_slice()
    else {
        panic!("expected one indexed MMIO write: {trace:#?}");
    };
    assert_eq!(registers.len(), 6);
    assert_eq!(guard.selector, SymbolicValue::input(0));
    assert_eq!(guard.maximum, 5);
    assert_eq!(value, &SymbolicValue::input(1));
    assert_eq!(
        address.memory_object_location_with_reads(&BTreeMap::new()),
        Some(MemoryObjectLocation {
            root: MemoryObjectRoot::Indexed {
                root: std::sync::Arc::new(MemoryObjectRoot::Absolute {
                    address: 0x5000_2000,
                }),
                argument: 0,
                stride: 8,
            },
            offset: 0,
        })
    );

    let unproven_bank = indexed_map(0x5000_2000, 8, 1, "SYNTHETIC.TABLE_ENTRY");
    let fallback = trace_binary_symbol(
        &symbol,
        &unproven_bank,
        &direct::StructuralRelocatedCalls::new(),
        &StructuralPointerContext::default(),
        None,
    )
    .unwrap();
    let [DraftReferenceEvent::Memory { region, .. }] = fallback.reference_events.as_slice() else {
        panic!("one register cannot prove an indexed MMIO domain: {fallback:#?}");
    };
    assert_eq!(region, "indexed RAM object");
}

#[test]
fn indexed_absolute_mmio_read_retains_indexed_register_provenance() {
    let slli_a1_a0_3 = (3_u32 << 20) | (10 << 15) | (1 << 12) | (11 << 7) | 0x13;
    let lui_a2 = (0x50002_u32 << 12) | (12 << 7) | 0x37;
    let add_a1_a1_a2 = (12_u32 << 20) | (11 << 15) | (11 << 7) | 0x33;
    let lw_a0_0_a1 = (11_u32 << 15) | (2 << 12) | (10 << 7) | 0x03;
    let symbol = artifact::ArtifactSymbolDefinition {
        member: Some("table.o".to_owned()),
        name: "read_table_entry".to_owned(),
        address: 0x1000_0000,
        bytes: [slli_a1_a0_3, lui_a2, add_a1_a1_a2, lw_a0_0_a1, 0x0000_8067]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect(),
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let trace = trace_binary_symbol(
        &symbol,
        &indexed_map(0x5000_2000, 8, 6, "SYNTHETIC.TABLE_ENTRY"),
        &direct::StructuralRelocatedCalls::new(),
        &StructuralPointerContext::default(),
        None,
    )
    .unwrap();

    assert!(matches!(
        trace.reference_events.as_slice(),
        [DraftReferenceEvent::IndexedMmio {
            access: MemoryAccess::Read,
            width: 32,
            guard: Some(_),
            value: None,
            ..
        }]
    ));
    assert!(matches!(
        trace.return_value,
        SymbolicValue::IndexedRegisterImage { .. }
    ));
}

#[test]
fn unsigned_set_less_than_keeps_snez_dataflow_codegen_ready() {
    let symbol = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "snez_write".to_owned(),
        address: 0x2010_0000,
        bytes: vec![
            0x33, 0x35, 0xa0, 0x00, // sltu a0, zero, a0 (snez a0, a0)
            0xb7, 0x75, 0x10, 0x20, // lui a1, 0x20107
            0x23, 0xa8, 0xa5, 0x02, // sw a0, 0x30(a1)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let trace = trace_binary_symbol(
        &symbol,
        &map(),
        &direct::StructuralRelocatedCalls::new(),
        &StructuralPointerContext::default(),
        None,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert_eq!(
        trace.events[0].memory_value().as_deref(),
        Some("expr:LessThanUnsigned(const:0x00000000,arg0)")
    );
    let generated = generate_reference(&trace, "fixture.elf", "sha256", None, &[]).unwrap();
    assert!(
        generated
            .source
            .contains("u32::from((0x00000000_u32) < (args[0] & 0xffffffff_u32))"),
        "{}",
        generated.source
    );
}

#[test]
fn single_read_backedge_becomes_a_structural_mmio_poll() {
    let symbol = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "wait_ready".to_owned(),
        address: 0x2010_0000,
        bytes: vec![
            0xb7, 0x75, 0x10, 0x20, // lui a1, 0x20107
            0x03, 0xa5, 0x05, 0x03, // lw a0, 0x30(a1)
            0x13, 0x75, 0x15, 0x00, // andi a0, a0, 1
            0xe3, 0x0c, 0x05, 0xfe, // beq a0, zero, -8
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let symbols = BTreeMap::from([(symbol.address as u32, symbol.clone())]);
    let trace = resolve_reference_trace(
        &symbol,
        &symbols,
        &direct::StructuralRelocatedCalls::new(),
        &StructuralPointerContext::default(),
        None,
        &map(),
        &mut BTreeSet::new(),
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert!(
        !trace.is_exact(),
        "poll multiplicity is not a linear direct trace"
    );
    assert_eq!(trace.reference_events.len(), 1);
    assert_eq!(trace.located_reference_events.len(), 1);
    assert_eq!(trace.located_reference_events[0].site, 0x2010_0004);
    assert!(matches!(
        trace.located_reference_events[0].event,
        DraftReferenceEvent::PollMmio { .. }
    ));
    let DraftReferenceEvent::PollMmio {
        width,
        address,
        mask,
        expected,
        ..
    } = &trace.reference_events[0]
    else {
        panic!("expected a polling event");
    };
    assert_eq!(
        (*width, address.as_constant(), *mask, *expected),
        (32, Some(0x2010_7030), 1, 1)
    );

    let generated = generate_reference(&trace, "fixture.elf", "sha256", None, &[]).unwrap();
    assert!(generated.source.contains("// Poll until"));
    assert!(
        generated
            .source
            .contains("if value & 0x00000001_u32 == 0x00000001_u32")
    );
}

#[test]
fn structural_poll_does_not_expose_an_unmodeled_final_read_value() {
    let symbol = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "wait_and_reuse".to_owned(),
        address: 0x2010_0000,
        bytes: vec![
            0xb7, 0x75, 0x10, 0x20, // lui a1, 0x20107
            0x03, 0xa5, 0x05, 0x03, // lw a0, 0x30(a1)
            0x13, 0x75, 0x15, 0x00, // andi a0, a0, 1
            0xe3, 0x0c, 0x05, 0xfe, // beq a0, zero, -8
            0x23, 0xa8, 0xa5, 0x02, // sw a0, 0x30(a1)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let symbols = BTreeMap::from([(symbol.address as u32, symbol.clone())]);
    let trace = resolve_reference_trace(
        &symbol,
        &symbols,
        &direct::StructuralRelocatedCalls::new(),
        &StructuralPointerContext::default(),
        None,
        &map(),
        &mut BTreeSet::new(),
    )
    .unwrap();

    assert!(!trace.is_reference_eligible());
    assert!(
        trace
            .reference_failure_reasons()
            .iter()
            .any(|reason| reason.contains("unresolved MMIO write value")),
        "{trace:#?}"
    );
}

#[test]
fn straight_line_rmw_becomes_canonical_events() {
    let disassembly = r#"
20100000 <disable>:
20100000: lui a4, 0x20107
20100004: lw a5, 0x30(a4)
20100008: lui a3, 0x20000
2010000c: or a5, a5, a3
20100010: sw a5, 0x30(a4)
20100014: ret
"#;
    let trace = trace_disassembly("disable", disassembly, &map());
    assert!(trace.is_exact());
    assert_eq!(trace.events.len(), 2);
    assert_eq!(
        trace.events[1].memory_value().as_deref(),
        Some("rmw:read0[0x20107030]&0xdfffffff|0x20000000")
    );
}

#[test]
fn repeated_mmio_reads_have_distinct_symbolic_identities() {
    let vendor = r#"
20100000 <vendor>:
20100000: lui a4, 0x20107
20100004: lw a5, 0x30(a4)
20100008: lw a3, 0x30(a4)
2010000c: sw a5, 0x30(a4)
20100010: ret
"#;
    let rust = r#"
20100100 <rust>:
20100100: lui a4, 0x20107
20100104: lw a5, 0x30(a4)
20100108: lw a3, 0x30(a4)
2010010c: sw a3, 0x30(a4)
20100110: ret
"#;
    let vendor = trace_disassembly("vendor", vendor, &map());
    let rust = trace_disassembly("rust", rust, &map());
    assert!(vendor.is_exact());
    assert!(rust.is_exact());
    assert!(!traces_equal(&vendor, &rust));
    assert_eq!(
        vendor.events[2].memory_value().as_deref(),
        Some("rmw:read0[0x20107030]&0xffffffff|0x00000000")
    );
    assert_eq!(
        rust.events[2].memory_value().as_deref(),
        Some("rmw:read1[0x20107030]&0xffffffff|0x00000000")
    );
}

#[test]
fn control_flow_fails_closed() {
    let disassembly = r#"
20100000 <conditional>:
20100000: beqz a0, 0x20100008
20100004: ret
"#;
    let trace = trace_disassembly("conditional", disassembly, &map());
    assert!(!trace.is_exact());
    assert_eq!(trace.blockers.len(), 1);
}

#[test]
fn unmodeled_ram_keeps_mmio_trace_exact_but_blocks_reference_generation() {
    let disassembly = r#"
20100000 <stateful>:
20100000: lw a0, 0x0(a1)
20100004: sw a0, 0x4(a1)
20100008: ret
"#;
    let trace = trace_disassembly("stateful", disassembly, &map());

    assert!(trace.is_exact());
    assert!(!trace.is_reference_eligible());
    assert_eq!(trace.reference_blockers.len(), 2);
    assert!(trace.reference_blockers[0].contains("unmodeled-memory-load"));
    assert!(trace.reference_blockers[1].contains("unmodeled-memory-store"));
}

#[test]
fn caller_owned_argument_ram_is_preserved_as_a_symbolic_memory_contract() {
    let symbol = artifact::ArtifactSymbolDefinition {
        member: Some("caller_memory.o".to_owned()),
        name: "caller_memory".to_owned(),
        address: 0,
        bytes: vec![
            0x03, 0x26, 0x45, 0x00, // lw a2, 4(a0)
            0x23, 0x24, 0xc5, 0x00, // sw a2, 8(a0)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: false,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let trace = trace_binary_symbol(
        &symbol,
        &map(),
        &direct::StructuralRelocatedCalls::new(),
        &StructuralPointerContext::default(),
        None,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert_eq!(trace.reference_events.len(), 2);
    let DraftReferenceEvent::Memory {
        access: MemoryAccess::Read,
        address: read_address,
        region,
        ..
    } = &trace.reference_events[0]
    else {
        panic!("expected caller-owned RAM read");
    };
    assert_eq!(region, "caller-owned ABI argument RAM");
    assert!(read_address.canonical().contains("arg0"));
    let DraftReferenceEvent::Memory {
        access: MemoryAccess::Write,
        address: write_address,
        value: Some(SymbolicValue::MemoryImage { read_token: 0, .. }),
        ..
    } = &trace.reference_events[1]
    else {
        panic!("expected caller-owned RAM write of the first read");
    };
    assert!(write_address.canonical().contains("arg0"));

    let generated = generate_reference(
        &trace,
        "caller-memory.a",
        "synthetic",
        symbol.member.as_deref(),
        &[],
    )
    .unwrap();
    assert!(generated.source.contains("memory.read(32,"));
    assert!(generated.source.contains("memory.write(32,"));
    assert!(generated.source.contains(".wrapping_add(0x00000004_u32)"));
    assert!(generated.source.contains(".wrapping_add(0x00000008_u32)"));
    assert_generated_reference_compiles("caller-memory", &generated.source);
}

#[test]
fn pointer_loaded_from_caller_ram_preserves_distinct_pointee_provenance() {
    let symbol = artifact::ArtifactSymbolDefinition {
        member: Some("indirect_pointer.o".to_owned()),
        name: "indirect_pointer".to_owned(),
        address: 0,
        bytes: vec![
            0x83, 0x25, 0x05, 0x00, // lw a1, 0(a0)
            0x03, 0xa6, 0x05, 0x00, // lw a2, 0(a1)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: false,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let trace = trace_binary_symbol(
        &symbol,
        &map(),
        &direct::StructuralRelocatedCalls::new(),
        &StructuralPointerContext::default(),
        None,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert_eq!(
        trace
            .reference_events
            .iter()
            .filter(|event| matches!(event, DraftReferenceEvent::Memory { .. }))
            .count(),
        2
    );
    let second = trace.reference_events.get(1).expect("pointee load");
    let DraftReferenceEvent::Memory {
        address, region, ..
    } = second
    else {
        panic!("expected pointee memory load");
    };
    assert_eq!(region, "dereferenced known pointer RAM");
    assert!(address.canonical().starts_with("ram:read0"));
}

#[test]
fn floating_word_memory_preserves_context_address_and_value_provenance() {
    let flw = (4_u32 << 20) | (10 << 15) | (2 << 12) | (10 << 7) | 0x07;
    let fsw = (10_u32 << 20) | (10 << 15) | (2 << 12) | (8 << 7) | 0x27;
    let symbol = artifact::ArtifactSymbolDefinition {
        member: Some("floating-memory.o".to_owned()),
        name: "floating_memory".to_owned(),
        address: 0,
        bytes: [flw, fsw, 0x0000_8067]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect(),
        addresses_resolved: false,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };

    let trace = trace_binary_symbol(
        &symbol,
        &map(),
        &direct::StructuralRelocatedCalls::new(),
        &StructuralPointerContext::default(),
        None,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert_eq!(trace.reference_events.len(), 2);
    let DraftReferenceEvent::Memory {
        access: MemoryAccess::Read,
        address: read_address,
        width: 32,
        ..
    } = &trace.reference_events[0]
    else {
        panic!("expected floating context read");
    };
    assert!(read_address.canonical().contains("arg0"));
    let DraftReferenceEvent::Memory {
        access: MemoryAccess::Write,
        address: write_address,
        width: 32,
        value: Some(SymbolicValue::MemoryImage { read_token: 0, .. }),
        ..
    } = &trace.reference_events[1]
    else {
        panic!("expected floating context write with load provenance");
    };
    assert!(write_address.canonical().contains("arg0"));
    assert_eq!(
        trace
            .blockers
            .iter()
            .filter(|blocker| blocker.contains("class=floating-point"))
            .count(),
        0
    );
}

#[test]
fn floating_bit_move_preserves_integer_argument_into_context_store() {
    let fmv_w_x = (0x78_u32 << 25) | (11 << 15) | (10 << 7) | 0x53;
    let fsw = (10_u32 << 20) | (10 << 15) | (2 << 12) | (8 << 7) | 0x27;
    let symbol = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "floating_bit_move".to_owned(),
        address: 0,
        bytes: [fmv_w_x, fsw, 0x0000_8067]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect(),
        addresses_resolved: false,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };

    let trace = trace_binary_symbol(
        &symbol,
        &map(),
        &direct::StructuralRelocatedCalls::new(),
        &StructuralPointerContext::default(),
        None,
    )
    .unwrap();

    let DraftReferenceEvent::Memory {
        access: MemoryAccess::Write,
        address,
        value: Some(value),
        ..
    } = &trace.reference_events[0]
    else {
        panic!("expected floating store");
    };
    assert!(address.canonical().contains("arg0"));
    assert_eq!(value.canonical(), SymbolicValue::input(1).canonical());
    assert!(trace.is_reference_eligible(), "{trace:#?}");
}

#[test]
fn floating_comparison_of_exact_bits_preserves_later_integer_control_data() {
    let lui_float = (0x3f800_u32 << 12) | (11 << 7) | 0x37; // a1 = 1.0f32 bits
    let fmv_f10 = (0x78_u32 << 25) | (11 << 15) | (10 << 7) | 0x53;
    let fmv_f11 = (0x78_u32 << 25) | (11 << 15) | (11 << 7) | 0x53;
    let feq = (0x50_u32 << 25) | (11 << 20) | (10 << 15) | (2 << 12) | (10 << 7) | 0x53;
    let lui_mmio = (0x20107_u32 << 12) | (12 << 7) | 0x37;
    let store_result = (1_u32 << 25) | (10 << 20) | (12 << 15) | (2 << 12) | (16 << 7) | 0x23;
    let symbol = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "floating_compare".to_owned(),
        address: 0,
        bytes: [
            lui_float,
            fmv_f10,
            fmv_f11,
            feq,
            lui_mmio,
            store_result,
            0x0000_8067,
        ]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect(),
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };

    let trace = trace_binary_symbol(
        &symbol,
        &map(),
        &direct::StructuralRelocatedCalls::new(),
        &StructuralPointerContext::default(),
        None,
    )
    .unwrap();

    assert_eq!(trace.events.len(), 1);
    assert_eq!(
        trace.events[0].memory_value().as_deref(),
        Some("const:0x00000001")
    );
    assert_eq!(
        trace
            .blockers
            .iter()
            .filter(|blocker| blocker.contains("class=floating-point"))
            .count(),
        0
    );
    assert!(trace.is_reference_eligible(), "{trace:#?}");
}

#[test]
fn hi20_lo12_relocations_preserve_symbolic_data_reads_and_writes() {
    let symbol = artifact::ArtifactSymbolDefinition {
        member: Some("relocated_state.o".to_owned()),
        name: "relocated_state".to_owned(),
        address: 0,
        bytes: vec![
            0xb7, 0x07, 0x00, 0x00, // lui a5, %hi(state)
            0x03, 0xa5, 0x47, 0x00, // lw a0, %lo(state+4)(a5)
            0x23, 0xa4, 0xa7, 0x00, // sw a0, %lo(state+8)(a5)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: false,
        memory_regions: Default::default(),
        relocations: vec![
            artifact::SymbolRelocation {
                address: 0,
                kind: artifact::RelocationKind::Hi20,
                symbol: "state".to_owned(),
                addend: 0,
            },
            artifact::SymbolRelocation {
                address: 4,
                kind: artifact::RelocationKind::Lo12I,
                symbol: "state".to_owned(),
                addend: 4,
            },
            artifact::SymbolRelocation {
                address: 8,
                kind: artifact::RelocationKind::Lo12S,
                symbol: "state".to_owned(),
                addend: 8,
            },
        ],
    };
    let trace = trace_binary_symbol(
        &symbol,
        &map(),
        &direct::StructuralRelocatedCalls::new(),
        &StructuralPointerContext::default(),
        None,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert_eq!(trace.reference_events.len(), 2);
    let DraftReferenceEvent::Memory {
        access: MemoryAccess::Read,
        address:
            SymbolicValue::SymbolAddress {
                hi_addend: 0,
                lo_addend: Some(4),
                ..
            },
        ..
    } = &trace.reference_events[0]
    else {
        panic!("expected relocated symbolic read");
    };
    let DraftReferenceEvent::Memory {
        access: MemoryAccess::Write,
        address:
            SymbolicValue::SymbolAddress {
                hi_addend: 0,
                lo_addend: Some(8),
                ..
            },
        value: Some(SymbolicValue::MemoryImage { read_token: 0, .. }),
        ..
    } = &trace.reference_events[1]
    else {
        panic!("expected relocated symbolic write");
    };

    let generated = generate_reference(
        &trace,
        "relocated-state.a",
        "synthetic",
        symbol.member.as_deref(),
        &[],
    )
    .unwrap();
    assert!(
        generated
            .source
            .contains("memory.symbol_address(Some(\"relocated_state.o\"), \"state\")")
    );
    assert!(generated.source.contains("riscv_hi20_lo12_address"));
    assert_generated_reference_compiles("relocated-state", &generated.source);
}

#[test]
fn projected_origin_relocation_accepts_final_linked_low_immediate() {
    let symbol = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "linked_read".to_owned(),
        address: 0x1000_0000,
        bytes: vec![
            0xb7, 0x37, 0x00, 0x10, // lui a5, 0x10003
            0x03, 0xa5, 0x07, 0x84, // lw a0, -1984(a5)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: vec![artifact::MemoryRegion {
            start: 0x1000_2840,
            length: 4,
            writable: true,
            name: "dram".to_owned(),
        }]
        .into(),
        relocations: Vec::new(),
    };
    let mut context = StructuralPointerContext::default();
    context.projected_relocations.insert(
        StructuralCallSite::new(&symbol, 0x1000_0004),
        vec![StructuralProjectedRelocation {
            origin_member: Some("origin.o".to_owned()),
            origin_symbol: "linked_read".to_owned(),
            origin_offsets: vec![4],
            kind: artifact::RelocationKind::Lo12I,
            symbol: ".LANCHOR0".to_owned(),
            addend: 0,
            correspondence: "same-shape",
        }],
    );

    let trace = trace_binary_symbol(
        &symbol,
        &map(),
        &direct::StructuralRelocatedCalls::new(),
        &context,
        None,
    )
    .unwrap();

    assert!(
        !trace
            .reference_blockers
            .iter()
            .any(|blocker| blocker.contains("malformed-projected-data-relocation")),
        "{trace:#?}"
    );
    assert!(matches!(
        trace.reference_events.first(),
        Some(DraftReferenceEvent::Memory {
            access: MemoryAccess::Read,
            address: SymbolicValue::Constant(0x1000_2840),
            ..
        })
    ));
}

#[test]
fn projected_relaxed_unknown_pointer_cell_preserves_pointee_provenance() {
    let symbol = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "linked_pointer_write".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x83, 0x27, 0x00, 0x00, // lw a5, 0(zero), relaxed our_instances_ptr
            0x23, 0x82, 0xa7, 0x00, // sb a0, 4(a5)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let mut context = StructuralPointerContext::default();
    context.projected_relocations.insert(
        StructuralCallSite::new(&symbol, 0x1000),
        vec![StructuralProjectedRelocation {
            origin_member: Some("lmac.o".to_owned()),
            origin_symbol: "linked_pointer_write".to_owned(),
            origin_offsets: vec![12, 16],
            kind: artifact::RelocationKind::Lo12I,
            symbol: "our_instances_ptr".to_owned(),
            addend: 0,
            correspondence: "linker-relaxation",
        }],
    );

    let trace = trace_binary_symbol(
        &symbol,
        &map(),
        &direct::StructuralRelocatedCalls::new(),
        &context,
        None,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert_eq!(trace.reference_events.len(), 2);
    assert!(matches!(
        &trace.reference_events[0],
        DraftReferenceEvent::Memory {
            access: MemoryAccess::Read,
            address: SymbolicValue::SymbolAddress { symbol, .. },
            ..
        } if symbol == "our_instances_ptr"
    ));
    let DraftReferenceEvent::Memory {
        access: MemoryAccess::Write,
        address,
        region,
        value: Some(SymbolicValue::Input { index: 0 }),
        ..
    } = &trace.reference_events[1]
    else {
        panic!("expected pointee write: {:#?}", trace.reference_events);
    };
    assert_eq!(region, "dereferenced known pointer RAM");
    let sources = BTreeMap::from([(
        0,
        MemoryObjectLocation {
            root: MemoryObjectRoot::RelocatedSymbol {
                member: Some("lmac.o".to_owned()),
                symbol: "our_instances_ptr".to_owned(),
            },
            offset: 0,
        },
    )]);
    assert!(matches!(
        address.memory_object_location_with_reads(&sources),
        Some(MemoryObjectLocation {
            root: MemoryObjectRoot::Dereferenced {
                pointer,
                pointer_offset: 0,
            },
            offset: 4,
        }) if matches!(pointer.as_ref(), MemoryObjectRoot::RelocatedSymbol { symbol, .. } if symbol == "our_instances_ptr")
    ));
}

#[test]
fn projected_relaxed_load_does_not_require_the_deleted_hi20_to_survive_projection() {
    let symbol = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "linked_pointer_read".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x83, 0x27, 0x00, 0x00, // lw a5, 0(zero), relaxed state pointer cell
            0x03, 0xa5, 0x47, 0x00, // lw a0, 4(a5)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    };
    let mut context = StructuralPointerContext::default();
    context.projected_relocations.insert(
        StructuralCallSite::new(&symbol, 0x1000),
        vec![StructuralProjectedRelocation {
            origin_member: Some("pp.o".to_owned()),
            origin_symbol: "linked_pointer_read".to_owned(),
            // The linker removed the origin HI20 instruction.  Only the
            // relocated load has a corresponding linked instruction.
            origin_offsets: vec![0x28],
            kind: artifact::RelocationKind::Lo12I,
            symbol: "g_intr_lock_mux".to_owned(),
            addend: 0,
            correspondence: "linker-relaxation",
        }],
    );

    let trace = trace_binary_symbol(
        &symbol,
        &map(),
        &direct::StructuralRelocatedCalls::new(),
        &context,
        None,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert!(matches!(
        trace.reference_events.as_slice(),
        [
            DraftReferenceEvent::Memory {
                access: MemoryAccess::Read,
                address: SymbolicValue::SymbolAddress { symbol, .. },
                ..
            },
            DraftReferenceEvent::Memory {
                access: MemoryAccess::Read,
                ..
            }
        ] if symbol == "g_intr_lock_mux"
    ));
}

#[test]
fn relocated_global_pointer_load_preserves_pointee_memory_provenance() {
    let symbol = artifact::ArtifactSymbolDefinition {
        member: Some("global_pointer.o".to_owned()),
        name: "read_global_pointee".to_owned(),
        address: 0,
        bytes: vec![
            0xb7, 0x07, 0x00, 0x00, // lui a5, %hi(g_state)
            0x83, 0xa7, 0x07, 0x00, // lw a5, %lo(g_state)(a5)
            0x03, 0xa5, 0xc7, 0x01, // lw a0, 0x1c(a5)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: false,
        memory_regions: Default::default(),
        relocations: vec![
            artifact::SymbolRelocation {
                address: 0,
                kind: artifact::RelocationKind::Hi20,
                symbol: "g_state".to_owned(),
                addend: 0,
            },
            artifact::SymbolRelocation {
                address: 4,
                kind: artifact::RelocationKind::Lo12I,
                symbol: "g_state".to_owned(),
                addend: 0,
            },
        ],
    };
    let trace = trace_binary_symbol(
        &symbol,
        &map(),
        &direct::StructuralRelocatedCalls::new(),
        &StructuralPointerContext::default(),
        None,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert_eq!(trace.reference_events.len(), 2);
    let DraftReferenceEvent::Memory {
        access: MemoryAccess::Read,
        address,
        region,
        ..
    } = &trace.reference_events[1]
    else {
        panic!("expected pointee read");
    };
    assert_eq!(region, "dereferenced known pointer RAM");
    let sources = BTreeMap::from([(
        0,
        MemoryObjectLocation {
            root: MemoryObjectRoot::RelocatedSymbol {
                member: Some("global_pointer.o".to_owned()),
                symbol: "g_state".to_owned(),
            },
            offset: 0,
        },
    )]);
    assert!(matches!(
        address.memory_object_location_with_reads(&sources),
        Some(MemoryObjectLocation {
            root: MemoryObjectRoot::Dereferenced {
                pointer,
                pointer_offset: 0,
            },
            offset: 0x1c,
        }) if matches!(pointer.as_ref(), MemoryObjectRoot::RelocatedSymbol { symbol, .. } if symbol == "g_state")
    ));
}

#[test]
fn absolute_ram_pointer_load_preserves_pointee_memory_provenance() {
    let symbol = artifact::ArtifactSymbolDefinition {
        member: Some("absolute_pointer.o".to_owned()),
        name: "read_absolute_pointee".to_owned(),
        address: 0,
        bytes: vec![
            0xb7, 0x47, 0x10, 0x20, // lui a5, 0x20104
            0x83, 0xa7, 0x07, 0x00, // lw a5, 0(a5)
            0x03, 0xa5, 0x87, 0x02, // lw a0, 0x28(a5)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: vec![artifact::MemoryRegion {
            start: 0x2010_4000,
            length: 4,
            writable: true,
            name: "dram".to_owned(),
        }]
        .into(),
        relocations: Vec::new(),
    };
    let trace = trace_binary_symbol(
        &symbol,
        &MmioMap {
            registers: Vec::new(),
            regions: Vec::new(),
        },
        &direct::StructuralRelocatedCalls::new(),
        &StructuralPointerContext::default(),
        None,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert_eq!(trace.reference_events.len(), 2);
    let DraftReferenceEvent::Memory {
        address, region, ..
    } = &trace.reference_events[1]
    else {
        panic!("expected pointee read");
    };
    assert_eq!(region, "dereferenced known pointer RAM");
    let sources = BTreeMap::from([(
        0,
        MemoryObjectLocation {
            root: MemoryObjectRoot::Absolute {
                address: 0x2010_4000,
            },
            offset: 0,
        },
    )]);
    assert_eq!(
        address.memory_object_location_with_reads(&sources),
        Some(MemoryObjectLocation {
            root: MemoryObjectRoot::Dereferenced {
                pointer: std::sync::Arc::new(MemoryObjectRoot::Absolute {
                    address: 0x2010_4000,
                }),
                pointer_offset: 0,
            },
            offset: 0x28,
        })
    );
}

#[test]
fn mismatched_hi20_lo12_symbols_fail_closed() {
    let symbol = artifact::ArtifactSymbolDefinition {
        member: Some("mismatched_relocation.o".to_owned()),
        name: "mismatched_relocation".to_owned(),
        address: 0,
        bytes: vec![
            0xb7, 0x07, 0x00, 0x00, // lui a5, %hi(first)
            0x03, 0xa5, 0x07, 0x00, // lw a0, %lo(second)(a5)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: false,
        memory_regions: Default::default(),
        relocations: vec![
            artifact::SymbolRelocation {
                address: 0,
                kind: artifact::RelocationKind::Hi20,
                symbol: "first".to_owned(),
                addend: 0,
            },
            artifact::SymbolRelocation {
                address: 4,
                kind: artifact::RelocationKind::Lo12I,
                symbol: "second".to_owned(),
                addend: 0,
            },
        ],
    };
    let trace = trace_binary_symbol(
        &symbol,
        &map(),
        &direct::StructuralRelocatedCalls::new(),
        &StructuralPointerContext::default(),
        None,
    )
    .unwrap();

    assert!(!trace.is_reference_eligible());
    assert!(trace.reference_events.is_empty());
    assert!(
        trace
            .reference_blockers
            .iter()
            .any(|blocker| blocker.contains("malformed-data-relocation"))
    );
}
