//! Generated-source scaffolding, fail-closed flow and ordered-memory output.

use super::*;

fn run_generated_program(name: &str, source: &str, harness: &str) {
    let directory = std::env::temp_dir().join(format!(
        "open-radio-generated-reference-{}-{name}",
        std::process::id()
    ));
    if directory.exists() {
        std::fs::remove_dir_all(&directory).unwrap();
    }
    std::fs::create_dir_all(&directory).unwrap();
    let source_path = directory.join("main.rs");
    let binary_path = directory.join("generated-reference");
    std::fs::write(&source_path, format!("{source}\n{harness}\n")).unwrap();

    let compiler = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let compilation = std::process::Command::new(compiler)
        .arg("--edition=2024")
        .arg(&source_path)
        .arg("-o")
        .arg(&binary_path)
        .output()
        .unwrap();
    if !compilation.status.success() {
        let diagnostics = String::from_utf8_lossy(&compilation.stderr).into_owned();
        std::fs::remove_dir_all(&directory).unwrap();
        panic!("generated reference did not compile:\n{diagnostics}");
    }

    let execution = std::process::Command::new(&binary_path).output().unwrap();
    std::fs::remove_dir_all(&directory).unwrap();
    assert!(
        execution.status.success(),
        "generated reference failed:\n{}",
        String::from_utf8_lossy(&execution.stderr)
    );
}

#[test]
fn generated_reference_executes_ordered_mmio_behavior() {
    let trace = FunctionAnalysis {
        symbol: "phy-example".to_owned(),
        events: vec![
            ObservableEvent::Memory {
                access: MemoryAccess::Read,
                width: 32,
                address: 0x2010_7030,
                register: "AGC.CONTROL".to_owned(),
                value: None,
            },
            ObservableEvent::Memory {
                access: MemoryAccess::Write,
                width: 32,
                address: 0x2010_7030,
                register: "AGC.CONTROL".to_owned(),
                value: Some(SymbolicValue::Constant(0x55)),
            },
        ],
        located_events: Vec::new(),
        located_reference_events: Vec::new(),
        reference_events: vec![
            DraftReferenceEvent::Observable(ObservableEvent::Memory {
                access: MemoryAccess::Read,
                width: 32,
                address: 0x2010_7030,
                register: "AGC.CONTROL".to_owned(),
                value: None,
            }),
            DraftReferenceEvent::Observable(ObservableEvent::Memory {
                access: MemoryAccess::Write,
                width: 32,
                address: 0x2010_7030,
                register: "AGC.CONTROL".to_owned(),
                value: Some(SymbolicValue::Constant(0x55)),
            }),
            DraftReferenceEvent::DelayMicros {
                micros: SymbolicValue::Constant(7),
            },
        ],
        reference_dependencies: Vec::new(),
        blockers: Vec::new(),
        reference_blockers: Vec::new(),
        return_value: SymbolicValue::input(0),
        reference_flow: None,
        unresolved_branch: None,
    };
    let generated = generate_from_trace(&trace, "oracle.elf", "abc123", None, &[]).unwrap();

    assert!(generated.exit_a0_modeled);
    run_generated_program(
        "ordered-mmio",
        &generated.source,
        r#"
#[derive(Debug, Eq, PartialEq)]
enum IoEvent { Read, Write, Delay }

struct Io(Vec<IoEvent>);
impl ReferenceIo for Io {
    fn read(&mut self, _width: u8, _address: u32) -> u32 {
        self.0.push(IoEvent::Read);
        0
    }
    fn write(&mut self, _width: u8, _address: u32, _value: u32) {
        self.0.push(IoEvent::Write);
    }
    fn delay_micros(&mut self, _micros: u32) { self.0.push(IoEvent::Delay); }
    fn fence(&mut self, _fm: u8, _predecessor: u8, _successor: u8) {}
}

struct Memory;
impl ReferenceMemory for Memory {
    fn symbol_address(&mut self, _member: Option<&str>, _symbol: &str) -> u32 { 0 }
    fn read(&mut self, _width: u8, _address: u32) -> u32 { 0 }
    fn write(&mut self, _width: u8, _address: u32, _value: u32) {}
}

struct Platform;
impl ReferencePlatform for Platform {
    fn external_call(&mut self, _contract: &str, _model: &str, _arguments: &[u32]) -> ReferenceExternalCallOutcome { ReferenceExternalCallOutcome::default() }
    fn direct_external_call(&mut self, _function: &str, _arguments: &[u32]) -> u32 { 0 }
    fn diagnostic_call(&mut self, _function: &str, _arguments: &[u32]) {}
    fn fail_stop(&mut self, _function: &str, _arguments: &[u32]) -> ! { panic!("unexpected fail-stop") }
}

fn main() {
    let mut io = Io(Vec::new());
    let mut memory = Memory;
    let mut platform = Platform;
    let mut registers = [0; 8];
    registers[0] = 23;
    let outcome = vendor_reference_phy_example(
        &mut io,
        &mut memory,
        &mut platform,
        Rv32ReferenceArguments { registers, stack: [0; 8] },
    );
    assert_eq!(io.0, vec![IoEvent::Read, IoEvent::Write, IoEvent::Delay]);
    assert_eq!(outcome.exit_a0, Some(23));
}
"#,
    );
}

#[test]
fn rejects_incomplete_control_flow_instead_of_emitting_a_partial_function() {
    let trace = FunctionAnalysis {
        symbol: "branchy".to_owned(),
        events: Vec::new(),
        located_events: Vec::new(),
        located_reference_events: Vec::new(),
        reference_events: Vec::new(),
        reference_dependencies: Vec::new(),
        blockers: vec!["control-flow instruction at 0x10".to_owned()],
        reference_blockers: Vec::new(),
        return_value: SymbolicValue::Unknown,
        reference_flow: None,
        unresolved_branch: None,
    };
    let error = generate_from_trace(&trace, "oracle.elf", "abc123", None, &[]).unwrap_err();
    assert!(error.contains("not eligible"));
    assert!(error.contains("control-flow"));
}

#[test]
fn renders_conditional_fail_stop_as_a_diverging_platform_boundary() {
    let trace = FunctionAnalysis {
        symbol: "conditional_assert".to_owned(),
        events: Vec::new(),
        located_events: Vec::new(),
        located_reference_events: Vec::new(),
        reference_events: Vec::new(),
        reference_dependencies: Vec::new(),
        blockers: Vec::new(),
        reference_blockers: Vec::new(),
        return_value: SymbolicValue::Unknown,
        reference_flow: Some(DraftReferenceFlow {
            events: Vec::new(),
            terminator: DraftReferenceTerminator::Branch {
                condition: BranchCondition {
                    site: 0x1002,
                    operation: BranchOperation::Equal,
                    left: SymbolicValue::input(4),
                    right: SymbolicValue::Constant(3),
                },
                taken: Box::new(DraftReferenceFlow {
                    events: Vec::new(),
                    terminator: DraftReferenceTerminator::FailStop {
                        site: 0x100c,
                        function: "controller_assert".to_owned(),
                        argument_count: 1,
                        arguments: Box::new([SymbolicValue::input(4)]),
                    },
                }),
                not_taken: Box::new(DraftReferenceFlow {
                    events: Vec::new(),
                    terminator: DraftReferenceTerminator::Return(SymbolicValue::Unknown),
                }),
            },
        }),
        unresolved_branch: None,
    };

    let generated = generate_from_trace(&trace, "oracle.elf", "abc123", None, &[]).unwrap();

    assert!(!generated.exit_a0_modeled);
    run_generated_program(
        "conditional-fail-stop",
        &generated.source,
        r#"
struct Io;
impl ReferenceIo for Io {
    fn read(&mut self, _width: u8, _address: u32) -> u32 { 0 }
    fn write(&mut self, _width: u8, _address: u32, _value: u32) {}
    fn delay_micros(&mut self, _micros: u32) {}
    fn fence(&mut self, _fm: u8, _predecessor: u8, _successor: u8) {}
}

struct Memory;
impl ReferenceMemory for Memory {
    fn symbol_address(&mut self, _member: Option<&str>, _symbol: &str) -> u32 { 0 }
    fn read(&mut self, _width: u8, _address: u32) -> u32 { 0 }
    fn write(&mut self, _width: u8, _address: u32, _value: u32) {}
}

struct Platform;
impl ReferencePlatform for Platform {
    fn external_call(&mut self, _contract: &str, _model: &str, _arguments: &[u32]) -> ReferenceExternalCallOutcome { ReferenceExternalCallOutcome::default() }
    fn direct_external_call(&mut self, _function: &str, _arguments: &[u32]) -> u32 { 0 }
    fn diagnostic_call(&mut self, _function: &str, _arguments: &[u32]) {}
    fn fail_stop(&mut self, _function: &str, _arguments: &[u32]) -> ! { panic!("fail-stop") }
}

fn invoke(value: u32) {
    let mut io = Io;
    let mut memory = Memory;
    let mut platform = Platform;
    let mut registers = [0; 8];
    registers[4] = value;
    let _ = vendor_reference_conditional_assert(
        &mut io,
        &mut memory,
        &mut platform,
        Rv32ReferenceArguments { registers, stack: [0; 8] },
    );
}

fn main() {
    invoke(2);
    assert!(std::panic::catch_unwind(|| invoke(3)).is_err());
}
"#,
    );
}

#[test]
fn preserves_ordered_elf_ram_reads_and_writes() {
    let address = 0x3fcd_0010;
    let trace = FunctionAnalysis {
        symbol: "state_leaf".to_owned(),
        events: Vec::new(),
        located_events: Vec::new(),
        located_reference_events: Vec::new(),
        reference_events: vec![
            DraftReferenceEvent::Memory {
                access: MemoryAccess::Read,
                width: 32,
                address: SymbolicValue::Constant(address),
                region: ".data".to_owned(),
                value: None,
            },
            DraftReferenceEvent::Memory {
                access: MemoryAccess::Write,
                width: 32,
                address: SymbolicValue::Constant(address),
                region: ".data".to_owned(),
                value: Some(SymbolicValue::Constant(0x55)),
            },
        ],
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
    let generated = generate_from_trace(&trace, "oracle.elf", "abc123", None, &[]).unwrap();

    run_generated_program(
        "ordered-memory",
        &generated.source,
        r#"
struct Io;
impl ReferenceIo for Io {
    fn read(&mut self, _width: u8, _address: u32) -> u32 { 0 }
    fn write(&mut self, _width: u8, _address: u32, _value: u32) {}
    fn delay_micros(&mut self, _micros: u32) {}
    fn fence(&mut self, _fm: u8, _predecessor: u8, _successor: u8) {}
}

#[derive(Debug, Eq, PartialEq)]
enum MemoryEvent { Read, Write }
struct Memory(Vec<MemoryEvent>);
impl ReferenceMemory for Memory {
    fn symbol_address(&mut self, _member: Option<&str>, _symbol: &str) -> u32 { 0 }
    fn read(&mut self, _width: u8, _address: u32) -> u32 {
        self.0.push(MemoryEvent::Read);
        0x1234_5678
    }
    fn write(&mut self, _width: u8, _address: u32, _value: u32) {
        self.0.push(MemoryEvent::Write);
    }
}

struct Platform;
impl ReferencePlatform for Platform {
    fn external_call(&mut self, _contract: &str, _model: &str, _arguments: &[u32]) -> ReferenceExternalCallOutcome { ReferenceExternalCallOutcome::default() }
    fn direct_external_call(&mut self, _function: &str, _arguments: &[u32]) -> u32 { 0 }
    fn diagnostic_call(&mut self, _function: &str, _arguments: &[u32]) {}
    fn fail_stop(&mut self, _function: &str, _arguments: &[u32]) -> ! { panic!("unexpected fail-stop") }
}

fn main() {
    let mut io = Io;
    let mut memory = Memory(Vec::new());
    let mut platform = Platform;
    let outcome = vendor_reference_state_leaf(
        &mut io,
        &mut memory,
        &mut platform,
        Rv32ReferenceArguments { registers: [0; 8], stack: [0; 8] },
    );
    assert_eq!(memory.0, vec![MemoryEvent::Read, MemoryEvent::Write]);
    assert_eq!(outcome.exit_a0, Some(0x1234_5678));
}
"#,
    );
}
