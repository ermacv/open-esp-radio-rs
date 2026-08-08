use super::*;

mod architecture_boundaries;

fn generate_reference(
    trace: &FunctionAnalysis,
    artifact: &str,
    artifact_sha256: &str,
    member: Option<&str>,
    companions: &[(String, String)],
) -> std::result::Result<codegen::GeneratedReference, String> {
    let program = ResolvedReferenceProgram::try_from(trace)?;
    codegen::generate(&program, artifact, artifact_sha256, member, companions)
}

fn map() -> MmioMap {
    MmioMap {
        registers: vec![Register {
            address: 0x2010_7030,
            name: "AGC.CONTROL".to_owned(),
        }],
        regions: vec![MmioRegion {
            name: "radio".to_owned(),
            start: 0x2010_0000,
            end: 0x2020_0000,
            readable: true,
            writable: true,
        }],
    }
}

fn indexed_map(base: u32, stride: u32, count: u32, family: &str) -> MmioMap {
    MmioMap {
        registers: (0..count)
            .map(|index| Register {
                address: base.wrapping_add(index.wrapping_mul(stride)),
                name: format!("{family}{index}"),
            })
            .collect(),
        regions: vec![MmioRegion {
            name: family.to_owned(),
            start: base,
            end: base.wrapping_add(count.wrapping_mul(stride)),
            readable: true,
            writable: true,
        }],
    }
}

#[test]
fn affine_indexed_mmio_requires_a_contiguous_svd_bank_and_emits_a_guard() {
    let address = SymbolicValue::expression(
        ExpressionOperation::Add,
        SymbolicValue::input(0).shift_left(3),
        SymbolicValue::Constant(0x2010_4004),
    );
    let domain =
        indexed_mmio_domain(&address, &indexed_map(0x2010_4004, 8, 4, "WIFI.BSSID_HIGH")).unwrap();

    assert_eq!(domain.registers.len(), 4);
    assert_eq!(domain.guard.unwrap().maximum, 3);

    let missing_middle = MmioMap {
        registers: vec![
            Register {
                address: 0x2010_4004,
                name: "WIFI.BSSID_HIGH0".to_owned(),
            },
            Register {
                address: 0x2010_4014,
                name: "WIFI.BSSID_HIGH2".to_owned(),
            },
        ],
        regions: vec![],
    };
    assert!(indexed_mmio_domain(&address, &missing_middle).is_none());
}

#[test]
fn masked_indexed_mmio_proves_its_entire_domain_without_an_argument_guard() {
    let address = SymbolicValue::input(1)
        .and(3)
        .shift_left(2)
        .add_constant(0x2010_4dbc);
    let domain = indexed_mmio_domain(
        &address,
        &indexed_map(0x2010_4dbc, 4, 4, "WIFI.MU_EDCA_TIMER"),
    )
    .unwrap();

    assert_eq!(domain.registers.len(), 4);
    assert!(domain.guard.is_none());
}

#[test]
fn mixed_resolved_bitwise_values_fall_back_to_exact_expressions() {
    let register = SymbolicValue::RegisterImage {
        read_token: 0,
        address: 0x2010_7030,
        and_mask: u32::MAX,
        or_mask: 0,
    };
    let argument = SymbolicValue::input(0).and(7);

    for value in [
        register.clone().symbolic_bitand(argument.clone()),
        register.clone().symbolic_bitor(argument.clone()),
        register.clone().symbolic_bitxor(argument.clone()),
    ] {
        assert!(matches!(value, SymbolicValue::Expression { .. }));
        assert!(value.is_resolved());
    }
    assert_eq!(
        argument.clone().symbolic_bitxor(argument),
        SymbolicValue::Constant(0)
    );

    let zero_test = SymbolicValue::input(2).seqz();
    assert!(matches!(zero_test, SymbolicValue::Expression { .. }));
    assert_eq!(evaluate_for_input(&zero_test, 2, 0), Some(1));
    assert_eq!(evaluate_for_input(&zero_test, 2, 7), Some(0));
}

#[test]
fn masked_addition_and_or_have_one_canonical_field_insertion() {
    let masked = SymbolicValue::register_read(0, 0x2010_086c, 32, false).and(0xffff_ff00);
    assert_eq!(masked.clone().add_constant(4), masked.or(4));

    let seeded_field = SymbolicValue::register_read(1, 0x2010_7460, 32, false)
        .and(0xffff_00ff)
        .or(0x7300);
    assert_eq!(
        seeded_field.add_constant(0x600),
        SymbolicValue::register_read(1, 0x2010_7460, 32, false)
            .and(0xffff_00ff)
            .or(0x7900)
    );
}

fn assert_generated_reference_compiles(name: &str, source: &str) {
    let stem = format!("open-esp-radio-{name}-{}", std::process::id());
    let source_path = env::temp_dir().join(format!("{stem}.rs"));
    let output_path = env::temp_dir().join(format!("lib{stem}.rlib"));
    fs::write(&source_path, source).unwrap();
    let output = std::process::Command::new("rustc")
        .arg("--edition=2024")
        .arg("--crate-type=lib")
        .arg("-Dwarnings")
        .arg("-o")
        .arg(&output_path)
        .arg(&source_path)
        .output()
        .unwrap();
    fs::remove_file(source_path).unwrap();
    if output_path.exists() {
        fs::remove_file(output_path).unwrap();
    }
    assert!(
        output.status.success(),
        "generated reference did not compile:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(feature = "esp32s31-harness")]
fn assert_generated_reference_tests_run(name: &str, source: &str, tests: &str) {
    let stem = format!("open-esp-radio-{name}-tests-{}", std::process::id());
    let source_path = env::temp_dir().join(format!("{stem}.rs"));
    let output_path = env::temp_dir().join(&stem);
    fs::write(&source_path, format!("{source}\n{tests}")).unwrap();
    let compile = std::process::Command::new("rustc")
        .arg("--edition=2024")
        .arg("--test")
        .arg("-Dwarnings")
        .arg("-o")
        .arg(&output_path)
        .arg(&source_path)
        .output()
        .unwrap();
    if !compile.status.success() {
        let _ = fs::remove_file(&source_path);
        if output_path.exists() {
            let _ = fs::remove_file(&output_path);
        }
        panic!(
            "generated reference tests did not compile:\n{}",
            String::from_utf8_lossy(&compile.stderr)
        );
    }
    let run = std::process::Command::new(&output_path).output().unwrap();
    fs::remove_file(source_path).unwrap();
    fs::remove_file(output_path).unwrap();
    assert!(
        run.status.success(),
        "generated reference tests failed:\n{}\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
}

const TEST_NONE_ENTRY_SPEC: EntryContractSpec = EntryContractSpec {
    id: "none",
    function_table: None,
    pointer_symbols: &[],
    data_pointer_binding: None,
};
const TEST_NONE_ENTRY: EntryContractRef = EntryContractRef::new(&TEST_NONE_ENTRY_SPEC);
const TEST_CONTRACTS: HarnessContractSpec = HarnessContractSpec {
    external_tables: &[],
    entry_contracts: &[TEST_NONE_ENTRY],
    diagnostic_calls: &[],
};

fn test_reference_intrinsic(
    symbol: &artifact::ArtifactSymbolDefinition,
    _svd: &MmioMap,
    _pointer_context: &StructuralPointerContext,
) -> Option<FunctionAnalysis> {
    (symbol.name == "ets_delay_us").then(|| FunctionAnalysis {
        symbol: symbol.name.clone(),
        events: Vec::new(),
        reference_events: vec![DraftReferenceEvent::DelayMicros {
            micros: SymbolicValue::input(0),
        }],
        reference_dependencies: Vec::new(),
        blockers: Vec::new(),
        reference_blockers: Vec::new(),
        return_value: SymbolicValue::Unknown,
        reference_flow: None,
        unresolved_branch: None,
    })
}

fn no_test_memory_intrinsic(
    _symbol: &artifact::ArtifactSymbolDefinition,
    _arguments: &Rv32CallArguments,
) -> Option<std::result::Result<FunctionAnalysis, String>> {
    None
}

fn no_test_direct_semantic(
    _symbol: &artifact::ArtifactSymbolDefinition,
) -> Option<&'static DirectSemanticFunctionSpec> {
    None
}

fn no_test_wide_divide(
    _symbol: &artifact::ArtifactSymbolDefinition,
    _arguments: &Rv32CallArguments,
) -> Option<(SymbolicValue, SymbolicValue)> {
    None
}

static TEST_SUMMARIES: RiscvSummaryHooks = RiscvSummaryHooks {
    secondary_return_target: |_| false,
    direct_semantic: no_test_direct_semantic,
    reference_intrinsic: test_reference_intrinsic,
    standard_memory_intrinsic: no_test_memory_intrinsic,
    wide_signed_divide: no_test_wide_divide,
};
static TEST_RISCV_HARNESS: RiscvHarnessSpec = RiscvHarnessSpec {
    contracts: &TEST_CONTRACTS,
    summaries: &TEST_SUMMARIES,
};

fn synthetic_delay_pointer_context() -> StructuralPointerContext {
    StructuralPointerContext::from_harness(&TEST_RISCV_HARNESS)
}

#[cfg(feature = "esp32s31-harness")]
fn wifi_osi_tail_symbol(slot_offset: u32) -> artifact::ArtifactSymbolDefinition {
    let slot_load = ((slot_offset & 0x0fff) << 20) | (15 << 15) | (2 << 12) | (15 << 7) | 0x03;
    let mut bytes = vec![
        0xb7, 0x07, 0x00, 0x00, // lui a5, %hi(g_osi_funcs_p)
        0x83, 0xa7, 0x07, 0x00, // lw a5, %lo(g_osi_funcs_p)(a5)
    ];
    bytes.extend_from_slice(&slot_load.to_le_bytes());
    bytes.extend_from_slice(&[0x82, 0x87]); // jr a5
    artifact::ArtifactSymbolDefinition {
        member: Some("synthetic.o".to_owned()),
        name: "wifi_osi_tail".to_owned(),
        address: 0x1000,
        bytes,
        addresses_resolved: false,
        memory_regions: Vec::new(),
        relocations: vec![artifact::SymbolRelocation {
            address: 0x1004,
            kind: artifact::RelocationKind::Lo12I,
            symbol: "g_osi_funcs_p".to_owned(),
            addend: 0,
        }],
    }
}

#[cfg(feature = "esp32s31-harness")]
#[path = "../harnesses/esp32s31/oracle_tests/mod.rs"]
mod oracle;
mod structural;
mod verification;
