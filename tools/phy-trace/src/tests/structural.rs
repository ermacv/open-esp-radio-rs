use super::*;

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
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let trace = trace_binary_symbol(
        &symbol,
        &map(),
        &BTreeMap::new(),
        &StructuralPointerContext::default(),
        None,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert_eq!(
        trace.events[0].memory_value().as_deref(),
        Some(
            "expr:LessThanUnsigned(const:0x00000000,bits:0=arg0.0,1=arg0.1,2=arg0.2,3=arg0.3,4=arg0.4,5=arg0.5,6=arg0.6,7=arg0.7,8=arg0.8,9=arg0.9,10=arg0.10,11=arg0.11,12=arg0.12,13=arg0.13,14=arg0.14,15=arg0.15,16=arg0.16,17=arg0.17,18=arg0.18,19=arg0.19,20=arg0.20,21=arg0.21,22=arg0.22,23=arg0.23,24=arg0.24,25=arg0.25,26=arg0.26,27=arg0.27,28=arg0.28,29=arg0.29,30=arg0.30,31=arg0.31)"
        )
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
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let symbols = BTreeMap::from([(symbol.address as u32, symbol.clone())]);
    let trace = resolve_reference_trace(
        &symbol,
        &symbols,
        &BTreeMap::new(),
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
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let symbols = BTreeMap::from([(symbol.address as u32, symbol.clone())]);
    let trace = resolve_reference_trace(
        &symbol,
        &symbols,
        &BTreeMap::new(),
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
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let trace = trace_binary_symbol(
        &symbol,
        &map(),
        &BTreeMap::new(),
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
fn pointer_loaded_from_caller_ram_does_not_inherit_argument_provenance() {
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
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let trace = trace_binary_symbol(
        &symbol,
        &map(),
        &BTreeMap::new(),
        &StructuralPointerContext::default(),
        None,
    )
    .unwrap();

    assert!(!trace.is_reference_eligible());
    assert_eq!(
        trace
            .reference_events
            .iter()
            .filter(|event| matches!(event, DraftReferenceEvent::Memory { .. }))
            .count(),
        1
    );
    assert!(
        trace
            .reference_blockers
            .iter()
            .any(|blocker| blocker.contains("unmodeled-memory-load"))
    );
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
        memory_regions: Vec::new(),
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
        &BTreeMap::new(),
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
        memory_regions: Vec::new(),
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
        &BTreeMap::new(),
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

#[test]
fn call_summary_substitutes_arguments_and_remaps_read_tokens() {
    let prefix = vec![DraftReferenceEvent::Observable(ObservableEvent::Memory {
        access: MemoryAccess::Read,
        width: 32,
        address: 0x2010_7030,
        register: "AGC.FIRST".to_owned(),
        value: None,
    })];
    let callee = FunctionAnalysis {
        symbol: "child".to_owned(),
        events: Vec::new(),
        reference_events: vec![
            DraftReferenceEvent::Observable(ObservableEvent::Memory {
                access: MemoryAccess::Read,
                width: 32,
                address: 0x2010_7034,
                register: "AGC.SECOND".to_owned(),
                value: None,
            }),
            DraftReferenceEvent::Observable(ObservableEvent::Memory {
                access: MemoryAccess::Write,
                width: 32,
                address: 0x2010_7038,
                register: "AGC.THIRD".to_owned(),
                value: Some(SymbolicValue::input(0)),
            }),
        ],
        reference_dependencies: Vec::new(),
        blockers: Vec::new(),
        reference_blockers: Vec::new(),
        return_value: SymbolicValue::RegisterImage {
            read_token: 0,
            address: 0x2010_7034,
            and_mask: u32::MAX,
            or_mask: 0,
        },
        reference_flow: None,
        unresolved_branch: None,
    };
    let arguments: Rv32CallArguments = core::array::from_fn(|index| {
        if index == 0 {
            SymbolicValue::input(1)
        } else {
            SymbolicValue::Unknown
        }
    });

    let (events, return_value) =
        inline_reference_summary(&prefix, &callee, &arguments, None).unwrap();

    assert_eq!(events.len(), 3);
    let DraftReferenceEvent::Observable(ObservableEvent::Memory {
        value: Some(write_value),
        ..
    }) = &events[2]
    else {
        panic!("expected substituted write");
    };
    assert!(write_value.canonical().contains("arg1"));
    assert_eq!(
        return_value.canonical(),
        "rmw:read1[0x20107034]&0xffffffff|0x00000000"
    );
}

#[test]
fn call_summary_substitutes_indexed_mmio_and_preserves_read_identity() {
    let prefix = vec![DraftReferenceEvent::Observable(ObservableEvent::Memory {
        access: MemoryAccess::Read,
        width: 32,
        address: 0x2010_7030,
        register: "AGC.FIRST".to_owned(),
        value: None,
    })];
    let callee = FunctionAnalysis {
        symbol: "indexed_child".to_owned(),
        events: Vec::new(),
        reference_events: vec![DraftReferenceEvent::IndexedMmio {
            access: MemoryAccess::Read,
            width: 32,
            address: SymbolicValue::input(0)
                .shift_left(2)
                .add_constant(0x2010_4000),
            registers: vec![
                IndexedMmioRegister {
                    address: 0x2010_4000,
                    name: "WIFI.QUEUE0".to_owned(),
                },
                IndexedMmioRegister {
                    address: 0x2010_4004,
                    name: "WIFI.QUEUE1".to_owned(),
                },
            ],
            guard: Some(IndexedMmioGuard {
                selector: SymbolicValue::input(0),
                maximum: 1,
            }),
            value: None,
        }],
        reference_dependencies: Vec::new(),
        blockers: Vec::new(),
        reference_blockers: Vec::new(),
        return_value: SymbolicValue::IndexedRegisterImage {
            read_token: 0,
            and_mask: u32::MAX,
            or_mask: 0,
        },
        reference_flow: None,
        unresolved_branch: None,
    };
    let arguments: Rv32CallArguments = core::array::from_fn(|index| {
        if index == 0 {
            SymbolicValue::input(1)
        } else {
            SymbolicValue::Unknown
        }
    });

    let (events, return_value) =
        inline_reference_summary(&prefix, &callee, &arguments, None).unwrap();
    let DraftReferenceEvent::IndexedMmio {
        address,
        guard: Some(guard),
        ..
    } = &events[1]
    else {
        panic!("expected indexed MMIO read");
    };
    assert!(address.canonical().contains("arg1"));
    assert!(guard.selector.canonical().contains("arg1"));
    assert_eq!(
        return_value.canonical(),
        "indexed-rmw:read1&0xffffffff|0x00000000"
    );
}

#[test]
fn call_summary_substitutes_caller_owned_memory_addresses() {
    let callee = FunctionAnalysis {
        symbol: "memory_child".to_owned(),
        events: Vec::new(),
        reference_events: vec![DraftReferenceEvent::Memory {
            access: MemoryAccess::Read,
            width: 32,
            address: SymbolicValue::input(0).add_constant(4),
            region: "caller-owned ABI argument RAM".to_owned(),
            value: None,
        }],
        reference_dependencies: Vec::new(),
        blockers: Vec::new(),
        reference_blockers: Vec::new(),
        return_value: SymbolicValue::MemoryImage {
            read_token: 0,
            and_mask: u32::MAX,
            or_mask: 0,
        },
        reference_flow: None,
        unresolved_branch: None,
    };
    let arguments: Rv32CallArguments = core::array::from_fn(|index| {
        if index == 0 {
            SymbolicValue::input(2).add_constant(8)
        } else {
            SymbolicValue::Unknown
        }
    });

    let (events, return_value) = inline_reference_summary(&[], &callee, &arguments, None).unwrap();
    let [
        DraftReferenceEvent::Memory {
            access: MemoryAccess::Read,
            address,
            ..
        },
    ] = events.as_slice()
    else {
        panic!("expected one substituted caller-memory read");
    };
    assert!(address.canonical().contains("arg2"));
    assert_eq!(
        return_value,
        SymbolicValue::MemoryImage {
            read_token: 0,
            and_mask: u32::MAX,
            or_mask: 0,
        }
    );
}

#[test]
fn private_stack_round_trips_symbolic_values_and_sign_extension() {
    let mut stack = SymbolicStack::default();
    stack.store(-8, 32, &SymbolicValue::input(2));
    assert_eq!(
        stack.load(-8, 32, false).unwrap().canonical(),
        SymbolicValue::input(2).canonical()
    );

    stack.store(-1, 8, &SymbolicValue::Constant(0x80));
    assert_eq!(
        stack.load(-1, 8, true).unwrap(),
        SymbolicValue::Constant(0xffff_ff80)
    );
    assert!(stack.load(-12, 32, false).is_none());
}

#[test]
fn unused_callee_write_to_caller_private_stack_is_internal_scratch() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "private_stack_parent".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x13, 0x05, 0xc1, 0x00, // addi a0, sp, 12
            0xef, 0x00, 0xd0, 0x7f, // jal ra, 0x2000
            0x13, 0x05, 0x00, 0x00, // li a0, 0
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let child = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "private_stack_writer".to_owned(),
        address: 0x2000,
        bytes: vec![
            0x93, 0x07, 0x20, 0x01, // li a5, 0x12
            0x23, 0x00, 0xf5, 0x00, // sb a5, 0(a0)
            0x13, 0x05, 0x00, 0x00, // li a0, 0
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let symbols = BTreeMap::from([(0x2000, child)]);
    let mut visiting = BTreeSet::from([0x1000]);

    let trace = resolve_reference_trace(
        &parent,
        &symbols,
        &BTreeMap::new(),
        &StructuralPointerContext::default(),
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert!(trace.reference_events.is_empty(), "{trace:#?}");
    assert_eq!(trace.reference_dependencies, ["private_stack_writer"]);
    assert_eq!(trace.return_value, SymbolicValue::Constant(0));
}

#[test]
fn consumed_callee_write_to_caller_private_stack_is_composed() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "private_stack_reader".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x13, 0x05, 0xc1, 0x00, // addi a0, sp, 12
            0xef, 0x00, 0xd0, 0x7f, // jal ra, 0x2000
            0x03, 0x45, 0xc1, 0x00, // lbu a0, 12(sp)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let child = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "private_stack_writer".to_owned(),
        address: 0x2000,
        bytes: vec![
            0x93, 0x07, 0x20, 0x01, // li a5, 0x12
            0x23, 0x00, 0xf5, 0x00, // sb a5, 0(a0)
            0x13, 0x05, 0x00, 0x00, // li a0, 0
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let symbols = BTreeMap::from([(0x2000, child)]);
    let mut visiting = BTreeSet::from([0x1000]);

    let trace = resolve_reference_trace(
        &parent,
        &symbols,
        &BTreeMap::new(),
        &StructuralPointerContext::default(),
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert!(trace.reference_events.is_empty(), "{trace:#?}");
    assert_eq!(trace.return_value, SymbolicValue::Constant(0x12));
}

#[test]
fn callee_read_from_initialized_caller_private_stack_is_composed() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "private_stack_input_parent".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x93, 0x07, 0x20, 0x01, // li a5, 0x12
            0x23, 0x06, 0xf1, 0x00, // sb a5, 12(sp)
            0x13, 0x05, 0xc1, 0x00, // addi a0, sp, 12
            0xef, 0x00, 0x50, 0x7f, // jal ra, 0x2000
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let child = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "private_stack_reader".to_owned(),
        address: 0x2000,
        bytes: vec![
            0x03, 0x45, 0x05, 0x00, // lbu a0, 0(a0)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let symbols = BTreeMap::from([(0x2000, child)]);
    let mut visiting = BTreeSet::from([0x1000]);

    let trace = resolve_reference_trace(
        &parent,
        &symbols,
        &BTreeMap::new(),
        &StructuralPointerContext::default(),
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert!(trace.reference_events.is_empty(), "{trace:#?}");
    assert_eq!(trace.return_value, SymbolicValue::Constant(0x12));
}

#[test]
fn entry_stack_argument_is_a_distinct_rv32_abi_input() {
    let symbol = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "stack_argument_reader".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x03, 0x45, 0x01, 0x00, // lbu a0, 0(sp): ninth ABI argument
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };

    let trace = trace_binary_symbol(
        &symbol,
        &map(),
        &BTreeMap::new(),
        &StructuralPointerContext::default(),
        None,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert_eq!(trace.return_value, SymbolicValue::input(8).and(0xff));
    let generated = generate_reference(&trace, "fixture.elf", "sha256", None, &[]).unwrap();
    assert!(
        generated
            .source
            .contains("pub struct Rv32ReferenceArguments"),
        "{}",
        generated.source
    );
    assert!(
        generated
            .source
            .contains("ReferenceOutcome { exit_a0: Some(args[8] & 0x000000ff_u32) }"),
        "{}",
        generated.source
    );
}

#[test]
fn outgoing_stack_argument_is_substituted_into_a_direct_callee() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "stack_argument_parent".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x93, 0x07, 0x20, 0x01, // li a5, 0x12
            0x23, 0x00, 0xf1, 0x00, // sb a5, 0(sp): ninth outgoing argument
            0xef, 0x00, 0xd0, 0x7f, // jal ra, 0x2004
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let child = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "stack_argument_child".to_owned(),
        address: 0x2004,
        bytes: vec![
            0x03, 0x45, 0x01, 0x00, // lbu a0, 0(sp)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let symbols = BTreeMap::from([(0x2004, child)]);
    let mut visiting = BTreeSet::from([0x1000]);

    let trace = resolve_reference_trace(
        &parent,
        &symbols,
        &BTreeMap::new(),
        &StructuralPointerContext::default(),
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert_eq!(trace.reference_dependencies, ["stack_argument_child"]);
    assert_eq!(trace.return_value, SymbolicValue::Constant(0x12));
}

#[test]
fn incoming_stack_argument_survives_an_unrelated_callee_stack_write() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "stack_argument_after_output_call".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x13, 0x05, 0xc1, 0x00, // addi a0, sp, 12
            0xef, 0x00, 0xd0, 0x7f, // jal ra, 0x2000
            0x03, 0x45, 0x01, 0x00, // lbu a0, 0(sp): ninth incoming argument
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let child = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "unrelated_stack_writer".to_owned(),
        address: 0x2000,
        bytes: vec![
            0x93, 0x07, 0x20, 0x01, // li a5, 0x12
            0x23, 0x00, 0xf5, 0x00, // sb a5, 0(a0)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let symbols = BTreeMap::from([(0x2000, child)]);
    let mut visiting = BTreeSet::from([0x1000]);

    let trace = resolve_reference_trace(
        &parent,
        &symbols,
        &BTreeMap::new(),
        &StructuralPointerContext::default(),
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert_eq!(trace.reference_dependencies, ["unrelated_stack_writer"]);
    assert_eq!(trace.return_value, SymbolicValue::input(8).and(0xff));
}

#[test]
fn pointer_reloaded_after_a_call_recovers_caller_memory_provenance() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "deferred_pointer_parent".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x23, 0x24, 0xb1, 0x00, // sw a1, 8(sp)
            0x13, 0x05, 0xc1, 0x00, // addi a0, sp, 12
            0xef, 0x00, 0x90, 0x7f, // jal ra, 0x2000
            0x03, 0x26, 0x81, 0x00, // lw a2, 8(sp)
            0x13, 0x07, 0x30, 0x12, // li a4, 0x123
            0x23, 0x10, 0xe6, 0x00, // sh a4, 0(a2)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let child = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "unrelated_stack_output".to_owned(),
        address: 0x2000,
        bytes: vec![
            0x93, 0x07, 0x20, 0x01, // li a5, 0x12
            0x23, 0x00, 0xf5, 0x00, // sb a5, 0(a0)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let symbols = BTreeMap::from([(0x2000, child)]);
    let mut visiting = BTreeSet::from([0x1000]);

    let trace = resolve_reference_trace(
        &parent,
        &symbols,
        &BTreeMap::new(),
        &StructuralPointerContext::default(),
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert_eq!(trace.reference_dependencies, ["unrelated_stack_output"]);
    assert_eq!(trace.reference_events.len(), 1, "{trace:#?}");
    let DraftReferenceEvent::Memory {
        access,
        width,
        address,
        value: Some(value),
        ..
    } = &trace.reference_events[0]
    else {
        panic!("expected a resolved caller-memory write: {trace:#?}");
    };
    assert_eq!((*access, *width), (MemoryAccess::Write, 16));
    assert_eq!(*address, SymbolicValue::input(1));
    assert_eq!(*value, SymbolicValue::Constant(0x123));
}

#[test]
fn deferred_pointer_without_caller_memory_provenance_fails_closed() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "invalid_deferred_pointer_parent".to_owned(),
        address: 0x1000,
        bytes: vec![
            0xb7, 0x75, 0x10, 0x20, // lui a1, 0x20107
            0x83, 0xa5, 0x05, 0x03, // lw a1, 0x30(a1): untrusted MMIO value
            0x23, 0x24, 0xb1, 0x00, // sw a1, 8(sp)
            0x13, 0x05, 0xc1, 0x00, // addi a0, sp, 12
            0xef, 0x00, 0x10, 0x7f, // jal ra, 0x2000
            0x03, 0x26, 0x81, 0x00, // lw a2, 8(sp)
            0x13, 0x07, 0x30, 0x12, // li a4, 0x123
            0x23, 0x10, 0xe6, 0x00, // sh a4, 0(a2)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let child = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "unrelated_stack_output".to_owned(),
        address: 0x2000,
        bytes: vec![
            0x93, 0x07, 0x20, 0x01, // li a5, 0x12
            0x23, 0x00, 0xf5, 0x00, // sb a5, 0(a0)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let symbols = BTreeMap::from([(0x2000, child)]);
    let mut visiting = BTreeSet::from([0x1000]);

    let trace = resolve_reference_trace(
        &parent,
        &symbols,
        &BTreeMap::new(),
        &StructuralPointerContext::default(),
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(!trace.is_reference_eligible(), "{trace:#?}");
    assert!(
        trace
            .reference_failure_reasons()
            .iter()
            .any(|reason| reason.contains("did not resolve to affine caller-owned RAM")),
        "{trace:#?}"
    );
}

#[test]
fn call_results_are_substituted_into_parent_dataflow() {
    let value = SymbolicValue::CallResult(7).and(0xff).shift_left(8).or(3);
    let call_results = BTreeMap::from([(7, SymbolicValue::Constant(0x1234))]);
    let private_stack_reads = BTreeMap::new();

    let rewritten = value
        .rewrite_call_context(&[], &[], &[], &call_results, &private_stack_reads)
        .unwrap();

    assert_eq!(rewritten, SymbolicValue::Constant(0x3403));
}

#[test]
fn returning_direct_call_is_flattened_from_binary_symbols() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "parent".to_owned(),
        address: 0x1000,
        bytes: vec![
            0xef, 0x10, 0x00, 0x00, // jal ra, 0x2000
            0x13, 0x75, 0xf5, 0x0f, // andi a0, a0, 255
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let child = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "child".to_owned(),
        address: 0x2000,
        bytes: vec![
            0x13, 0x05, 0x05, 0x00, // mv a0, a0
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let symbols = BTreeMap::from([(0x2000, child)]);
    let mut visiting = BTreeSet::from([0x1000]);

    let trace = resolve_reference_trace(
        &parent,
        &symbols,
        &BTreeMap::new(),
        &StructuralPointerContext::default(),
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert_eq!(trace.reference_dependencies, ["child"]);
    assert_eq!(trace.return_value, SymbolicValue::input(0).and(0xff));
}

#[test]
fn direct_call_to_symbolic_cfg_callee_is_scoped_and_composed() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "branch_parent".to_owned(),
        address: 0x1000,
        bytes: vec![
            0xef, 0x10, 0x00, 0x00, // jal ra, 0x2000
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let child = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "branch_child".to_owned(),
        address: 0x2000,
        bytes: vec![
            0x63, 0x06, 0x05, 0x00, // beq a0, zero, 0x200c
            0x13, 0x05, 0x10, 0x00, // li a0, 1
            0x67, 0x80, 0x00, 0x00, // ret
            0x13, 0x05, 0x20, 0x00, // li a0, 2
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let symbols = BTreeMap::from([(0x2000, child)]);
    let mut visiting = BTreeSet::from([0x1000]);

    let trace = resolve_reference_trace(
        &parent,
        &symbols,
        &BTreeMap::new(),
        &StructuralPointerContext::default(),
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert!(matches!(
        trace.reference_events.as_slice(),
        [DraftReferenceEvent::ComposedCall {
            token: 0,
            symbol,
            result_modeled: true,
            ..
        }] if symbol == "branch_child"
    ));
    let generated = generate_reference(&trace, "oracle.elf", "abc123", None, &[]).unwrap();
    assert!(generated.source.contains("let call_result0 = {"));
    assert!(
        generated
            .source
            .contains("// Composed direct call: branch_child.")
    );
    assert!(generated.source.contains("if (call0_arg0"));
    assert!(
        generated
            .source
            .contains("ReferenceOutcome { exit_a0: Some(call_result0 & 0xffffffff_u32) }")
    );
    assert_generated_reference_compiles("scoped-callee", &generated.source);
}

#[test]
fn nested_call_graph_keeps_each_composed_token_scope_local() {
    let grandparent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "grandparent".to_owned(),
        address: 0x0800,
        bytes: vec![
            0x17, 0x03, 0x00, 0x00, // auipc t1, 0
            0xe7, 0x00, 0x03, 0x00, // jalr ra, 0(t1)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "branch_parent".to_owned(),
        address: 0x1000,
        bytes: vec![
            0xef, 0x10, 0x00, 0x00, // jal ra, 0x2000
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let child = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "branch_child".to_owned(),
        address: 0x2000,
        bytes: vec![
            0x63, 0x06, 0x05, 0x00, // beq a0, zero, 0x200c
            0x13, 0x05, 0x10, 0x00, // li a0, 1
            0x67, 0x80, 0x00, 0x00, // ret
            0x13, 0x05, 0x20, 0x00, // li a0, 2
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let symbols = BTreeMap::from([(0x1000, parent), (0x2000, child)]);
    let relocations = BTreeMap::from([(
        StructuralCallSite::new(&grandparent, 0x0800),
        ("branch_parent".to_owned(), Some(0x1000)),
    )]);
    let mut visiting = BTreeSet::from([0x0800]);

    let trace = resolve_reference_trace(
        &grandparent,
        &symbols,
        &relocations,
        &StructuralPointerContext::default(),
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert_eq!(
        trace.reference_dependencies,
        ["branch_parent", "branch_child"]
    );
    let generated = generate_reference(&trace, "oracle.elf", "abc123", None, &[]).unwrap();
    assert_eq!(generated.source.matches("let call_result0 = {").count(), 2);
    assert_generated_reference_compiles("nested-call-scopes", &generated.source);
}

#[test]
fn caller_cfg_can_branch_on_a_composed_callee_result() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "branch_on_call_result".to_owned(),
        address: 0x1000,
        bytes: vec![
            0xef, 0x10, 0x00, 0x00, // jal ra, 0x2000
            0x63, 0x06, 0x05, 0x00, // beq a0, zero, 0x1010
            0x13, 0x05, 0x30, 0x00, // li a0, 3
            0x67, 0x80, 0x00, 0x00, // ret
            0x13, 0x05, 0x40, 0x00, // li a0, 4
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let child = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "branch_child".to_owned(),
        address: 0x2000,
        bytes: vec![
            0x63, 0x06, 0x05, 0x00, // beq a0, zero, 0x200c
            0x13, 0x05, 0x10, 0x00, // li a0, 1
            0x67, 0x80, 0x00, 0x00, // ret
            0x13, 0x05, 0x20, 0x00, // li a0, 2
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let symbols = BTreeMap::from([(0x2000, child)]);
    let mut visiting = BTreeSet::from([0x1000]);

    let trace = resolve_reference_trace(
        &parent,
        &symbols,
        &BTreeMap::new(),
        &StructuralPointerContext::default(),
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    let generated = generate_reference(&trace, "oracle.elf", "abc123", None, &[]).unwrap();
    assert!(generated.source.contains("let call_result0 = {"));
    assert!(generated.source.contains("if (call0_arg0"));
    assert!(generated.source.contains("if (call_result0"));
    assert!(
        generated
            .source
            .contains("ReferenceOutcome { exit_a0: Some(0x00000004_u32) }")
    );
    assert!(
        generated
            .source
            .contains("ReferenceOutcome { exit_a0: Some(0x00000003_u32) }")
    );
    assert_generated_reference_compiles("branch-on-call-result", &generated.source);
}

#[test]
fn caller_cfg_rejects_an_unmodeled_callee_result_used_as_a_condition() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "branch_on_void_call".to_owned(),
        address: 0x1000,
        bytes: vec![
            0xef, 0x10, 0x00, 0x00, // jal ra, 0x2000
            0x63, 0x06, 0x05, 0x00, // beq a0, zero, 0x1010
            0x13, 0x05, 0x30, 0x00, // li a0, 3
            0x67, 0x80, 0x00, 0x00, // ret
            0x13, 0x05, 0x40, 0x00, // li a0, 4
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let delay = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "ets_delay_us".to_owned(),
        address: 0x2000,
        bytes: vec![0x73, 0x00, 0x10, 0x00],
        addresses_resolved: true,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let symbols = BTreeMap::from([(0x2000, delay)]);
    let mut visiting = BTreeSet::from([0x1000]);

    let trace = resolve_reference_trace(
        &parent,
        &symbols,
        &BTreeMap::new(),
        &StructuralPointerContext::default(),
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(!trace.is_reference_eligible());
    assert!(trace.reference_blockers.iter().any(|blocker| {
        blocker.contains("composed call result is used without a modeled callee `a0`")
    }));
}

#[test]
fn caller_cfg_allows_an_unmodeled_callee_result_when_it_is_discarded() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "branch_with_void_call".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x63, 0x0a, 0x05, 0x00, // beq a0, zero, 0x1014
            0x17, 0x03, 0x00, 0x00, // auipc t1, 0
            0xe7, 0x00, 0x03, 0x00, // jalr ra, 0(t1)
            0x13, 0x05, 0x70, 0x00, // li a0, 7
            0x67, 0x80, 0x00, 0x00, // ret
            0x13, 0x05, 0x80, 0x00, // li a0, 8
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let delay = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "ets_delay_us".to_owned(),
        address: 0x2000,
        bytes: vec![0x73, 0x00, 0x10, 0x00],
        addresses_resolved: true,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let symbols = BTreeMap::from([(0x2000, delay)]);
    let relocations = BTreeMap::from([(
        StructuralCallSite::new(&parent, 0x1004),
        ("ets_delay_us".to_owned(), Some(0x2000)),
    )]);
    let mut visiting = BTreeSet::from([0x1000]);

    let trace = resolve_reference_trace(
        &parent,
        &symbols,
        &relocations,
        &StructuralPointerContext::default(),
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    let generated = generate_reference(&trace, "oracle.elf", "abc123", None, &[]).unwrap();
    assert!(
        generated
            .source
            .contains("// Composed direct call: ets_delay_us.")
    );
    assert!(generated.source.contains("let call0_arg0 ="));
    assert!(generated.source.contains("io.delay_micros("));
    assert!(!generated.source.contains("let call_result0 = {"));
    assert_generated_reference_compiles("discarded-call-result", &generated.source);
}

#[test]
fn relocated_returning_call_is_flattened_without_executing_auipc_jalr() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "relocated_parent".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x17, 0x03, 0x00, 0x00, // auipc t1, 0
            0xe7, 0x00, 0x03, 0x00, // jalr ra, 0(t1)
            0x13, 0x75, 0xf5, 0x0f, // andi a0, a0, 255
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let child = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "companion_child".to_owned(),
        address: 0x2000,
        bytes: vec![0x67, 0x80, 0x00, 0x00], // ret
        addresses_resolved: true,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let symbols = BTreeMap::from([(0x2000, child)]);
    let relocations = BTreeMap::from([(
        StructuralCallSite::new(&parent, 0x1000),
        ("companion_child".to_owned(), Some(0x2000)),
    )]);
    let mut visiting = BTreeSet::from([0x1000]);

    let trace = resolve_reference_trace(
        &parent,
        &symbols,
        &relocations,
        &StructuralPointerContext::default(),
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert_eq!(trace.reference_dependencies, ["companion_child"]);
    assert_eq!(trace.return_value, SymbolicValue::input(0).and(0xff));
}

#[test]
fn constant_size_memcpy_relocation_becomes_ordered_memory_effects() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: Some("memory.o".to_owned()),
        name: "copy_four".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x13, 0x06, 0x40, 0x00, // li a2, 4
            0x17, 0x03, 0x00, 0x00, // auipc t1, 0
            0xe7, 0x00, 0x03, 0x00, // jalr ra, 0(t1)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: false,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let relocations = BTreeMap::from([(
        StructuralCallSite::new(&parent, 0x1004),
        ("memcpy".to_owned(), None),
    )]);
    let mut visiting = BTreeSet::from([0x1000]);

    let trace = resolve_reference_trace(
        &parent,
        &BTreeMap::new(),
        &relocations,
        &StructuralPointerContext::default(),
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert_eq!(trace.reference_events.len(), 8);
    assert!(trace.reference_events[..4].iter().all(|event| matches!(
        event,
        DraftReferenceEvent::Memory {
            access: MemoryAccess::Read,
            width: 8,
            ..
        }
    )));
    assert!(trace.reference_events[4..].iter().all(|event| matches!(
        event,
        DraftReferenceEvent::Memory {
            access: MemoryAccess::Write,
            width: 8,
            ..
        }
    )));
    assert_eq!(trace.return_value, SymbolicValue::input(0));
    let generated = generate_reference(&trace, "memory.o", "abc123", None, &[]).unwrap();
    assert_eq!(generated.source.matches("memory.read(8,").count(), 4);
    assert_eq!(generated.source.matches("memory.write(8,").count(), 4);
    assert_generated_reference_compiles("constant-memcpy", &generated.source);
}

#[test]
fn constant_size_memset_relocation_preserves_byte_and_return_pointer() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: Some("memory.o".to_owned()),
        name: "fill_three".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x13, 0x06, 0x30, 0x00, // li a2, 3
            0x17, 0x03, 0x00, 0x00, // auipc t1, 0
            0xe7, 0x00, 0x03, 0x00, // jalr ra, 0(t1)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: false,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let relocations = BTreeMap::from([(
        StructuralCallSite::new(&parent, 0x1004),
        ("memset".to_owned(), None),
    )]);
    let mut visiting = BTreeSet::from([0x1000]);

    let trace = resolve_reference_trace(
        &parent,
        &BTreeMap::new(),
        &relocations,
        &StructuralPointerContext::default(),
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert_eq!(trace.reference_events.len(), 3);
    for event in &trace.reference_events {
        let DraftReferenceEvent::Memory {
            access: MemoryAccess::Write,
            width: 8,
            value: Some(value),
            ..
        } = event
        else {
            panic!("expected one memset byte write");
        };
        assert_eq!(
            value.canonical(),
            SymbolicValue::input(1).and(0xff).canonical()
        );
    }
    assert_eq!(trace.return_value, SymbolicValue::input(0));
}

#[test]
fn dynamic_size_memcpy_relocation_remains_fail_closed() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: Some("memory.o".to_owned()),
        name: "copy_dynamic".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x17, 0x03, 0x00, 0x00, // auipc t1, 0
            0xe7, 0x00, 0x03, 0x00, // jalr ra, 0(t1)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: false,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let relocations = BTreeMap::from([(
        StructuralCallSite::new(&parent, 0x1000),
        ("memcpy".to_owned(), None),
    )]);
    let mut visiting = BTreeSet::from([0x1000]);

    let trace = resolve_reference_trace(
        &parent,
        &BTreeMap::new(),
        &relocations,
        &StructuralPointerContext::default(),
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(!trace.is_reference_eligible());
    assert!(trace.reference_blockers.iter().any(|blocker| {
        blocker.contains("standard-memory-intrinsic")
            && blocker.contains("memcpy length is not constant")
    }));
}

#[test]
fn unresolved_call_relocation_fails_closed() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "unresolved_parent".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x17, 0x03, 0x00, 0x00, // auipc t1, 0
            0x67, 0x00, 0x03, 0x00, // jalr zero, 0(t1)
        ],
        addresses_resolved: true,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let relocations = BTreeMap::from([(
        StructuralCallSite::new(&parent, 0x1000),
        ("missing_child".to_owned(), None),
    )]);

    let trace = trace_binary_symbol(
        &parent,
        &map(),
        &relocations,
        &StructuralPointerContext::default(),
        None,
    )
    .unwrap();

    assert!(!trace.is_reference_eligible());
    assert!(
        trace
            .reference_blockers
            .iter()
            .any(|blocker| blocker.contains("unresolved-call-relocation"))
    );
}

#[test]
fn forward_local_jump_skips_dead_instructions() {
    let symbol = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "local_jump".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x6f, 0x00, 0x80, 0x00, // j 0x1008
            0x73, 0x00, 0x10, 0x00, // ebreak (unreachable)
            0x13, 0x05, 0x05, 0x00, // mv a0, a0
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };

    let trace = trace_binary_symbol(
        &symbol,
        &map(),
        &BTreeMap::new(),
        &StructuralPointerContext::default(),
        None,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert_eq!(trace.return_value, SymbolicValue::input(0));
}

#[test]
fn local_jump_loop_fails_closed() {
    let symbol = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "local_loop".to_owned(),
        address: 0x1000,
        bytes: vec![0x6f, 0x00, 0x00, 0x00], // j 0x1000
        addresses_resolved: true,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };

    let trace = trace_binary_symbol(
        &symbol,
        &map(),
        &BTreeMap::new(),
        &StructuralPointerContext::default(),
        None,
    )
    .unwrap();

    assert!(!trace.is_reference_eligible());
    assert!(trace.blockers[0].contains("control-flow loop"));
}

#[test]
fn constant_counted_loop_is_bounded_and_fully_unrolled() {
    let symbol = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "constant_counted_loop".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x93, 0x05, 0x00, 0x00, // li a1, 0
            0x13, 0x06, 0x30, 0x00, // li a2, 3
            0x93, 0x85, 0x15, 0x00, // addi a1, a1, 1
            0xe3, 0x9e, 0xc5, 0xfe, // bne a1, a2, -4
            0x13, 0x85, 0x05, 0x00, // mv a0, a1
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };

    let trace = trace_binary_symbol(
        &symbol,
        &map(),
        &BTreeMap::new(),
        &StructuralPointerContext::default(),
        None,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert!(trace.blockers.is_empty(), "{trace:#?}");
    assert_eq!(trace.return_value, SymbolicValue::Constant(3));
}

#[test]
fn backward_edge_to_an_unvisited_return_block_is_not_a_loop() {
    let symbol = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "backward_acyclic_edge".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x63, 0x08, 0x05, 0x00, // beq a0, zero, 0x1010
            0x13, 0x05, 0x10, 0x00, // li a0, 1
            0x67, 0x80, 0x00, 0x00, // ret
            0x13, 0x00, 0x00, 0x00, // nop (unreachable padding)
            0xe3, 0x0c, 0x05, 0xfe, // beq a0, zero, 0x1008
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let symbols = BTreeMap::from([(0x1000, symbol.clone())]);

    let trace = resolve_reference_trace(
        &symbol,
        &symbols,
        &BTreeMap::new(),
        &StructuralPointerContext::default(),
        None,
        &map(),
        &mut BTreeSet::new(),
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert!(trace.reference_flow.is_some(), "{trace:#?}");
}

#[test]
fn delay_intrinsic_is_composed_without_decoding_its_rom_body() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "delay_wrapper".to_owned(),
        address: 0x1000,
        bytes: vec![0x6f, 0x10, 0x00, 0x00], // j 0x2000
        addresses_resolved: true,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let delay = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "ets_delay_us".to_owned(),
        address: 0x2000,
        bytes: vec![0x73, 0x00, 0x10, 0x00], // body is deliberately unsupported
        addresses_resolved: true,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let symbols = BTreeMap::from([(0x2000, delay)]);
    let mut visiting = BTreeSet::from([0x1000]);

    let trace = resolve_reference_trace(
        &parent,
        &symbols,
        &BTreeMap::new(),
        &StructuralPointerContext::default(),
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert_eq!(trace.reference_dependencies, ["ets_delay_us"]);
    assert_eq!(
        trace.reference_events,
        [DraftReferenceEvent::DelayMicros {
            micros: SymbolicValue::input(0)
        }]
    );
}

#[test]
fn constant_conditional_branch_follows_only_the_feasible_edge() {
    let symbol = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "constant_branch".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x63, 0x04, 0x00, 0x00, // beq zero, zero, 0x1008
            0x73, 0x00, 0x10, 0x00, // ebreak (infeasible)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };

    let trace = trace_binary_symbol(
        &symbol,
        &map(),
        &BTreeMap::new(),
        &StructuralPointerContext::default(),
        None,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
}

#[test]
fn symbolic_conditional_branch_fails_closed() {
    let symbol = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "symbolic_branch".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x63, 0x04, 0x05, 0x00, // beq a0, zero, 0x1008
            0x67, 0x80, 0x00, 0x00, // ret
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };

    let trace = trace_binary_symbol(
        &symbol,
        &map(),
        &BTreeMap::new(),
        &StructuralPointerContext::default(),
        None,
    )
    .unwrap();

    assert!(!trace.is_reference_eligible());
    assert!(trace.blockers[0].contains("input-dependent control-flow"));
}

#[test]
fn bounded_symbolic_cfg_becomes_structured_reference_flow() {
    let symbol = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "symbolic_branch_reference".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x63, 0x06, 0x05, 0x00, // beq a0, zero, 0x100c
            0x13, 0x05, 0x10, 0x00, // li a0, 1
            0x67, 0x80, 0x00, 0x00, // ret
            0x13, 0x05, 0x20, 0x00, // li a0, 2
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let mut visiting = BTreeSet::from([0x1000]);

    let trace = resolve_reference_trace(
        &symbol,
        &BTreeMap::new(),
        &BTreeMap::new(),
        &StructuralPointerContext::default(),
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert!(!trace.is_exact());
    let DraftReferenceTerminator::Branch {
        condition,
        taken,
        not_taken,
    } = &trace.reference_flow.as_ref().unwrap().terminator
    else {
        panic!("expected a structured branch");
    };
    assert_eq!(condition.site, 0x1000);
    assert!(matches!(
        taken.terminator,
        DraftReferenceTerminator::Return(SymbolicValue::Constant(2))
    ));
    assert!(matches!(
        not_taken.terminator,
        DraftReferenceTerminator::Return(SymbolicValue::Constant(1))
    ));

    let generated = generate_reference(&trace, "oracle.elf", "abc123", None, &[]).unwrap();
    assert!(generated.exit_a0_modeled);
    assert!(
        generated
            .source
            .contains("// Symbolic branch from 0x00001000.")
    );
    assert!(generated.source.contains("if (args[0]"));
    assert!(
        generated
            .source
            .contains("ReferenceOutcome { exit_a0: Some(0x00000002_u32) }")
    );
    assert!(
        generated
            .source
            .contains("ReferenceOutcome { exit_a0: Some(0x00000001_u32) }")
    );
}

#[test]
fn constant_call_argument_specializes_a_child_branch() {
    let parent = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "constant_wrapper".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x13, 0x05, 0x00, 0x00, // li a0, 0
            0x6f, 0x00, 0x40, 0x00, // j 0x1008
        ],
        addresses_resolved: true,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let child = artifact::ArtifactSymbolDefinition {
        member: None,
        name: "conditional_child".to_owned(),
        address: 0x1008,
        bytes: vec![
            0x63, 0x04, 0x05, 0x00, // beq a0, zero, 0x1010
            0x73, 0x00, 0x10, 0x00, // ebreak (infeasible for this call)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        addresses_resolved: true,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let symbols = BTreeMap::from([(0x1008, child)]);
    let mut visiting = BTreeSet::from([0x1000]);

    let trace = resolve_reference_trace(
        &parent,
        &symbols,
        &BTreeMap::new(),
        &StructuralPointerContext::default(),
        None,
        &map(),
        &mut visiting,
    )
    .unwrap();

    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert_eq!(trace.reference_dependencies, ["conditional_child"]);
    assert_eq!(trace.return_value, SymbolicValue::Constant(0));
}

#[test]
fn local_basic_block_labels_do_not_truncate_the_function() {
    let disassembly = r#"
20100000 <conditional>:
20100000: beqz a0, 0x20100008 <.Ldone>
20100004: nop
20100008 <.Ldone>:
20100008: j 0x20100010 <child>
20100010 <next_function>:
20100010: ret
"#;
    let trace = trace_disassembly("conditional", disassembly, &map());
    assert!(!trace.is_exact());
    assert_eq!(trace.blockers.len(), 2);
    assert!(trace.blockers[0].contains("beqz"));
    assert!(trace.blockers[1].contains("j"));
}

#[test]
fn input_dependent_rmw_is_canonical_across_instruction_selection() {
    let vendor = r#"
20100000 <vendor>:
20100000: lui a4, 0x20107
20100004: lw a5, 0x30(a4)
20100008: slli a0, a0, 0x5
2010000c: andi a0, a0, 0x20
20100010: andi a5, a5, -0x21
20100014: or a0, a0, a5
20100018: sw a0, 0x30(a4)
2010001c: ret
"#;
    let rust = r#"
20100100 <rust>:
20100100: lui a4, 0x20107
20100104: lw a5, 0x30(a4)
20100108: andi a0, a0, 0x1
2010010c: slli a0, a0, 0x5
20100110: andi a5, a5, -0x21
20100114: or a5, a5, a0
20100118: sw a5, 0x30(a4)
2010011c: ret
"#;
    let vendor = trace_disassembly("vendor", vendor, &map());
    let rust = trace_disassembly("rust", rust, &map());
    assert!(vendor.is_exact());
    assert!(rust.is_exact());
    assert!(traces_equal(&vendor, &rust));
    assert!(
        vendor.events[1]
            .memory_value()
            .is_some_and(|value| value.contains("5=arg0.0"))
    );
}

#[test]
fn return_comparison_detects_a_wrong_field_from_the_same_read() {
    let vendor = r#"
20100000 <vendor>:
20100000: lui a4, 0x20107
20100004: lw a0, 0x30(a4)
20100008: srli a0, a0, 0xa
2010000c: andi a0, a0, 0x1
20100010: ret
"#;
    let rust = r#"
20100100 <rust>:
20100100: lui a4, 0x20107
20100104: lw a0, 0x30(a4)
20100108: srli a0, a0, 0x9
2010010c: andi a0, a0, 0x1
20100110: ret
"#;
    let vendor = trace_disassembly("vendor", vendor, &map());
    let rust = trace_disassembly("rust", rust, &map());
    assert!(traces_equal(&vendor, &rust));
    assert!(!returns_equal(&vendor, &rust));
}

#[test]
fn tail_jump_and_unresolved_write_both_fail_closed() {
    let tail = r#"
20100000 <tailing>:
20100000: j 0x20100020
"#;
    let trace = trace_disassembly("tailing", tail, &map());
    assert!(!trace.is_exact());
    assert_eq!(trace.blockers.len(), 1);

    let unresolved = r#"
20100000 <dynamic>:
20100000: lui a4, 0x20107
20100002: mul a0, a0, a1
20100004: sw a0, 0x30(a4)
20100008: ret
"#;
    let trace = trace_disassembly("dynamic", unresolved, &map());
    assert!(!trace.is_exact());
    assert_eq!(trace.blockers.len(), 1);
}

#[test]
fn fence_presence_and_position_are_compared() {
    let vendor = r#"
20100000 <vendor>:
20100000: fence r, w
20100004: fence w, r
20100008: ret
"#;
    let without_fence = r#"
20100100 <rust>:
20100100: ret
"#;
    let reversed = r#"
20100200 <rust>:
20100200: fence w, r
20100204: fence r, w
20100208: ret
"#;
    let vendor = trace_disassembly("vendor", vendor, &map());
    assert!(vendor.is_exact());
    assert!(!traces_equal(
        &vendor,
        &trace_disassembly("rust", without_fence, &map())
    ));
    assert!(!traces_equal(
        &vendor,
        &trace_disassembly("rust", reversed, &map())
    ));
}
