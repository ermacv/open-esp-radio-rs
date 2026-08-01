//! SVD-aware extraction of direct MMIO traces from compiled RISC-V code.
//!
//! This is deliberately an instruction/ELF tool, not a source-text policy
//! checker. Direct trace comparison handles straight-line leaves exactly. The
//! stricter reference resolver additionally composes supported calls and
//! bounded acyclic symbolic branches while failing closed on unresolved MMIO
//! addressing, loops and unsupported effects.

mod binary;
mod dispositions;
mod emulator;
mod external_abi;
mod profiles;
mod reference_codegen;
mod semantic;

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    env,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

use rv_asm::{Inst, Reg};
use sha2::{Digest, Sha256};

type Error = Box<dyn std::error::Error>;
type Result<T> = std::result::Result<T, Error>;
type EvidenceSet = BTreeMap<(String, String), String>;

const ESP32S31_LIBPHY_SHA256: &str =
    "51497819736295c9b33d6775495dade4c6fb39db887edfe095608c670d9ae223";
const ESP32S31_LIBPP_SHA256: &str =
    "f863c65c3ed89cf5d2a2cbe0d6bca3b783ca35788a704bb68e13958e4b94958e";
const ESP32S31_REV0_ROM_LOCAL_SHA256: &str =
    "d01bde81d9b3806e37ef1d9ac3b58af4f5b3d91eeef4f44d20e79d6a9f227542";
const ESP32S31_REV0_ROM_CANONICAL_SHA256: &str =
    "a52ad7513deb656a910a5740125f1cce2c7941f11ce57213b7b43aea93d5ab87";
const ESP32S31_LINKED_LIBPHY_SHA256: &str =
    "a38df8f225107786bbb77c03cdc2ec62d8aa68178d8412279745073c4a991524";

fn is_pinned_vendor_digest(digest: &str) -> bool {
    matches!(
        digest,
        ESP32S31_LIBPHY_SHA256
            | ESP32S31_LIBPP_SHA256
            | ESP32S31_REV0_ROM_LOCAL_SHA256
            | ESP32S31_REV0_ROM_CANONICAL_SHA256
            | ESP32S31_LINKED_LIBPHY_SHA256
    )
}

fn artifact_sha256(path: &Path) -> Result<String> {
    let bytes = fs::read(path)?;
    Ok(format!("{:x}", Sha256::digest(&bytes)))
}

fn pinned_vendor_digest(path: &Path) -> Result<String> {
    let digest = artifact_sha256(path)?;
    if !is_pinned_vendor_digest(&digest) {
        return Err(
            format!("vendor artifact is not a pinned ESP32-S31 oracle: sha256 {digest}").into(),
        );
    }
    Ok(digest)
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Register {
    address: u32,
    name: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Window {
    start: u32,
    end: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SvdMap {
    registers: Vec<Register>,
    windows: Vec<Window>,
}

impl SvdMap {
    fn load(path: &Path) -> Result<Self> {
        let xml = fs::read_to_string(path)?;
        let document = roxmltree::Document::parse(&xml)?;
        let mut registers = Vec::new();
        for peripheral in document
            .descendants()
            .filter(|node| node.has_tag_name("peripheral"))
        {
            let Some(name) = child_text(peripheral, "name") else {
                continue;
            };
            let Some(base) = child_text(peripheral, "baseAddress").and_then(parse_u32) else {
                continue;
            };
            let Some(container) = peripheral
                .children()
                .find(|node| node.has_tag_name("registers"))
            else {
                continue;
            };
            collect_registers(container, base, name, &mut registers)?;
        }
        registers.sort_by_key(|register| (register.address, register.name.clone()));

        let mut windows = Vec::new();
        for node in document
            .descendants()
            .filter(|node| node.has_tag_name("window"))
        {
            let (Some(start), Some(end)) = (
                node.attribute("start").and_then(parse_u32),
                node.attribute("endExclusive").and_then(parse_u32),
            ) else {
                continue;
            };
            windows.push(Window { start, end });
        }
        if windows.is_empty() {
            return Err("SVD has no openEspRadioAddressWindows".into());
        }
        Ok(Self { registers, windows })
    }

    fn load_all(paths: &[PathBuf]) -> Result<Self> {
        let mut combined = Self {
            registers: Vec::new(),
            windows: Vec::new(),
        };
        for path in paths {
            let map = Self::load(path)?;
            combined.registers.extend(map.registers);
            combined.windows.extend(map.windows);
        }
        combined
            .registers
            .sort_by_key(|register| (register.address, register.name.clone()));
        combined.registers.dedup();
        reject_register_collisions(&combined.registers)?;
        combined
            .windows
            .sort_by_key(|window| (window.start, window.end));
        combined.windows.dedup();
        Ok(combined)
    }

    fn contains_mmio(&self, address: u32) -> bool {
        self.windows
            .iter()
            .any(|window| address >= window.start && address < window.end)
    }

    fn register_name(&self, address: u32) -> String {
        let names: Vec<_> = self
            .registers
            .iter()
            .filter(|register| register.address == address)
            .map(|register| register.name.as_str())
            .collect();
        if names.is_empty() {
            "UNMAPPED".to_owned()
        } else {
            names.join("|")
        }
    }

    fn register(&self, address: u32) -> Option<&Register> {
        self.registers
            .binary_search_by_key(&address, |register| register.address)
            .ok()
            .map(|index| &self.registers[index])
    }
}

fn reject_register_collisions(registers: &[Register]) -> Result<()> {
    for registers in registers.chunk_by(|left, right| left.address == right.address) {
        if registers.len() > 1 {
            let names = registers
                .iter()
                .map(|register| register.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!(
                "conflicting SVD register definitions at {:#010x}: {names}",
                registers[0].address
            )
            .into());
        }
    }
    Ok(())
}

fn record_evidence(
    evidence: &mut EvidenceSet,
    source: &str,
    symbol: &str,
    kind: impl Into<String>,
) -> Result<()> {
    let key = (source.to_owned(), symbol.to_owned());
    let kind = kind.into();
    if let Some(previous) = evidence.insert(key, kind.clone())
        && previous != kind
    {
        return Err(
            format!("conflicting evidence for {source} {symbol}: {previous} and {kind}").into(),
        );
    }
    Ok(())
}

fn load_evidence_baseline(path: &Path) -> Result<EvidenceSet> {
    let text = fs::read_to_string(path)?;
    let mut evidence = EvidenceSet::new();
    for (index, raw) in text.lines().enumerate() {
        let line_number = index + 1;
        let line = raw.split_once('#').map_or(raw, |(before, _)| before).trim();
        if line.is_empty() {
            continue;
        }
        let fields = line.split_whitespace().collect::<Vec<_>>();
        let ["evidence", source, symbol, kind] = fields.as_slice() else {
            return Err(format!(
                "invalid evidence baseline line {line_number}; expected: evidence SOURCE SYMBOL KIND"
            )
            .into());
        };
        record_evidence(&mut evidence, source, symbol, *kind)?;
    }
    if evidence.is_empty() {
        return Err(format!("evidence baseline {} is empty", path.display()).into());
    }
    Ok(evidence)
}

fn check_evidence_baseline(expected: &EvidenceSet, actual: &EvidenceSet) -> bool {
    let mut passed = true;
    for ((source, symbol), expected_kind) in expected {
        match actual.get(&(source.clone(), symbol.clone())) {
            Some(actual_kind) if actual_kind == expected_kind => {}
            Some(actual_kind) => {
                passed = false;
                println!(
                    "EVIDENCE-REGRESSION\t{source}\t{symbol}\texpected={expected_kind}\tactual={actual_kind}"
                );
            }
            None => {
                passed = false;
                println!(
                    "EVIDENCE-REGRESSION\t{source}\t{symbol}\texpected={expected_kind}\tactual=missing"
                );
            }
        }
    }
    for ((source, symbol), kind) in actual {
        if !expected.contains_key(&(source.clone(), symbol.clone())) {
            println!("EVIDENCE-ADDITION\t{source}\t{symbol}\t{kind}");
        }
    }
    println!(
        "EVIDENCE-BASELINE\t{}\texpected={}\tactual={}",
        if passed { "PASS" } else { "FAIL" },
        expected.len(),
        actual.len()
    );
    passed
}

fn print_evidence(evidence: &EvidenceSet) {
    for ((source, symbol), kind) in evidence {
        println!("EVIDENCE\t{source}\t{symbol}\t{kind}");
    }
}

fn write_json_string(output: &mut String, value: &str) {
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character.is_control() => {
                write!(output, "\\u{:04x}", character as u32)
                    .expect("writing to String cannot fail");
            }
            character => output.push(character),
        }
    }
    output.push('"');
}

#[allow(
    clippy::too_many_arguments,
    reason = "the report boundary deliberately receives a complete immutable proof record"
)]
fn write_verification_json_report(
    path: &Path,
    gate: VerificationGate,
    summary: VerifySummary,
    orphan_probes: usize,
    evidence_baseline_passed: bool,
    passed: bool,
    evidence: &EvidenceSet,
    artifacts: &[(&str, &Path)],
    qualification_gaps: &[&dispositions::Entry],
) -> Result<()> {
    let mut output = String::new();
    output.push_str("{\n  \"schema_version\": 1,\n  \"command\": \"verify-all\",\n");
    output.push_str("  \"gate\": ");
    write_json_string(
        &mut output,
        match gate {
            VerificationGate::Completion => "completion",
            VerificationGate::Regression { .. } => "regression",
        },
    );
    writeln!(output, ",\n  \"passed\": {passed},").expect("writing to String cannot fail");
    writeln!(
        output,
        "  \"evidence_baseline_passed\": {evidence_baseline_passed},"
    )
    .expect("writing to String cannot fail");
    output.push_str("  \"summary\": {\n");
    for (name, value, trailing) in [
        ("vendor_functions", summary.vendor_functions, true),
        ("matched", summary.matched, true),
        ("symbolic_matches", summary.symbolic_matches, true),
        ("scenario_matches", summary.scenario_matches, true),
        ("state_matches", summary.state_matches, true),
        ("composition_matches", summary.composition_matches, true),
        ("mismatched", summary.mismatched, true),
        ("incomplete", summary.incomplete, true),
        ("missing", summary.missing, true),
        (
            "implemented_unqualified",
            summary.implemented_unqualified,
            true,
        ),
        ("not_yet_ported", summary.not_yet_ported, true),
        ("orphan_rust_probes", orphan_probes, false),
    ] {
        writeln!(
            output,
            "    \"{name}\": {value}{}",
            if trailing { "," } else { "" }
        )
        .expect("writing to String cannot fail");
    }
    output.push_str("  },\n  \"qualification_gaps\": [\n");
    for (index, gap) in qualification_gaps.iter().enumerate() {
        output.push_str("    {\"source\": ");
        write_json_string(&mut output, &gap.source);
        output.push_str(", \"symbol\": ");
        write_json_string(&mut output, &gap.symbol);
        output.push_str(", \"rust_component\": ");
        write_json_string(
            &mut output,
            gap.rust_component.as_deref().unwrap_or("missing"),
        );
        output.push_str(", \"blocked_by\": [");
        for (blocker_index, (source, symbol)) in gap.qualification_blockers.iter().enumerate() {
            if blocker_index != 0 {
                output.push_str(", ");
            }
            output.push_str("{\"source\": ");
            write_json_string(&mut output, source);
            output.push_str(", \"symbol\": ");
            write_json_string(&mut output, symbol);
            output.push('}');
        }
        output.push_str("]}");
        output.push_str(if index + 1 == qualification_gaps.len() {
            "\n"
        } else {
            ",\n"
        });
    }
    output.push_str("  ],\n  \"artifacts\": [\n");
    for (index, (role, artifact)) in artifacts.iter().enumerate() {
        let bytes = fs::read(artifact)?;
        let digest = format!("{:x}", Sha256::digest(&bytes));
        output.push_str("    {\"role\": ");
        write_json_string(&mut output, role);
        output.push_str(", \"path\": ");
        write_json_string(&mut output, &artifact.display().to_string());
        output.push_str(", \"sha256\": ");
        write_json_string(&mut output, &digest);
        output.push('}');
        output.push_str(if index + 1 == artifacts.len() {
            "\n"
        } else {
            ",\n"
        });
    }
    output.push_str("  ],\n  \"evidence\": [\n");
    for (index, ((source, symbol), kind)) in evidence.iter().enumerate() {
        output.push_str("    {\"source\": ");
        write_json_string(&mut output, source);
        output.push_str(", \"symbol\": ");
        write_json_string(&mut output, symbol);
        output.push_str(", \"kind\": ");
        write_json_string(&mut output, kind);
        output.push('}');
        output.push_str(if index + 1 == evidence.len() {
            "\n"
        } else {
            ",\n"
        });
    }
    output.push_str("  ]\n}\n");
    fs::write(path, output)?;
    println!("JSON-REPORT\t{}", path.display());
    Ok(())
}

fn profile_evidence(profile: &profiles::Profile) -> String {
    // `Profile` is composed only of ordered vectors and ordered maps. Hashing
    // its parsed canonical Debug form binds evidence to every scenario input,
    // observation and response without making comments or whitespace part of
    // the contract identity. The repository pins the Rust toolchain that
    // defines this representation.
    let canonical = format!("{profile:#?}");
    format!(
        "{}/profile:{}/sha256:{:x}",
        profile.contract.evidence(),
        profile.name,
        Sha256::digest(canonical.as_bytes())
    )
}

fn semantic_contract_digest_from_sources(label: &str, sources: &[(&str, &str)]) -> String {
    let mut digest = Sha256::new();
    digest.update(b"open-esp-radio semantic contract\0");
    digest.update(label.as_bytes());
    for (name, source) in sources {
        digest.update([0]);
        digest.update(name.as_bytes());
        digest.update([0]);
        digest.update(source.as_bytes());
    }
    format!("{:x}", digest.finalize())
}

fn semantic_contract_evidence(label: &str) -> String {
    // Composition evidence must identify the validator that produced it, not
    // merely a human-maintained version label. `main.rs` owns the scenario
    // matrix and qualification wiring, `semantic.rs` owns footprints and
    // normalization, and `emulator.rs` owns execution and persistence.
    let digest = semantic_contract_digest_from_sources(
        label,
        &[
            ("main.rs", include_str!("main.rs")),
            ("semantic.rs", include_str!("semantic.rs")),
            ("emulator.rs", include_str!("emulator.rs")),
        ],
    );
    format!("composition-state-scenario/{label}/sha256:{digest}")
}

fn child_text<'a, 'input>(node: roxmltree::Node<'a, 'input>, tag: &str) -> Option<&'a str> {
    node.children()
        .find(|child| child.has_tag_name(tag))
        .and_then(|child| child.text())
        .map(str::trim)
}

fn collect_registers(
    container: roxmltree::Node<'_, '_>,
    base: u32,
    prefix: &str,
    output: &mut Vec<Register>,
) -> Result<()> {
    for node in container.children().filter(roxmltree::Node::is_element) {
        if node.has_tag_name("register") {
            let name = child_text(node, "name").ok_or("SVD register has no name")?;
            let offset = child_text(node, "addressOffset")
                .and_then(parse_u32)
                .ok_or("SVD register has no addressOffset")?;
            let dim = child_text(node, "dim").and_then(parse_u32).unwrap_or(1);
            let increment = child_text(node, "dimIncrement")
                .and_then(parse_u32)
                .unwrap_or(0);
            for index in 0..dim {
                output.push(Register {
                    address: base.wrapping_add(offset).wrapping_add(index * increment),
                    name: if dim == 1 {
                        format!("{prefix}.{name}")
                    } else {
                        format!("{prefix}.{}", name.replace("%s", &index.to_string()))
                    },
                });
            }
        } else if node.has_tag_name("cluster") {
            let name = child_text(node, "name").ok_or("SVD cluster has no name")?;
            let offset = child_text(node, "addressOffset")
                .and_then(parse_u32)
                .ok_or("SVD cluster has no addressOffset")?;
            collect_registers(
                node,
                base.wrapping_add(offset),
                &format!("{prefix}.{name}"),
                output,
            )?;
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Value {
    Unknown,
    Constant(u32),
    InputConstant {
        index: u8,
        value: u32,
    },
    StackAddress(i32),
    SymbolAddress {
        member: Option<String>,
        symbol: String,
        hi_addend: i64,
        lo_addend: Option<i64>,
        post_offset: i64,
    },
    CallResult(u32),
    ExternalTable(external_abi::Table),
    ExternalFunction {
        table: external_abi::Table,
        function: external_abi::Function,
    },
    ExternalResult(u32),
    Expression {
        operation: ExpressionOperation,
        left: Box<Value>,
        right: Box<Value>,
    },
    RegisterImage {
        read_token: u32,
        address: u32,
        and_mask: u32,
        or_mask: u32,
    },
    IndexedRegisterImage {
        read_token: u32,
        and_mask: u32,
        or_mask: u32,
    },
    MemoryImage {
        read_token: u32,
        and_mask: u32,
        or_mask: u32,
    },
    Bits(Box<[BitSource; 32]>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExpressionOperation {
    Add,
    Subtract,
    Multiply,
    DivideSigned,
    DivideUnsigned,
    RemainderSigned,
    RemainderUnsigned,
    BitAnd,
    BitOr,
    BitXor,
    ShiftLeft,
    ShiftRight,
    ShiftRightArithmetic,
    Equal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BitSource {
    Unknown,
    Constant(bool),
    Input {
        index: u8,
        bit: u8,
        inverted: bool,
    },
    Register {
        read_token: u32,
        address: u32,
        bit: u8,
        inverted: bool,
    },
    IndexedRegister {
        read_token: u32,
        bit: u8,
        inverted: bool,
    },
    Memory {
        read_token: u32,
        bit: u8,
        inverted: bool,
    },
    CallResult {
        call_token: u32,
        bit: u8,
        inverted: bool,
    },
    ExternalResult {
        call_token: u32,
        bit: u8,
        inverted: bool,
    },
}

impl BitSource {
    fn inverted(self) -> Self {
        match self {
            Self::Unknown => Self::Unknown,
            Self::Constant(value) => Self::Constant(!value),
            Self::Input {
                index,
                bit,
                inverted,
            } => Self::Input {
                index,
                bit,
                inverted: !inverted,
            },
            Self::Register {
                read_token,
                address,
                bit,
                inverted,
            } => Self::Register {
                read_token,
                address,
                bit,
                inverted: !inverted,
            },
            Self::IndexedRegister {
                read_token,
                bit,
                inverted,
            } => Self::IndexedRegister {
                read_token,
                bit,
                inverted: !inverted,
            },
            Self::Memory {
                read_token,
                bit,
                inverted,
            } => Self::Memory {
                read_token,
                bit,
                inverted: !inverted,
            },
            Self::CallResult {
                call_token,
                bit,
                inverted,
            } => Self::CallResult {
                call_token,
                bit,
                inverted: !inverted,
            },
            Self::ExternalResult {
                call_token,
                bit,
                inverted,
            } => Self::ExternalResult {
                call_token,
                bit,
                inverted: !inverted,
            },
        }
    }
}

impl Value {
    fn input(index: u8) -> Self {
        Self::Bits(Box::new(core::array::from_fn(|bit| BitSource::Input {
            index,
            bit: bit as u8,
            inverted: false,
        })))
    }

    fn bits(&self) -> [BitSource; 32] {
        match self {
            Self::Unknown => [BitSource::Unknown; 32],
            Self::Constant(value) => {
                core::array::from_fn(|bit| BitSource::Constant(value & (1 << bit) != 0))
            }
            Self::InputConstant { index, .. } => core::array::from_fn(|bit| BitSource::Input {
                index: *index,
                bit: bit as u8,
                inverted: false,
            }),
            Self::StackAddress(_)
            | Self::SymbolAddress { .. }
            | Self::ExternalTable(_)
            | Self::ExternalFunction { .. }
            | Self::Expression { .. } => [BitSource::Unknown; 32],
            Self::CallResult(call_token) => core::array::from_fn(|bit| BitSource::CallResult {
                call_token: *call_token,
                bit: bit as u8,
                inverted: false,
            }),
            Self::ExternalResult(call_token) => {
                core::array::from_fn(|bit| BitSource::ExternalResult {
                    call_token: *call_token,
                    bit: bit as u8,
                    inverted: false,
                })
            }
            Self::RegisterImage {
                read_token,
                address,
                and_mask,
                or_mask,
            } => core::array::from_fn(|bit| {
                if or_mask & (1 << bit) != 0 {
                    BitSource::Constant(true)
                } else if and_mask & (1 << bit) != 0 {
                    BitSource::Register {
                        read_token: *read_token,
                        address: *address,
                        bit: bit as u8,
                        inverted: false,
                    }
                } else {
                    BitSource::Constant(false)
                }
            }),
            Self::IndexedRegisterImage {
                read_token,
                and_mask,
                or_mask,
            } => core::array::from_fn(|bit| {
                if or_mask & (1 << bit) != 0 {
                    BitSource::Constant(true)
                } else if and_mask & (1 << bit) != 0 {
                    BitSource::IndexedRegister {
                        read_token: *read_token,
                        bit: bit as u8,
                        inverted: false,
                    }
                } else {
                    BitSource::Constant(false)
                }
            }),
            Self::MemoryImage {
                read_token,
                and_mask,
                or_mask,
            } => core::array::from_fn(|bit| {
                if or_mask & (1 << bit) != 0 {
                    BitSource::Constant(true)
                } else if and_mask & (1 << bit) != 0 {
                    BitSource::Memory {
                        read_token: *read_token,
                        bit: bit as u8,
                        inverted: false,
                    }
                } else {
                    BitSource::Constant(false)
                }
            }),
            Self::Bits(bits) => **bits,
        }
    }

    fn register_read(read_token: u32, address: u32, width: u8, signed: bool) -> Self {
        if width == 32 {
            return Self::RegisterImage {
                read_token,
                address,
                and_mask: u32::MAX,
                or_mask: 0,
            };
        }
        Self::from_bits(core::array::from_fn(|bit| {
            if bit < usize::from(width) {
                BitSource::Register {
                    read_token,
                    address,
                    bit: bit as u8,
                    inverted: false,
                }
            } else if signed {
                BitSource::Register {
                    read_token,
                    address,
                    bit: width - 1,
                    inverted: false,
                }
            } else {
                BitSource::Constant(false)
            }
        }))
    }

    fn indexed_register_read(read_token: u32, width: u8, signed: bool) -> Self {
        if width == 32 {
            return Self::IndexedRegisterImage {
                read_token,
                and_mask: u32::MAX,
                or_mask: 0,
            };
        }
        Self::from_bits(core::array::from_fn(|bit| {
            if bit < usize::from(width) {
                BitSource::IndexedRegister {
                    read_token,
                    bit: bit as u8,
                    inverted: false,
                }
            } else if signed {
                BitSource::IndexedRegister {
                    read_token,
                    bit: width - 1,
                    inverted: false,
                }
            } else {
                BitSource::Constant(false)
            }
        }))
    }

    fn memory_read(read_token: u32, width: u8, signed: bool) -> Self {
        if width == 32 {
            return Self::MemoryImage {
                read_token,
                and_mask: u32::MAX,
                or_mask: 0,
            };
        }
        Self::from_bits(core::array::from_fn(|bit| {
            if bit < usize::from(width) {
                BitSource::Memory {
                    read_token,
                    bit: bit as u8,
                    inverted: false,
                }
            } else if signed {
                BitSource::Memory {
                    read_token,
                    bit: width - 1,
                    inverted: false,
                }
            } else {
                BitSource::Constant(false)
            }
        }))
    }

    fn substitute(
        &self,
        arguments: &[Value; 8],
        read_tokens: &[u32],
        memory_read_tokens: &[u32],
        external_tokens: &[u32],
    ) -> std::result::Result<Self, String> {
        if let Some(index) = self.direct_input_index() {
            return arguments
                .get(usize::from(index))
                .cloned()
                .ok_or_else(|| format!("call argument {index} is outside the RV32 ABI"));
        }
        if let Self::SymbolAddress { lo_addend, .. } = self {
            return lo_addend
                .is_some()
                .then(|| self.clone())
                .ok_or_else(|| "incomplete relocation escaped across a call boundary".to_owned());
        }
        if let Self::Expression {
            operation,
            left,
            right,
        } = self
        {
            return Ok(Self::Expression {
                operation: *operation,
                left: Box::new(left.substitute(
                    arguments,
                    read_tokens,
                    memory_read_tokens,
                    external_tokens,
                )?),
                right: Box::new(right.substitute(
                    arguments,
                    read_tokens,
                    memory_read_tokens,
                    external_tokens,
                )?),
            });
        }
        if matches!(
            self,
            Self::StackAddress(_) | Self::ExternalTable(_) | Self::ExternalFunction { .. }
        ) {
            return Err("non-scalar value escaped across a call boundary".to_owned());
        }
        let bits = self.bits();
        let mut substituted = [BitSource::Unknown; 32];
        for (destination, source) in bits.into_iter().enumerate() {
            substituted[destination] = match source {
                BitSource::Unknown => BitSource::Unknown,
                BitSource::Constant(value) => BitSource::Constant(value),
                BitSource::Input {
                    index,
                    bit,
                    inverted,
                } => {
                    let argument = arguments
                        .get(usize::from(index))
                        .ok_or_else(|| format!("call argument {index} is outside the RV32 ABI"))?;
                    let source = argument.bits()[usize::from(bit)];
                    if inverted { source.inverted() } else { source }
                }
                BitSource::Register {
                    read_token,
                    address,
                    bit,
                    inverted,
                } => BitSource::Register {
                    read_token: *read_tokens.get(read_token as usize).ok_or_else(|| {
                        format!("callee MMIO read token {read_token} has no caller mapping")
                    })?,
                    address,
                    bit,
                    inverted,
                },
                BitSource::IndexedRegister {
                    read_token,
                    bit,
                    inverted,
                } => BitSource::IndexedRegister {
                    read_token: *read_tokens.get(read_token as usize).ok_or_else(|| {
                        format!("callee MMIO read token {read_token} has no caller mapping")
                    })?,
                    bit,
                    inverted,
                },
                BitSource::Memory {
                    read_token,
                    bit,
                    inverted,
                } => BitSource::Memory {
                    read_token: *memory_read_tokens.get(read_token as usize).ok_or_else(|| {
                        format!("callee memory read token {read_token} has no caller mapping")
                    })?,
                    bit,
                    inverted,
                },
                BitSource::CallResult {
                    call_token,
                    bit,
                    inverted,
                } => BitSource::CallResult {
                    call_token,
                    bit,
                    inverted,
                },
                BitSource::ExternalResult {
                    call_token,
                    bit,
                    inverted,
                } => BitSource::ExternalResult {
                    call_token: *external_tokens.get(call_token as usize).ok_or_else(|| {
                        format!("callee external-call token {call_token} has no caller mapping")
                    })?,
                    bit,
                    inverted,
                },
            };
        }
        Ok(Self::from_bits(substituted))
    }

    fn rewrite_call_context(
        &self,
        read_tokens: &[u32],
        memory_read_tokens: &[u32],
        external_tokens: &[u32],
        call_results: &BTreeMap<u32, Value>,
    ) -> std::result::Result<Self, String> {
        if let Self::SymbolAddress { lo_addend, .. } = self {
            return lo_addend
                .is_some()
                .then(|| self.clone())
                .ok_or_else(|| "incomplete relocation escaped across a call boundary".to_owned());
        }
        if let Self::Expression {
            operation,
            left,
            right,
        } = self
        {
            return Ok(Self::Expression {
                operation: *operation,
                left: Box::new(left.rewrite_call_context(
                    read_tokens,
                    memory_read_tokens,
                    external_tokens,
                    call_results,
                )?),
                right: Box::new(right.rewrite_call_context(
                    read_tokens,
                    memory_read_tokens,
                    external_tokens,
                    call_results,
                )?),
            });
        }
        if matches!(
            self,
            Self::StackAddress(_) | Self::ExternalTable(_) | Self::ExternalFunction { .. }
        ) {
            return Err("non-scalar value escaped across a call boundary".to_owned());
        }
        let bits = self.bits();
        let mut rewritten = [BitSource::Unknown; 32];
        for (destination, source) in bits.into_iter().enumerate() {
            rewritten[destination] = match source {
                BitSource::Unknown => BitSource::Unknown,
                BitSource::Constant(value) => BitSource::Constant(value),
                BitSource::Input {
                    index,
                    bit,
                    inverted,
                } => BitSource::Input {
                    index,
                    bit,
                    inverted,
                },
                BitSource::Register {
                    read_token,
                    address,
                    bit,
                    inverted,
                } => BitSource::Register {
                    read_token: *read_tokens.get(read_token as usize).ok_or_else(|| {
                        format!("caller MMIO read token {read_token} has no flattened mapping")
                    })?,
                    address,
                    bit,
                    inverted,
                },
                BitSource::IndexedRegister {
                    read_token,
                    bit,
                    inverted,
                } => BitSource::IndexedRegister {
                    read_token: *read_tokens.get(read_token as usize).ok_or_else(|| {
                        format!("caller MMIO read token {read_token} has no flattened mapping")
                    })?,
                    bit,
                    inverted,
                },
                BitSource::Memory {
                    read_token,
                    bit,
                    inverted,
                } => BitSource::Memory {
                    read_token: *memory_read_tokens.get(read_token as usize).ok_or_else(|| {
                        format!("caller memory read token {read_token} has no flattened mapping")
                    })?,
                    bit,
                    inverted,
                },
                BitSource::CallResult {
                    call_token,
                    bit,
                    inverted,
                } => {
                    let result = call_results
                        .get(&call_token)
                        .ok_or_else(|| format!("call result {call_token} is not available"))?;
                    let source = result.bits()[usize::from(bit)];
                    if inverted { source.inverted() } else { source }
                }
                BitSource::ExternalResult {
                    call_token,
                    bit,
                    inverted,
                } => BitSource::ExternalResult {
                    call_token: *external_tokens.get(call_token as usize).ok_or_else(|| {
                        format!("caller external-call token {call_token} has no flattened mapping")
                    })?,
                    bit,
                    inverted,
                },
            };
        }
        Ok(Self::from_bits(rewritten))
    }

    fn from_bits(bits: [BitSource; 32]) -> Self {
        let mut constant = 0_u32;
        let mut all_constant = true;
        for (bit, source) in bits.iter().enumerate() {
            match source {
                BitSource::Constant(true) => constant |= 1 << bit,
                BitSource::Constant(false) => {}
                _ => all_constant = false,
            }
        }
        if all_constant {
            return Self::Constant(constant);
        }

        let register = bits.iter().find_map(|source| match source {
            BitSource::Register {
                read_token,
                address,
                ..
            } => Some((*read_token, *address)),
            _ => None,
        });
        if let Some((read_token, address)) = register {
            let mut and_mask = 0_u32;
            let mut or_mask = 0_u32;
            let mut register_image = true;
            for (bit, source) in bits.iter().enumerate() {
                match source {
                    BitSource::Register {
                        read_token: source_token,
                        address: source_address,
                        bit: source_bit,
                        inverted: false,
                    } if *source_token == read_token
                        && *source_address == address
                        && usize::from(*source_bit) == bit =>
                    {
                        and_mask |= 1 << bit;
                    }
                    BitSource::Constant(true) => or_mask |= 1 << bit,
                    BitSource::Constant(false) => {}
                    _ => register_image = false,
                }
            }
            if register_image {
                return Self::RegisterImage {
                    read_token,
                    address,
                    and_mask,
                    or_mask,
                };
            }
        }

        let indexed_register = bits.iter().find_map(|source| match source {
            BitSource::IndexedRegister { read_token, .. } => Some(*read_token),
            _ => None,
        });
        if let Some(read_token) = indexed_register {
            let mut and_mask = 0_u32;
            let mut or_mask = 0_u32;
            let mut register_image = true;
            for (bit, source) in bits.iter().enumerate() {
                match source {
                    BitSource::IndexedRegister {
                        read_token: source_token,
                        bit: source_bit,
                        inverted: false,
                    } if *source_token == read_token && usize::from(*source_bit) == bit => {
                        and_mask |= 1 << bit;
                    }
                    BitSource::Constant(true) => or_mask |= 1 << bit,
                    BitSource::Constant(false) => {}
                    _ => register_image = false,
                }
            }
            if register_image {
                return Self::IndexedRegisterImage {
                    read_token,
                    and_mask,
                    or_mask,
                };
            }
        }

        let memory = bits.iter().find_map(|source| match source {
            BitSource::Memory { read_token, .. } => Some(*read_token),
            _ => None,
        });
        if let Some(read_token) = memory {
            let mut and_mask = 0_u32;
            let mut or_mask = 0_u32;
            let mut memory_image = true;
            for (bit, source) in bits.iter().enumerate() {
                match source {
                    BitSource::Memory {
                        read_token: source_token,
                        bit: source_bit,
                        inverted: false,
                    } if *source_token == read_token && usize::from(*source_bit) == bit => {
                        and_mask |= 1 << bit;
                    }
                    BitSource::Constant(true) => or_mask |= 1 << bit,
                    BitSource::Constant(false) => {}
                    _ => memory_image = false,
                }
            }
            if memory_image {
                return Self::MemoryImage {
                    read_token,
                    and_mask,
                    or_mask,
                };
            }
        }
        Self::Bits(Box::new(bits))
    }

    fn and(self, constant: u32) -> Self {
        if matches!(&self, Self::Expression { .. }) {
            return Self::expression(ExpressionOperation::BitAnd, self, Self::Constant(constant));
        }
        Self::from_bits(core::array::from_fn(|bit| {
            if constant & (1 << bit) == 0 {
                BitSource::Constant(false)
            } else {
                self.bits()[bit]
            }
        }))
    }

    fn or(self, constant: u32) -> Self {
        if matches!(&self, Self::Expression { .. }) {
            return Self::expression(ExpressionOperation::BitOr, self, Self::Constant(constant));
        }
        Self::from_bits(core::array::from_fn(|bit| {
            if constant & (1 << bit) != 0 {
                BitSource::Constant(true)
            } else {
                self.bits()[bit]
            }
        }))
    }

    fn bitand(self, other: Self) -> Self {
        if let Some(constant) = self.as_constant() {
            return other.and(constant);
        }
        if let Some(constant) = other.as_constant() {
            return self.and(constant);
        }
        if matches!(&self, Self::Expression { .. }) || matches!(&other, Self::Expression { .. }) {
            return Self::expression(ExpressionOperation::BitAnd, self, other);
        }
        let left = self.bits();
        let right = other.bits();
        let simplified =
            Self::from_bits(core::array::from_fn(|bit| match (left[bit], right[bit]) {
                (BitSource::Constant(false), _) | (_, BitSource::Constant(false)) => {
                    BitSource::Constant(false)
                }
                (BitSource::Constant(true), source) | (source, BitSource::Constant(true)) => source,
                (left, right) if left == right => left,
                _ => BitSource::Unknown,
            }));
        if simplified.is_resolved() {
            simplified
        } else {
            Self::expression(ExpressionOperation::BitAnd, self, other)
        }
    }

    fn bitor(self, other: Self) -> Self {
        if let Some(constant) = self.as_constant() {
            return other.or(constant);
        }
        if let Some(constant) = other.as_constant() {
            return self.or(constant);
        }
        if matches!(&self, Self::Expression { .. }) || matches!(&other, Self::Expression { .. }) {
            return Self::expression(ExpressionOperation::BitOr, self, other);
        }
        let left = self.bits();
        let right = other.bits();
        let simplified =
            Self::from_bits(core::array::from_fn(|bit| match (left[bit], right[bit]) {
                (BitSource::Constant(true), _) | (_, BitSource::Constant(true)) => {
                    BitSource::Constant(true)
                }
                (BitSource::Constant(false), source) | (source, BitSource::Constant(false)) => {
                    source
                }
                (left, right) if left == right => left,
                _ => BitSource::Unknown,
            }));
        if simplified.is_resolved() {
            simplified
        } else {
            Self::expression(ExpressionOperation::BitOr, self, other)
        }
    }

    fn shift_left(self, amount: u32) -> Self {
        if matches!(&self, Self::Expression { .. }) {
            return Self::expression(ExpressionOperation::ShiftLeft, self, Self::Constant(amount));
        }
        let source = self.bits();
        Self::from_bits(core::array::from_fn(|bit| {
            bit.checked_sub(amount as usize)
                .map_or(BitSource::Constant(false), |source_bit| source[source_bit])
        }))
    }

    fn shift_right(self, amount: u32) -> Self {
        if matches!(&self, Self::Expression { .. }) {
            return Self::expression(
                ExpressionOperation::ShiftRight,
                self,
                Self::Constant(amount),
            );
        }
        let source = self.bits();
        Self::from_bits(core::array::from_fn(|bit| {
            source
                .get(bit + amount as usize)
                .copied()
                .unwrap_or(BitSource::Constant(false))
        }))
    }

    fn add_constant(self, constant: u32) -> Self {
        if constant == 0 {
            return self;
        }
        if let Self::Constant(value) = self {
            return Self::Constant(value.wrapping_add(constant));
        }
        if let Self::StackAddress(offset) = self {
            return Self::StackAddress(offset.wrapping_add(constant as i32));
        }
        if let Self::SymbolAddress {
            member,
            symbol,
            hi_addend,
            lo_addend,
            post_offset,
        } = self
        {
            return Self::SymbolAddress {
                member,
                symbol,
                hi_addend,
                lo_addend,
                post_offset: post_offset.wrapping_add(i64::from(constant as i32)),
            };
        }
        Self::expression(ExpressionOperation::Add, self, Self::Constant(constant))
    }

    fn direct_input_index(&self) -> Option<u8> {
        if let Self::InputConstant { index, .. } = self {
            return Some(*index);
        }
        let Self::Bits(bits) = self else {
            return None;
        };
        let mut index = None;
        for (destination, source) in bits.iter().enumerate() {
            let BitSource::Input {
                index: source_index,
                bit,
                inverted: false,
            } = source
            else {
                return None;
            };
            if usize::from(*bit) != destination {
                return None;
            }
            match index {
                Some(index) if index != *source_index => return None,
                Some(_) => {}
                None => index = Some(*source_index),
            }
        }
        index
    }

    fn caller_memory_address(&self) -> bool {
        if self.direct_input_index().is_some() {
            return true;
        }
        match self {
            Self::Expression {
                operation: ExpressionOperation::Add,
                left,
                right,
            } => {
                (left.caller_memory_address() && matches!(right.as_ref(), Self::Constant(_)))
                    || (right.caller_memory_address() && matches!(left.as_ref(), Self::Constant(_)))
            }
            Self::Expression {
                operation: ExpressionOperation::Subtract,
                left,
                right,
            } => left.caller_memory_address() && matches!(right.as_ref(), Self::Constant(_)),
            _ => false,
        }
    }

    #[cfg(test)]
    fn not(self) -> Self {
        Self::from_bits(self.bits().map(|source| match source {
            BitSource::Constant(value) => BitSource::Constant(!value),
            BitSource::Input {
                index,
                bit,
                inverted,
            } => BitSource::Input {
                index,
                bit,
                inverted: !inverted,
            },
            BitSource::Register {
                read_token,
                address,
                bit,
                inverted,
            } => BitSource::Register {
                read_token,
                address,
                bit,
                inverted: !inverted,
            },
            BitSource::IndexedRegister {
                read_token,
                bit,
                inverted,
            } => BitSource::IndexedRegister {
                read_token,
                bit,
                inverted: !inverted,
            },
            BitSource::Memory {
                read_token,
                bit,
                inverted,
            } => BitSource::Memory {
                read_token,
                bit,
                inverted: !inverted,
            },
            BitSource::CallResult {
                call_token,
                bit,
                inverted,
            } => BitSource::CallResult {
                call_token,
                bit,
                inverted: !inverted,
            },
            BitSource::ExternalResult {
                call_token,
                bit,
                inverted,
            } => BitSource::ExternalResult {
                call_token,
                bit,
                inverted: !inverted,
            },
            BitSource::Unknown => BitSource::Unknown,
        }))
    }

    fn xor(self, constant: u32) -> Self {
        if matches!(&self, Self::Expression { .. }) {
            return Self::expression(ExpressionOperation::BitXor, self, Self::Constant(constant));
        }
        let bits = self.bits();
        Self::from_bits(core::array::from_fn(|bit| {
            if constant & (1 << bit) == 0 {
                bits[bit]
            } else {
                match bits[bit] {
                    BitSource::Constant(value) => BitSource::Constant(!value),
                    BitSource::Input {
                        index,
                        bit,
                        inverted,
                    } => BitSource::Input {
                        index,
                        bit,
                        inverted: !inverted,
                    },
                    BitSource::Register {
                        read_token,
                        address,
                        bit,
                        inverted,
                    } => BitSource::Register {
                        read_token,
                        address,
                        bit,
                        inverted: !inverted,
                    },
                    BitSource::IndexedRegister {
                        read_token,
                        bit,
                        inverted,
                    } => BitSource::IndexedRegister {
                        read_token,
                        bit,
                        inverted: !inverted,
                    },
                    BitSource::Memory {
                        read_token,
                        bit,
                        inverted,
                    } => BitSource::Memory {
                        read_token,
                        bit,
                        inverted: !inverted,
                    },
                    BitSource::CallResult {
                        call_token,
                        bit,
                        inverted,
                    } => BitSource::CallResult {
                        call_token,
                        bit,
                        inverted: !inverted,
                    },
                    BitSource::ExternalResult {
                        call_token,
                        bit,
                        inverted,
                    } => BitSource::ExternalResult {
                        call_token,
                        bit,
                        inverted: !inverted,
                    },
                    BitSource::Unknown => BitSource::Unknown,
                }
            }
        }))
    }

    fn bitxor(self, other: Self) -> Self {
        if self == other {
            return Self::Constant(0);
        }
        match (self, other) {
            (Self::Constant(constant), value) | (value, Self::Constant(constant)) => {
                value.xor(constant)
            }
            (left, right) => Self::expression(ExpressionOperation::BitXor, left, right),
        }
    }

    fn expression(operation: ExpressionOperation, left: Self, right: Self) -> Self {
        if !left.is_resolved() || !right.is_resolved() {
            return Self::Unknown;
        }
        Self::Expression {
            operation,
            left: Box::new(left),
            right: Box::new(right),
        }
    }

    fn as_constant(&self) -> Option<u32> {
        match self {
            Self::Constant(value) => Some(*value),
            Self::InputConstant { value, .. } => Some(*value),
            _ => None,
        }
    }

    fn seqz(self) -> Self {
        if let Self::Constant(value) = &self {
            return Self::Constant((*value == 0) as u32);
        }
        let mut nonzero = self
            .bits()
            .into_iter()
            .filter(|source| *source != BitSource::Constant(false));
        let source = nonzero.next();
        if nonzero.next().is_some() {
            return Self::expression(ExpressionOperation::Equal, self, Self::Constant(0));
        }
        let inverse = match source {
            None => BitSource::Constant(true),
            Some(BitSource::Constant(true)) => BitSource::Constant(false),
            Some(BitSource::Input {
                index,
                bit,
                inverted,
            }) => BitSource::Input {
                index,
                bit,
                inverted: !inverted,
            },
            Some(BitSource::Register {
                read_token,
                address,
                bit,
                inverted,
            }) => BitSource::Register {
                read_token,
                address,
                bit,
                inverted: !inverted,
            },
            Some(BitSource::IndexedRegister {
                read_token,
                bit,
                inverted,
            }) => BitSource::IndexedRegister {
                read_token,
                bit,
                inverted: !inverted,
            },
            Some(BitSource::Memory {
                read_token,
                bit,
                inverted,
            }) => BitSource::Memory {
                read_token,
                bit,
                inverted: !inverted,
            },
            Some(BitSource::CallResult {
                call_token,
                bit,
                inverted,
            }) => BitSource::CallResult {
                call_token,
                bit,
                inverted: !inverted,
            },
            Some(BitSource::ExternalResult {
                call_token,
                bit,
                inverted,
            }) => BitSource::ExternalResult {
                call_token,
                bit,
                inverted: !inverted,
            },
            Some(BitSource::Unknown) => BitSource::Unknown,
            Some(BitSource::Constant(false)) => unreachable!(),
        };
        Self::from_bits(core::array::from_fn(|bit| {
            if bit == 0 {
                inverse
            } else {
                BitSource::Constant(false)
            }
        }))
    }

    fn is_resolved(&self) -> bool {
        match self {
            Self::Expression { left, right, .. } => left.is_resolved() && right.is_resolved(),
            Self::SymbolAddress { lo_addend, .. } => lo_addend.is_some(),
            Self::ExternalResult(_) => true,
            Self::ExternalTable(_) | Self::ExternalFunction { .. } | Self::StackAddress(_) => false,
            _ => !matches!(self, Self::Unknown) && !self.bits().contains(&BitSource::Unknown),
        }
    }

    fn canonical(&self) -> String {
        match self {
            Self::Unknown => "unknown".to_owned(),
            Self::Constant(value) => format!("const:{value:#010x}"),
            Self::InputConstant { index, .. } => Self::input(*index).canonical(),
            Self::StackAddress(offset) => format!("private-stack:{offset:+#x}"),
            Self::SymbolAddress {
                member,
                symbol,
                hi_addend,
                lo_addend,
                post_offset,
            } => format!(
                "symbol:{}::{symbol}:hi{hi_addend:+#x}:lo{}:post{post_offset:+#x}",
                member.as_deref().unwrap_or("<linked>"),
                lo_addend.map_or_else(|| "?".to_owned(), |addend| format!("{addend:+#x}"))
            ),
            Self::CallResult(call_token) => format!("call-result:{call_token}"),
            Self::ExternalTable(table) => {
                format!("external-table:{}", external_abi::table_spec(*table).id)
            }
            Self::ExternalFunction { table, function } => format!(
                "external-function:{}::{function:?}",
                external_abi::table_spec(*table).id
            ),
            Self::ExternalResult(call_token) => format!("external-result:{call_token}"),
            Self::Expression {
                operation,
                left,
                right,
            } => format!(
                "expr:{operation:?}({},{})",
                left.canonical(),
                right.canonical()
            ),
            Self::RegisterImage {
                read_token,
                address,
                and_mask,
                or_mask,
            } => format!("rmw:read{read_token}[{address:#010x}]&{and_mask:#010x}|{or_mask:#010x}"),
            Self::IndexedRegisterImage {
                read_token,
                and_mask,
                or_mask,
            } => format!("indexed-rmw:read{read_token}&{and_mask:#010x}|{or_mask:#010x}"),
            Self::MemoryImage {
                read_token,
                and_mask,
                or_mask,
            } => format!("ram:read{read_token}&{and_mask:#010x}|{or_mask:#010x}"),
            Self::Bits(bits) => {
                let terms = bits
                    .iter()
                    .enumerate()
                    .filter_map(|(bit, source)| match source {
                        BitSource::Constant(false) => None,
                        BitSource::Constant(true) => Some(format!("{bit}=1")),
                        BitSource::Input {
                            index,
                            bit: source,
                            inverted,
                        } => {
                            let inverse = if *inverted { "!" } else { "" };
                            Some(format!("{bit}={inverse}arg{index}.{source}"))
                        }
                        BitSource::Register {
                            read_token,
                            address,
                            bit: source,
                            inverted,
                        } => {
                            let inverse = if *inverted { "!" } else { "" };
                            Some(format!(
                                "{bit}={inverse}read{read_token}[{address:#010x}].{source}"
                            ))
                        }
                        BitSource::IndexedRegister {
                            read_token,
                            bit: source,
                            inverted,
                        } => {
                            let inverse = if *inverted { "!" } else { "" };
                            Some(format!("{bit}={inverse}indexed-read{read_token}.{source}"))
                        }
                        BitSource::Memory {
                            read_token,
                            bit: source,
                            inverted,
                        } => {
                            let inverse = if *inverted { "!" } else { "" };
                            Some(format!("{bit}={inverse}ramread{read_token}.{source}"))
                        }
                        BitSource::CallResult {
                            call_token,
                            bit: source,
                            inverted,
                        } => {
                            let inverse = if *inverted { "!" } else { "" };
                            Some(format!("{bit}={inverse}call{call_token}.a0.{source}"))
                        }
                        BitSource::ExternalResult {
                            call_token,
                            bit: source,
                            inverted,
                        } => {
                            let inverse = if *inverted { "!" } else { "" };
                            Some(format!("{bit}={inverse}external{call_token}.{source}"))
                        }
                        BitSource::Unknown => Some(format!("{bit}=?")),
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                format!("bits:{terms}")
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct AffineInput {
    index: Option<u8>,
    scale: u32,
    offset: u32,
}

fn merge_affine_input(left: Option<u8>, right: Option<u8>) -> Option<Option<u8>> {
    match (left, right) {
        (Some(left), Some(right)) if left != right => None,
        (Some(index), _) | (_, Some(index)) => Some(Some(index)),
        (None, None) => Some(None),
    }
}

fn affine_input(value: &Value) -> Option<AffineInput> {
    match value {
        Value::Constant(value) => Some(AffineInput {
            index: None,
            scale: 0,
            offset: *value,
        }),
        Value::InputConstant { index, .. } => Some(AffineInput {
            index: Some(*index),
            scale: 1,
            offset: 0,
        }),
        Value::Bits(bits) => {
            let first_input = bits.iter().find_map(|source| match source {
                BitSource::Input { index, .. } => Some(*index),
                _ => None,
            })?;
            for shift in 0..32_usize {
                let matches = bits.iter().enumerate().all(|(destination, source)| {
                    if destination < shift {
                        *source == BitSource::Constant(false)
                    } else {
                        *source
                            == BitSource::Input {
                                index: first_input,
                                bit: (destination - shift) as u8,
                                inverted: false,
                            }
                    }
                });
                if matches {
                    return Some(AffineInput {
                        index: Some(first_input),
                        scale: 1_u32 << shift,
                        offset: 0,
                    });
                }
            }
            None
        }
        Value::Expression {
            operation,
            left,
            right,
        } => {
            let left = affine_input(left)?;
            let right = affine_input(right)?;
            match operation {
                ExpressionOperation::Add | ExpressionOperation::Subtract => {
                    let index = merge_affine_input(left.index, right.index)?;
                    let (scale, offset) = if *operation == ExpressionOperation::Add {
                        (
                            left.scale.wrapping_add(right.scale),
                            left.offset.wrapping_add(right.offset),
                        )
                    } else {
                        (
                            left.scale.wrapping_sub(right.scale),
                            left.offset.wrapping_sub(right.offset),
                        )
                    };
                    Some(AffineInput {
                        index,
                        scale,
                        offset,
                    })
                }
                ExpressionOperation::Multiply if left.index.is_none() => Some(AffineInput {
                    index: right.index,
                    scale: right.scale.wrapping_mul(left.offset),
                    offset: right.offset.wrapping_mul(left.offset),
                }),
                ExpressionOperation::Multiply if right.index.is_none() => Some(AffineInput {
                    index: left.index,
                    scale: left.scale.wrapping_mul(right.offset),
                    offset: left.offset.wrapping_mul(right.offset),
                }),
                ExpressionOperation::ShiftLeft if right.index.is_none() => {
                    let shift = right.offset & 31;
                    Some(AffineInput {
                        index: left.index,
                        scale: left.scale.wrapping_shl(shift),
                        offset: left.offset.wrapping_shl(shift),
                    })
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn collect_evaluable_input_bits(
    value: &Value,
    index: &mut Option<u8>,
    bits: &mut BTreeSet<u8>,
) -> bool {
    match value {
        Value::Constant(_) => true,
        Value::InputConstant {
            index: source_index,
            ..
        } => {
            if index.is_some_and(|index| index != *source_index) {
                return false;
            }
            *index = Some(*source_index);
            bits.extend(0..32);
            true
        }
        Value::Expression { left, right, .. } => {
            collect_evaluable_input_bits(left, index, bits)
                && collect_evaluable_input_bits(right, index, bits)
        }
        Value::Bits(sources) => sources.iter().all(|source| match source {
            BitSource::Constant(_) => true,
            BitSource::Input {
                index: source_index,
                bit,
                ..
            } => {
                if index.is_some_and(|index| index != *source_index) {
                    return false;
                }
                *index = Some(*source_index);
                bits.insert(*bit);
                true
            }
            _ => false,
        }),
        _ => false,
    }
}

fn evaluate_for_input(value: &Value, input_index: u8, input: u32) -> Option<u32> {
    match value {
        Value::Constant(value) => Some(*value),
        Value::InputConstant { index, .. } if *index == input_index => Some(input),
        Value::Expression {
            operation,
            left,
            right,
        } => {
            let left = evaluate_for_input(left, input_index, input)?;
            let right = evaluate_for_input(right, input_index, input)?;
            Some(match operation {
                ExpressionOperation::Add => left.wrapping_add(right),
                ExpressionOperation::Subtract => left.wrapping_sub(right),
                ExpressionOperation::Multiply => left.wrapping_mul(right),
                ExpressionOperation::DivideSigned => {
                    let (left, right) = (left as i32, right as i32);
                    if right == 0 {
                        u32::MAX
                    } else if left == i32::MIN && right == -1 {
                        i32::MIN as u32
                    } else {
                        left.wrapping_div(right) as u32
                    }
                }
                ExpressionOperation::DivideUnsigned => left.checked_div(right).unwrap_or(u32::MAX),
                ExpressionOperation::RemainderSigned => {
                    let (left, right) = (left as i32, right as i32);
                    if right == 0 {
                        left as u32
                    } else if left == i32::MIN && right == -1 {
                        0
                    } else {
                        left.wrapping_rem(right) as u32
                    }
                }
                ExpressionOperation::RemainderUnsigned => left.checked_rem(right).unwrap_or(left),
                ExpressionOperation::BitAnd => left & right,
                ExpressionOperation::BitOr => left | right,
                ExpressionOperation::BitXor => left ^ right,
                ExpressionOperation::ShiftLeft => left.wrapping_shl(right & 31),
                ExpressionOperation::ShiftRight => left.wrapping_shr(right & 31),
                ExpressionOperation::ShiftRightArithmetic => {
                    (left as i32).wrapping_shr(right & 31) as u32
                }
                ExpressionOperation::Equal => u32::from(left == right),
            })
        }
        Value::Bits(sources) => {
            let mut output = 0_u32;
            for (destination, source) in sources.iter().enumerate() {
                let bit = match source {
                    BitSource::Constant(value) => *value,
                    BitSource::Input {
                        index,
                        bit,
                        inverted,
                    } if *index == input_index => ((input >> bit) & 1 != 0) ^ *inverted,
                    _ => return None,
                };
                output |= u32::from(bit) << destination;
            }
            Some(output)
        }
        _ => None,
    }
}

fn register_family(name: &str) -> String {
    let mut output = String::with_capacity(name.len());
    let mut in_digits = false;
    for character in name.chars() {
        if character.is_ascii_digit() {
            if !in_digits {
                output.push('%');
                in_digits = true;
            }
        } else {
            in_digits = false;
            output.push(character);
        }
    }
    output
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct IndexedMmioRegister {
    address: u32,
    name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct IndexedMmioGuard {
    selector: Value,
    maximum: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct IndexedMmioDomain {
    registers: Vec<IndexedMmioRegister>,
    guard: Option<IndexedMmioGuard>,
}

fn indexed_mmio_domain(address: &Value, svd: &SvdMap) -> Option<IndexedMmioDomain> {
    const MAX_EXHAUSTIVE_INPUT_BITS: usize = 8;
    const MAX_GUARDED_REGISTERS: u32 = 32;

    let mut input_index = None;
    let mut input_bits = BTreeSet::new();
    if !collect_evaluable_input_bits(address, &mut input_index, &mut input_bits) {
        return None;
    }
    let input_index = input_index?;

    if input_bits.len() <= MAX_EXHAUSTIVE_INPUT_BITS {
        let input_bits = input_bits.into_iter().collect::<Vec<_>>();
        let mut registers = BTreeMap::<u32, String>::new();
        let mut family = None;
        for combination in 0..(1_u32 << input_bits.len()) {
            let input =
                input_bits
                    .iter()
                    .enumerate()
                    .fold(0_u32, |value, (source, destination)| {
                        value | (((combination >> source) & 1) << destination)
                    });
            let address = evaluate_for_input(address, input_index, input)?;
            let register = svd.register(address)?;
            let register_family = register_family(&register.name);
            if family
                .as_ref()
                .is_some_and(|family| family != &register_family)
            {
                return None;
            }
            family = Some(register_family);
            registers.insert(register.address, register.name.clone());
        }
        if registers.len() >= 2 {
            return Some(IndexedMmioDomain {
                registers: registers
                    .into_iter()
                    .map(|(address, name)| IndexedMmioRegister { address, name })
                    .collect(),
                guard: None,
            });
        }
    }

    let affine = affine_input(address)?;
    if affine.index != Some(input_index) || affine.scale == 0 {
        return None;
    }
    let mut registers = Vec::new();
    let mut family = None;
    for selector in 0..=MAX_GUARDED_REGISTERS {
        let candidate_address = evaluate_for_input(address, input_index, selector)?;
        let Some(register) = svd.register(candidate_address) else {
            break;
        };
        let register_family = register_family(&register.name);
        if family
            .as_ref()
            .is_some_and(|family| family != &register_family)
        {
            break;
        }
        family = Some(register_family);
        if registers
            .iter()
            .any(|candidate: &IndexedMmioRegister| candidate.address == register.address)
        {
            return None;
        }
        registers.push(IndexedMmioRegister {
            address: register.address,
            name: register.name.clone(),
        });
    }
    if !(2..=MAX_GUARDED_REGISTERS as usize).contains(&registers.len()) {
        return None;
    }
    Some(IndexedMmioDomain {
        guard: Some(IndexedMmioGuard {
            selector: Value::input(input_index),
            maximum: registers.len() as u32 - 1,
        }),
        registers,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Access {
    Read,
    Write,
}

fn encode_fence_set(set: rv_asm::FenceSet) -> u8 {
    u8::from(set.device_input) << 3
        | u8::from(set.device_output) << 2
        | u8::from(set.memory_read) << 1
        | u8::from(set.memory_write)
}

#[cfg(test)]
fn parse_fence_set(value: &str) -> Option<u8> {
    let mut encoded = 0_u8;
    for character in value.chars() {
        encoded |= match character.to_ascii_lowercase() {
            'i' => 1 << 3,
            'o' => 1 << 2,
            'r' => 1 << 1,
            'w' => 1,
            _ => return None,
        };
    }
    Some(encoded)
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Event {
    Memory {
        access: Access,
        width: u8,
        address: u32,
        register: String,
        value: Option<Value>,
    },
    Fence {
        fm: u8,
        predecessor: u8,
        successor: u8,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ReferenceEvent {
    Observable(Event),
    IndexedMmio {
        access: Access,
        width: u8,
        address: Value,
        registers: Vec<IndexedMmioRegister>,
        guard: Option<IndexedMmioGuard>,
        value: Option<Value>,
    },
    DelayMicros {
        micros: Value,
    },
    Memory {
        access: Access,
        width: u8,
        address: Value,
        region: String,
        value: Option<Value>,
    },
    ExternalCall {
        token: u32,
        table: external_abi::Table,
        function: external_abi::Function,
        arguments: Box<[Value; 8]>,
    },
    DiagnosticCall {
        function: String,
        argument_count: u8,
        arguments: Box<[Value; 8]>,
    },
    TailCall {
        token: u32,
        site: u32,
        target: u32,
        arguments: Box<[Value; 8]>,
    },
    Call {
        token: u32,
        site: u32,
        target: u32,
        arguments: Box<[Value; 8]>,
    },
    ComposedCall {
        token: u32,
        symbol: String,
        arguments: Box<[Value; 8]>,
        flow: Box<ReferenceFlow>,
        result_modeled: bool,
    },
    BranchDecision {
        condition: BranchCondition,
        taken: bool,
    },
}

fn reference_event_is_mmio_read(event: &ReferenceEvent) -> bool {
    matches!(
        event,
        ReferenceEvent::Observable(Event::Memory {
            access: Access::Read,
            ..
        }) | ReferenceEvent::IndexedMmio {
            access: Access::Read,
            ..
        }
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BranchOperation {
    Equal,
    NotEqual,
    LessSigned,
    GreaterEqualSigned,
    LessUnsigned,
    GreaterEqualUnsigned,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct BranchCondition {
    site: u32,
    operation: BranchOperation,
    left: Value,
    right: Value,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ReferenceTerminator {
    Return(Value),
    Branch {
        condition: BranchCondition,
        taken: Box<ReferenceFlow>,
        not_taken: Box<ReferenceFlow>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ReferenceFlow {
    events: Vec<ReferenceEvent>,
    terminator: ReferenceTerminator,
}

fn collect_value_inputs(value: &Value, output: &mut BTreeSet<u8>) {
    match value {
        Value::InputConstant { index, .. } => {
            output.insert(*index);
        }
        Value::Expression { left, right, .. } => {
            collect_value_inputs(left, output);
            collect_value_inputs(right, output);
        }
        Value::Bits(bits) => {
            output.extend(bits.iter().filter_map(|source| match source {
                BitSource::Input { index, .. } => Some(*index),
                _ => None,
            }));
        }
        _ => {}
    }
}

fn collect_reference_flow_inputs(flow: &ReferenceFlow, output: &mut BTreeSet<u8>) {
    for event in &flow.events {
        match event {
            ReferenceEvent::Observable(Event::Memory {
                value: Some(value), ..
            }) => collect_value_inputs(value, output),
            ReferenceEvent::IndexedMmio {
                address,
                guard,
                value,
                ..
            } => {
                collect_value_inputs(address, output);
                if let Some(guard) = guard {
                    collect_value_inputs(&guard.selector, output);
                }
                if let Some(value) = value {
                    collect_value_inputs(value, output);
                }
            }
            ReferenceEvent::Memory { address, value, .. } => {
                collect_value_inputs(address, output);
                if let Some(value) = value {
                    collect_value_inputs(value, output);
                }
            }
            ReferenceEvent::DelayMicros { micros } => collect_value_inputs(micros, output),
            ReferenceEvent::ExternalCall {
                table,
                function,
                arguments,
                ..
            } => {
                let argument_count = external_abi::function(*table, *function).argument_count;
                for value in arguments.iter().take(usize::from(argument_count)) {
                    collect_value_inputs(value, output);
                }
            }
            ReferenceEvent::DiagnosticCall {
                argument_count,
                arguments,
                ..
            } => {
                for value in arguments.iter().take(usize::from(*argument_count)) {
                    collect_value_inputs(value, output);
                }
            }
            ReferenceEvent::ComposedCall {
                arguments, flow, ..
            } => {
                for index in reference_flow_input_indices(flow) {
                    collect_value_inputs(&arguments[usize::from(index)], output);
                }
            }
            _ => {}
        }
    }
    match &flow.terminator {
        ReferenceTerminator::Return(value) => collect_value_inputs(value, output),
        ReferenceTerminator::Branch {
            condition,
            taken,
            not_taken,
        } => {
            collect_value_inputs(&condition.left, output);
            collect_value_inputs(&condition.right, output);
            collect_reference_flow_inputs(taken, output);
            collect_reference_flow_inputs(not_taken, output);
        }
    }
}

fn reference_flow_input_indices(flow: &ReferenceFlow) -> BTreeSet<u8> {
    let mut output = BTreeSet::new();
    collect_reference_flow_inputs(flow, &mut output);
    output
}

fn reference_flow_exit_modeled(flow: &ReferenceFlow) -> bool {
    reference_flow_exit_modeled_with_calls(flow, BTreeMap::new())
}

fn reference_flow_exit_modeled_with_calls(
    flow: &ReferenceFlow,
    available: BTreeMap<u32, bool>,
) -> bool {
    let Some(available) = validate_reference_events(&flow.events, available) else {
        return false;
    };
    match &flow.terminator {
        ReferenceTerminator::Return(value) => {
            value.is_resolved() && value_call_results_available(value, &available)
        }
        ReferenceTerminator::Branch {
            condition,
            taken,
            not_taken,
        } => {
            value_call_results_available(&condition.left, &available)
                && value_call_results_available(&condition.right, &available)
                && reference_flow_exit_modeled_with_calls(taken, available.clone())
                && reference_flow_exit_modeled_with_calls(not_taken, available)
        }
    }
}

fn value_call_results_available(value: &Value, available: &BTreeMap<u32, bool>) -> bool {
    match value {
        Value::CallResult(token) => available.get(token).copied() == Some(true),
        Value::Expression { left, right, .. } => {
            value_call_results_available(left, available)
                && value_call_results_available(right, available)
        }
        Value::Bits(bits) => bits.iter().all(|source| match source {
            BitSource::CallResult { call_token, .. } => {
                available.get(call_token).copied() == Some(true)
            }
            _ => true,
        }),
        _ => true,
    }
}

fn validate_reference_events(
    events: &[ReferenceEvent],
    mut available: BTreeMap<u32, bool>,
) -> Option<BTreeMap<u32, bool>> {
    for event in events {
        let values_are_available = match event {
            ReferenceEvent::Observable(Event::Memory {
                value: Some(value), ..
            }) => value_call_results_available(value, &available),
            ReferenceEvent::IndexedMmio {
                address,
                guard,
                value,
                ..
            } => {
                value_call_results_available(address, &available)
                    && guard.as_ref().is_none_or(|guard| {
                        value_call_results_available(&guard.selector, &available)
                    })
                    && value
                        .as_ref()
                        .is_none_or(|value| value_call_results_available(value, &available))
            }
            ReferenceEvent::Memory { address, value, .. } => {
                value_call_results_available(address, &available)
                    && value
                        .as_ref()
                        .is_none_or(|value| value_call_results_available(value, &available))
            }
            ReferenceEvent::DelayMicros { micros } => {
                value_call_results_available(micros, &available)
            }
            ReferenceEvent::ExternalCall {
                table,
                function,
                arguments,
                ..
            } => arguments
                .iter()
                .take(usize::from(
                    external_abi::function(*table, *function).argument_count,
                ))
                .all(|value| value_call_results_available(value, &available)),
            ReferenceEvent::DiagnosticCall {
                argument_count,
                arguments,
                ..
            } => arguments
                .iter()
                .take(usize::from(*argument_count))
                .all(|value| value_call_results_available(value, &available)),
            ReferenceEvent::ComposedCall {
                token,
                arguments,
                flow,
                result_modeled,
                ..
            } => {
                let used_inputs = reference_flow_input_indices(flow);
                if *token != available.len() as u32
                    || used_inputs.iter().any(|index| {
                        !value_call_results_available(&arguments[usize::from(*index)], &available)
                    })
                    || !reference_flow_calls_are_valid(flow)
                    || *result_modeled != reference_flow_exit_modeled(flow)
                {
                    return None;
                }
                available.insert(*token, *result_modeled);
                true
            }
            ReferenceEvent::Call { .. }
            | ReferenceEvent::TailCall { .. }
            | ReferenceEvent::BranchDecision { .. } => return None,
            _ => true,
        };
        if !values_are_available {
            return None;
        }
    }
    Some(available)
}

fn reference_flow_calls_are_valid(flow: &ReferenceFlow) -> bool {
    let Some(available) = validate_reference_events(&flow.events, BTreeMap::new()) else {
        return false;
    };
    match &flow.terminator {
        ReferenceTerminator::Return(_) => true,
        ReferenceTerminator::Branch {
            condition,
            taken,
            not_taken,
        } => {
            value_call_results_available(&condition.left, &available)
                && value_call_results_available(&condition.right, &available)
                && validate_reference_flow_with_calls(taken, available.clone())
                && validate_reference_flow_with_calls(not_taken, available)
        }
    }
}

fn validate_reference_flow_with_calls(
    flow: &ReferenceFlow,
    available: BTreeMap<u32, bool>,
) -> bool {
    let Some(available) = validate_reference_events(&flow.events, available) else {
        return false;
    };
    match &flow.terminator {
        ReferenceTerminator::Return(_) => true,
        ReferenceTerminator::Branch {
            condition,
            taken,
            not_taken,
        } => {
            value_call_results_available(&condition.left, &available)
                && value_call_results_available(&condition.right, &available)
                && validate_reference_flow_with_calls(taken, available.clone())
                && validate_reference_flow_with_calls(not_taken, available)
        }
    }
}

impl Event {
    fn canonical(&self) -> String {
        match self {
            Self::Memory {
                access,
                width,
                address,
                register,
                value,
            } => {
                let access = match access {
                    Access::Read => "R",
                    Access::Write => "W",
                };
                let value = value
                    .as_ref()
                    .map_or_else(|| "-".to_owned(), Value::canonical);
                format!("{access}\t{width}\t{address:#010x}\t{register}\t{value}")
            }
            Self::Fence {
                fm,
                predecessor,
                successor,
            } => format!("FENCE\tfm={fm:#x}\tpred={predecessor:#x}\tsucc={successor:#x}"),
        }
    }

    fn equivalent(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Memory {
                    access: left_access,
                    width: left_width,
                    address: left_address,
                    value: left_value,
                    ..
                },
                Self::Memory {
                    access: right_access,
                    width: right_width,
                    address: right_address,
                    value: right_value,
                    ..
                },
            ) => {
                left_access == right_access
                    && left_width == right_width
                    && left_address == right_address
                    && left_value == right_value
            }
            (
                Self::Fence {
                    fm: left_fm,
                    predecessor: left_predecessor,
                    successor: left_successor,
                },
                Self::Fence {
                    fm: right_fm,
                    predecessor: right_predecessor,
                    successor: right_successor,
                },
            ) => {
                left_fm == right_fm
                    && left_predecessor == right_predecessor
                    && left_successor == right_successor
            }
            _ => false,
        }
    }

    fn unmapped_address(&self) -> Option<u32> {
        match self {
            Self::Memory {
                address, register, ..
            } if register == "UNMAPPED" => Some(*address),
            _ => None,
        }
    }

    #[cfg(test)]
    fn memory_value(&self) -> Option<String> {
        match self {
            Self::Memory { value, .. } => value.as_ref().map(Value::canonical),
            Self::Fence { .. } => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Trace {
    symbol: String,
    events: Vec<Event>,
    reference_events: Vec<ReferenceEvent>,
    reference_dependencies: Vec<String>,
    blockers: Vec<String>,
    reference_blockers: Vec<String>,
    return_value: Value,
    reference_flow: Option<ReferenceFlow>,
    unresolved_branch: Option<BranchCondition>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ArtifactSymbol {
    member: Option<String>,
    name: String,
}

impl Trace {
    fn reference_indexed_mmio_count(&self) -> usize {
        fn flow_count(flow: &ReferenceFlow) -> usize {
            let events = flow
                .events
                .iter()
                .map(|event| match event {
                    ReferenceEvent::IndexedMmio { .. } => 1,
                    ReferenceEvent::ComposedCall { flow, .. } => flow_count(flow),
                    _ => 0,
                })
                .sum::<usize>();
            events
                + match &flow.terminator {
                    ReferenceTerminator::Return(_) => 0,
                    ReferenceTerminator::Branch {
                        taken, not_taken, ..
                    } => flow_count(taken) + flow_count(not_taken),
                }
        }

        self.reference_flow.as_ref().map_or_else(
            || {
                self.reference_events
                    .iter()
                    .map(|event| match event {
                        ReferenceEvent::IndexedMmio { .. } => 1,
                        ReferenceEvent::ComposedCall { flow, .. } => flow_count(flow),
                        _ => 0,
                    })
                    .sum()
            },
            flow_count,
        )
    }

    fn is_exact(&self) -> bool {
        self.reference_flow.is_none()
            && self.blockers.is_empty()
            && self
                .events
                .iter()
                .all(|event| event.unmapped_address().is_none())
    }

    fn is_reference_eligible(&self) -> bool {
        self.blockers.is_empty()
            && self.reference_blockers.is_empty()
            && self.unresolved_branch.is_none()
            && self.reference_observables_are_mapped()
            && self.reference_calls_are_valid()
    }

    fn reference_observables_are_mapped(&self) -> bool {
        fn flow_is_mapped(flow: &ReferenceFlow) -> bool {
            flow.events.iter().all(|event| match event {
                ReferenceEvent::Observable(event) => event.unmapped_address().is_none(),
                ReferenceEvent::IndexedMmio { registers, .. } => !registers.is_empty(),
                ReferenceEvent::ComposedCall { flow, .. } => flow_is_mapped(flow),
                _ => true,
            }) && match &flow.terminator {
                ReferenceTerminator::Return(_) => true,
                ReferenceTerminator::Branch {
                    taken, not_taken, ..
                } => flow_is_mapped(taken) && flow_is_mapped(not_taken),
            }
        }

        fn reference_event_is_mapped(event: &ReferenceEvent) -> bool {
            match event {
                ReferenceEvent::Observable(event) => event.unmapped_address().is_none(),
                ReferenceEvent::IndexedMmio { registers, .. } => !registers.is_empty(),
                ReferenceEvent::ComposedCall { flow, .. } => flow_is_mapped(flow),
                _ => true,
            }
        }

        self.reference_flow.as_ref().map_or_else(
            || self.reference_events.iter().all(reference_event_is_mapped),
            flow_is_mapped,
        )
    }

    fn reference_calls_are_valid(&self) -> bool {
        if let Some(flow) = &self.reference_flow {
            reference_flow_calls_are_valid(flow)
        } else {
            validate_reference_events(&self.reference_events, BTreeMap::new()).is_some()
        }
    }

    fn reference_exit_a0_modeled(&self) -> bool {
        self.reference_flow.as_ref().map_or_else(
            || {
                validate_reference_events(&self.reference_events, BTreeMap::new()).is_some_and(
                    |available| {
                        self.return_value.is_resolved()
                            && value_call_results_available(&self.return_value, &available)
                    },
                )
            },
            reference_flow_exit_modeled,
        )
    }
}

fn parse_u32(value: &str) -> Option<u32> {
    let value = value.trim();
    if let Some(hex) = value.strip_prefix("0x") {
        u32::from_str_radix(hex, 16).ok()
    } else {
        value.parse().ok()
    }
}

#[cfg(test)]
fn parse_i64(value: &str) -> Option<i64> {
    let value = value.trim();
    if let Some(hex) = value.strip_prefix("-0x") {
        i64::from_str_radix(hex, 16).ok().map(|number| -number)
    } else if let Some(hex) = value.strip_prefix("0x") {
        i64::from_str_radix(hex, 16).ok()
    } else {
        value.parse().ok()
    }
}

#[cfg(test)]
fn split_operands(operands: &str) -> Vec<&str> {
    operands.split(',').map(str::trim).collect()
}

#[cfg(test)]
fn memory_operand(operand: &str) -> Option<(i64, &str)> {
    let open = operand.find('(')?;
    let close = operand.rfind(')')?;
    let offset = parse_i64(operand[..open].trim())?;
    Some((offset, operand[open + 1..close].trim()))
}

#[cfg(test)]
fn effective_address(values: &HashMap<String, Value>, operand: &str) -> Option<u32> {
    let (offset, base) = memory_operand(operand)?;
    let Value::Constant(base) = values.get(base)? else {
        return None;
    };
    Some(base.wrapping_add(offset as u32))
}

#[cfg(test)]
fn width_for(mnemonic: &str) -> Option<u8> {
    match mnemonic {
        "lb" | "lbu" | "sb" => Some(8),
        "lh" | "lhu" | "sh" => Some(16),
        "lw" | "sw" => Some(32),
        _ => None,
    }
}

#[cfg(test)]
fn disassembly_label(line: &str) -> Option<&str> {
    let (_, remainder) = line.split_once('<')?;
    let (name, suffix) = remainder.split_once('>')?;
    suffix.trim().eq(":").then_some(name)
}

#[cfg(test)]
fn trace_disassembly(symbol: &str, disassembly: &str, svd: &SvdMap) -> Trace {
    let mut values: HashMap<String, Value> = HashMap::new();
    for (index, register) in ["a0", "a1", "a2", "a3", "a4", "a5", "a6", "a7"]
        .into_iter()
        .enumerate()
    {
        values.insert(register.to_owned(), Value::input(index as u8));
    }
    let mut events = Vec::new();
    let mut reference_events = Vec::new();
    let mut blockers = Vec::new();
    let mut reference_blockers = Vec::new();
    let mut return_value = Value::Unknown;
    let mut next_mmio_read_token = 0_u32;
    let mut in_symbol = false;

    for line in disassembly.lines() {
        let trimmed = line.trim();
        if let Some(label) = disassembly_label(trimmed) {
            if label == symbol {
                in_symbol = true;
            } else if in_symbol && !label.starts_with('.') {
                break;
            }
            continue;
        }
        if !in_symbol {
            continue;
        }
        let Some((pc_text, instruction)) = trimmed.split_once(':') else {
            continue;
        };
        if u64::from_str_radix(pc_text.trim(), 16).is_err() {
            continue;
        }
        let instruction = instruction.trim();
        if instruction.is_empty() {
            continue;
        }
        let (mnemonic, operands) = instruction
            .split_once(char::is_whitespace)
            .map(|(mnemonic, operands)| (mnemonic, operands.trim()))
            .unwrap_or((instruction, ""));
        let operands = split_operands(operands);

        if mnemonic.starts_with('b') && mnemonic != "bseti" && mnemonic != "bclri" {
            blockers.push(format!(
                "control-flow instruction at 0x{}: {instruction}",
                pc_text.trim()
            ));
        }
        if matches!(
            mnemonic,
            "j" | "jr" | "jal" | "jalr" | "call" | "tail" | "c.j" | "c.jr" | "c.jal" | "c.jalr"
        ) {
            blockers.push(format!(
                "call/jump instruction at 0x{}: {instruction}",
                pc_text.trim()
            ));
        }

        match mnemonic {
            "lui" if operands.len() == 2 => {
                let value = parse_i64(operands[1])
                    .map(|value| Value::Constant((value as u32) << 12))
                    .unwrap_or(Value::Unknown);
                values.insert(operands[0].to_owned(), value);
            }
            "li" if operands.len() == 2 => {
                let value = parse_i64(operands[1])
                    .map(|value| Value::Constant(value as u32))
                    .unwrap_or(Value::Unknown);
                values.insert(operands[0].to_owned(), value);
            }
            "mv" if operands.len() == 2 => {
                let value = values.get(operands[1]).cloned().unwrap_or(Value::Unknown);
                values.insert(operands[0].to_owned(), value);
            }
            "addi" if operands.len() == 3 => {
                let value = match (values.get(operands[1]).cloned(), parse_i64(operands[2])) {
                    (Some(source), Some(offset)) => source.add_constant(offset as u32),
                    _ => Value::Unknown,
                };
                values.insert(operands[0].to_owned(), value);
            }
            "and" | "or" if operands.len() == 3 => {
                let left = values.get(operands[1]).cloned().unwrap_or(Value::Unknown);
                let right = values.get(operands[2]).cloned().unwrap_or(Value::Unknown);
                let value = match mnemonic {
                    "and" => left.bitand(right),
                    "or" => left.bitor(right),
                    _ => unreachable!(),
                };
                values.insert(operands[0].to_owned(), value);
            }
            "andi" | "ori" | "xori" if operands.len() == 3 => {
                let source = values.get(operands[1]).cloned().unwrap_or(Value::Unknown);
                let value = match parse_i64(operands[2]) {
                    Some(constant) if mnemonic == "andi" => source.and(constant as u32),
                    Some(constant) if mnemonic == "ori" => source.or(constant as u32),
                    Some(constant) => source.xor(constant as u32),
                    None => Value::Unknown,
                };
                values.insert(operands[0].to_owned(), value);
            }
            "slli" | "srli" if operands.len() == 3 => {
                let source = values.get(operands[1]).cloned().unwrap_or(Value::Unknown);
                let value = match parse_u32(operands[2]).filter(|amount| *amount < 32) {
                    Some(amount) if mnemonic == "slli" => source.shift_left(amount),
                    Some(amount) => source.shift_right(amount),
                    None => Value::Unknown,
                };
                values.insert(operands[0].to_owned(), value);
            }
            "not" if operands.len() == 2 => {
                let source = values.get(operands[1]).cloned().unwrap_or(Value::Unknown);
                values.insert(operands[0].to_owned(), source.not());
            }
            "seqz" if operands.len() == 2 => {
                let source = values.get(operands[1]).cloned().unwrap_or(Value::Unknown);
                values.insert(operands[0].to_owned(), source.seqz());
            }
            "bseti" | "bclri" if operands.len() == 3 => {
                let source = values.get(operands[1]).cloned().unwrap_or(Value::Unknown);
                let value = match parse_u32(operands[2]).filter(|bit| *bit < 32) {
                    Some(bit) if mnemonic == "bseti" => source.or(1 << bit),
                    Some(bit) => source.and(!(1 << bit)),
                    None => Value::Unknown,
                };
                values.insert(operands[0].to_owned(), value);
            }
            "lb" | "lbu" | "lh" | "lhu" | "lw" if operands.len() == 2 => {
                let address = effective_address(&values, operands[1]);
                if let Some(address) = address.filter(|address| svd.contains_mmio(*address)) {
                    let width = width_for(mnemonic).unwrap();
                    let read_token = next_mmio_read_token;
                    next_mmio_read_token += 1;
                    let event = Event::Memory {
                        access: Access::Read,
                        width,
                        address,
                        register: svd.register_name(address),
                        value: None,
                    };
                    events.push(event.clone());
                    reference_events.push(ReferenceEvent::Observable(event));
                    values.insert(
                        operands[0].to_owned(),
                        if width == 32 {
                            Value::RegisterImage {
                                read_token,
                                address,
                                and_mask: u32::MAX,
                                or_mask: 0,
                            }
                        } else {
                            Value::Unknown
                        },
                    );
                } else {
                    reference_blockers.push(format!(
                        "unmodeled-memory-load at 0x{}: {instruction}",
                        pc_text.trim()
                    ));
                    values.insert(operands[0].to_owned(), Value::Unknown);
                }
            }
            "sb" | "sh" | "sw" if operands.len() == 2 => {
                if let Some(address) = effective_address(&values, operands[1])
                    .filter(|address| svd.contains_mmio(*address))
                {
                    let value = values.get(operands[0]).cloned().unwrap_or(Value::Unknown);
                    if !value.is_resolved() {
                        blockers.push(format!(
                            "unresolved MMIO write value at 0x{}: {instruction}",
                            pc_text.trim()
                        ));
                    }
                    let event = Event::Memory {
                        access: Access::Write,
                        width: width_for(mnemonic).unwrap(),
                        address,
                        register: svd.register_name(address),
                        value: Some(value),
                    };
                    events.push(event.clone());
                    reference_events.push(ReferenceEvent::Observable(event));
                } else {
                    reference_blockers.push(format!(
                        "unmodeled-memory-store at 0x{}: {instruction}",
                        pc_text.trim()
                    ));
                }
            }
            "ret" => {
                return_value = values.get("a0").cloned().unwrap_or(Value::Unknown);
            }
            "fence" if operands.len() == 2 => {
                match (parse_fence_set(operands[0]), parse_fence_set(operands[1])) {
                    (Some(predecessor), Some(successor)) => {
                        let event = Event::Fence {
                            fm: 0,
                            predecessor,
                            successor,
                        };
                        events.push(event.clone());
                        reference_events.push(ReferenceEvent::Observable(event));
                    }
                    _ => blockers.push(format!(
                        "unsupported fence at 0x{}: {instruction}",
                        pc_text.trim()
                    )),
                }
            }
            "nop" => {}
            "fence.i" => blockers.push(format!(
                "unsupported instruction-cache fence at 0x{}: {instruction}",
                pc_text.trim()
            )),
            _ => {
                if let Some(destination) = operands.first()
                    && is_register(destination)
                    && !matches!(mnemonic, "sw" | "sh" | "sb")
                {
                    values.insert((*destination).to_owned(), Value::Unknown);
                }
            }
        }
    }
    if !in_symbol {
        blockers.push("symbol was not present in decoded instruction stream".to_owned());
    }
    Trace {
        symbol: symbol.to_owned(),
        events,
        reference_events,
        reference_dependencies: Vec::new(),
        blockers,
        reference_blockers,
        return_value,
        reference_flow: None,
        unresolved_branch: None,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum StructuralAddress {
    Absolute(u32),
    PrivateStack(i32),
    ExternalTableSlot(external_abi::Table, i32),
    CallerMemory(Value),
    SymbolMemory(Value),
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct StructuralCallSite {
    member: Option<String>,
    symbol: String,
    address: u32,
}

impl StructuralCallSite {
    fn new(owner: &binary::BinarySymbol, address: u32) -> Self {
        Self {
            member: owner.member.clone(),
            symbol: owner.name.clone(),
            address,
        }
    }
}

type StructuralRelocatedCalls = BTreeMap<StructuralCallSite, (String, Option<u32>)>;

fn structural_effective_address(
    values: &[Value; 32],
    base: Reg,
    offset: i32,
) -> Option<StructuralAddress> {
    let base = &values[usize::from(base.0)];
    match base {
        Value::Constant(base) => Some(StructuralAddress::Absolute(
            base.wrapping_add(offset as u32),
        )),
        Value::StackAddress(base) => {
            Some(StructuralAddress::PrivateStack(base.wrapping_add(offset)))
        }
        Value::ExternalTable(table) => Some(StructuralAddress::ExternalTableSlot(*table, offset)),
        Value::SymbolAddress {
            lo_addend: Some(_), ..
        } => Some(StructuralAddress::SymbolMemory(
            base.clone().add_constant(offset as u32),
        )),
        _ if base.caller_memory_address() => Some(StructuralAddress::CallerMemory(
            base.clone().add_constant(offset as u32),
        )),
        _ => None,
    }
}

fn structural_indexed_mmio_address(
    values: &[Value; 32],
    base: Reg,
    offset: i32,
    svd: &SvdMap,
) -> Option<(Value, IndexedMmioDomain)> {
    let address = values[usize::from(base.0)]
        .clone()
        .add_constant(offset as u32);
    let domain = indexed_mmio_domain(&address, svd)?;
    Some((address, domain))
}

fn relocation_symbol_address(
    owner: &binary::BinarySymbol,
    relocation: &binary::SymbolRelocation,
) -> Value {
    Value::SymbolAddress {
        member: owner.member.clone(),
        symbol: relocation.symbol.clone(),
        hi_addend: relocation.addend,
        lo_addend: None,
        post_offset: 0,
    }
}

fn complete_low_relocation(
    owner: &binary::BinarySymbol,
    pc: u32,
    kind: binary::RelocationKind,
    base: &Value,
    encoded_offset: i32,
) -> std::result::Result<Option<Value>, String> {
    if owner.addresses_resolved {
        return Ok(None);
    }
    let Some(relocation) = owner.relocation(pc, kind) else {
        return Ok(None);
    };
    let expected_offset = ((relocation.addend as u32) << 20) as i32 >> 20;
    if encoded_offset != expected_offset {
        return Err(format!(
            "relocation {kind:?} at {pc:#x} encodes {encoded_offset:+#x}, expected low addend {expected_offset:+#x}"
        ));
    }
    let Value::SymbolAddress {
        member,
        symbol,
        hi_addend,
        lo_addend: None,
        post_offset: 0,
    } = base
    else {
        return Err(format!(
            "relocation {kind:?} at {pc:#x} has no matching incomplete HI20 base"
        ));
    };
    if member != &owner.member || symbol != &relocation.symbol {
        return Err(format!(
            "relocation {kind:?} at {pc:#x} does not match its HI20 base: low={:?}::{}{:+#x}, high={member:?}::{symbol}{hi_addend:+#x}",
            owner.member, relocation.symbol, relocation.addend
        ));
    }
    Ok(Some(Value::SymbolAddress {
        member: member.clone(),
        symbol: symbol.clone(),
        hi_addend: *hi_addend,
        lo_addend: Some(relocation.addend),
        post_offset: 0,
    }))
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct SymbolicStack {
    bytes: BTreeMap<i32, [BitSource; 8]>,
}

impl SymbolicStack {
    fn store(&mut self, offset: i32, width: u8, value: &Value) {
        let bits = value.bits();
        for byte in 0..usize::from(width / 8) {
            self.bytes.insert(
                offset.wrapping_add(byte as i32),
                core::array::from_fn(|bit| bits[byte * 8 + bit]),
            );
        }
    }

    fn load(&self, offset: i32, width: u8, signed: bool) -> Option<Value> {
        let width = usize::from(width);
        let mut bits = [BitSource::Constant(false); 32];
        for destination in 0..width {
            let byte = self
                .bytes
                .get(&offset.wrapping_add((destination / 8) as i32))?;
            bits[destination] = byte[destination % 8];
        }
        if signed {
            let sign = bits[width - 1];
            bits[width..].fill(sign);
        }
        Some(Value::from_bits(bits))
    }
}

fn structural_set(values: &mut [Value; 32], register: Reg, value: Value) {
    if register != Reg::ZERO {
        values[usize::from(register.0)] = value;
    }
}

fn structural_finish_call(values: &mut [Value; 32], return_address: u32, call_token: u32) {
    structural_finish_call_with_result(values, return_address, Value::CallResult(call_token));
}

fn structural_finish_call_with_result(
    values: &mut [Value; 32],
    return_address: u32,
    result: Value,
) {
    for register in [
        Reg::RA,
        Reg::T0,
        Reg::T1,
        Reg::T2,
        Reg::A0,
        Reg::A1,
        Reg::A2,
        Reg::A3,
        Reg::A4,
        Reg::A5,
        Reg::A6,
        Reg::A7,
        Reg::T3,
        Reg::T4,
        Reg::T5,
        Reg::T6,
    ] {
        structural_set(values, register, Value::Unknown);
    }
    structural_set(values, Reg::RA, Value::Constant(return_address));
    structural_set(values, Reg::A0, result);
}

fn trace_binary_symbol(
    symbol: &binary::BinarySymbol,
    svd: &SvdMap,
    relocated_calls: &StructuralRelocatedCalls,
    external_pointer_cells: &BTreeMap<u32, external_abi::Table>,
    specialized_arguments: Option<&[Value; 8]>,
) -> Result<Trace> {
    trace_binary_symbol_with_branches(
        symbol,
        svd,
        relocated_calls,
        external_pointer_cells,
        specialized_arguments,
        &BTreeMap::new(),
    )
}

fn trace_binary_symbol_with_branches(
    symbol: &binary::BinarySymbol,
    svd: &SvdMap,
    relocated_calls: &StructuralRelocatedCalls,
    external_pointer_cells: &BTreeMap<u32, external_abi::Table>,
    specialized_arguments: Option<&[Value; 8]>,
    forced_branches: &BTreeMap<u32, bool>,
) -> Result<Trace> {
    let mut values: [Value; 32] = core::array::from_fn(|_| Value::Unknown);
    values[0] = Value::Constant(0);
    values[usize::from(Reg::SP.0)] = Value::StackAddress(0);
    for index in 0..8 {
        values[10 + index] = specialized_arguments
            .and_then(|arguments| arguments[index].as_constant())
            .map_or_else(
                || Value::input(index as u8),
                |value| Value::InputConstant {
                    index: index as u8,
                    value,
                },
            );
    }
    let mut events = Vec::new();
    let mut reference_events = Vec::new();
    let mut blockers = Vec::new();
    let mut reference_blockers = Vec::new();
    let mut return_value = Value::Unknown;
    let mut unresolved_branch = None;
    let mut next_mmio_read_token = 0_u32;
    let mut next_memory_read_token = 0_u32;
    let mut next_call_token = 0_u32;
    let mut next_external_call_token = 0_u32;
    let mut stack = SymbolicStack::default();

    let instructions = binary::decode_symbol(symbol)?;
    let instruction_indices = instructions
        .iter()
        .enumerate()
        .map(|(index, instruction)| (instruction.address as u32, index))
        .collect::<BTreeMap<_, _>>();
    let mut instruction_index = 0usize;
    let mut visited_instructions = BTreeSet::new();
    while let Some(decoded) = instructions.get(instruction_index).copied() {
        let pc = decoded.address;
        let width = decoded.width;
        let instruction = decoded.instruction;
        if !visited_instructions.insert(pc as u32) {
            blockers.push(format!(
                "control-flow loop revisits instruction at {pc:#x}: {instruction}"
            ));
            break;
        }
        if let Some((name, target)) =
            relocated_calls.get(&StructuralCallSite::new(symbol, pc as u32))
        {
            blockers.push(format!(
                "call/jump instruction at {pc:#x}: relocated call to {name}"
            ));
            let Some(jalr) = instructions.get(instruction_index + 1).copied() else {
                reference_blockers.push(format!(
                    "malformed-call-relocation at {pc:#x}: {name} has no following JALR"
                ));
                break;
            };
            if jalr.address != pc.wrapping_add(4) {
                reference_blockers.push(format!(
                    "malformed-call-relocation at {pc:#x}: {name} is not a two-instruction call"
                ));
                break;
            }
            let Inst::Jalr { dest, .. } = jalr.instruction else {
                reference_blockers.push(format!(
                    "malformed-call-relocation at {pc:#x}: {name} is not followed by JALR"
                ));
                break;
            };
            if target.is_none()
                && let Some(argument_count) = external_abi::diagnostic_argument_count(name)
            {
                if dest != Reg::RA {
                    reference_blockers.push(format!(
                        "unsupported-diagnostic-call-link-register at {pc:#x}: {name} uses {dest}"
                    ));
                    break;
                }
                let arguments = Box::new(core::array::from_fn(|index| {
                    if index < usize::from(argument_count) {
                        values[10 + index].clone()
                    } else {
                        Value::Constant(0)
                    }
                }));
                reference_events.push(ReferenceEvent::DiagnosticCall {
                    function: name.clone(),
                    argument_count,
                    arguments,
                });
                structural_finish_call_with_result(
                    &mut values,
                    (pc as u32).wrapping_add(8),
                    Value::Unknown,
                );
                values[0] = Value::Constant(0);
                instruction_index += 2;
                continue;
            }
            let Some(target) = *target else {
                reference_blockers.push(format!("unresolved-call-relocation at {pc:#x}: {name}"));
                break;
            };
            let arguments = Box::new(core::array::from_fn(|index| values[10 + index].clone()));
            if dest == Reg::ZERO {
                let call_token = next_call_token;
                reference_events.push(ReferenceEvent::TailCall {
                    token: call_token,
                    site: pc as u32,
                    target,
                    arguments,
                });
                return_value = Value::CallResult(call_token);
                break;
            } else if dest == Reg::RA {
                let call_token = next_call_token;
                next_call_token += 1;
                reference_events.push(ReferenceEvent::Call {
                    token: call_token,
                    site: pc as u32,
                    target,
                    arguments,
                });
                structural_finish_call(&mut values, (pc as u32).wrapping_add(8), call_token);
            } else {
                reference_blockers.push(format!(
                    "unsupported-call-link-register at {pc:#x}: {name} uses {dest}"
                ));
            }
            values[0] = Value::Constant(0);
            instruction_index += 2;
            continue;
        }
        match instruction {
            Inst::Lui { uimm, dest } => {
                if !symbol.addresses_resolved
                    && let Some(relocation) =
                        symbol.relocation(pc as u32, binary::RelocationKind::Hi20)
                {
                    if uimm.as_u32() != 0 {
                        reference_blockers.push(format!(
                            "malformed-data-relocation at {pc:#x}: HI20 retains encoded immediate {:#x}",
                            uimm.as_u32()
                        ));
                        structural_set(&mut values, dest, Value::Unknown);
                    } else {
                        structural_set(
                            &mut values,
                            dest,
                            relocation_symbol_address(symbol, relocation),
                        );
                    }
                } else {
                    structural_set(&mut values, dest, Value::Constant(uimm.as_u32()));
                }
            }
            Inst::Auipc { uimm, dest } => {
                structural_set(
                    &mut values,
                    dest,
                    Value::Constant((pc as u32).wrapping_add(uimm.as_u32())),
                );
            }
            Inst::Addi { imm, dest, src1 } => {
                let value = match complete_low_relocation(
                    symbol,
                    pc as u32,
                    binary::RelocationKind::Lo12I,
                    &values[usize::from(src1.0)],
                    imm.as_i32(),
                ) {
                    Ok(Some(address)) => address,
                    Ok(None) => values[usize::from(src1.0)]
                        .clone()
                        .add_constant(imm.as_u32()),
                    Err(error) => {
                        reference_blockers
                            .push(format!("malformed-data-relocation at {pc:#x}: {error}"));
                        Value::Unknown
                    }
                };
                structural_set(&mut values, dest, value);
            }
            Inst::Andi { imm, dest, src1 } => {
                let value = values[usize::from(src1.0)]
                    .clone()
                    .and(binary::andi_immediate(imm, width));
                structural_set(&mut values, dest, value);
            }
            Inst::Ori { imm, dest, src1 } => {
                let value = values[usize::from(src1.0)].clone().or(imm.as_u32());
                structural_set(&mut values, dest, value);
            }
            Inst::Xori { imm, dest, src1 } => {
                let value = values[usize::from(src1.0)].clone().xor(imm.as_u32());
                structural_set(&mut values, dest, value);
            }
            Inst::Slli { imm, dest, src1 } => {
                let value = values[usize::from(src1.0)].clone().shift_left(imm.as_u32());
                structural_set(&mut values, dest, value);
            }
            Inst::Srli { imm, dest, src1 } => {
                let value = values[usize::from(src1.0)]
                    .clone()
                    .shift_right(imm.as_u32());
                structural_set(&mut values, dest, value);
            }
            Inst::Srai { imm, dest, src1 } => {
                let source = values[usize::from(src1.0)].clone();
                let value = source.as_constant().map_or_else(
                    || {
                        Value::expression(
                            ExpressionOperation::ShiftRightArithmetic,
                            source,
                            Value::Constant(imm.as_u32()),
                        )
                    },
                    |value| Value::Constant((value as i32).wrapping_shr(imm.as_u32()) as u32),
                );
                structural_set(&mut values, dest, value);
            }
            Inst::Sltiu { imm, dest, src1 } if imm.as_u32() == 1 => {
                let value = values[usize::from(src1.0)].clone().seqz();
                structural_set(&mut values, dest, value);
            }
            Inst::Slti { dest, .. }
            | Inst::Sltiu { dest, .. }
            | Inst::Slt { dest, .. }
            | Inst::Sltu { dest, .. } => {
                structural_set(&mut values, dest, Value::Unknown);
            }
            Inst::And { dest, src1, src2 } => {
                let value = values[usize::from(src1.0)]
                    .clone()
                    .bitand(values[usize::from(src2.0)].clone());
                structural_set(&mut values, dest, value);
            }
            Inst::Or { dest, src1, src2 } => {
                let value = values[usize::from(src1.0)]
                    .clone()
                    .bitor(values[usize::from(src2.0)].clone());
                structural_set(&mut values, dest, value);
            }
            Inst::Xor { dest, src1, src2 } => {
                let value = values[usize::from(src1.0)]
                    .clone()
                    .bitxor(values[usize::from(src2.0)].clone());
                structural_set(&mut values, dest, value);
            }
            Inst::Add { dest, src1, src2 } => {
                let left = values[usize::from(src1.0)].clone();
                let right = values[usize::from(src2.0)].clone();
                let value = match (left.as_constant(), right.as_constant()) {
                    (Some(left), Some(right)) => Value::Constant(left.wrapping_add(right)),
                    (_, Some(right)) => left.add_constant(right),
                    (Some(left), _) => right.add_constant(left),
                    _ => Value::expression(ExpressionOperation::Add, left, right),
                };
                structural_set(&mut values, dest, value);
            }
            Inst::Sub { dest, src1, src2 } => {
                let left_value = values[usize::from(src1.0)].clone();
                let right_value = values[usize::from(src2.0)].clone();
                let value = match (left_value.as_constant(), right_value.as_constant()) {
                    (Some(left), Some(right)) => Value::Constant(left.wrapping_sub(right)),
                    _ => Value::expression(ExpressionOperation::Subtract, left_value, right_value),
                };
                structural_set(&mut values, dest, value);
            }
            Inst::Sll { dest, src1, src2 }
            | Inst::Srl { dest, src1, src2 }
            | Inst::Sra { dest, src1, src2 } => {
                let source = values[usize::from(src1.0)].clone();
                let amount = values[usize::from(src2.0)].clone();
                let constant_amount = amount.as_constant().map(|value| value & 31);
                let value = match (instruction, constant_amount) {
                    (Inst::Sll { .. }, Some(amount)) => source.shift_left(amount),
                    (Inst::Srl { .. }, Some(amount)) => source.shift_right(amount),
                    (Inst::Sra { .. }, Some(amount)) => source.as_constant().map_or_else(
                        || {
                            Value::expression(
                                ExpressionOperation::ShiftRightArithmetic,
                                source,
                                Value::Constant(amount),
                            )
                        },
                        |value| Value::Constant((value as i32).wrapping_shr(amount) as u32),
                    ),
                    (Inst::Sll { .. }, None) => {
                        Value::expression(ExpressionOperation::ShiftLeft, source, amount)
                    }
                    (Inst::Srl { .. }, None) => {
                        Value::expression(ExpressionOperation::ShiftRight, source, amount)
                    }
                    (Inst::Sra { .. }, None) => {
                        Value::expression(ExpressionOperation::ShiftRightArithmetic, source, amount)
                    }
                    _ => unreachable!(),
                };
                structural_set(&mut values, dest, value);
            }
            Inst::Mul { dest, src1, src2 }
            | Inst::Div { dest, src1, src2 }
            | Inst::Divu { dest, src1, src2 }
            | Inst::Rem { dest, src1, src2 }
            | Inst::Remu { dest, src1, src2 } => {
                let left_value = values[usize::from(src1.0)].clone();
                let right_value = values[usize::from(src2.0)].clone();
                let left = left_value.as_constant();
                let right = right_value.as_constant();
                let value = match (instruction, left, right) {
                    (Inst::Mul { .. }, Some(left), Some(right)) => {
                        Value::Constant(left.wrapping_mul(right))
                    }
                    (Inst::Div { .. }, Some(left), Some(right)) => Value::Constant(if right == 0 {
                        u32::MAX
                    } else if left == i32::MIN as u32 && right == u32::MAX {
                        i32::MIN as u32
                    } else {
                        ((left as i32) / (right as i32)) as u32
                    }),
                    (Inst::Divu { .. }, Some(left), Some(right)) => {
                        Value::Constant(left.checked_div(right).unwrap_or(u32::MAX))
                    }
                    (Inst::Rem { .. }, Some(left), Some(right)) => Value::Constant(if right == 0 {
                        left
                    } else if left == i32::MIN as u32 && right == u32::MAX {
                        0
                    } else {
                        ((left as i32) % (right as i32)) as u32
                    }),
                    (Inst::Remu { .. }, Some(left), Some(right)) => {
                        Value::Constant(if right == 0 { left } else { left % right })
                    }
                    (Inst::Mul { .. }, _, _) => {
                        Value::expression(ExpressionOperation::Multiply, left_value, right_value)
                    }
                    (Inst::Div { .. }, _, _) => Value::expression(
                        ExpressionOperation::DivideSigned,
                        left_value,
                        right_value,
                    ),
                    (Inst::Divu { .. }, _, _) => Value::expression(
                        ExpressionOperation::DivideUnsigned,
                        left_value,
                        right_value,
                    ),
                    (Inst::Rem { .. }, _, _) => Value::expression(
                        ExpressionOperation::RemainderSigned,
                        left_value,
                        right_value,
                    ),
                    (Inst::Remu { .. }, _, _) => Value::expression(
                        ExpressionOperation::RemainderUnsigned,
                        left_value,
                        right_value,
                    ),
                    _ => unreachable!(),
                };
                structural_set(&mut values, dest, value);
            }
            Inst::Mulh { dest, .. } | Inst::Mulhsu { dest, .. } | Inst::Mulhu { dest, .. } => {
                structural_set(&mut values, dest, Value::Unknown);
            }
            Inst::Lb { offset, dest, base }
            | Inst::Lbu { offset, dest, base }
            | Inst::Lh { offset, dest, base }
            | Inst::Lhu { offset, dest, base }
            | Inst::Lw { offset, dest, base } => {
                let width = match instruction {
                    Inst::Lb { .. } | Inst::Lbu { .. } => 8,
                    Inst::Lh { .. } | Inst::Lhu { .. } => 16,
                    _ => 32,
                };
                let signed = matches!(instruction, Inst::Lb { .. } | Inst::Lh { .. });
                let relocated_external_table = symbol
                    .relocation(pc as u32, binary::RelocationKind::Lo12I)
                    .and_then(|relocation| {
                        (relocation.addend == 0 && offset.as_i32() == 0)
                            .then(|| external_abi::table_for_pointer_symbol(&relocation.symbol))
                            .flatten()
                    });
                let address = if relocated_external_table.is_some() {
                    None
                } else {
                    match complete_low_relocation(
                        symbol,
                        pc as u32,
                        binary::RelocationKind::Lo12I,
                        &values[usize::from(base.0)],
                        offset.as_i32(),
                    ) {
                        Ok(Some(address)) => Some(StructuralAddress::SymbolMemory(address)),
                        Ok(None) => structural_effective_address(&values, base, offset.as_i32()),
                        Err(error) => {
                            reference_blockers
                                .push(format!("malformed-data-relocation at {pc:#x}: {error}"));
                            structural_set(&mut values, dest, Value::Unknown);
                            values[0] = Value::Constant(0);
                            instruction_index += 1;
                            continue;
                        }
                    }
                };
                let value = match (relocated_external_table, address) {
                    (Some(table), _) if width == 32 => Value::ExternalTable(table),
                    (_, Some(StructuralAddress::Absolute(address)))
                        if width == 32 && external_pointer_cells.contains_key(&address) =>
                    {
                        Value::ExternalTable(external_pointer_cells[&address])
                    }
                    (_, Some(StructuralAddress::ExternalTableSlot(table, offset)))
                        if width == 32 =>
                    {
                        let Ok(offset) = u32::try_from(offset) else {
                            reference_blockers.push(format!(
                                "negative-external-abi-slot at {pc:#x}: {instruction}"
                            ));
                            structural_set(&mut values, dest, Value::Unknown);
                            values[0] = Value::Constant(0);
                            instruction_index += 1;
                            continue;
                        };
                        match external_abi::slot(table, offset) {
                            Some(slot) => Value::ExternalFunction {
                                table,
                                function: slot.function,
                            },
                            None => {
                                reference_blockers.push(format!(
                                    "unregistered-external-abi-slot at {pc:#x}: {}+{offset:#x}",
                                    external_abi::table_spec(table).id
                                ));
                                Value::Unknown
                            }
                        }
                    }
                    (_, Some(StructuralAddress::PrivateStack(offset))) => {
                        stack.load(offset, width, signed).unwrap_or_else(|| {
                            reference_blockers.push(format!(
                                "uninitialized-private-stack-load at {pc:#x}: {instruction}"
                            ));
                            Value::Unknown
                        })
                    }
                    (_, Some(StructuralAddress::CallerMemory(address))) => {
                        let read_token = next_memory_read_token;
                        next_memory_read_token += 1;
                        reference_events.push(ReferenceEvent::Memory {
                            access: Access::Read,
                            width,
                            address,
                            region: "caller-owned ABI argument RAM".to_owned(),
                            value: None,
                        });
                        Value::memory_read(read_token, width, signed)
                    }
                    (_, Some(StructuralAddress::SymbolMemory(address))) => {
                        let read_token = next_memory_read_token;
                        next_memory_read_token += 1;
                        reference_events.push(ReferenceEvent::Memory {
                            access: Access::Read,
                            width,
                            region: address.canonical(),
                            address,
                            value: None,
                        });
                        Value::memory_read(read_token, width, signed)
                    }
                    (_, Some(StructuralAddress::Absolute(address)))
                        if svd.contains_mmio(address) =>
                    {
                        let read_token = next_mmio_read_token;
                        next_mmio_read_token += 1;
                        let event = Event::Memory {
                            access: Access::Read,
                            width,
                            address,
                            register: svd.register_name(address),
                            value: None,
                        };
                        events.push(event.clone());
                        reference_events.push(ReferenceEvent::Observable(event));
                        Value::register_read(read_token, address, width, signed)
                    }
                    (_, Some(StructuralAddress::Absolute(address)))
                        if symbol.memory_region(address, width).is_some() =>
                    {
                        let region = symbol.memory_region(address, width).unwrap();
                        let read_token = next_memory_read_token;
                        next_memory_read_token += 1;
                        reference_events.push(ReferenceEvent::Memory {
                            access: Access::Read,
                            width,
                            address: Value::Constant(address),
                            region: region.name.clone(),
                            value: None,
                        });
                        Value::memory_read(read_token, width, signed)
                    }
                    _ => {
                        if let Some((address, domain)) =
                            structural_indexed_mmio_address(&values, base, offset.as_i32(), svd)
                        {
                            let read_token = next_mmio_read_token;
                            next_mmio_read_token += 1;
                            reference_events.push(ReferenceEvent::IndexedMmio {
                                access: Access::Read,
                                width,
                                address,
                                registers: domain.registers,
                                guard: domain.guard,
                                value: None,
                            });
                            Value::indexed_register_read(read_token, width, signed)
                        } else {
                            reference_blockers.push(format!(
                                "unmodeled-memory-load at {pc:#x}: {instruction}{}; base {} = {}",
                                if symbol.addresses_resolved {
                                    ""
                                } else {
                                    " (relocatable addresses)"
                                },
                                base,
                                values[usize::from(base.0)].canonical(),
                            ));
                            Value::Unknown
                        }
                    }
                };
                structural_set(&mut values, dest, value);
            }
            Inst::Sb { offset, src, base }
            | Inst::Sh { offset, src, base }
            | Inst::Sw { offset, src, base } => {
                let width = match instruction {
                    Inst::Sb { .. } => 8,
                    Inst::Sh { .. } => 16,
                    _ => 32,
                };
                let value = values[usize::from(src.0)].clone();
                let address = match complete_low_relocation(
                    symbol,
                    pc as u32,
                    binary::RelocationKind::Lo12S,
                    &values[usize::from(base.0)],
                    offset.as_i32(),
                ) {
                    Ok(Some(address)) => Some(StructuralAddress::SymbolMemory(address)),
                    Ok(None) => structural_effective_address(&values, base, offset.as_i32()),
                    Err(error) => {
                        reference_blockers
                            .push(format!("malformed-data-relocation at {pc:#x}: {error}"));
                        values[0] = Value::Constant(0);
                        instruction_index += 1;
                        continue;
                    }
                };
                match address {
                    Some(StructuralAddress::PrivateStack(offset)) => {
                        stack.store(offset, width, &value);
                    }
                    Some(StructuralAddress::CallerMemory(address)) => {
                        if !value.is_resolved() {
                            reference_blockers
                                .push(format!("unresolved-memory-write at {pc:#x}: {instruction}"));
                        }
                        reference_events.push(ReferenceEvent::Memory {
                            access: Access::Write,
                            width,
                            address,
                            region: "caller-owned ABI argument RAM".to_owned(),
                            value: Some(value),
                        });
                    }
                    Some(StructuralAddress::SymbolMemory(address)) => {
                        if !value.is_resolved() {
                            reference_blockers
                                .push(format!("unresolved-memory-write at {pc:#x}: {instruction}"));
                        }
                        reference_events.push(ReferenceEvent::Memory {
                            access: Access::Write,
                            width,
                            region: address.canonical(),
                            address,
                            value: Some(value),
                        });
                    }
                    Some(StructuralAddress::Absolute(address)) if svd.contains_mmio(address) => {
                        if !value.is_resolved() {
                            blockers.push(format!(
                                "unresolved MMIO write value at {pc:#x}: {instruction}"
                            ));
                        }
                        let event = Event::Memory {
                            access: Access::Write,
                            width,
                            address,
                            register: svd.register_name(address),
                            value: Some(value),
                        };
                        events.push(event.clone());
                        reference_events.push(ReferenceEvent::Observable(event));
                    }
                    Some(StructuralAddress::Absolute(address))
                        if symbol.memory_region(address, width).is_some() =>
                    {
                        let region = symbol.memory_region(address, width).unwrap();
                        if !region.writable {
                            reference_blockers.push(format!(
                                "read-only-memory-store at {pc:#x}: {instruction} ({})",
                                region.name
                            ));
                        }
                        if !value.is_resolved() {
                            reference_blockers
                                .push(format!("unresolved-memory-write at {pc:#x}: {instruction}"));
                        }
                        reference_events.push(ReferenceEvent::Memory {
                            access: Access::Write,
                            width,
                            address: Value::Constant(address),
                            region: region.name.clone(),
                            value: Some(value),
                        });
                    }
                    _ => {
                        if let Some((address, domain)) =
                            structural_indexed_mmio_address(&values, base, offset.as_i32(), svd)
                        {
                            if !value.is_resolved() {
                                reference_blockers.push(format!(
                                    "unresolved indexed MMIO write value at {pc:#x}: {instruction}"
                                ));
                            }
                            reference_events.push(ReferenceEvent::IndexedMmio {
                                access: Access::Write,
                                width,
                                address,
                                registers: domain.registers,
                                guard: domain.guard,
                                value: Some(value),
                            });
                        } else {
                            reference_blockers.push(format!(
                                "unmodeled-memory-store at {pc:#x}: {instruction}{}; base {} = {}",
                                if symbol.addresses_resolved {
                                    ""
                                } else {
                                    " (relocatable addresses)"
                                },
                                base,
                                values[usize::from(base.0)].canonical(),
                            ));
                        }
                    }
                }
            }
            Inst::Beq { offset, src1, src2 }
            | Inst::Bne { offset, src1, src2 }
            | Inst::Blt { offset, src1, src2 }
            | Inst::Bge { offset, src1, src2 }
            | Inst::Bltu { offset, src1, src2 }
            | Inst::Bgeu { offset, src1, src2 } => {
                let left_value = values[usize::from(src1.0)].clone();
                let right_value = values[usize::from(src2.0)].clone();
                let left = left_value.as_constant();
                let right = right_value.as_constant();
                let taken = if let Some((left, right)) = left.zip(right) {
                    match instruction {
                        Inst::Beq { .. } => left == right,
                        Inst::Bne { .. } => left != right,
                        Inst::Blt { .. } => (left as i32) < (right as i32),
                        Inst::Bge { .. } => (left as i32) >= (right as i32),
                        Inst::Bltu { .. } => left < right,
                        Inst::Bgeu { .. } => left >= right,
                        _ => unreachable!(),
                    }
                } else {
                    let operation = match instruction {
                        Inst::Beq { .. } => BranchOperation::Equal,
                        Inst::Bne { .. } => BranchOperation::NotEqual,
                        Inst::Blt { .. } => BranchOperation::LessSigned,
                        Inst::Bge { .. } => BranchOperation::GreaterEqualSigned,
                        Inst::Bltu { .. } => BranchOperation::LessUnsigned,
                        Inst::Bgeu { .. } => BranchOperation::GreaterEqualUnsigned,
                        _ => unreachable!(),
                    };
                    let condition = BranchCondition {
                        site: pc as u32,
                        operation,
                        left: left_value,
                        right: right_value,
                    };
                    if !condition.left.is_resolved() || !condition.right.is_resolved() {
                        blockers.push(format!(
                            "unresolved input-dependent control-flow at {pc:#x}: {instruction}"
                        ));
                        break;
                    }
                    let Some(taken) = forced_branches.get(&(pc as u32)).copied() else {
                        blockers.push(format!(
                            "input-dependent control-flow at {pc:#x}: {instruction}"
                        ));
                        unresolved_branch = Some(condition);
                        break;
                    };
                    reference_events.push(ReferenceEvent::BranchDecision { condition, taken });
                    taken
                };
                let target = if taken {
                    (pc as u32).wrapping_add(offset.as_u32())
                } else {
                    (pc as u32).wrapping_add(u32::from(width))
                };
                let Some(target_index) = instruction_indices.get(&target).copied() else {
                    blockers.push(format!(
                        "invalid conditional target at {pc:#x}: {instruction}"
                    ));
                    break;
                };
                instruction_index = target_index;
                values[0] = Value::Constant(0);
                continue;
            }
            Inst::Jal { offset, dest } => {
                let target = (pc as u32).wrapping_add(offset.as_u32());
                let symbol_start = symbol.address as u32;
                let symbol_end = symbol_start.wrapping_add(symbol.bytes.len() as u32);
                if dest == Reg::ZERO && target >= symbol_start && target < symbol_end {
                    let Some(target_index) = instruction_indices.get(&target).copied() else {
                        blockers.push(format!(
                            "invalid local jump target at {pc:#x}: {instruction}"
                        ));
                        break;
                    };
                    instruction_index = target_index;
                    values[0] = Value::Constant(0);
                    continue;
                }
                blockers.push(format!("call/jump instruction at {pc:#x}: {instruction}"));
                if target < symbol_start || target >= symbol_end {
                    let arguments =
                        Box::new(core::array::from_fn(|index| values[10 + index].clone()));
                    if dest == Reg::ZERO {
                        let call_token = next_call_token;
                        reference_events.push(ReferenceEvent::TailCall {
                            token: call_token,
                            site: pc as u32,
                            target,
                            arguments,
                        });
                        return_value = Value::CallResult(call_token);
                        break;
                    } else if dest == Reg::RA {
                        let call_token = next_call_token;
                        next_call_token += 1;
                        reference_events.push(ReferenceEvent::Call {
                            token: call_token,
                            site: pc as u32,
                            target,
                            arguments,
                        });
                        structural_finish_call(
                            &mut values,
                            (pc as u32).wrapping_add(u32::from(width)),
                            call_token,
                        );
                    }
                }
            }
            Inst::Jalr { offset, base, dest }
                if matches!(&values[usize::from(base.0)], Value::ExternalFunction { .. }) =>
            {
                let Value::ExternalFunction { table, function } =
                    values[usize::from(base.0)].clone()
                else {
                    unreachable!()
                };
                let slot = external_abi::function(table, function);
                if offset.as_u32() != 0 || !matches!(dest, Reg::ZERO | Reg::RA) {
                    blockers.push(format!(
                        "unsupported external ABI call shape at {pc:#x}: {instruction}"
                    ));
                    break;
                }
                let mut arguments = Box::new(core::array::from_fn(|index| {
                    if index < usize::from(slot.argument_count) {
                        values[10 + index].clone()
                    } else {
                        Value::Constant(0)
                    }
                }));
                let result = match slot.return_model {
                    external_abi::ReturnModel::Constant(value) => Value::Constant(value),
                    external_abi::ReturnModel::SymbolicU32 => {
                        Value::ExternalResult(next_external_call_token)
                    }
                    external_abi::ReturnModel::PrivateStackOutputU8 { pointer_argument } => {
                        let Some(Value::StackAddress(offset)) =
                            arguments.get(usize::from(pointer_argument))
                        else {
                            blockers.push(format!(
                                "call/jump instruction at {pc:#x}: external ABI {}::{}",
                                external_abi::table_spec(table).id,
                                slot.c_name
                            ));
                            reference_blockers.push(format!(
                                "unsupported-external-output-pointer at {pc:#x}: {}::{} argument a{pointer_argument} is not private stack",
                                external_abi::table_spec(table).id,
                                slot.c_name
                            ));
                            break;
                        };
                        let output = Value::ExternalResult(next_external_call_token).and(0xff);
                        stack.store(*offset, 8, &output);
                        // The validated private pointer has already been
                        // consumed by the internal stack effect. Do not let a
                        // callee-local address escape into call composition or
                        // generated behavior.
                        arguments[usize::from(pointer_argument)] = Value::Constant(0);
                        // The C callback returns an int, but this model only
                        // claims its output-byte effect. Any later use of a0
                        // therefore remains fail-closed.
                        Value::Unknown
                    }
                };
                reference_events.push(ReferenceEvent::ExternalCall {
                    token: next_external_call_token,
                    table,
                    function,
                    arguments,
                });
                next_external_call_token += 1;
                if dest == Reg::ZERO {
                    return_value = result;
                    break;
                }
                structural_finish_call_with_result(
                    &mut values,
                    (pc as u32).wrapping_add(u32::from(width)),
                    result,
                );
            }
            Inst::Jalr { offset, base, dest }
                if dest == Reg::ZERO && base == Reg::RA && offset.as_u32() == 0 =>
            {
                return_value = values[usize::from(Reg::A0.0)].clone();
                break;
            }
            Inst::Jalr { .. } => {
                blockers.push(format!("call/jump instruction at {pc:#x}: {instruction}"));
            }
            Inst::Fence { fence } => {
                let event = Event::Fence {
                    fm: fence.fm,
                    predecessor: encode_fence_set(fence.pred),
                    successor: encode_fence_set(fence.succ),
                };
                events.push(event.clone());
                reference_events.push(ReferenceEvent::Observable(event));
            }
            Inst::Ecall
            | Inst::Ebreak
            | Inst::LrW { .. }
            | Inst::ScW { .. }
            | Inst::AmoW { .. } => {
                blockers.push(format!(
                    "unsupported execution edge at {pc:#x}: {instruction}"
                ));
            }
            _ => {
                blockers.push(format!("unsupported instruction at {pc:#x}: {instruction}"));
            }
        }
        values[0] = Value::Constant(0);
        instruction_index += 1;
    }

    Ok(Trace {
        symbol: symbol.name.clone(),
        events,
        reference_events,
        reference_dependencies: Vec::new(),
        blockers,
        reference_blockers,
        return_value,
        reference_flow: None,
        unresolved_branch,
    })
}

#[cfg(test)]
fn is_register(value: &str) -> bool {
    matches!(
        value,
        "zero"
            | "ra"
            | "sp"
            | "gp"
            | "tp"
            | "t0"
            | "t1"
            | "t2"
            | "s0"
            | "fp"
            | "s1"
            | "a0"
            | "a1"
            | "a2"
            | "a3"
            | "a4"
            | "a5"
            | "a6"
            | "a7"
            | "s2"
            | "s3"
            | "s4"
            | "s5"
            | "s6"
            | "s7"
            | "s8"
            | "s9"
            | "s10"
            | "s11"
            | "t3"
            | "t4"
            | "t5"
            | "t6"
    )
}

fn list_code_symbols(artifact: &Path, prefix: &str) -> Result<Vec<ArtifactSymbol>> {
    Ok(binary::load_symbols(artifact, prefix)?
        .into_iter()
        .map(|symbol| ArtifactSymbol {
            member: symbol.member,
            name: symbol.name,
        })
        .collect())
}

#[derive(Clone, Debug)]
struct Input {
    artifact: PathBuf,
    member: Option<String>,
    symbol: String,
}

fn take_value(arguments: &mut impl Iterator<Item = String>, option: &str) -> Result<String> {
    arguments
        .next()
        .ok_or_else(|| format!("{option} requires a value").into())
}

fn parse_input(arguments: &mut impl Iterator<Item = String>, prefix: &str) -> Result<Input> {
    let mut artifact = None;
    let mut member = None;
    let mut symbol = None;
    while let Some(argument) = arguments.next() {
        let plain = prefix.is_empty();
        let artifact_option = if plain {
            "--artifact".to_owned()
        } else {
            format!("--{prefix}-artifact")
        };
        let member_option = if plain {
            "--member".to_owned()
        } else {
            format!("--{prefix}-member")
        };
        let symbol_option = if plain {
            "--symbol".to_owned()
        } else {
            format!("--{prefix}-symbol")
        };
        if argument == artifact_option {
            artifact = Some(PathBuf::from(take_value(arguments, &artifact_option)?));
        } else if argument == member_option {
            member = Some(take_value(arguments, &member_option)?);
        } else if argument == symbol_option {
            symbol = Some(take_value(arguments, &symbol_option)?);
        } else {
            return Err(format!("unknown {prefix} input option: {argument}").into());
        }
        if artifact.is_some() && symbol.is_some() && (!plain || argument == symbol_option) {
            break;
        }
    }
    Ok(Input {
        artifact: artifact.ok_or_else(|| format!("missing --{prefix}-artifact"))?,
        member,
        symbol: symbol.ok_or_else(|| format!("missing --{prefix}-symbol"))?,
    })
}

fn extract(input: &Input, svd: &SvdMap) -> Result<Trace> {
    let symbols = binary::load_symbols(&input.artifact, &input.symbol)?;
    let symbol = symbols
        .iter()
        .find(|candidate| {
            candidate.name == input.symbol
                && input
                    .member
                    .as_deref()
                    .is_none_or(|member| candidate.member.as_deref() == Some(member))
        })
        .ok_or_else(|| {
            format!(
                "symbol {} in member {:?} was not found",
                input.symbol, input.member
            )
        })?;
    trace_binary_symbol(symbol, svd, &BTreeMap::new(), &BTreeMap::new(), None)
}

fn inline_reference_summary(
    prefix: &[ReferenceEvent],
    callee: &Trace,
    arguments: &[Value; 8],
) -> std::result::Result<(Vec<ReferenceEvent>, Value), String> {
    if callee.reference_flow.is_some() {
        return Err(format!(
            "callee {} contains symbolic control flow and must be represented as a scoped call before flattening",
            callee.symbol
        ));
    }
    let mut output = prefix.to_vec();
    let mut next_read_token = prefix
        .iter()
        .filter(|event| reference_event_is_mmio_read(event))
        .count() as u32;
    let mut next_memory_read_token = prefix
        .iter()
        .filter(|event| {
            matches!(
                event,
                ReferenceEvent::Memory {
                    access: Access::Read,
                    ..
                }
            )
        })
        .count() as u32;
    let mut next_external_token = prefix
        .iter()
        .filter(|event| matches!(event, ReferenceEvent::ExternalCall { .. }))
        .count() as u32;
    let mut read_tokens = Vec::new();
    let mut memory_read_tokens = Vec::new();
    let mut external_tokens = Vec::new();

    for event in &callee.reference_events {
        let event = match event {
            ReferenceEvent::Observable(Event::Memory {
                access: Access::Read,
                ..
            }) => {
                read_tokens.push(next_read_token);
                next_read_token += 1;
                event.clone()
            }
            ReferenceEvent::Observable(Event::Memory {
                access: Access::Write,
                width,
                address,
                register,
                value: Some(value),
            }) => {
                let value = value.substitute(
                    arguments,
                    &read_tokens,
                    &memory_read_tokens,
                    &external_tokens,
                )?;
                if !value.is_resolved() {
                    return Err(format!(
                        "callee {} has a write that is unresolved after argument substitution",
                        callee.symbol
                    ));
                }
                ReferenceEvent::Observable(Event::Memory {
                    access: Access::Write,
                    width: *width,
                    address: *address,
                    register: register.clone(),
                    value: Some(value),
                })
            }
            ReferenceEvent::IndexedMmio {
                access,
                width,
                address,
                registers,
                guard,
                value,
            } => {
                let address = address.substitute(
                    arguments,
                    &read_tokens,
                    &memory_read_tokens,
                    &external_tokens,
                )?;
                let guard = guard
                    .as_ref()
                    .map(|guard| -> std::result::Result<IndexedMmioGuard, String> {
                        Ok(IndexedMmioGuard {
                            selector: guard.selector.substitute(
                                arguments,
                                &read_tokens,
                                &memory_read_tokens,
                                &external_tokens,
                            )?,
                            maximum: guard.maximum,
                        })
                    })
                    .transpose()?;
                let value = value
                    .as_ref()
                    .map(|value| {
                        value.substitute(
                            arguments,
                            &read_tokens,
                            &memory_read_tokens,
                            &external_tokens,
                        )
                    })
                    .transpose()?;
                if *access == Access::Read {
                    read_tokens.push(next_read_token);
                    next_read_token += 1;
                }
                ReferenceEvent::IndexedMmio {
                    access: *access,
                    width: *width,
                    address,
                    registers: registers.clone(),
                    guard,
                    value,
                }
            }
            ReferenceEvent::DelayMicros { micros } => {
                let micros = micros.substitute(
                    arguments,
                    &read_tokens,
                    &memory_read_tokens,
                    &external_tokens,
                )?;
                if !micros.is_resolved() {
                    return Err(format!(
                        "callee {} has an unresolved delay after argument substitution",
                        callee.symbol
                    ));
                }
                ReferenceEvent::DelayMicros { micros }
            }
            ReferenceEvent::Memory {
                access: Access::Read,
                width,
                address,
                region,
                value: None,
            } => {
                let address = address.substitute(
                    arguments,
                    &read_tokens,
                    &memory_read_tokens,
                    &external_tokens,
                )?;
                if !address.is_resolved() {
                    return Err(format!(
                        "callee {} has a memory-read address that is unresolved after argument substitution",
                        callee.symbol
                    ));
                }
                memory_read_tokens.push(next_memory_read_token);
                next_memory_read_token += 1;
                ReferenceEvent::Memory {
                    access: Access::Read,
                    width: *width,
                    address,
                    region: region.clone(),
                    value: None,
                }
            }
            ReferenceEvent::Memory {
                access: Access::Write,
                width,
                address,
                region,
                value: Some(value),
            } => {
                let address = address.substitute(
                    arguments,
                    &read_tokens,
                    &memory_read_tokens,
                    &external_tokens,
                )?;
                let value = value.substitute(
                    arguments,
                    &read_tokens,
                    &memory_read_tokens,
                    &external_tokens,
                )?;
                if !address.is_resolved() || !value.is_resolved() {
                    return Err(format!(
                        "callee {} has a memory write that is unresolved after argument substitution",
                        callee.symbol
                    ));
                }
                ReferenceEvent::Memory {
                    access: Access::Write,
                    width: *width,
                    address,
                    region: region.clone(),
                    value: Some(value),
                }
            }
            ReferenceEvent::ExternalCall {
                table,
                function,
                arguments: external_arguments,
                ..
            } => {
                let mapped_arguments = external_arguments
                    .iter()
                    .map(|value| {
                        value.substitute(
                            arguments,
                            &read_tokens,
                            &memory_read_tokens,
                            &external_tokens,
                        )
                    })
                    .collect::<std::result::Result<Vec<_>, _>>()?
                    .try_into()
                    .map_err(|_| "internal external argument count changed".to_owned())?;
                let token = next_external_token;
                next_external_token += 1;
                external_tokens.push(token);
                ReferenceEvent::ExternalCall {
                    token,
                    table: *table,
                    function: *function,
                    arguments: Box::new(mapped_arguments),
                }
            }
            ReferenceEvent::DiagnosticCall {
                function,
                argument_count,
                arguments: diagnostic_arguments,
            } => {
                let mapped_arguments = diagnostic_arguments
                    .iter()
                    .map(|value| {
                        value.substitute(
                            arguments,
                            &read_tokens,
                            &memory_read_tokens,
                            &external_tokens,
                        )
                    })
                    .collect::<std::result::Result<Vec<_>, _>>()?
                    .try_into()
                    .map_err(|_| "internal diagnostic argument count changed".to_owned())?;
                ReferenceEvent::DiagnosticCall {
                    function: function.clone(),
                    argument_count: *argument_count,
                    arguments: Box::new(mapped_arguments),
                }
            }
            ReferenceEvent::TailCall { site, target, .. } => {
                return Err(format!(
                    "callee {} still contains an unresolved call at {site:#010x} to {target:#010x}",
                    callee.symbol
                ));
            }
            ReferenceEvent::Call {
                token,
                site,
                target,
                ..
            } => {
                return Err(format!(
                    "callee {} still contains unresolved call {token} at {site:#010x} to {target:#010x}",
                    callee.symbol
                ));
            }
            _ => event.clone(),
        };
        output.push(event);
    }
    let return_value = callee.return_value.substitute(
        arguments,
        &read_tokens,
        &memory_read_tokens,
        &external_tokens,
    )?;
    Ok((output, return_value))
}

fn reference_intrinsic_trace(symbol: &binary::BinarySymbol) -> Option<Trace> {
    match symbol.name.as_str() {
        "ets_delay_us" => Some(Trace {
            symbol: symbol.name.clone(),
            events: Vec::new(),
            reference_events: vec![ReferenceEvent::DelayMicros {
                micros: Value::input(0),
            }],
            reference_dependencies: Vec::new(),
            blockers: Vec::new(),
            reference_blockers: Vec::new(),
            return_value: Value::Unknown,
            reference_flow: None,
            unresolved_branch: None,
        }),
        _ => None,
    }
}

struct ReferenceCalleeContext<'a> {
    symbols_by_address: &'a BTreeMap<u32, binary::BinarySymbol>,
    relocated_calls: &'a StructuralRelocatedCalls,
    external_pointer_cells: &'a BTreeMap<u32, external_abi::Table>,
    svd: &'a SvdMap,
}

#[derive(Clone, Debug)]
struct ReferencePath {
    events: VecDeque<ReferenceEvent>,
    return_value: Value,
}

fn build_reference_flow(
    mut paths: Vec<ReferencePath>,
) -> std::result::Result<ReferenceFlow, String> {
    if paths.is_empty() {
        return Err("bounded branch exploration produced no complete paths".to_owned());
    }

    let mut events = Vec::new();
    loop {
        let Some(first) = paths[0].events.front().cloned() else {
            if paths.iter().any(|path| !path.events.is_empty()) {
                return Err("symbolic paths do not share a structured event boundary".to_owned());
            }
            let return_value = paths[0].return_value.clone();
            if paths.iter().any(|path| path.return_value != return_value) {
                return Err("symbolic paths merge with incompatible return states".to_owned());
            }
            return Ok(ReferenceFlow {
                events,
                terminator: ReferenceTerminator::Return(return_value),
            });
        };

        if let ReferenceEvent::BranchDecision { condition, .. } = first {
            let mut taken_paths = Vec::new();
            let mut not_taken_paths = Vec::new();
            for mut path in paths {
                let Some(ReferenceEvent::BranchDecision {
                    condition: path_condition,
                    taken,
                }) = path.events.pop_front()
                else {
                    return Err("symbolic paths diverge before a common branch boundary".to_owned());
                };
                if path_condition != condition {
                    return Err(format!(
                        "symbolic paths disagree about branch condition at {:#010x}",
                        condition.site
                    ));
                }
                if taken {
                    taken_paths.push(path);
                } else {
                    not_taken_paths.push(path);
                }
            }
            if taken_paths.is_empty() || not_taken_paths.is_empty() {
                return Err(format!(
                    "branch exploration did not cover both outcomes at {:#010x}",
                    condition.site
                ));
            }
            return Ok(ReferenceFlow {
                events,
                terminator: ReferenceTerminator::Branch {
                    condition,
                    taken: Box::new(build_reference_flow(taken_paths)?),
                    not_taken: Box::new(build_reference_flow(not_taken_paths)?),
                },
            });
        }

        if paths.iter().any(|path| path.events.front() != Some(&first)) {
            return Err("symbolic paths have incompatible observable event prefixes".to_owned());
        }
        for path in &mut paths {
            path.events.pop_front();
        }
        events.push(first);
    }
}

fn explore_reference_flow(
    symbol: &binary::BinarySymbol,
    svd: &SvdMap,
    relocated_calls: &StructuralRelocatedCalls,
    external_pointer_cells: &BTreeMap<u32, external_abi::Table>,
    specialized_arguments: Option<&[Value; 8]>,
) -> std::result::Result<ReferenceFlow, String> {
    const MAX_COMPLETE_PATHS: usize = 64;
    const MAX_EXPLORED_STATES: usize = MAX_COMPLETE_PATHS * 2 - 1;
    const MAX_BRANCH_DECISIONS: usize = 12;

    let mut queue = VecDeque::from([BTreeMap::<u32, bool>::new()]);
    let mut queued = BTreeSet::from([BTreeMap::<u32, bool>::new()]);
    let mut paths = Vec::new();
    let mut explored_states = 0usize;

    while let Some(forced_branches) = queue.pop_front() {
        explored_states += 1;
        if explored_states > MAX_EXPLORED_STATES {
            return Err(format!(
                "symbolic CFG exceeds the exploration limit of {MAX_COMPLETE_PATHS} complete paths"
            ));
        }
        let trace = trace_binary_symbol_with_branches(
            symbol,
            svd,
            relocated_calls,
            external_pointer_cells,
            specialized_arguments,
            &forced_branches,
        )
        .map_err(|error| error.to_string())?;

        let typed_calls = trace
            .reference_events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    ReferenceEvent::Call { .. }
                        | ReferenceEvent::TailCall { .. }
                        | ReferenceEvent::DiagnosticCall { .. }
                )
            })
            .count();
        let call_blockers = trace
            .blockers
            .iter()
            .filter(|blocker| blocker.starts_with("call/jump instruction"))
            .count();

        if let Some(branch) = trace.unresolved_branch {
            let branch_blockers = trace
                .blockers
                .iter()
                .filter(|blocker| blocker.starts_with("input-dependent control-flow"))
                .count();
            if !trace.reference_blockers.is_empty()
                || branch_blockers != 1
                || trace.blockers.len() != call_blockers + branch_blockers
                || typed_calls != call_blockers
            {
                return Err(format!(
                    "path to branch {:#010x} has unsupported effects: {}",
                    branch.site,
                    trace
                        .blockers
                        .iter()
                        .chain(&trace.reference_blockers)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("; ")
                ));
            }
            if forced_branches.len() >= MAX_BRANCH_DECISIONS {
                return Err(format!(
                    "symbolic CFG exceeds the limit of {MAX_BRANCH_DECISIONS} branch decisions per path"
                ));
            }
            for taken in [false, true] {
                let mut next = forced_branches.clone();
                if next.insert(branch.site, taken).is_some() {
                    return Err(format!(
                        "symbolic CFG revisits branch {:#010x}; loops are not supported",
                        branch.site
                    ));
                }
                if queued.insert(next.clone()) {
                    queue.push_back(next);
                }
            }
            continue;
        }

        if !trace.reference_blockers.is_empty()
            || trace.blockers.len() != call_blockers
            || typed_calls != call_blockers
        {
            return Err(format!(
                "symbolic path has unsupported effects: {}",
                trace
                    .blockers
                    .iter()
                    .chain(&trace.reference_blockers)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
        paths.push(ReferencePath {
            events: trace.reference_events.into(),
            return_value: trace.return_value,
        });
        if paths.len() > MAX_COMPLETE_PATHS {
            return Err(format!(
                "symbolic CFG exceeds the exploration limit of {MAX_COMPLETE_PATHS} complete paths"
            ));
        }
    }

    build_reference_flow(paths)
}

fn resolve_reference_callee(
    target: u32,
    site: u32,
    arguments: &[Value; 8],
    context: &ReferenceCalleeContext<'_>,
    visiting: &mut BTreeSet<u32>,
) -> std::result::Result<(String, Trace), String> {
    let callee = context
        .symbols_by_address
        .get(&target)
        .ok_or_else(|| format!("unresolved-call at {site:#010x} to {target:#010x}"))?;
    if let Some(trace) = reference_intrinsic_trace(callee) {
        return Ok((callee.name.clone(), trace));
    }
    if !visiting.insert(target) {
        return Err(format!("recursive-call at {site:#010x} to {}", callee.name));
    }
    let result = resolve_reference_trace(
        callee,
        context.symbols_by_address,
        context.relocated_calls,
        context.external_pointer_cells,
        Some(arguments),
        context.svd,
        visiting,
    )
    .map_err(|error| format!("callee-decode at {site:#010x}: {}: {error}", callee.name));
    visiting.remove(&target);
    let trace = result?;
    if !trace.is_reference_eligible() {
        return Err(format!(
            "callee-ineligible at {site:#010x}: {}",
            callee.name
        ));
    }
    Ok((callee.name.clone(), trace))
}

fn trace_into_reference_flow(mut trace: Trace) -> ReferenceFlow {
    trace.reference_flow.take().unwrap_or(ReferenceFlow {
        events: std::mem::take(&mut trace.reference_events),
        terminator: ReferenceTerminator::Return(trace.return_value),
    })
}

fn compose_calls_in_reference_flow(
    mut flow: ReferenceFlow,
    context: &ReferenceCalleeContext<'_>,
    visiting: &mut BTreeSet<u32>,
    dependencies: &mut Vec<String>,
) -> std::result::Result<ReferenceFlow, String> {
    let mut events = Vec::with_capacity(flow.events.len());
    for event in flow.events {
        let (token, site, target, arguments) = match event {
            ReferenceEvent::Call {
                token,
                site,
                target,
                arguments,
            }
            | ReferenceEvent::TailCall {
                token,
                site,
                target,
                arguments,
            } => (token, site, target, arguments),
            ReferenceEvent::BranchDecision { condition, .. } => {
                return Err(format!(
                    "branch decision at {:#010x} escaped structured flow assembly",
                    condition.site
                ));
            }
            other => {
                events.push(other);
                continue;
            }
        };

        let (callee_name, callee_trace) =
            resolve_reference_callee(target, site, &arguments, context, visiting)?;
        let result_modeled = callee_trace.reference_exit_a0_modeled();
        dependencies.push(callee_name.clone());
        dependencies.extend(callee_trace.reference_dependencies.iter().cloned());
        events.push(ReferenceEvent::ComposedCall {
            token,
            symbol: callee_name,
            arguments,
            flow: Box::new(trace_into_reference_flow(callee_trace)),
            result_modeled,
        });
    }
    flow.events = events;
    flow.terminator = match flow.terminator {
        ReferenceTerminator::Return(value) => ReferenceTerminator::Return(value),
        ReferenceTerminator::Branch {
            condition,
            taken,
            not_taken,
        } => ReferenceTerminator::Branch {
            condition,
            taken: Box::new(compose_calls_in_reference_flow(
                *taken,
                context,
                visiting,
                dependencies,
            )?),
            not_taken: Box::new(compose_calls_in_reference_flow(
                *not_taken,
                context,
                visiting,
                dependencies,
            )?),
        },
    };
    Ok(flow)
}

fn resolve_reference_trace(
    symbol: &binary::BinarySymbol,
    symbols_by_address: &BTreeMap<u32, binary::BinarySymbol>,
    relocated_calls: &StructuralRelocatedCalls,
    external_pointer_cells: &BTreeMap<u32, external_abi::Table>,
    specialized_arguments: Option<&[Value; 8]>,
    svd: &SvdMap,
    visiting: &mut BTreeSet<u32>,
) -> Result<Trace> {
    let mut trace = trace_binary_symbol(
        symbol,
        svd,
        relocated_calls,
        external_pointer_cells,
        specialized_arguments,
    )?;
    let typed_calls = trace
        .reference_events
        .iter()
        .filter(|event| {
            matches!(
                event,
                ReferenceEvent::TailCall { .. }
                    | ReferenceEvent::Call { .. }
                    | ReferenceEvent::DiagnosticCall { .. }
            )
        })
        .count();
    if trace.unresolved_branch.is_some() {
        match explore_reference_flow(
            symbol,
            svd,
            relocated_calls,
            external_pointer_cells,
            specialized_arguments,
        )
        .and_then(|flow| {
            compose_calls_in_reference_flow(
                flow,
                &ReferenceCalleeContext {
                    symbols_by_address,
                    relocated_calls,
                    external_pointer_cells,
                    svd,
                },
                visiting,
                &mut trace.reference_dependencies,
            )
        }) {
            Ok(flow) if reference_flow_calls_are_valid(&flow) => {
                trace.events.clear();
                trace.reference_events.clear();
                trace.blockers.clear();
                trace.reference_flow = Some(flow);
                trace.unresolved_branch = None;
            }
            Ok(_) => trace.reference_blockers.push(
                "symbolic-cfg: composed call result is used without a modeled callee `a0`"
                    .to_owned(),
            ),
            Err(error) => trace
                .reference_blockers
                .push(format!("symbolic-cfg: {error}")),
        }
        return Ok(trace);
    }
    if typed_calls == 0 {
        return Ok(trace);
    }

    let call_blockers = trace
        .blockers
        .iter()
        .filter(|blocker| blocker.starts_with("call/jump instruction"))
        .count();
    if typed_calls != call_blockers {
        trace.reference_blockers.push(format!(
            "unsupported-call-shape: typed-calls={typed_calls} call-blockers={call_blockers}"
        ));
        return Ok(trace);
    }

    let source_events = std::mem::take(&mut trace.reference_events);
    let mut output = Vec::new();
    let mut read_tokens = Vec::new();
    let mut memory_read_tokens = Vec::new();
    let mut external_tokens = Vec::new();
    let mut call_results = BTreeMap::<u32, Value>::new();
    let mut tail_return = None;
    for (index, event) in source_events.iter().enumerate() {
        let result = match event {
            ReferenceEvent::Observable(Event::Memory {
                access: Access::Read,
                ..
            }) => {
                let token = output
                    .iter()
                    .filter(|event| reference_event_is_mmio_read(event))
                    .count() as u32;
                read_tokens.push(token);
                output.push(event.clone());
                Ok(())
            }
            ReferenceEvent::Observable(Event::Memory {
                access: Access::Write,
                width,
                address,
                register,
                value: Some(value),
            }) => value
                .rewrite_call_context(
                    &read_tokens,
                    &memory_read_tokens,
                    &external_tokens,
                    &call_results,
                )
                .and_then(|value| {
                    value.is_resolved().then_some(value).ok_or_else(|| {
                        format!("MMIO write after a call remains unresolved at {address:#010x}")
                    })
                })
                .map(|value| {
                    output.push(ReferenceEvent::Observable(Event::Memory {
                        access: Access::Write,
                        width: *width,
                        address: *address,
                        register: register.clone(),
                        value: Some(value),
                    }));
                }),
            ReferenceEvent::IndexedMmio {
                access,
                width,
                address,
                registers,
                guard,
                value,
            } => (|| -> std::result::Result<(), String> {
                let address = address.rewrite_call_context(
                    &read_tokens,
                    &memory_read_tokens,
                    &external_tokens,
                    &call_results,
                )?;
                let guard = guard
                    .as_ref()
                    .map(|guard| -> std::result::Result<IndexedMmioGuard, String> {
                        Ok(IndexedMmioGuard {
                            selector: guard.selector.rewrite_call_context(
                                &read_tokens,
                                &memory_read_tokens,
                                &external_tokens,
                                &call_results,
                            )?,
                            maximum: guard.maximum,
                        })
                    })
                    .transpose()?;
                let value = value
                    .as_ref()
                    .map(|value| {
                        value.rewrite_call_context(
                            &read_tokens,
                            &memory_read_tokens,
                            &external_tokens,
                            &call_results,
                        )
                    })
                    .transpose()?;
                if *access == Access::Read {
                    let token = output
                        .iter()
                        .filter(|event| reference_event_is_mmio_read(event))
                        .count() as u32;
                    read_tokens.push(token);
                }
                output.push(ReferenceEvent::IndexedMmio {
                    access: *access,
                    width: *width,
                    address,
                    registers: registers.clone(),
                    guard,
                    value,
                });
                Ok(())
            })(),
            ReferenceEvent::DelayMicros { micros } => micros
                .rewrite_call_context(
                    &read_tokens,
                    &memory_read_tokens,
                    &external_tokens,
                    &call_results,
                )
                .and_then(|micros| {
                    micros
                        .is_resolved()
                        .then_some(micros)
                        .ok_or_else(|| "delay after a call remains unresolved".to_owned())
                })
                .map(|micros| output.push(ReferenceEvent::DelayMicros { micros })),
            ReferenceEvent::Memory {
                access: Access::Read,
                width,
                address,
                region,
                value: None,
            } => (|| -> std::result::Result<(), String> {
                let address = address.rewrite_call_context(
                    &read_tokens,
                    &memory_read_tokens,
                    &external_tokens,
                    &call_results,
                )?;
                if !address.is_resolved() {
                    return Err("memory-read address after a call remains unresolved".to_owned());
                }
                let token = output
                    .iter()
                    .filter(|event| {
                        matches!(
                            event,
                            ReferenceEvent::Memory {
                                access: Access::Read,
                                ..
                            }
                        )
                    })
                    .count() as u32;
                memory_read_tokens.push(token);
                output.push(ReferenceEvent::Memory {
                    access: Access::Read,
                    width: *width,
                    address,
                    region: region.clone(),
                    value: None,
                });
                Ok(())
            })(),
            ReferenceEvent::Memory {
                access: Access::Write,
                width,
                address,
                region,
                value: Some(value),
            } => (|| -> std::result::Result<(), String> {
                let address = address.rewrite_call_context(
                    &read_tokens,
                    &memory_read_tokens,
                    &external_tokens,
                    &call_results,
                )?;
                let value = value.rewrite_call_context(
                    &read_tokens,
                    &memory_read_tokens,
                    &external_tokens,
                    &call_results,
                )?;
                if !address.is_resolved() || !value.is_resolved() {
                    return Err("memory write after a call remains unresolved".to_owned());
                }
                output.push(ReferenceEvent::Memory {
                    access: Access::Write,
                    width: *width,
                    address,
                    region: region.clone(),
                    value: Some(value),
                });
                Ok(())
            })(),
            ReferenceEvent::Observable(Event::Fence { .. }) => {
                output.push(event.clone());
                Ok(())
            }
            ReferenceEvent::ExternalCall {
                token,
                table,
                function,
                arguments,
            } => (|| -> std::result::Result<(), String> {
                let arguments = arguments
                    .iter()
                    .map(|value| {
                        value.rewrite_call_context(
                            &read_tokens,
                            &memory_read_tokens,
                            &external_tokens,
                            &call_results,
                        )
                    })
                    .collect::<std::result::Result<Vec<_>, _>>()?
                    .try_into()
                    .map_err(|_| "internal external argument count changed".to_owned())?;
                let mapped_token = output
                    .iter()
                    .filter(|event| matches!(event, ReferenceEvent::ExternalCall { .. }))
                    .count() as u32;
                external_tokens.push(mapped_token);
                output.push(ReferenceEvent::ExternalCall {
                    token: mapped_token,
                    table: *table,
                    function: *function,
                    arguments: Box::new(arguments),
                });
                if usize::try_from(*token).ok() != Some(external_tokens.len() - 1) {
                    return Err(format!(
                        "external call token {token} is not ordered in the source trace"
                    ));
                }
                Ok(())
            })(),
            ReferenceEvent::DiagnosticCall {
                function,
                argument_count,
                arguments,
            } => (|| -> std::result::Result<(), String> {
                let arguments = arguments
                    .iter()
                    .map(|value| {
                        value.rewrite_call_context(
                            &read_tokens,
                            &memory_read_tokens,
                            &external_tokens,
                            &call_results,
                        )
                    })
                    .collect::<std::result::Result<Vec<_>, _>>()?
                    .try_into()
                    .map_err(|_| "internal diagnostic argument count changed".to_owned())?;
                output.push(ReferenceEvent::DiagnosticCall {
                    function: function.clone(),
                    argument_count: *argument_count,
                    arguments: Box::new(arguments),
                });
                Ok(())
            })(),
            ReferenceEvent::Call {
                token: source_call_token,
                site,
                target,
                arguments,
            }
            | ReferenceEvent::TailCall {
                token: source_call_token,
                site,
                target,
                arguments,
            } => {
                let is_tail = matches!(event, ReferenceEvent::TailCall { .. });
                if is_tail && index + 1 != source_events.len() {
                    Err(format!(
                        "tail-call-not-terminal at {site:#010x} to {target:#010x}"
                    ))
                } else {
                    (|| -> std::result::Result<(), String> {
                        let arguments = arguments
                            .iter()
                            .map(|value| {
                                value.rewrite_call_context(
                                    &read_tokens,
                                    &memory_read_tokens,
                                    &external_tokens,
                                    &call_results,
                                )
                            })
                            .collect::<std::result::Result<Vec<_>, _>>()?
                            .try_into()
                            .map_err(|_| "internal call argument count changed".to_owned())?;
                        let (callee_name, callee_trace) = resolve_reference_callee(
                            *target,
                            *site,
                            &arguments,
                            &ReferenceCalleeContext {
                                symbols_by_address,
                                relocated_calls,
                                external_pointer_cells,
                                svd,
                            },
                            visiting,
                        )?;
                        let requires_scoped_call = callee_trace.reference_flow.is_some()
                            || callee_trace
                                .reference_events
                                .iter()
                                .any(|event| matches!(event, ReferenceEvent::ComposedCall { .. }));
                        let callee_dependencies = callee_trace.reference_dependencies.clone();
                        trace.reference_dependencies.push(callee_name.clone());
                        trace.reference_dependencies.extend(callee_dependencies);
                        if requires_scoped_call {
                            let result_modeled = callee_trace.reference_exit_a0_modeled();
                            let mapped_token = output
                                .iter()
                                .filter(|event| {
                                    matches!(event, ReferenceEvent::ComposedCall { .. })
                                })
                                .count() as u32;
                            output.push(ReferenceEvent::ComposedCall {
                                token: mapped_token,
                                symbol: callee_name,
                                arguments: Box::new(arguments),
                                flow: Box::new(trace_into_reference_flow(callee_trace)),
                                result_modeled,
                            });
                            let return_value = if result_modeled {
                                Value::CallResult(mapped_token)
                            } else {
                                Value::Unknown
                            };
                            if is_tail {
                                tail_return = Some(return_value);
                            } else {
                                call_results.insert(*source_call_token, return_value);
                            }
                        } else {
                            let (events, return_value) =
                                inline_reference_summary(&output, &callee_trace, &arguments)?;
                            output = events;
                            if is_tail {
                                tail_return = Some(return_value);
                            } else {
                                call_results.insert(*source_call_token, return_value);
                            }
                        }
                        Ok(())
                    })()
                }
            }
            _ => Err("internal reference event has an invalid value shape".to_owned()),
        };
        if let Err(error) = result {
            trace.reference_events = source_events.clone();
            trace
                .reference_blockers
                .push(format!("call-summary-flattening: {error}"));
            return Ok(trace);
        }
    }

    trace.return_value = if let Some(value) = tail_return {
        value
    } else {
        match trace.return_value.rewrite_call_context(
            &read_tokens,
            &memory_read_tokens,
            &external_tokens,
            &call_results,
        ) {
            Ok(value) => value,
            Err(error) => {
                trace.reference_events = source_events.clone();
                trace
                    .reference_blockers
                    .push(format!("call-return-flattening: {error}"));
                return Ok(trace);
            }
        }
    };
    trace.reference_events = output;
    trace
        .blockers
        .retain(|blocker| !blocker.starts_with("call/jump instruction"));
    Ok(trace)
}

struct ReferenceCatalog {
    symbols: Vec<binary::BinarySymbol>,
    symbols_by_address: BTreeMap<u32, binary::BinarySymbol>,
    symbol_ids: BTreeMap<(Option<String>, String), u32>,
    relocated_calls: StructuralRelocatedCalls,
    external_pointer_cells: BTreeMap<u32, external_abi::Table>,
}

impl ReferenceCatalog {
    fn load(artifact: &Path, companions: &[PathBuf]) -> Result<Self> {
        let symbols = binary::load_symbols(artifact, "")?;
        let mut symbols_by_address = symbols
            .iter()
            .filter(|symbol| symbol.addresses_resolved)
            .map(|symbol| (symbol.address as u32, symbol.clone()))
            .collect::<BTreeMap<_, _>>();
        let mut symbol_ids = symbols
            .iter()
            .filter(|symbol| symbol.addresses_resolved)
            .map(|symbol| {
                (
                    (symbol.member.clone(), symbol.name.clone()),
                    symbol.address as u32,
                )
            })
            .collect::<BTreeMap<_, _>>();
        let mut next_archive_symbol_id = 0x8000_0000_u32;
        for symbol in symbols.iter().filter(|symbol| !symbol.addresses_resolved) {
            while symbols_by_address.contains_key(&next_archive_symbol_id) {
                next_archive_symbol_id = next_archive_symbol_id.wrapping_add(1);
            }
            let identity = (symbol.member.clone(), symbol.name.clone());
            if symbol_ids
                .insert(identity.clone(), next_archive_symbol_id)
                .is_some()
            {
                return Err(format!(
                    "duplicate archive symbol identity {:?}::{}",
                    identity.0, identity.1
                )
                .into());
            }
            symbols_by_address.insert(next_archive_symbol_id, symbol.clone());
            next_archive_symbol_id = next_archive_symbol_id.wrapping_add(1);
        }
        let mut image = if symbols.iter().any(|symbol| symbol.addresses_resolved) {
            Some(emulator::ExecutableImage::load(artifact)?)
        } else {
            None
        };
        for companion in companions {
            let Some(image) = image.as_mut() else {
                return Err(format!(
                    "reference companions require a linked ELF primary artifact: {}",
                    artifact.display()
                )
                .into());
            };
            image.add_companion(companion)?;
            symbols_by_address.extend(
                binary::load_symbols(companion, "")?
                    .into_iter()
                    .filter(|symbol| symbol.addresses_resolved)
                    .map(|symbol| (symbol.address as u32, symbol)),
            );
        }
        let external_pointer_cells = image.as_ref().map_or_else(BTreeMap::new, |image| {
            external_abi::all_tables()
                .into_iter()
                .filter_map(|table| {
                    image
                        .symbol_address(external_abi::table_spec(table).pointer_symbol)
                        .map(|address| (address, table))
                })
                .collect()
        });
        let mut relocated_calls = StructuralRelocatedCalls::new();
        if let Some(image) = image.as_ref() {
            for (address, call) in image.relocated_calls() {
                let Some(owner) = symbols_by_address.values().find(|symbol| {
                    symbol.addresses_resolved
                        && address >= symbol.address as u32
                        && address < (symbol.address as u32).wrapping_add(symbol.bytes.len() as u32)
                }) else {
                    continue;
                };
                relocated_calls.insert(StructuralCallSite::new(owner, address), call);
            }
        }

        let mut archive_definitions = BTreeMap::<String, Vec<(Option<String>, u32)>>::new();
        for symbol in symbols.iter().filter(|symbol| !symbol.addresses_resolved) {
            let identity = (symbol.member.clone(), symbol.name.clone());
            archive_definitions
                .entry(symbol.name.clone())
                .or_default()
                .push((
                    symbol.member.clone(),
                    *symbol_ids
                        .get(&identity)
                        .expect("every archive symbol received a synthetic identity"),
                ));
        }
        for owner in symbols.iter().filter(|symbol| !symbol.addresses_resolved) {
            for relocation in owner.relocations.iter().filter(|relocation| {
                matches!(
                    relocation.kind,
                    binary::RelocationKind::Call | binary::RelocationKind::CallPlt
                )
            }) {
                let candidates = archive_definitions
                    .get(&relocation.symbol)
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                let same_member = candidates
                    .iter()
                    .filter(|(member, _)| member == &owner.member)
                    .map(|(_, target)| *target)
                    .collect::<Vec<_>>();
                let target = if relocation.addend != 0 {
                    None
                } else if same_member.len() == 1 {
                    Some(same_member[0])
                } else if candidates.len() == 1 {
                    Some(candidates[0].1)
                } else {
                    None
                };
                relocated_calls.insert(
                    StructuralCallSite::new(owner, relocation.address),
                    (relocation.symbol.clone(), target),
                );
            }
        }
        Ok(Self {
            symbols,
            symbols_by_address,
            symbol_ids,
            relocated_calls,
            external_pointer_cells,
        })
    }

    fn trace(&self, member: Option<&str>, name: &str, svd: &SvdMap) -> Result<Trace> {
        let symbol = self
            .symbols
            .iter()
            .find(|candidate| {
                candidate.name == name
                    && member.is_none_or(|member| candidate.member.as_deref() == Some(member))
            })
            .ok_or_else(|| format!("symbol {name} in member {member:?} was not found"))?;
        let identity = (symbol.member.clone(), symbol.name.clone());
        let symbol_id = *self
            .symbol_ids
            .get(&identity)
            .expect("catalog lookup returned a symbol without an identity");
        let mut visiting = BTreeSet::from([symbol.address as u32, symbol_id]);
        resolve_reference_trace(
            symbol,
            &self.symbols_by_address,
            &self.relocated_calls,
            &self.external_pointer_cells,
            None,
            svd,
            &mut visiting,
        )
    }
}

fn extract_reference(input: &Input, companions: &[PathBuf], svd: &SvdMap) -> Result<Trace> {
    ReferenceCatalog::load(&input.artifact, companions)?.trace(
        input.member.as_deref(),
        &input.symbol,
        svd,
    )
}

fn print_trace(trace: &Trace) {
    println!("TRACE\t{}\texact={}", trace.symbol, trace.is_exact());
    for (index, event) in trace.events.iter().enumerate() {
        println!("{index}\t{}", event.canonical());
    }
    for blocker in &trace.blockers {
        println!("BLOCKER\t{blocker}");
    }
}

fn returns_equal(left: &Trace, right: &Trace) -> bool {
    left.return_value.is_resolved()
        && right.return_value.is_resolved()
        && left.return_value.canonical() == right.return_value.canonical()
}

fn traces_equal(left: &Trace, right: &Trace) -> bool {
    left.events.len() == right.events.len()
        && left
            .events
            .iter()
            .zip(&right.events)
            .all(|(left, right)| left.equivalent(right))
}

fn print_uncovered(symbol: &str, side: &str, trace: &Trace) -> usize {
    let mut count = 0;
    for blocker in &trace.blockers {
        println!("UNCOVERED\t{symbol}\t{side}\t{blocker}");
        count += 1;
    }
    for address in trace.events.iter().filter_map(Event::unmapped_address) {
        println!(
            "UNCOVERED\t{symbol}\t{side}\tunmapped-register {:#010x}",
            address
        );
        count += 1;
    }
    count
}

fn usage() {
    eprintln!(
        "usage:\n  open-esp-radio-phy-trace execute --svd PATH [--svd PATH]... --artifact PATH [--companion PATH] --symbol NAME [--concrete-only] [--timeline] [--arg VALUE] [--mmio ADDRESS=VALUE] [--read ADDRESS=VALUE] [--ram ADDRESS=VALUE] [--observe ADDRESS=LENGTH] [--max-steps COUNT]\n  open-esp-radio-phy-trace execute-compare --svd PATH [--svd PATH]... --vendor-artifact PATH [--vendor-companion PATH] --vendor-symbol NAME --rust-artifact PATH [--rust-companion PATH] --rust-symbol NAME [--compare-return] [--case NAME [--arg VALUE] [--mmio ADDRESS=VALUE] [--read ADDRESS=VALUE] [--ram ADDRESS=VALUE] [--vendor-ram-symbol ADDRESS=SYMBOL] [--rust-ram-symbol ADDRESS=SYMBOL] [--observe ADDRESS=LENGTH] [--max-steps COUNT]]...\n  open-esp-radio-phy-trace qualify-esp32s31-channel --svd PATH [--svd PATH]... --vendor-artifact PATH --vendor-companion PATH\n  open-esp-radio-phy-trace qualify-esp32s31-rf-init --svd PATH [--svd PATH]... --vendor-artifact PATH --vendor-companion PATH\n  open-esp-radio-phy-trace verify-profiles --svd PATH [--svd PATH]... --profiles PATH --vendor-artifact PATH [--vendor-companion PATH] --rust-artifact PATH [--rust-companion PATH]\n  open-esp-radio-phy-trace analyze --svd PATH [--svd PATH]... --artifact PATH [--companion PATH]... [--symbol-prefix PREFIX]\n  open-esp-radio-phy-trace generate-reference --svd PATH [--svd PATH]... --artifact PATH [--companion PATH]... [--member NAME] --symbol NAME [--output PATH]\n  open-esp-radio-phy-trace verify --svd PATH [--svd PATH]... --vendor-artifact PATH [--vendor-inventory PATH] --rust-artifact PATH [--profiles PATH] [--vendor-companion PATH] [--rust-companion PATH] [--vendor-prefix PREFIX] [--rust-prefix PREFIX] [--gate completion|regression] [--match-floor COUNT] [--evidence-baseline PATH]\n  open-esp-radio-phy-trace verify-all --svd PATH [--svd PATH]... --rom-artifact PATH --archive-artifact PATH --archive-inventory PATH --rust-artifact PATH [--profiles PATH] [--dispositions PATH] [--rom-companion PATH] [--archive-companion PATH] [--rust-companion PATH] [--rom-prefix PREFIX] [--archive-prefix PREFIX] [--rust-prefix PREFIX] [--gate completion|regression] [--match-floor COUNT] [--evidence-baseline PATH] [--json-report PATH]\n  open-esp-radio-phy-trace extract --svd PATH [--svd PATH]... --artifact PATH [--member NAME] --symbol NAME\n  open-esp-radio-phy-trace compare --svd PATH [--svd PATH]... --left-artifact PATH [--left-member NAME] --left-symbol NAME --right-artifact PATH [--right-member NAME] --right-symbol NAME"
    );
}

fn parse_assignment(value: &str, option: &str) -> Result<(u32, u32)> {
    let (address, value) = value
        .split_once('=')
        .ok_or_else(|| format!("{option} requires ADDRESS=VALUE"))?;
    let address = parse_u32(address).ok_or_else(|| format!("invalid {option} address"))?;
    let value = parse_u32(value).ok_or_else(|| format!("invalid {option} value"))?;
    Ok((address, value))
}

fn parse_symbol_word(value: &str, option: &str) -> Result<SymbolWord> {
    let (address, symbol) = value
        .split_once('=')
        .ok_or_else(|| format!("{option} requires ADDRESS=SYMBOL"))?;
    let address = parse_u32(address).ok_or_else(|| format!("invalid {option} address"))?;
    if symbol.is_empty() {
        return Err(format!("{option} requires a non-empty symbol").into());
    }
    Ok(SymbolWord {
        address,
        symbol: symbol.to_owned(),
    })
}

fn parse_symbol_observation(value: &str, option: &str) -> Result<MemoryObservation> {
    let (target, length) = value
        .split_once('=')
        .ok_or_else(|| format!("{option} requires SYMBOL[+OFFSET]=LENGTH"))?;
    let length = parse_u32(length).ok_or_else(|| format!("invalid {option} length"))?;
    if length == 0 {
        return Err(format!("{option} length must be non-zero").into());
    }
    let (symbol, offset) = target
        .split_once('+')
        .map_or((target, 0), |(symbol, offset)| {
            (symbol, parse_u32(offset).unwrap_or(u32::MAX))
        });
    if symbol.is_empty() || offset == u32::MAX {
        return Err(format!("invalid {option} symbol or offset").into());
    }
    Ok(MemoryObservation::Symbol {
        symbol: symbol.to_owned(),
        offset,
        length,
    })
}

fn seed_ram_word(scenario: &mut emulator::Scenario, address: u32, value: u32) {
    write_ram_word(scenario, address, value);
    scenario.observed_memory.push(emulator::MemoryRange {
        start: address,
        length: 4,
    });
}

fn write_ram_word(scenario: &mut emulator::Scenario, address: u32, value: u32) {
    for (offset, byte) in value.to_le_bytes().into_iter().enumerate() {
        scenario
            .memory_initial
            .insert(address.wrapping_add(offset as u32), byte);
    }
}

fn observe_memory(scenario: &mut emulator::Scenario, address: u32, length: u32) -> Result<()> {
    if length == 0 {
        return Err("--observe length must be non-zero".into());
    }
    scenario.observed_memory.push(emulator::MemoryRange {
        start: address,
        length,
    });
    Ok(())
}

#[derive(Clone, Debug)]
struct SymbolWord {
    address: u32,
    symbol: String,
}

#[derive(Clone, Debug)]
enum MemoryObservation {
    Absolute {
        address: u32,
        length: u32,
    },
    Symbol {
        symbol: String,
        offset: u32,
        length: u32,
    },
}

impl MemoryObservation {
    const fn length(&self) -> u32 {
        match self {
            Self::Absolute { length, .. } | Self::Symbol { length, .. } => *length,
        }
    }
}

#[derive(Clone, Debug)]
struct NamedScenario {
    name: String,
    scenario: emulator::Scenario,
    vendor_symbol_words: Vec<SymbolWord>,
    rust_symbol_words: Vec<SymbolWord>,
    vendor_ram_words: Vec<(u32, u32)>,
    rust_ram_words: Vec<(u32, u32)>,
    vendor_observations: Vec<MemoryObservation>,
    rust_observations: Vec<MemoryObservation>,
}

impl NamedScenario {
    fn new(name: String) -> Self {
        Self {
            name,
            scenario: emulator::Scenario::default(),
            vendor_symbol_words: Vec::new(),
            rust_symbol_words: Vec::new(),
            vendor_ram_words: Vec::new(),
            rust_ram_words: Vec::new(),
            vendor_observations: Vec::new(),
            rust_observations: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ComparisonVerdict {
    Match,
    Mismatch,
    Incomplete,
}

impl ComparisonVerdict {
    const fn label(self) -> &'static str {
        match self {
            Self::Match => "MATCH",
            Self::Mismatch => "MISMATCH",
            Self::Incomplete => "INCOMPLETE",
        }
    }
}

fn print_execution_event(side: &str, index: usize, event: &emulator::ExecutionEvent) {
    match event {
        emulator::ExecutionEvent::Read {
            width,
            address,
            register,
            value,
        } => println!(
            "TRACE-EVENT\t{side}\t{index}\tR\t{width}\t{address:#010x}\t{register}\tvalue={value:#010x}"
        ),
        emulator::ExecutionEvent::Write {
            width,
            address,
            register,
            value,
        } => println!(
            "TRACE-EVENT\t{side}\t{index}\tW\t{width}\t{address:#010x}\t{register}\tvalue={value:#010x}",
        ),
        emulator::ExecutionEvent::DelayMicros(micros) => {
            println!("TRACE-EVENT\t{side}\t{index}\tDELAY\tmicros={micros}");
        }
        emulator::ExecutionEvent::Fence {
            fm,
            predecessor,
            successor,
        } => println!(
            "TRACE-EVENT\t{side}\t{index}\tFENCE\tfm={fm:#x}\tpred={predecessor:#x}\tsucc={successor:#x}"
        ),
    }
}

fn unmapped_execution_address(event: &emulator::ExecutionEvent) -> Option<u32> {
    match event {
        emulator::ExecutionEvent::Read {
            address, register, ..
        }
        | emulator::ExecutionEvent::Write {
            address, register, ..
        } if register == "UNMAPPED" => Some(*address),
        _ => None,
    }
}

fn print_branch_coverage(
    side: &str,
    image: &emulator::ExecutableImage,
    required: &BTreeSet<(u32, bool)>,
    covered: &BTreeSet<(u32, bool)>,
) -> usize {
    let mut uncovered = 0;
    for (site, taken) in required {
        let location = image.location(*site);
        if covered.contains(&(*site, *taken)) {
            println!("COVERED-BRANCH\t{side}\t{location}\ttaken={taken}");
        } else {
            println!("UNCOVERED-BRANCH\t{side}\t{location}\ttaken={taken}");
            uncovered += 1;
        }
    }
    let sites: BTreeSet<_> = required.iter().map(|(site, _)| *site).collect();
    println!(
        "SUMMARY-BRANCHES\t{side}\tsites={}\toutcomes={}\tcovered={}\tuncovered={uncovered}",
        sites.len(),
        required.len(),
        required.len() - uncovered,
    );
    uncovered
}

fn extend_dynamic_inventory(
    image: &emulator::ExecutableImage,
    inventory: &mut emulator::CoverageInventory,
    indirect_calls: &BTreeSet<emulator::IndirectCall>,
) -> Result<()> {
    for call in indirect_calls {
        let dynamic =
            image.coverage_inventory_with_arguments(&call.symbol, Some(&call.arguments))?;
        inventory.branch_sites.extend(dynamic.branch_sites);
        inventory.branch_outcomes.extend(dynamic.branch_outcomes);
        inventory.unresolved_edges.extend(dynamic.unresolved_edges);
    }
    Ok(())
}

fn print_control_flow_coverage(
    side: &str,
    image: &emulator::ExecutableImage,
    inventory: &emulator::CoverageInventory,
    indirect_calls: &BTreeSet<emulator::IndirectCall>,
) -> usize {
    let mut uncovered = 0;
    for (address, edge) in &inventory.unresolved_edges {
        let targets: Vec<_> = indirect_calls
            .iter()
            .filter_map(|call| (call.site == *address).then_some(call.symbol.as_str()))
            .collect();
        if targets.is_empty() {
            println!(
                "UNCOVERED-CONTROL-FLOW\t{side}\t{}\t{edge}",
                image.location(*address)
            );
            uncovered += 1;
        } else {
            println!(
                "COVERED-CONTROL-FLOW\t{side}\t{}\ttargets={}",
                image.location(*address),
                targets.join(",")
            );
        }
    }
    uncovered
}

#[derive(Clone, Copy)]
struct ExecutionInput<'a> {
    artifact: &'a Path,
    companion: Option<&'a Path>,
    symbol: &'a str,
}

fn resolved_scenario(
    named: &NamedScenario,
    image: &emulator::ExecutableImage,
    vendor: bool,
) -> Result<emulator::Scenario> {
    let mut scenario = named.scenario.clone();
    let words = if vendor {
        &named.vendor_symbol_words
    } else {
        &named.rust_symbol_words
    };
    let ram_words = if vendor {
        &named.vendor_ram_words
    } else {
        &named.rust_ram_words
    };
    for (address, value) in ram_words {
        write_ram_word(&mut scenario, *address, *value);
    }
    for word in words {
        let value = image.symbol_address(&word.symbol).ok_or_else(|| {
            format!(
                "scenario {} refers to missing {} symbol {}",
                named.name,
                if vendor { "vendor" } else { "Rust" },
                word.symbol
            )
        })?;
        seed_ram_word(&mut scenario, word.address, value);
    }
    let observations = if vendor {
        &named.vendor_observations
    } else {
        &named.rust_observations
    };
    let mut comparison_start = 0_u32;
    for observation in observations {
        let (start, length) = match observation {
            MemoryObservation::Absolute { address, length } => (*address, *length),
            MemoryObservation::Symbol {
                symbol,
                offset,
                length,
            } => {
                let address = image.symbol_address(symbol).ok_or_else(|| {
                    format!(
                        "scenario {} refers to missing {} observation symbol {}",
                        named.name,
                        if vendor { "vendor" } else { "Rust" },
                        symbol
                    )
                })?;
                (address.wrapping_add(*offset), *length)
            }
        };
        scenario.memory_aliases.push(emulator::MemoryAlias {
            start,
            length,
            comparison_start,
        });
        comparison_start = comparison_start
            .checked_add(length)
            .ok_or("normalized observation length overflow")?;
    }
    Ok(scenario)
}

fn compare_execution_scenarios(
    svd: &SvdMap,
    vendor: ExecutionInput<'_>,
    rust: ExecutionInput<'_>,
    compare_return: bool,
    scenarios: &[NamedScenario],
) -> Result<ComparisonVerdict> {
    let vendor_digest = pinned_vendor_digest(vendor.artifact)?;
    println!(
        "ORACLE\t{}\tsha256={vendor_digest}",
        vendor.artifact.display()
    );
    if let Some(companion) = vendor.companion {
        let companion_digest = pinned_vendor_digest(companion)?;
        println!("ORACLE\t{}\tsha256={companion_digest}", companion.display());
    }
    let mut vendor_image = emulator::ExecutableImage::load(vendor.artifact)?;
    if let Some(companion) = vendor.companion {
        vendor_image.add_companion(companion)?;
    }
    let mut rust_image = emulator::ExecutableImage::load(rust.artifact)?;
    if let Some(companion) = rust.companion {
        rust_image.add_companion(companion)?;
    }
    let mut vendor_inventory = vendor_image.coverage_inventory(vendor.symbol)?;
    let mut rust_inventory = rust_image.coverage_inventory(rust.symbol)?;
    let mut vendor_covered = BTreeSet::new();
    let mut rust_covered = BTreeSet::new();
    let mut vendor_calls = BTreeSet::new();
    let mut rust_calls = BTreeSet::new();
    let mut vendor_indirect_calls = BTreeSet::new();
    let mut rust_indirect_calls = BTreeSet::new();
    let mut vendor_unmapped = BTreeSet::new();
    let mut rust_unmapped = BTreeSet::new();
    let mut matched_cases = 0_usize;
    let mut mismatched_cases = 0_usize;
    let mut incomplete_cases = 0_usize;

    for named in scenarios {
        let vendor_lengths: Vec<_> = named
            .vendor_observations
            .iter()
            .map(MemoryObservation::length)
            .collect();
        let rust_lengths: Vec<_> = named
            .rust_observations
            .iter()
            .map(MemoryObservation::length)
            .collect();
        if vendor_lengths != rust_lengths {
            return Err(format!(
                "scenario {} has different vendor/Rust observation layouts",
                named.name
            )
            .into());
        }
        let vendor_result = emulator::execute(
            &vendor_image,
            svd,
            vendor.symbol,
            resolved_scenario(named, &vendor_image, true)?,
        );
        let rust_result = emulator::execute(
            &rust_image,
            svd,
            rust.symbol,
            resolved_scenario(named, &rust_image, false)?,
        );
        let (vendor_result, rust_result) = match (vendor_result, rust_result) {
            (Ok(vendor_result), Ok(rust_result)) => (vendor_result, rust_result),
            (vendor_result, rust_result) => {
                incomplete_cases += 1;
                println!(
                    "CASE\t{}\tINCOMPLETE\tvendor={}\trust={}",
                    named.name,
                    vendor_result
                        .err()
                        .map_or_else(|| "complete".to_owned(), |error| error.to_string()),
                    rust_result
                        .err()
                        .map_or_else(|| "complete".to_owned(), |error| error.to_string()),
                );
                continue;
            }
        };
        vendor_covered.extend(vendor_result.branches.iter().copied());
        rust_covered.extend(rust_result.branches.iter().copied());
        vendor_calls.extend(vendor_result.calls.iter().cloned());
        rust_calls.extend(rust_result.calls.iter().cloned());
        vendor_indirect_calls.extend(vendor_result.indirect_calls.iter().cloned());
        rust_indirect_calls.extend(rust_result.indirect_calls.iter().cloned());
        vendor_unmapped.extend(
            vendor_result
                .events
                .iter()
                .filter_map(unmapped_execution_address),
        );
        rust_unmapped.extend(
            rust_result
                .events
                .iter()
                .filter_map(unmapped_execution_address),
        );

        let events_equal = vendor_result.events == rust_result.events;
        let memory_equal = vendor_result.memory_changes == rust_result.memory_changes;
        let returns_equal =
            !compare_return || vendor_result.return_value == rust_result.return_value;
        if events_equal && memory_equal && returns_equal {
            matched_cases += 1;
            println!(
                "CASE\t{}\tMATCH\tevents={}\tmemory-changes={}\treturn={}",
                named.name,
                vendor_result.events.len(),
                vendor_result.memory_changes.len(),
                if compare_return { "checked" } else { "ignored" }
            );
        } else {
            mismatched_cases += 1;
            println!(
                "CASE\t{}\tMISMATCH\tvendor-events={}\trust-events={}\tvendor-memory-changes={}\trust-memory-changes={}\tvendor-return={:#010x}\trust-return={:#010x}",
                named.name,
                vendor_result.events.len(),
                rust_result.events.len(),
                vendor_result.memory_changes.len(),
                rust_result.memory_changes.len(),
                vendor_result.return_value,
                rust_result.return_value,
            );
            for (index, event) in vendor_result.events.iter().enumerate() {
                print_execution_event("vendor", index, event);
            }
            for (index, event) in rust_result.events.iter().enumerate() {
                print_execution_event("rust", index, event);
            }
            for change in &vendor_result.memory_changes {
                println!(
                    "MEMORY-CHANGE\tvendor\t{:#010x}\tbefore={:#04x}\tafter={:#04x}",
                    change.address, change.before, change.after
                );
            }
            for change in &rust_result.memory_changes {
                println!(
                    "MEMORY-CHANGE\trust\t{:#010x}\tbefore={:#04x}\tafter={:#04x}",
                    change.address, change.before, change.after
                );
            }
        }
    }

    for call in vendor_calls {
        println!("COVERED-CALL\tvendor\t{call}");
    }
    for call in rust_calls {
        println!("COVERED-CALL\trust\t{call}");
    }
    extend_dynamic_inventory(&vendor_image, &mut vendor_inventory, &vendor_indirect_calls)?;
    extend_dynamic_inventory(&rust_image, &mut rust_inventory, &rust_indirect_calls)?;
    let vendor_uncovered = print_branch_coverage(
        "vendor",
        &vendor_image,
        &vendor_inventory.branch_outcomes,
        &vendor_covered,
    );
    let rust_uncovered = print_branch_coverage(
        "rust",
        &rust_image,
        &rust_inventory.branch_outcomes,
        &rust_covered,
    );
    let vendor_unresolved = print_control_flow_coverage(
        "vendor",
        &vendor_image,
        &vendor_inventory,
        &vendor_indirect_calls,
    );
    let rust_unresolved =
        print_control_flow_coverage("rust", &rust_image, &rust_inventory, &rust_indirect_calls);
    for address in &vendor_unmapped {
        println!("UNCOVERED-MMIO\tvendor\t{address:#010x}");
    }
    for address in &rust_unmapped {
        println!("UNCOVERED-MMIO\trust\t{address:#010x}");
    }
    let cases_match = matched_cases == scenarios.len();
    let coverage_complete = vendor_uncovered == 0
        && rust_uncovered == 0
        && vendor_unresolved == 0
        && rust_unresolved == 0
        && vendor_unmapped.is_empty()
        && rust_unmapped.is_empty();
    let verdict = if mismatched_cases != 0 {
        ComparisonVerdict::Mismatch
    } else if incomplete_cases != 0 || !coverage_complete || !cases_match {
        ComparisonVerdict::Incomplete
    } else {
        ComparisonVerdict::Match
    };
    println!(
        "SUMMARY\tcases={}\tmatched={matched_cases}\tmismatched={mismatched_cases}\tincomplete={incomplete_cases}\tvendor-uncovered-branch-outcomes={vendor_uncovered}\trust-uncovered-branch-outcomes={rust_uncovered}\tvendor-unresolved-control-flow={}\trust-unresolved-control-flow={}\tvendor-unmapped-mmio={}\trust-unmapped-mmio={}",
        scenarios.len(),
        vendor_unresolved,
        rust_unresolved,
        vendor_unmapped.len(),
        rust_unmapped.len(),
    );
    println!("VERDICT\t{}", verdict.label());
    Ok(verdict)
}

#[derive(Clone, Copy)]
struct VerifySource<'a> {
    name: &'a str,
    artifact: &'a Path,
    inventory: Option<&'a Path>,
    companion: Option<&'a Path>,
    prefix: &'a str,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct VerifySummary {
    vendor_functions: usize,
    matched: usize,
    symbolic_matches: usize,
    scenario_matches: usize,
    state_matches: usize,
    composition_matches: usize,
    mismatched: usize,
    incomplete: usize,
    missing: usize,
    implemented_unqualified: usize,
    not_yet_ported: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum VerificationGate {
    Completion,
    Regression { match_floor: usize },
}

impl VerificationGate {
    fn parse(name: &str, match_floor: Option<usize>) -> Result<Self> {
        match (name, match_floor) {
            ("completion", None) => Ok(Self::Completion),
            ("completion", Some(_)) => Err("--match-floor requires --gate regression".into()),
            ("regression", Some(match_floor)) => Ok(Self::Regression { match_floor }),
            ("regression", None) => Err("--gate regression requires --match-floor".into()),
            _ => Err(format!("unsupported verification gate {name:?}").into()),
        }
    }

    const fn passes(self, summary: VerifySummary, orphan_probes: usize) -> bool {
        match self {
            Self::Completion => summary.is_complete() && orphan_probes == 0,
            Self::Regression { match_floor } => {
                summary.mismatched == 0
                    && summary.incomplete == 0
                    && summary.matched >= match_floor
                    && orphan_probes == 0
            }
        }
    }

    fn report(self, passed: bool) {
        let result = if passed { "PASS" } else { "FAIL" };
        match self {
            Self::Completion => println!("GATE\tcompletion\t{result}"),
            Self::Regression { match_floor } => {
                println!("GATE\tregression\t{result}\tmatch-floor={match_floor}");
            }
        }
    }
}

impl VerifySummary {
    const fn is_complete(self) -> bool {
        self.mismatched == 0 && self.incomplete == 0 && self.missing == 0
    }

    fn add(&mut self, other: Self) {
        self.vendor_functions += other.vendor_functions;
        self.matched += other.matched;
        self.symbolic_matches += other.symbolic_matches;
        self.scenario_matches += other.scenario_matches;
        self.state_matches += other.state_matches;
        self.composition_matches += other.composition_matches;
        self.mismatched += other.mismatched;
        self.incomplete += other.incomplete;
        self.missing += other.missing;
        self.implemented_unqualified += other.implemented_unqualified;
        self.not_yet_ported += other.not_yet_ported;
    }
}

fn vendor_symbols(source: VerifySource<'_>) -> Result<Vec<ArtifactSymbol>> {
    list_code_symbols(source.inventory.unwrap_or(source.artifact), source.prefix)
}

fn print_protocol_inventory(
    manifest: &dispositions::Manifest,
    sources: &[(&str, &[ArtifactSymbol])],
) {
    let mut shared = 0;
    let mut wifi = 0;
    let mut bluetooth = 0;
    let mut ble = 0;
    let mut coex = 0;
    let mut ieee802154 = 0;
    let mut unknown = 0;
    for (source, symbols) in sources {
        for symbol in *symbols {
            match manifest.resolve(source, &symbol.name).protocol {
                dispositions::Protocol::Shared => shared += 1,
                dispositions::Protocol::Wifi => wifi += 1,
                dispositions::Protocol::Bluetooth => bluetooth += 1,
                dispositions::Protocol::Ble => ble += 1,
                dispositions::Protocol::Coex => coex += 1,
                dispositions::Protocol::Ieee802154 => ieee802154 += 1,
                dispositions::Protocol::Unknown => unknown += 1,
            }
        }
    }
    println!(
        "PROTOCOL-INVENTORY\tshared={shared}\twifi={wifi}\tbluetooth={bluetooth}\tble={ble}\tcoex={coex}\tieee802154={ieee802154}\tunknown={unknown}\texact-dispositions={}",
        manifest.entries().count()
    );
}

#[allow(
    clippy::too_many_arguments,
    reason = "source verification keeps all artifact and policy inputs explicit"
)]
fn verify_source(
    svd: &SvdMap,
    source: VerifySource<'_>,
    rust_artifact: &Path,
    rust_companion: Option<&Path>,
    rust_prefix: &str,
    execution_profiles: &[profiles::Profile],
    disposition_manifest: Option<&dispositions::Manifest>,
    evidence: &mut EvidenceSet,
) -> Result<VerifySummary> {
    let vendor_digest = pinned_vendor_digest(source.artifact)?;
    println!(
        "ORACLE\t{}\t{}\tsha256={vendor_digest}",
        source.name,
        source.artifact.display()
    );
    if let Some(inventory) = source.inventory.filter(|path| *path != source.artifact) {
        let inventory_digest = pinned_vendor_digest(inventory)?;
        println!(
            "ORACLE\t{}-inventory\t{}\tsha256={inventory_digest}",
            source.name,
            inventory.display()
        );
    }
    if let Some(companion) = source.companion {
        let companion_digest = pinned_vendor_digest(companion)?;
        println!(
            "ORACLE\t{}-companion\t{}\tsha256={companion_digest}",
            source.name,
            companion.display()
        );
    }
    let vendor_symbols = vendor_symbols(source)?;
    let rust_symbols = list_code_symbols(rust_artifact, rust_prefix)?;
    let mut profiled_vendor_symbols = BTreeSet::new();
    for profile in execution_profiles {
        if profile.vendor_source != source.name && source.name != "vendor" {
            return Err(format!(
                "profile {} targets {}, but was routed to {}",
                profile.name, profile.vendor_source, source.name
            )
            .into());
        }
        if !profiled_vendor_symbols.insert(profile.vendor_symbol.as_str()) {
            return Err(format!(
                "multiple execution profiles target {} in {}",
                profile.vendor_symbol, source.name
            )
            .into());
        }
        if !vendor_symbols
            .iter()
            .any(|symbol| symbol.name == profile.vendor_symbol)
        {
            return Err(format!(
                "profile {} refers to missing {} vendor symbol {}",
                profile.name, source.name, profile.vendor_symbol
            )
            .into());
        }
        if !rust_symbols
            .iter()
            .any(|symbol| symbol.name == profile.rust_symbol)
        {
            return Err(format!(
                "profile {} refers to missing Rust symbol {}",
                profile.name, profile.rust_symbol
            )
            .into());
        }
    }
    let mut rust_by_suffix = HashMap::new();
    for symbol in &rust_symbols {
        let Some(suffix) = symbol.name.strip_prefix(rust_prefix) else {
            continue;
        };
        let (suffix, compare_return) = suffix
            .strip_prefix("ret_")
            .map_or((suffix, false), |suffix| (suffix, true));
        if let Some((previous, _)) = rust_by_suffix.insert(suffix, (symbol, compare_return)) {
            return Err(format!(
                "Rust probe suffix {suffix:?} is ambiguous between {} and {}",
                previous.name, symbol.name
            )
            .into());
        }
    }

    let mut summary = VerifySummary {
        vendor_functions: vendor_symbols.len(),
        ..VerifySummary::default()
    };
    for vendor in &vendor_symbols {
        let suffix = vendor
            .name
            .strip_prefix(source.prefix)
            .expect("symbol was filtered by vendor prefix");
        let source_qualified_suffix = format!("{}_{suffix}", source.name);
        let Some((rust, compare_return)) = rust_by_suffix
            .get(source_qualified_suffix.as_str())
            .or_else(|| rust_by_suffix.get(suffix))
        else {
            if let Some(manifest) = disposition_manifest {
                let resolved = manifest.resolve(source.name, &vendor.name);
                if resolved.disposition.is_implemented() {
                    let entry = resolved
                        .entry
                        .expect("implemented disposition must be an exact function entry");
                    if let Some(contract) = entry.semantic_contract {
                        let matched = match contract {
                            dispositions::SemanticContract::Esp32s31Channel => {
                                if source.name != "archive" || vendor.name != "phy_chip_set_chan" {
                                    return Err(format!(
                                        "semantic contract {} cannot qualify {} {}",
                                        contract.label(),
                                        source.name,
                                        vendor.name,
                                    )
                                    .into());
                                }
                                let companion = source.companion.ok_or_else(|| {
                                    format!(
                                        "semantic contract {} requires an archive companion",
                                        contract.label()
                                    )
                                })?;
                                qualify_esp32s31_channel(svd, source.artifact, companion, false)?
                            }
                            dispositions::SemanticContract::Esp32s31RfInit => {
                                if source.name != "archive" || vendor.name != "phy_rf_init" {
                                    return Err(format!(
                                        "semantic contract {} cannot qualify {} {}",
                                        contract.label(),
                                        source.name,
                                        vendor.name,
                                    )
                                    .into());
                                }
                                let companion = source.companion.ok_or_else(|| {
                                    format!(
                                        "semantic contract {} requires an archive companion",
                                        contract.label()
                                    )
                                })?;
                                qualify_esp32s31_rf_init(svd, source.artifact, companion, false)?
                            }
                            dispositions::SemanticContract::Esp32s31BluetoothTxDc => {
                                if source.name != "archive" || vendor.name != "phy_bt_txdc_cal_new"
                                {
                                    return Err(format!(
                                        "semantic contract {} cannot qualify {} {}",
                                        contract.label(),
                                        source.name,
                                        vendor.name,
                                    )
                                    .into());
                                }
                                let companion = source.companion.ok_or_else(|| {
                                    format!(
                                        "semantic contract {} requires an archive companion",
                                        contract.label()
                                    )
                                })?;
                                qualify_esp32s31_bluetooth_txdc(
                                    svd,
                                    source.artifact,
                                    companion,
                                    false,
                                )?
                            }
                            dispositions::SemanticContract::Esp32s31BluetoothTxDcPwdet => {
                                if source.name != "archive"
                                    || vendor.name != "phy_txdc_cal_pwdet_init"
                                {
                                    return Err(format!(
                                        "semantic contract {} cannot qualify {} {}",
                                        contract.label(),
                                        source.name,
                                        vendor.name,
                                    )
                                    .into());
                                }
                                let companion = source.companion.ok_or_else(|| {
                                    format!(
                                        "semantic contract {} requires an archive companion",
                                        contract.label()
                                    )
                                })?;
                                qualify_esp32s31_bluetooth_txdc_pwdet(
                                    svd,
                                    source.artifact,
                                    companion,
                                    false,
                                )?
                            }
                            dispositions::SemanticContract::Esp32s31BluetoothTxPower => {
                                if source.name != "archive"
                                    || vendor.name != "phy_bt_tx_pwctrl_init"
                                {
                                    return Err(format!(
                                        "semantic contract {} cannot qualify {} {}",
                                        contract.label(),
                                        source.name,
                                        vendor.name,
                                    )
                                    .into());
                                }
                                let companion = source.companion.ok_or_else(|| {
                                    format!(
                                        "semantic contract {} requires an archive companion",
                                        contract.label()
                                    )
                                })?;
                                qualify_esp32s31_bluetooth_tx_power(
                                    svd,
                                    source.artifact,
                                    companion,
                                    false,
                                )?
                            }
                        };
                        if matched {
                            summary.matched += 1;
                            summary.composition_matches += 1;
                            record_evidence(
                                evidence,
                                source.name,
                                &vendor.name,
                                semantic_contract_evidence(contract.label()),
                            )?;
                            println!(
                                "FUNCTION\t{}\t{}\tMATCH\trust-component={}\tevidence=composition-state-scenario\tcontract={}\thil-evidence={}",
                                source.name,
                                vendor.name,
                                entry
                                    .rust_component
                                    .as_deref()
                                    .expect("implemented entry has a Rust component"),
                                contract.label(),
                                entry.hil_evidence.as_deref().unwrap_or("none"),
                            );
                        } else {
                            summary.mismatched += 1;
                            println!(
                                "FUNCTION\t{}\t{}\tMISMATCH\trust-component={}\tevidence=composition-state-scenario\tcontract={}",
                                source.name,
                                vendor.name,
                                entry
                                    .rust_component
                                    .as_deref()
                                    .expect("implemented entry has a Rust component"),
                                contract.label(),
                            );
                        }
                    } else {
                        summary.missing += 1;
                        summary.implemented_unqualified += 1;
                        let qualification_blockers = entry
                            .qualification_blockers
                            .iter()
                            .map(|(source, symbol)| format!("{source}:{symbol}"))
                            .collect::<Vec<_>>()
                            .join(",");
                        println!(
                            "FUNCTION\t{}\t{}\tIMPLEMENTED-UNQUALIFIED\tdisposition={}\tprotocol={}\trust-component={}\thil-evidence={}\tqualification-blockers={}\tmissing-semantic-contract",
                            source.name,
                            vendor.name,
                            resolved.disposition.label(),
                            resolved.protocol.label(),
                            entry
                                .rust_component
                                .as_deref()
                                .expect("implemented entry has a Rust component"),
                            entry.hil_evidence.as_deref().unwrap_or("none"),
                            if qualification_blockers.is_empty() {
                                "none"
                            } else {
                                &qualification_blockers
                            },
                        );
                    }
                } else {
                    summary.missing += 1;
                    summary.not_yet_ported += 1;
                    println!(
                        "FUNCTION\t{}\t{}\tUNCOVERED\tdisposition={}\tprotocol={}\tmissing-rust-probe {}{suffix} or {}{source_qualified_suffix}",
                        source.name,
                        vendor.name,
                        resolved.disposition.label(),
                        resolved.protocol.label(),
                        rust_prefix,
                        rust_prefix,
                    );
                }
            } else {
                summary.missing += 1;
                println!(
                    "FUNCTION\t{}\t{}\tUNCOVERED\tmissing-rust-probe {}{suffix} or {}{source_qualified_suffix}",
                    source.name, vendor.name, rust_prefix, rust_prefix
                );
            }
            continue;
        };
        let vendor_trace = extract(
            &Input {
                artifact: source.artifact.to_path_buf(),
                member: source
                    .inventory
                    .map_or_else(|| vendor.member.clone(), |_| None),
                symbol: vendor.name.clone(),
            },
            svd,
        )?;
        let rust_trace = extract(
            &Input {
                artifact: rust_artifact.to_path_buf(),
                member: rust.member.clone(),
                symbol: rust.name.clone(),
            },
            svd,
        )?;
        if let Some(profile) = execution_profiles
            .iter()
            .find(|profile| profile.vendor_symbol == vendor.name)
        {
            println!("PROFILE\t{}\t{}\tBEGIN", source.name, profile.name);
            let verdict = compare_execution_scenarios(
                svd,
                ExecutionInput {
                    artifact: source.artifact,
                    companion: source.companion,
                    symbol: &profile.vendor_symbol,
                },
                ExecutionInput {
                    artifact: rust_artifact,
                    companion: rust_companion,
                    symbol: &profile.rust_symbol,
                },
                profile.compare_return,
                &profile.scenarios,
            )?;
            match verdict {
                ComparisonVerdict::Match => {
                    summary.matched += 1;
                    match profile.contract {
                        profiles::ProfileContract::Scenario => summary.scenario_matches += 1,
                        profiles::ProfileContract::State => summary.state_matches += 1,
                    }
                    record_evidence(
                        evidence,
                        source.name,
                        &vendor.name,
                        profile_evidence(profile),
                    )?;
                }
                ComparisonVerdict::Mismatch => summary.mismatched += 1,
                ComparisonVerdict::Incomplete => summary.incomplete += 1,
            }
            println!(
                "FUNCTION\t{}\t{}\t{}\trust={}\tevidence={}\tbranch-outcomes=complete\tprofile={}",
                source.name,
                vendor.name,
                verdict.label(),
                rust.name,
                profile.contract.evidence(),
                profile.name
            );
            continue;
        }
        if !vendor_trace.is_exact()
            || !rust_trace.is_exact()
            || (*compare_return
                && (!vendor_trace.return_value.is_resolved()
                    || !rust_trace.return_value.is_resolved()))
        {
            summary.incomplete += 1;
            let mut uncovered = print_uncovered(&vendor.name, source.name, &vendor_trace)
                + print_uncovered(&vendor.name, "rust", &rust_trace);
            if *compare_return && !vendor_trace.return_value.is_resolved() {
                println!(
                    "UNCOVERED\t{}\t{}\tvendor\tunresolved-return",
                    source.name, vendor.name
                );
                uncovered += 1;
            }
            if *compare_return && !rust_trace.return_value.is_resolved() {
                println!(
                    "UNCOVERED\t{}\t{}\trust\tunresolved-return",
                    source.name, vendor.name
                );
                uncovered += 1;
            }
            println!(
                "FUNCTION\t{}\t{}\tINCOMPLETE\trust={}\tuncovered={uncovered}",
                source.name, vendor.name, rust.name
            );
        } else if traces_equal(&vendor_trace, &rust_trace)
            && (!*compare_return || returns_equal(&vendor_trace, &rust_trace))
        {
            summary.matched += 1;
            summary.symbolic_matches += 1;
            record_evidence(evidence, source.name, &vendor.name, "symbolic")?;
            println!(
                "FUNCTION\t{}\t{}\tMATCH\trust={}\tevidence=symbolic\tevents={}\treturn={}",
                source.name,
                vendor.name,
                rust.name,
                vendor_trace.events.len(),
                if *compare_return { "checked" } else { "void" }
            );
        } else {
            summary.mismatched += 1;
            println!(
                "FUNCTION\t{}\t{}\tMISMATCH\trust={}\tvendor-events={}\trust-events={}",
                source.name,
                vendor.name,
                rust.name,
                vendor_trace.events.len(),
                rust_trace.events.len()
            );
        }
    }
    println!(
        "SOURCE-SUMMARY\t{}\tvendor-functions={}\tmatch={}\tsymbolic-match={}\tscenario-match={}\tstate-match={}\tcomposition-match={}\tmismatch={}\tincomplete={}\tmissing-rust-probe={}\timplemented-unqualified={}\tnot-yet-ported={}",
        source.name,
        summary.vendor_functions,
        summary.matched,
        summary.symbolic_matches,
        summary.scenario_matches,
        summary.state_matches,
        summary.composition_matches,
        summary.mismatched,
        summary.incomplete,
        summary.missing,
        summary.implemented_unqualified,
        summary.not_yet_ported,
    );
    Ok(summary)
}

fn orphan_probe_count(
    rust_artifact: &Path,
    rust_prefix: &str,
    sources: &[(VerifySource<'_>, &[ArtifactSymbol])],
) -> Result<usize> {
    let rust_symbols = list_code_symbols(rust_artifact, rust_prefix)?;
    Ok(rust_symbols
        .iter()
        .filter(|rust| {
            let suffix = rust
                .name
                .strip_prefix(rust_prefix)
                .expect("symbol was filtered by Rust prefix");
            let suffix = suffix.strip_prefix("ret_").unwrap_or(suffix);
            !sources.iter().any(|(source, symbols)| {
                symbols.iter().any(|vendor| {
                    vendor
                        .name
                        .strip_prefix(source.prefix)
                        .is_some_and(|vendor_suffix| {
                            rust_probe_suffix_matches(source.name, vendor_suffix, suffix)
                        })
                })
            })
        })
        .count())
}

fn rust_probe_suffix_matches(source: &str, vendor_suffix: &str, rust_suffix: &str) -> bool {
    rust_suffix == vendor_suffix
        || rust_suffix
            .strip_prefix(source)
            .and_then(|suffix| suffix.strip_prefix('_'))
            == Some(vendor_suffix)
}

fn qualify_esp32s31_channel(
    svd: &SvdMap,
    vendor_artifact: &Path,
    vendor_companion: &Path,
    print_oracles: bool,
) -> Result<bool> {
    let artifact_digest = pinned_vendor_digest(vendor_artifact)?;
    let companion_digest = pinned_vendor_digest(vendor_companion)?;
    if print_oracles {
        println!(
            "ORACLE\tarchive\t{}\tsha256={artifact_digest}",
            vendor_artifact.display()
        );
        println!(
            "ORACLE\trom-companion\t{}\tsha256={companion_digest}",
            vendor_companion.display()
        );
    }

    let mut image = emulator::ExecutableImage::load(vendor_artifact)?;
    image.add_companion(vendor_companion)?;
    let phy_param = image
        .symbol_address("phy_param")
        .ok_or("vendor artifact has no phy_param symbol")?;
    let phy_functions_pointer = image
        .symbol_address("g_phyFuns")
        .ok_or("vendor artifact has no g_phyFuns symbol")?;

    let mut cases = Vec::new();
    for channel in 1_u16..=13 {
        cases.push((format!("channel-{channel}-cbw-0"), channel, 0_u8));
        cases.push((format!("channel-{channel}-cbw-1"), channel, 1_u8));
        cases.push((
            format!("frequency-{}-cbw-0", 2_407 + channel * 5),
            2_407 + channel * 5,
            0_u8,
        ));
    }
    for frequency in [2_413_u16, 2_439, 2_476] {
        for cbw in [0_u8, 1] {
            cases.push((
                format!("off-grid-frequency-{frequency}-cbw-{cbw}"),
                frequency,
                cbw,
            ));
        }
    }
    // A reproducible generated tail exercises state carry-over under a less
    // regular sequence than the reviewed edge matrix. Keep the seed and LCG
    // fixed so a failure is directly replayable from its printed case name.
    let mut generated = 0x6d2b_79f5_u32;
    for index in 0..32 {
        generated = generated
            .wrapping_mul(1_664_525)
            .wrapping_add(1_013_904_223);
        let frequency = 2_412 + ((generated >> 8) % 65) as u16;
        let cbw = (generated >> 31) as u8;
        cases.push((
            format!("generated-{index:02}-seed-{generated:08x}-frequency-{frequency}-cbw-{cbw}"),
            frequency,
            cbw,
        ));
    }

    let mut all_branches = BTreeSet::new();
    let mut all_calls = BTreeSet::new();
    let mut total_steps = 0_u64;
    let mut passed = 0_usize;
    let mut reported_full_diff = false;
    let total = cases.len();
    let mut vendor_session = emulator::ExecutionSession::default();
    let mut rust_state = open_esp_radio_esp32s31_phy::phy_cold::PhyColdState::new();
    for (case_index, (name, channel_or_frequency, cbw)) in cases.into_iter().enumerate() {
        let mut scenario = semantic::vendor_channel_scenario(
            channel_or_frequency,
            cbw,
            phy_param,
            phy_functions_pointer,
        )?;
        scenario.reset_policy = if case_index == 0 {
            emulator::ResetPolicy::ColdBoot
        } else {
            emulator::ResetPolicy::Continue
        };
        let result = vendor_session.execute(&image, svd, "phy_chip_set_chan", scenario)?;
        let unmapped: BTreeSet<_> = result
            .events
            .iter()
            .filter_map(unmapped_execution_address)
            .collect();
        if !unmapped.is_empty() {
            for address in &unmapped {
                println!("SEMANTIC-UNCOVERED\t{name}\tunmapped-mmio\t{address:#010x}");
            }
            println!(
                "QUALIFICATION-CASE\t{name}\tINCOMPLETE\tunmapped-mmio={}",
                unmapped.len()
            );
            continue;
        }
        let footprint = semantic::vendor_channel_state_footprint(&result, phy_param)?;
        let vendor_events =
            semantic::normalize_vendor_channel(&image, &result, phy_param, channel_or_frequency)?;
        let (rust_events, next_state) =
            semantic::rust_channel_events_with_state(rust_state, channel_or_frequency, cbw)?;
        rust_state = next_state;
        if vendor_events != rust_events {
            let divergence = vendor_events
                .iter()
                .zip(&rust_events)
                .position(|(vendor, rust)| vendor != rust)
                .unwrap_or_else(|| vendor_events.len().min(rust_events.len()));
            println!(
                "SEMANTIC-DIFF\t{name}\tindex={divergence}\tvendor={:?}\trust={:?}",
                vendor_events.get(divergence),
                rust_events.get(divergence),
            );
            if !reported_full_diff {
                for (index, event) in vendor_events.iter().enumerate() {
                    println!("SEMANTIC-EVENT\t{name}\tvendor\t{index}\t{event:?}");
                }
                for (index, event) in rust_events.iter().enumerate() {
                    println!("SEMANTIC-EVENT\t{name}\trust\t{index}\t{event:?}");
                }
                reported_full_diff = true;
            }
            println!(
                "QUALIFICATION-CASE\t{name}\tMISMATCH\tvendor-events={}\trust-events={}",
                vendor_events.len(),
                rust_events.len(),
            );
            continue;
        }

        passed += 1;
        total_steps = total_steps.saturating_add(result.steps);
        all_branches.extend(result.branches.iter().copied());
        all_calls.extend(result.calls.iter().cloned());
        println!(
            "QUALIFICATION-CASE\t{name}\tSTATE-SCENARIO-MATCH\tevents={}\tsteps={}\tbranch-outcomes={}\tbranch-events={}\tcalls={}\tcall-events={}\tstate-read-bytes={}\tstate-written-bytes={}\tstate-ranges={}",
            vendor_events.len(),
            result.steps,
            result.branches.len(),
            result.ordered_branches.len(),
            result.calls.len(),
            result.ordered_calls.len(),
            footprint.read_bytes,
            footprint.written_bytes,
            footprint.classified_ranges,
        );
    }
    let verdict = if passed == total {
        "STATE-SCENARIO-MATCH"
    } else {
        "FAIL"
    };
    println!(
        "QUALIFICATION-SUMMARY\tphy_chip_set_chan\t{verdict}\tscenarios={total}\tmatched={passed}\tfailed={}\tsteps={total_steps}\tbranch-outcomes={}\tcalls={}",
        total - passed,
        all_branches.len(),
        all_calls.len(),
    );
    Ok(passed == total)
}

fn qualify_esp32s31_rf_init(
    svd: &SvdMap,
    vendor_artifact: &Path,
    vendor_companion: &Path,
    print_oracles: bool,
) -> Result<bool> {
    let artifact_digest = pinned_vendor_digest(vendor_artifact)?;
    let companion_digest = pinned_vendor_digest(vendor_companion)?;
    if print_oracles {
        println!(
            "ORACLE\tarchive\t{}\tsha256={artifact_digest}",
            vendor_artifact.display()
        );
        println!(
            "ORACLE\trom-companion\t{}\tsha256={companion_digest}",
            vendor_companion.display()
        );
    }

    let mut image = emulator::ExecutableImage::load(vendor_artifact)?;
    image.add_companion(vendor_companion)?;
    let phy_param = image
        .symbol_address("phy_param")
        .ok_or("vendor artifact has no phy_param symbol")?;
    let phy_functions_pointer = image
        .symbol_address("g_phyFuns")
        .ok_or("vendor artifact has no g_phyFuns symbol")?;

    let mut vendor_session = emulator::ExecutionSession::default();
    let mut rust_state = open_esp_radio_esp32s31_phy::phy_cold::PhyColdState::new();
    let mut passed = 0_usize;
    let mut total_steps = 0_u64;
    let mut all_branches = BTreeSet::new();
    let mut all_calls = BTreeSet::new();
    let cases = ["cold-image", "retained-state"];
    for (case_index, name) in cases.into_iter().enumerate() {
        let mut scenario = semantic::vendor_rf_init_scenario(phy_param, phy_functions_pointer);
        scenario.reset_policy = if case_index == 0 {
            emulator::ResetPolicy::ColdBoot
        } else {
            emulator::ResetPolicy::Continue
        };
        let result = vendor_session.execute(&image, svd, "phy_rf_init", scenario)?;
        let unmapped: BTreeSet<_> = result
            .events
            .iter()
            .filter_map(unmapped_execution_address)
            .collect();
        if !unmapped.is_empty() {
            for address in &unmapped {
                println!("SEMANTIC-UNCOVERED\t{name}\tunmapped-mmio\t{address:#010x}");
            }
            println!(
                "QUALIFICATION-CASE\t{name}\tINCOMPLETE\tunmapped-mmio={}",
                unmapped.len()
            );
            continue;
        }

        let footprint = semantic::vendor_rf_init_state_footprint(&result, phy_param)?;
        let vendor_events = semantic::normalize_vendor_rf_init(&image, &result, phy_param)?;
        let (rust_events, next_state) = semantic::rust_rf_init_events(rust_state)?;
        rust_state = next_state;
        if vendor_events != rust_events {
            let divergence = vendor_events
                .iter()
                .zip(&rust_events)
                .position(|(vendor, rust)| vendor != rust)
                .unwrap_or_else(|| vendor_events.len().min(rust_events.len()));
            println!(
                "SEMANTIC-DIFF\t{name}\tindex={divergence}\tvendor={:?}\trust={:?}",
                vendor_events.get(divergence),
                rust_events.get(divergence),
            );
            for (index, event) in vendor_events.iter().enumerate() {
                println!("SEMANTIC-EVENT\t{name}\tvendor\t{index}\t{event:?}");
            }
            for (index, event) in rust_events.iter().enumerate() {
                println!("SEMANTIC-EVENT\t{name}\trust\t{index}\t{event:?}");
            }
            println!(
                "QUALIFICATION-CASE\t{name}\tMISMATCH\tvendor-events={}\trust-events={}",
                vendor_events.len(),
                rust_events.len(),
            );
            continue;
        }

        let retained_rc = vendor_session
            .byte(&image, phy_param + 0xa6)
            .ok_or("persistent vendor session lost phy_param RC state")?
            & 0x80
            != 0;
        if retained_rc != rust_state.rc_calibration_complete() {
            println!(
                "QUALIFICATION-CASE\t{name}\tMISMATCH\tpersistent-rc-vendor={retained_rc}\tpersistent-rc-rust={}",
                rust_state.rc_calibration_complete()
            );
            continue;
        }

        passed += 1;
        total_steps = total_steps.saturating_add(result.steps);
        all_branches.extend(result.branches.iter().copied());
        all_calls.extend(result.calls.iter().cloned());
        println!(
            "QUALIFICATION-CASE\t{name}\tSTATE-SEQUENCE-MATCH\tevents={}\tsteps={}\tbranch-outcomes={}\tbranch-events={}\tcalls={}\tcall-events={}\tstate-read-bytes={}\tstate-written-bytes={}\tstate-ranges={}",
            vendor_events.len(),
            result.steps,
            result.branches.len(),
            result.ordered_branches.len(),
            result.calls.len(),
            result.ordered_calls.len(),
            footprint.read_bytes,
            footprint.written_bytes,
            footprint.classified_ranges,
        );
    }

    let verdict = if passed == cases.len() {
        "STATE-SEQUENCE-MATCH"
    } else {
        "FAIL"
    };
    println!(
        "QUALIFICATION-SUMMARY\tphy_rf_init\t{verdict}\tscenarios={}\tmatched={passed}\tfailed={}\tsteps={total_steps}\tbranch-outcomes={}\tcalls={}",
        cases.len(),
        cases.len() - passed,
        all_branches.len(),
        all_calls.len(),
    );
    Ok(passed == cases.len())
}

fn qualify_esp32s31_bluetooth_txdc(
    svd: &SvdMap,
    vendor_artifact: &Path,
    vendor_companion: &Path,
    print_oracles: bool,
) -> Result<bool> {
    let artifact_digest = pinned_vendor_digest(vendor_artifact)?;
    let companion_digest = pinned_vendor_digest(vendor_companion)?;
    if print_oracles {
        println!(
            "ORACLE\tarchive\t{}\tsha256={artifact_digest}",
            vendor_artifact.display()
        );
        println!(
            "ORACLE\trom-companion\t{}\tsha256={companion_digest}",
            vendor_companion.display()
        );
    }

    let mut image = emulator::ExecutableImage::load(vendor_artifact)?;
    image.add_companion(vendor_companion)?;
    let phy_param = image
        .symbol_address("phy_param")
        .ok_or("vendor artifact has no phy_param symbol")?;
    let phy_functions_pointer = image
        .symbol_address("g_phyFuns")
        .ok_or("vendor artifact has no g_phyFuns symbol")?;
    let scenario = semantic::vendor_bluetooth_txdc_scenario(phy_param, phy_functions_pointer);
    let result = emulator::execute(&image, svd, "phy_bt_txdc_cal_new", scenario)?;
    let unmapped: BTreeSet<_> = result
        .events
        .iter()
        .filter_map(unmapped_execution_address)
        .collect();
    if !unmapped.is_empty() {
        for address in &unmapped {
            println!("SEMANTIC-UNCOVERED\tbluetooth-txdc\tunmapped-mmio\t{address:#010x}");
        }
        println!(
            "QUALIFICATION-SUMMARY\tphy_bt_txdc_cal_new\tINCOMPLETE\tunmapped-mmio={}",
            unmapped.len()
        );
        return Ok(false);
    }

    let vendor_events = semantic::normalize_vendor_bluetooth_txdc(&image, &result, phy_param)?;
    let (rust_events, _) = semantic::rust_bluetooth_txdc_events(
        open_esp_radio_esp32s31_phy::phy_cold::PhyColdState::new(),
    )?;
    let matched = vendor_events == rust_events;
    if !matched {
        let divergence = vendor_events
            .iter()
            .zip(&rust_events)
            .position(|(vendor, rust)| vendor != rust)
            .unwrap_or_else(|| vendor_events.len().min(rust_events.len()));
        println!(
            "SEMANTIC-DIFF\tbluetooth-txdc\tindex={divergence}\tvendor={:?}\trust={:?}",
            vendor_events.get(divergence),
            rust_events.get(divergence),
        );
        for (index, event) in vendor_events.iter().enumerate() {
            println!("SEMANTIC-EVENT\tbluetooth-txdc\tvendor\t{index}\t{event:?}");
        }
        for (index, event) in rust_events.iter().enumerate() {
            println!("SEMANTIC-EVENT\tbluetooth-txdc\trust\t{index}\t{event:?}");
        }
    }
    let verdict = if matched {
        "STATE-SCENARIO-MATCH"
    } else {
        "MISMATCH"
    };
    println!(
        "QUALIFICATION-SUMMARY\tphy_bt_txdc_cal_new\t{verdict}\tscenarios=1\tmatched={}\tfailed={}\tevents={}\tsteps={}\tbranch-outcomes={}\tcalls={}",
        usize::from(matched),
        usize::from(!matched),
        vendor_events.len(),
        result.steps,
        result.branches.len(),
        result.calls.len(),
    );
    Ok(matched)
}

fn qualify_esp32s31_bluetooth_tx_power(
    svd: &SvdMap,
    vendor_artifact: &Path,
    vendor_companion: &Path,
    print_oracles: bool,
) -> Result<bool> {
    let artifact_digest = pinned_vendor_digest(vendor_artifact)?;
    let companion_digest = pinned_vendor_digest(vendor_companion)?;
    if print_oracles {
        println!(
            "ORACLE\tarchive\t{}\tsha256={artifact_digest}",
            vendor_artifact.display()
        );
        println!(
            "ORACLE\trom-companion\t{}\tsha256={companion_digest}",
            vendor_companion.display()
        );
    }

    let mut image = emulator::ExecutableImage::load(vendor_artifact)?;
    image.add_companion(vendor_companion)?;
    let phy_param = image
        .symbol_address("phy_param")
        .ok_or("vendor artifact has no phy_param symbol")?;
    let phy_functions_pointer = image
        .symbol_address("g_phyFuns")
        .ok_or("vendor artifact has no g_phyFuns symbol")?;
    let scenario = semantic::vendor_bluetooth_tx_power_scenario(phy_param, phy_functions_pointer);
    let result = emulator::execute(&image, svd, "phy_bt_tx_pwctrl_init", scenario)?;
    let unmapped: BTreeSet<_> = result
        .events
        .iter()
        .filter_map(unmapped_execution_address)
        .collect();
    if !unmapped.is_empty() {
        for address in &unmapped {
            println!("SEMANTIC-UNCOVERED\tbluetooth-tx-power\tunmapped-mmio\t{address:#010x}");
        }
        println!(
            "QUALIFICATION-SUMMARY\tphy_bt_tx_pwctrl_init\tINCOMPLETE\tunmapped-mmio={}",
            unmapped.len()
        );
        return Ok(false);
    }

    let footprint = semantic::vendor_bluetooth_tx_power_state_footprint(&result, phy_param)?;
    let vendor_events = semantic::normalize_vendor_bluetooth_tx_power(&image, &result, phy_param)?;
    let (rust_events, _) = semantic::rust_bluetooth_tx_power_events(
        open_esp_radio_esp32s31_phy::phy_cold::PhyColdState::new(),
    )?;
    let matched = vendor_events == rust_events;
    if !matched {
        let divergence = vendor_events
            .iter()
            .zip(&rust_events)
            .position(|(vendor, rust)| vendor != rust)
            .unwrap_or_else(|| vendor_events.len().min(rust_events.len()));
        println!(
            "SEMANTIC-DIFF\tbluetooth-tx-power\tindex={divergence}\tvendor={:?}\trust={:?}",
            vendor_events.get(divergence),
            rust_events.get(divergence),
        );
        for (index, event) in vendor_events.iter().enumerate() {
            println!("SEMANTIC-EVENT\tbluetooth-tx-power\tvendor\t{index}\t{event:?}");
        }
        for (index, event) in rust_events.iter().enumerate() {
            println!("SEMANTIC-EVENT\tbluetooth-tx-power\trust\t{index}\t{event:?}");
        }
    }
    let verdict = if matched {
        "STATE-SCENARIO-MATCH"
    } else {
        "MISMATCH"
    };
    println!(
        "QUALIFICATION-SUMMARY\tphy_bt_tx_pwctrl_init\t{verdict}\tscenarios=1\tmatched={}\tfailed={}\tevents={}\tsteps={}\tbranch-outcomes={}\tcalls={}\tstate-read-bytes={}\tstate-write-bytes={}\tstate-ranges={}",
        usize::from(matched),
        usize::from(!matched),
        vendor_events.len(),
        result.steps,
        result.branches.len(),
        result.calls.len(),
        footprint.read_bytes,
        footprint.written_bytes,
        footprint.classified_ranges,
    );
    Ok(matched)
}

fn qualify_esp32s31_bluetooth_txdc_pwdet(
    svd: &SvdMap,
    vendor_artifact: &Path,
    vendor_companion: &Path,
    print_oracles: bool,
) -> Result<bool> {
    let artifact_digest = pinned_vendor_digest(vendor_artifact)?;
    let companion_digest = pinned_vendor_digest(vendor_companion)?;
    if print_oracles {
        println!(
            "ORACLE\tarchive\t{}\tsha256={artifact_digest}",
            vendor_artifact.display()
        );
        println!(
            "ORACLE\trom-companion\t{}\tsha256={companion_digest}",
            vendor_companion.display()
        );
    }

    let mut image = emulator::ExecutableImage::load(vendor_artifact)?;
    image.add_companion(vendor_companion)?;
    let phy_param = image
        .symbol_address("phy_param")
        .ok_or("vendor artifact has no phy_param symbol")?;
    let phy_functions_pointer = image
        .symbol_address("g_phyFuns")
        .ok_or("vendor artifact has no g_phyFuns symbol")?;
    let scenario = semantic::vendor_bluetooth_txdc_pwdet_scenario(phy_param, phy_functions_pointer);
    let result = emulator::execute(&image, svd, "phy_txdc_cal_pwdet_init", scenario)?;
    let unmapped: BTreeSet<_> = result
        .events
        .iter()
        .filter_map(unmapped_execution_address)
        .collect();
    if !unmapped.is_empty() {
        for address in &unmapped {
            println!("SEMANTIC-UNCOVERED\tbluetooth-txdc-pwdet\tunmapped-mmio\t{address:#010x}");
        }
        println!(
            "QUALIFICATION-SUMMARY\tphy_txdc_cal_pwdet_init\tINCOMPLETE\tunmapped-mmio={}",
            unmapped.len()
        );
        return Ok(false);
    }

    let footprint = semantic::vendor_bluetooth_txdc_pwdet_state_footprint(&result, phy_param)?;
    let vendor_events =
        semantic::normalize_vendor_bluetooth_txdc_pwdet(&image, &result, phy_param)?;
    let (rust_events, _) = semantic::rust_bluetooth_txdc_pwdet_events(
        open_esp_radio_esp32s31_phy::phy_cold::PhyColdState::new(),
    )?;
    let matched = vendor_events == rust_events;
    if !matched {
        let divergence = vendor_events
            .iter()
            .zip(&rust_events)
            .position(|(vendor, rust)| vendor != rust)
            .unwrap_or_else(|| vendor_events.len().min(rust_events.len()));
        println!(
            "SEMANTIC-DIFF\tbluetooth-txdc-pwdet\tindex={divergence}\tvendor={:?}\trust={:?}",
            vendor_events.get(divergence),
            rust_events.get(divergence),
        );
        let window_start = divergence.saturating_sub(8);
        let window_end = divergence
            .saturating_add(9)
            .max(window_start)
            .min(vendor_events.len().max(rust_events.len()));
        for (index, event) in vendor_events
            .iter()
            .enumerate()
            .skip(window_start)
            .take(window_end - window_start)
        {
            println!("SEMANTIC-EVENT\tbluetooth-txdc-pwdet\tvendor\t{index}\t{event:?}");
        }
        for (index, event) in rust_events
            .iter()
            .enumerate()
            .skip(window_start)
            .take(window_end - window_start)
        {
            println!("SEMANTIC-EVENT\tbluetooth-txdc-pwdet\trust\t{index}\t{event:?}");
        }
    }
    let verdict = if matched {
        "STATE-SCENARIO-MATCH"
    } else {
        "MISMATCH"
    };
    println!(
        "QUALIFICATION-SUMMARY\tphy_txdc_cal_pwdet_init\t{verdict}\tscenarios=1\tmatched={}\tfailed={}\tevents={}\tsteps={}\tbranch-outcomes={}\tcalls={}\tstate-read-bytes={}\tstate-write-bytes={}\tstate-ranges={}",
        usize::from(matched),
        usize::from(!matched),
        vendor_events.len(),
        result.steps,
        result.branches.len(),
        result.calls.len(),
        footprint.read_bytes,
        footprint.written_bytes,
        footprint.classified_ranges,
    );
    Ok(matched)
}

fn run() -> Result<bool> {
    let mut arguments = env::args().skip(1);
    let command = arguments.next().ok_or("missing command")?;
    let remaining: Vec<String> = arguments.collect();
    let mut svd_paths = Vec::new();
    let mut filtered = Vec::new();
    let mut index = 0;
    while index < remaining.len() {
        if remaining[index] == "--svd" {
            let path = remaining.get(index + 1).ok_or("--svd requires a value")?;
            svd_paths.push(PathBuf::from(path));
            index += 2;
        } else {
            filtered.push(remaining[index].clone());
            index += 1;
        }
    }
    if svd_paths.is_empty() {
        return Err("missing --svd".into());
    }
    let svd = SvdMap::load_all(&svd_paths)?;
    match command.as_str() {
        "qualify-esp32s31-channel" => {
            let mut vendor_artifact = None;
            let mut vendor_companion = None;
            let mut arguments = filtered.into_iter();
            while let Some(argument) = arguments.next() {
                match argument.as_str() {
                    "--vendor-artifact" => {
                        vendor_artifact = Some(PathBuf::from(take_value(
                            &mut arguments,
                            "--vendor-artifact",
                        )?));
                    }
                    "--vendor-companion" => {
                        vendor_companion = Some(PathBuf::from(take_value(
                            &mut arguments,
                            "--vendor-companion",
                        )?));
                    }
                    _ => {
                        return Err(
                            format!("unknown qualify-esp32s31-channel option: {argument}").into(),
                        );
                    }
                }
            }
            let vendor_artifact = vendor_artifact.ok_or("missing --vendor-artifact")?;
            let vendor_companion = vendor_companion.ok_or("missing --vendor-companion")?;
            qualify_esp32s31_channel(&svd, &vendor_artifact, &vendor_companion, true)
        }
        "qualify-esp32s31-rf-init" => {
            let mut vendor_artifact = None;
            let mut vendor_companion = None;
            let mut arguments = filtered.into_iter();
            while let Some(argument) = arguments.next() {
                match argument.as_str() {
                    "--vendor-artifact" => {
                        vendor_artifact = Some(PathBuf::from(take_value(
                            &mut arguments,
                            "--vendor-artifact",
                        )?));
                    }
                    "--vendor-companion" => {
                        vendor_companion = Some(PathBuf::from(take_value(
                            &mut arguments,
                            "--vendor-companion",
                        )?));
                    }
                    _ => {
                        return Err(
                            format!("unknown qualify-esp32s31-rf-init option: {argument}").into(),
                        );
                    }
                }
            }
            let vendor_artifact = vendor_artifact.ok_or("missing --vendor-artifact")?;
            let vendor_companion = vendor_companion.ok_or("missing --vendor-companion")?;
            qualify_esp32s31_rf_init(&svd, &vendor_artifact, &vendor_companion, true)
        }
        "execute" => {
            let mut artifact = None;
            let mut companion = None;
            let mut symbol = None;
            let mut concrete_only = false;
            let mut print_timeline = false;
            let mut scenario = emulator::Scenario::default();
            let mut arguments = filtered.into_iter();
            while let Some(argument) = arguments.next() {
                match argument.as_str() {
                    "--artifact" => {
                        artifact = Some(PathBuf::from(take_value(&mut arguments, "--artifact")?));
                    }
                    "--companion" => {
                        companion = Some(PathBuf::from(take_value(&mut arguments, "--companion")?));
                    }
                    "--symbol" => symbol = Some(take_value(&mut arguments, "--symbol")?),
                    "--concrete-only" => concrete_only = true,
                    "--timeline" => print_timeline = true,
                    "--arg" => {
                        let value = take_value(&mut arguments, "--arg")?;
                        scenario
                            .arguments
                            .push(parse_u32(&value).ok_or("invalid --arg value")?);
                    }
                    "--mmio" => {
                        let assignment = take_value(&mut arguments, "--mmio")?;
                        let (address, value) = parse_assignment(&assignment, "--mmio")?;
                        scenario.mmio_initial.insert(address, value);
                    }
                    "--read" => {
                        let assignment = take_value(&mut arguments, "--read")?;
                        let (address, value) = parse_assignment(&assignment, "--read")?;
                        scenario
                            .mmio_reads
                            .entry(address)
                            .or_default()
                            .push_back(value);
                    }
                    "--ram" => {
                        let assignment = take_value(&mut arguments, "--ram")?;
                        let (address, value) = parse_assignment(&assignment, "--ram")?;
                        seed_ram_word(&mut scenario, address, value);
                    }
                    "--observe" => {
                        let assignment = take_value(&mut arguments, "--observe")?;
                        let (address, length) = parse_assignment(&assignment, "--observe")?;
                        observe_memory(&mut scenario, address, length)?;
                    }
                    "--max-steps" => {
                        let value = take_value(&mut arguments, "--max-steps")?;
                        scenario.max_steps = value.parse()?;
                    }
                    _ => return Err(format!("unknown execute option: {argument}").into()),
                }
            }
            let artifact = artifact.ok_or("missing --artifact")?;
            let symbol = symbol.ok_or("missing --symbol")?;
            let mut image = emulator::ExecutableImage::load(&artifact)?;
            if let Some(companion) = companion {
                image.add_companion(&companion)?;
            }
            let inventory = if concrete_only {
                emulator::CoverageInventory::default()
            } else {
                image.coverage_inventory(&symbol)?
            };
            let result = emulator::execute(&image, &svd, &symbol, scenario)?;
            let unmapped: BTreeSet<_> = result
                .events
                .iter()
                .filter_map(unmapped_execution_address)
                .collect();
            for event in result.events {
                match event {
                    emulator::ExecutionEvent::Read {
                        width,
                        address,
                        register,
                        value,
                    } => println!(
                        "EVENT\tR\t{width}\t{address:#010x}\t{register}\tvalue={value:#010x}"
                    ),
                    emulator::ExecutionEvent::Write {
                        width,
                        address,
                        register,
                        value,
                    } => println!(
                        "EVENT\tW\t{width}\t{address:#010x}\t{register}\tvalue={value:#010x}",
                    ),
                    emulator::ExecutionEvent::DelayMicros(micros) => {
                        println!("EVENT\tDELAY\tmicros={micros}");
                    }
                    emulator::ExecutionEvent::Fence {
                        fm,
                        predecessor,
                        successor,
                    } => println!(
                        "EVENT\tFENCE\tfm={fm:#x}\tpred={predecessor:#x}\tsucc={successor:#x}"
                    ),
                }
            }
            for call in &result.calls {
                println!("COVERED-CALL\t{call}");
            }
            if print_timeline {
                for (index, event) in result.timeline.iter().enumerate() {
                    match event {
                        emulator::ExecutionTimelineEvent::Observable(event) => {
                            println!("TIMELINE-EVENT\t{index}\tOBSERVABLE\t{event:?}");
                        }
                        emulator::ExecutionTimelineEvent::Call(call) => println!(
                            "TIMELINE-EVENT\t{index}\tCALL\t{}\t{}\targs={:08x},{:08x},{:08x},{:08x},{:08x},{:08x},{:08x},{:08x}",
                            image.location(call.site),
                            call.symbol,
                            call.arguments[0],
                            call.arguments[1],
                            call.arguments[2],
                            call.arguments[3],
                            call.arguments[4],
                            call.arguments[5],
                            call.arguments[6],
                            call.arguments[7],
                        ),
                        emulator::ExecutionTimelineEvent::Branch { site, taken } => println!(
                            "TIMELINE-EVENT\t{index}\tBRANCH\t{}\ttaken={taken}",
                            image.location(*site)
                        ),
                        emulator::ExecutionTimelineEvent::RamRead {
                            width,
                            address,
                            value,
                        } => println!(
                            "TIMELINE-EVENT\t{index}\tRAM-READ\t{width}\t{address:#010x}\tvalue={value:#010x}"
                        ),
                        emulator::ExecutionTimelineEvent::RamWrite {
                            width,
                            address,
                            value,
                        } => println!(
                            "TIMELINE-EVENT\t{index}\tRAM-WRITE\t{width}\t{address:#010x}\tvalue={value:#010x}"
                        ),
                    }
                }
            }
            let uncovered_branches = print_branch_coverage(
                "image",
                &image,
                &inventory.branch_outcomes,
                &result.branches,
            );
            for (address, edge) in &inventory.unresolved_edges {
                println!(
                    "UNCOVERED-CONTROL-FLOW\timage\t{}\t{edge}",
                    image.location(*address)
                );
            }
            for address in &unmapped {
                println!("UNCOVERED-MMIO\timage\t{address:#010x}");
            }
            for change in &result.memory_changes {
                println!(
                    "MEMORY-CHANGE\t{:#010x}\tbefore={:#04x}\tafter={:#04x}",
                    change.address, change.before, change.after
                );
            }
            println!(
                "RESULT\tsymbol={symbol}\tevidence={}\tsteps={}\treturn={:#010x}\tbranches={}\tbranch-events={}\tcalls={}\tcall-events={}\ttimeline-events={}\tmemory-changes={}\tuncovered-branch-outcomes={uncovered_branches}\tunresolved-control-flow={}\tunmapped-mmio={}",
                if concrete_only {
                    "concrete-only"
                } else {
                    "branch-complete"
                },
                result.steps,
                result.return_value,
                result.branches.len(),
                result.ordered_branches.len(),
                result.calls.len(),
                result.ordered_calls.len(),
                result.timeline.len(),
                result.memory_changes.len(),
                inventory.unresolved_edges.len(),
                unmapped.len(),
            );
            Ok(uncovered_branches == 0
                && inventory.unresolved_edges.is_empty()
                && unmapped.is_empty())
        }
        "execute-compare" => {
            let mut vendor_artifact = None;
            let mut vendor_companion = None;
            let mut vendor_symbol = None;
            let mut rust_artifact = None;
            let mut rust_companion = None;
            let mut rust_symbol = None;
            let mut compare_return = false;
            let mut scenarios = Vec::new();
            let mut current_scenario: Option<NamedScenario> = None;
            let mut arguments = filtered.into_iter();
            while let Some(argument) = arguments.next() {
                match argument.as_str() {
                    "--vendor-artifact" => {
                        vendor_artifact = Some(PathBuf::from(take_value(
                            &mut arguments,
                            "--vendor-artifact",
                        )?));
                    }
                    "--vendor-symbol" => {
                        vendor_symbol = Some(take_value(&mut arguments, "--vendor-symbol")?);
                    }
                    "--vendor-companion" => {
                        vendor_companion = Some(PathBuf::from(take_value(
                            &mut arguments,
                            "--vendor-companion",
                        )?));
                    }
                    "--rust-artifact" => {
                        rust_artifact = Some(PathBuf::from(take_value(
                            &mut arguments,
                            "--rust-artifact",
                        )?));
                    }
                    "--rust-symbol" => {
                        rust_symbol = Some(take_value(&mut arguments, "--rust-symbol")?);
                    }
                    "--rust-companion" => {
                        rust_companion = Some(PathBuf::from(take_value(
                            &mut arguments,
                            "--rust-companion",
                        )?));
                    }
                    "--compare-return" => compare_return = true,
                    "--case" => {
                        if let Some(scenario) = current_scenario.take() {
                            scenarios.push(scenario);
                        }
                        current_scenario =
                            Some(NamedScenario::new(take_value(&mut arguments, "--case")?));
                    }
                    "--arg" => {
                        let value = take_value(&mut arguments, "--arg")?;
                        current_scenario
                            .get_or_insert_with(|| NamedScenario::new("default".to_owned()))
                            .scenario
                            .arguments
                            .push(parse_u32(&value).ok_or("invalid --arg value")?);
                    }
                    "--mmio" => {
                        let assignment = take_value(&mut arguments, "--mmio")?;
                        let (address, value) = parse_assignment(&assignment, "--mmio")?;
                        current_scenario
                            .get_or_insert_with(|| NamedScenario::new("default".to_owned()))
                            .scenario
                            .mmio_initial
                            .insert(address, value);
                    }
                    "--read" => {
                        let assignment = take_value(&mut arguments, "--read")?;
                        let (address, value) = parse_assignment(&assignment, "--read")?;
                        current_scenario
                            .get_or_insert_with(|| NamedScenario::new("default".to_owned()))
                            .scenario
                            .mmio_reads
                            .entry(address)
                            .or_default()
                            .push_back(value);
                    }
                    "--ram" => {
                        let assignment = take_value(&mut arguments, "--ram")?;
                        let (address, value) = parse_assignment(&assignment, "--ram")?;
                        let scenario = &mut current_scenario
                            .get_or_insert_with(|| NamedScenario::new("default".to_owned()))
                            .scenario;
                        seed_ram_word(scenario, address, value);
                    }
                    "--vendor-ram-symbol" => {
                        let assignment = take_value(&mut arguments, "--vendor-ram-symbol")?;
                        current_scenario
                            .get_or_insert_with(|| NamedScenario::new("default".to_owned()))
                            .vendor_symbol_words
                            .push(parse_symbol_word(&assignment, "--vendor-ram-symbol")?);
                    }
                    "--rust-ram-symbol" => {
                        let assignment = take_value(&mut arguments, "--rust-ram-symbol")?;
                        current_scenario
                            .get_or_insert_with(|| NamedScenario::new("default".to_owned()))
                            .rust_symbol_words
                            .push(parse_symbol_word(&assignment, "--rust-ram-symbol")?);
                    }
                    "--observe" => {
                        let assignment = take_value(&mut arguments, "--observe")?;
                        let (address, length) = parse_assignment(&assignment, "--observe")?;
                        let scenario = &mut current_scenario
                            .get_or_insert_with(|| NamedScenario::new("default".to_owned()))
                            .scenario;
                        observe_memory(scenario, address, length)?;
                    }
                    "--max-steps" => {
                        let value = take_value(&mut arguments, "--max-steps")?;
                        current_scenario
                            .get_or_insert_with(|| NamedScenario::new("default".to_owned()))
                            .scenario
                            .max_steps = value.parse()?;
                    }
                    _ => return Err(format!("unknown execute-compare option: {argument}").into()),
                }
            }
            if let Some(scenario) = current_scenario {
                scenarios.push(scenario);
            }
            if scenarios.is_empty() {
                scenarios.push(NamedScenario::new("default".to_owned()));
            }

            let vendor_artifact = vendor_artifact.ok_or("missing --vendor-artifact")?;
            let vendor_symbol = vendor_symbol.ok_or("missing --vendor-symbol")?;
            let rust_artifact = rust_artifact.ok_or("missing --rust-artifact")?;
            let rust_symbol = rust_symbol.ok_or("missing --rust-symbol")?;
            Ok(compare_execution_scenarios(
                &svd,
                ExecutionInput {
                    artifact: &vendor_artifact,
                    companion: vendor_companion.as_deref(),
                    symbol: &vendor_symbol,
                },
                ExecutionInput {
                    artifact: &rust_artifact,
                    companion: rust_companion.as_deref(),
                    symbol: &rust_symbol,
                },
                compare_return,
                &scenarios,
            )? == ComparisonVerdict::Match)
        }
        "verify-profiles" => {
            let mut profile_path = None;
            let mut vendor_artifact = None;
            let mut vendor_companion = None;
            let mut rust_artifact = None;
            let mut rust_companion = None;
            let mut arguments = filtered.into_iter();
            while let Some(argument) = arguments.next() {
                match argument.as_str() {
                    "--profiles" => {
                        profile_path =
                            Some(PathBuf::from(take_value(&mut arguments, "--profiles")?));
                    }
                    "--vendor-artifact" => {
                        vendor_artifact = Some(PathBuf::from(take_value(
                            &mut arguments,
                            "--vendor-artifact",
                        )?));
                    }
                    "--vendor-companion" => {
                        vendor_companion = Some(PathBuf::from(take_value(
                            &mut arguments,
                            "--vendor-companion",
                        )?));
                    }
                    "--rust-artifact" => {
                        rust_artifact = Some(PathBuf::from(take_value(
                            &mut arguments,
                            "--rust-artifact",
                        )?));
                    }
                    "--rust-companion" => {
                        rust_companion = Some(PathBuf::from(take_value(
                            &mut arguments,
                            "--rust-companion",
                        )?));
                    }
                    _ => return Err(format!("unknown verify-profiles option: {argument}").into()),
                }
            }
            let profile_path = profile_path.ok_or("missing --profiles")?;
            let vendor_artifact = vendor_artifact.ok_or("missing --vendor-artifact")?;
            let rust_artifact = rust_artifact.ok_or("missing --rust-artifact")?;
            let loaded_profiles = profiles::load(&profile_path)?;
            let mut matched = 0_usize;
            let mut mismatched = 0_usize;
            for profile in &loaded_profiles {
                println!("PROFILE\t{}\tBEGIN", profile.name);
                let result = compare_execution_scenarios(
                    &svd,
                    ExecutionInput {
                        artifact: &vendor_artifact,
                        companion: vendor_companion.as_deref(),
                        symbol: &profile.vendor_symbol,
                    },
                    ExecutionInput {
                        artifact: &rust_artifact,
                        companion: rust_companion.as_deref(),
                        symbol: &profile.rust_symbol,
                    },
                    profile.compare_return,
                    &profile.scenarios,
                )?;
                match result {
                    ComparisonVerdict::Match => matched += 1,
                    ComparisonVerdict::Mismatch => mismatched += 1,
                    ComparisonVerdict::Incomplete => {}
                }
                println!("PROFILE\t{}\t{}", profile.name, result.label());
            }
            println!(
                "PROFILE-SUMMARY\tprofiles={}\tmatch={matched}\tmismatch={mismatched}\tincomplete={}",
                loaded_profiles.len(),
                loaded_profiles.len() - matched - mismatched,
            );
            Ok(matched == loaded_profiles.len())
        }
        "generate-reference" => {
            let mut artifact = None;
            let mut companions = Vec::new();
            let mut member = None;
            let mut symbol = None;
            let mut output = None;
            let mut arguments = filtered.into_iter();
            while let Some(argument) = arguments.next() {
                match argument.as_str() {
                    "--artifact" => {
                        artifact = Some(PathBuf::from(take_value(&mut arguments, "--artifact")?));
                    }
                    "--companion" => {
                        companions.push(PathBuf::from(take_value(&mut arguments, "--companion")?));
                    }
                    "--member" => {
                        member = Some(take_value(&mut arguments, "--member")?);
                    }
                    "--symbol" => {
                        symbol = Some(take_value(&mut arguments, "--symbol")?);
                    }
                    "--output" => {
                        output = Some(PathBuf::from(take_value(&mut arguments, "--output")?));
                    }
                    _ => {
                        return Err(format!("unknown generate-reference option: {argument}").into());
                    }
                }
            }
            let artifact = artifact.ok_or("missing --artifact")?;
            let symbol = symbol.ok_or("missing --symbol")?;
            let input = Input {
                artifact: artifact.clone(),
                member: member.clone(),
                symbol,
            };
            let trace = extract_reference(&input, &companions, &svd)?;
            let digest = artifact_sha256(&artifact)?;
            let companion_provenance = companions
                .iter()
                .map(|companion| Ok((companion.display().to_string(), artifact_sha256(companion)?)))
                .collect::<Result<Vec<_>>>()?;
            let generated = reference_codegen::generate(
                &trace,
                &artifact.display().to_string(),
                &digest,
                member.as_deref(),
                &companion_provenance,
            )
            .map_err(|error| -> Error { error.into() })?;
            if let Some(output) = output {
                fs::write(&output, generated.source)?;
                println!(
                    "GENERATED\t{}\t{}\texit-a0={}",
                    trace.symbol,
                    output.display(),
                    if generated.exit_a0_modeled {
                        "modeled"
                    } else {
                        "unresolved"
                    }
                );
            } else {
                print!("{}", generated.source);
            }
            Ok(true)
        }
        "analyze" => {
            let mut artifact = None;
            let mut companions = Vec::new();
            let mut prefix = "phy_".to_owned();
            let mut arguments = filtered.into_iter();
            while let Some(argument) = arguments.next() {
                match argument.as_str() {
                    "--artifact" => {
                        artifact = Some(PathBuf::from(take_value(&mut arguments, "--artifact")?));
                    }
                    "--companion" => {
                        companions.push(PathBuf::from(take_value(&mut arguments, "--companion")?));
                    }
                    "--symbol-prefix" => {
                        prefix = take_value(&mut arguments, "--symbol-prefix")?;
                    }
                    _ => return Err(format!("unknown analyze option: {argument}").into()),
                }
            }
            let artifact = artifact.ok_or("missing --artifact")?;
            let symbols = list_code_symbols(&artifact, &prefix)?;
            if symbols.is_empty() {
                return Err(format!("no external code symbols start with {prefix:?}").into());
            }
            let reference_catalog = ReferenceCatalog::load(&artifact, &companions)?;

            let mut exact = 0usize;
            let mut incomplete = 0usize;
            let mut reference_codegen_eligible = 0usize;
            let mut reasons = BTreeMap::<String, usize>::new();
            let mut reference_reasons = BTreeMap::<String, usize>::new();
            for symbol in &symbols {
                let input = Input {
                    artifact: artifact.clone(),
                    member: symbol.member.clone(),
                    symbol: symbol.name.clone(),
                };
                let trace = extract(&input, &svd)?;
                let reference_trace =
                    reference_catalog.trace(symbol.member.as_deref(), &symbol.name, &svd)?;
                let owner = symbol.member.as_deref().unwrap_or("-");
                let reference_status = if reference_trace.is_reference_eligible() {
                    reference_codegen_eligible += 1;
                    "eligible"
                } else {
                    "blocked"
                };
                if trace.is_exact() {
                    exact += 1;
                    println!(
                        "FUNCTION\t{}\t{owner}\tDIRECT-TRACE-EXACT\tevents={}\treference-codegen={reference_status}\treference-dependencies={}\tindexed-mmio={}",
                        symbol.name,
                        trace.events.len(),
                        reference_trace.reference_dependencies.len(),
                        reference_trace.reference_indexed_mmio_count(),
                    );
                } else {
                    incomplete += 1;
                    println!(
                        "FUNCTION\t{}\t{owner}\tINCOMPLETE\tevents={}\tuncovered={}\treference-codegen={reference_status}\treference-dependencies={}\tindexed-mmio={}",
                        symbol.name,
                        trace.events.len(),
                        trace.blockers.len()
                            + trace
                                .events
                                .iter()
                                .filter_map(Event::unmapped_address)
                                .count(),
                        reference_trace.reference_dependencies.len(),
                        reference_trace.reference_indexed_mmio_count(),
                    );
                    for blocker in &trace.blockers {
                        let kind = blocker
                            .split_once(' ')
                            .map_or(blocker.as_str(), |pair| pair.0);
                        *reasons.entry(kind.to_owned()).or_default() += 1;
                        println!("UNCOVERED\t{}\t{blocker}", symbol.name);
                    }
                    for address in trace.events.iter().filter_map(Event::unmapped_address) {
                        *reasons.entry("unmapped-register".to_owned()).or_default() += 1;
                        println!(
                            "UNCOVERED\t{}\tunmapped-register {:#010x}",
                            symbol.name, address
                        );
                    }
                }
                for blocker in &reference_trace.reference_blockers {
                    let kind = blocker
                        .split_once(' ')
                        .map_or(blocker.as_str(), |pair| pair.0);
                    *reference_reasons.entry(kind.to_owned()).or_default() += 1;
                }
            }
            println!(
                "SUMMARY\tfunctions={}\tdirect_trace_exact={exact}\tincomplete={incomplete}\treference_codegen_eligible={reference_codegen_eligible}\treference_codegen_blocked={}",
                symbols.len(),
                symbols.len() - reference_codegen_eligible,
            );
            for (reason, count) in reasons {
                println!("SUMMARY-UNCOVERED\t{reason}\t{count}");
            }
            for (reason, count) in reference_reasons {
                println!("SUMMARY-REFERENCE-BLOCKED\t{reason}\t{count}");
            }
            Ok(incomplete == 0)
        }
        "verify-all" => {
            let mut rom_artifact = None;
            let mut rom_companion = None;
            let mut archive_artifact = None;
            let mut archive_inventory = None;
            let mut archive_companion = None;
            let mut rust_artifact = None;
            let mut rust_companion = None;
            let mut profile_path = None;
            let mut disposition_path = None;
            let mut rom_prefix = "phy_".to_owned();
            let mut archive_prefix = String::new();
            let mut rust_prefix = "open_phy_trace_".to_owned();
            let mut gate_name = "completion".to_owned();
            let mut match_floor = None;
            let mut evidence_baseline = None;
            let mut json_report = None;
            let mut arguments = filtered.into_iter();
            while let Some(argument) = arguments.next() {
                match argument.as_str() {
                    "--rom-artifact" => {
                        rom_artifact =
                            Some(PathBuf::from(take_value(&mut arguments, "--rom-artifact")?));
                    }
                    "--rom-companion" => {
                        rom_companion = Some(PathBuf::from(take_value(
                            &mut arguments,
                            "--rom-companion",
                        )?));
                    }
                    "--archive-artifact" => {
                        archive_artifact = Some(PathBuf::from(take_value(
                            &mut arguments,
                            "--archive-artifact",
                        )?));
                    }
                    "--archive-inventory" => {
                        archive_inventory = Some(PathBuf::from(take_value(
                            &mut arguments,
                            "--archive-inventory",
                        )?));
                    }
                    "--archive-companion" => {
                        archive_companion = Some(PathBuf::from(take_value(
                            &mut arguments,
                            "--archive-companion",
                        )?));
                    }
                    "--rust-artifact" => {
                        rust_artifact = Some(PathBuf::from(take_value(
                            &mut arguments,
                            "--rust-artifact",
                        )?));
                    }
                    "--rust-companion" => {
                        rust_companion = Some(PathBuf::from(take_value(
                            &mut arguments,
                            "--rust-companion",
                        )?));
                    }
                    "--profiles" => {
                        profile_path =
                            Some(PathBuf::from(take_value(&mut arguments, "--profiles")?));
                    }
                    "--dispositions" => {
                        disposition_path =
                            Some(PathBuf::from(take_value(&mut arguments, "--dispositions")?));
                    }
                    "--rom-prefix" => {
                        rom_prefix = take_value(&mut arguments, "--rom-prefix")?;
                    }
                    "--archive-prefix" => {
                        archive_prefix = take_value(&mut arguments, "--archive-prefix")?;
                    }
                    "--rust-prefix" => {
                        rust_prefix = take_value(&mut arguments, "--rust-prefix")?;
                    }
                    "--gate" => gate_name = take_value(&mut arguments, "--gate")?,
                    "--match-floor" => {
                        match_floor =
                            Some(take_value(&mut arguments, "--match-floor")?.parse::<usize>()?);
                    }
                    "--evidence-baseline" => {
                        evidence_baseline = Some(PathBuf::from(take_value(
                            &mut arguments,
                            "--evidence-baseline",
                        )?));
                    }
                    "--json-report" => {
                        json_report =
                            Some(PathBuf::from(take_value(&mut arguments, "--json-report")?));
                    }
                    _ => return Err(format!("unknown verify-all option: {argument}").into()),
                }
            }
            let rom_artifact = rom_artifact.ok_or("missing --rom-artifact")?;
            let archive_artifact = archive_artifact.ok_or("missing --archive-artifact")?;
            let archive_inventory = archive_inventory.ok_or("missing --archive-inventory")?;
            let rust_artifact = rust_artifact.ok_or("missing --rust-artifact")?;
            let gate = VerificationGate::parse(&gate_name, match_floor)?;
            if matches!(gate, VerificationGate::Regression { .. }) && evidence_baseline.is_none() {
                return Err("--gate regression requires --evidence-baseline".into());
            }
            let execution_profiles = profile_path
                .as_deref()
                .map(profiles::load)
                .transpose()?
                .unwrap_or_default();
            let disposition_manifest = disposition_path
                .as_deref()
                .map(dispositions::Manifest::load)
                .transpose()?;
            let rom = VerifySource {
                name: "rom",
                artifact: &rom_artifact,
                inventory: None,
                companion: rom_companion.as_deref(),
                prefix: &rom_prefix,
            };
            let archive = VerifySource {
                name: "archive",
                artifact: &archive_artifact,
                inventory: Some(&archive_inventory),
                companion: archive_companion.as_deref(),
                prefix: &archive_prefix,
            };
            let rom_symbols = vendor_symbols(rom)?;
            let archive_symbols = vendor_symbols(archive)?;
            if let Some(manifest) = disposition_manifest.as_ref() {
                manifest.validate(&[
                    ("rom", rom_symbols.as_slice()),
                    ("archive", archive_symbols.as_slice()),
                ])?;
                print_protocol_inventory(
                    manifest,
                    &[
                        ("rom", rom_symbols.as_slice()),
                        ("archive", archive_symbols.as_slice()),
                    ],
                );
            }
            let mut rom_profiles = Vec::new();
            let mut archive_profiles = Vec::new();
            for profile in execution_profiles {
                let in_rom = rom_symbols
                    .iter()
                    .any(|symbol| symbol.name == profile.vendor_symbol);
                let in_archive = archive_symbols
                    .iter()
                    .any(|symbol| symbol.name == profile.vendor_symbol);
                match profile.vendor_source.as_str() {
                    "rom" if in_rom => rom_profiles.push(profile),
                    "archive" if in_archive => archive_profiles.push(profile),
                    source @ ("rom" | "archive") => {
                        return Err(format!(
                            "profile {} refers to {} symbol {} which does not exist",
                            profile.name, source, profile.vendor_symbol
                        )
                        .into());
                    }
                    source => {
                        return Err(format!(
                            "profile {} has unsupported vendor source {source}",
                            profile.name
                        )
                        .into());
                    }
                }
            }
            println!(
                "INVENTORY\trom={}\tarchive={}\ttotal={}",
                rom_symbols.len(),
                archive_symbols.len(),
                rom_symbols.len() + archive_symbols.len()
            );
            let mut total = VerifySummary::default();
            let mut evidence = EvidenceSet::new();
            total.add(verify_source(
                &svd,
                rom,
                &rust_artifact,
                rust_companion.as_deref(),
                &rust_prefix,
                &rom_profiles,
                disposition_manifest.as_ref(),
                &mut evidence,
            )?);
            total.add(verify_source(
                &svd,
                archive,
                &rust_artifact,
                rust_companion.as_deref(),
                &rust_prefix,
                &archive_profiles,
                disposition_manifest.as_ref(),
                &mut evidence,
            )?);
            let orphan_probes = orphan_probe_count(
                &rust_artifact,
                &rust_prefix,
                &[(rom, &rom_symbols), (archive, &archive_symbols)],
            )?;
            println!(
                "TOTAL-SUMMARY\tvendor-functions={}\tmatch={}\tsymbolic-match={}\tscenario-match={}\tstate-match={}\tcomposition-match={}\tmismatch={}\tincomplete={}\tmissing-rust-probe={}\timplemented-unqualified={}\tnot-yet-ported={}\torphan-rust-probe={orphan_probes}",
                total.vendor_functions,
                total.matched,
                total.symbolic_matches,
                total.scenario_matches,
                total.state_matches,
                total.composition_matches,
                total.mismatched,
                total.incomplete,
                total.missing,
                total.implemented_unqualified,
                total.not_yet_ported,
            );
            print_evidence(&evidence);
            let evidence_passed = evidence_baseline
                .as_deref()
                .map(load_evidence_baseline)
                .transpose()?
                .is_none_or(|baseline| check_evidence_baseline(&baseline, &evidence));
            let passed = gate.passes(total, orphan_probes) && evidence_passed;
            if let Some(path) = json_report.as_deref() {
                let mut artifacts = vec![
                    ("rom", rom_artifact.as_path()),
                    ("archive", archive_artifact.as_path()),
                    ("archive-inventory", archive_inventory.as_path()),
                    ("rust-probes", rust_artifact.as_path()),
                ];
                if let Some(companion) = rom_companion.as_deref() {
                    artifacts.push(("rom-companion", companion));
                }
                if let Some(companion) = archive_companion.as_deref() {
                    artifacts.push(("archive-companion", companion));
                }
                if let Some(companion) = rust_companion.as_deref() {
                    artifacts.push(("rust-companion", companion));
                }
                if let Some(profiles) = profile_path.as_deref() {
                    artifacts.push(("profiles", profiles));
                }
                if let Some(dispositions) = disposition_path.as_deref() {
                    artifacts.push(("dispositions", dispositions));
                }
                if let Some(baseline) = evidence_baseline.as_deref() {
                    artifacts.push(("evidence-baseline", baseline));
                }
                write_verification_json_report(
                    path,
                    gate,
                    total,
                    orphan_probes,
                    evidence_passed,
                    passed,
                    &evidence,
                    &artifacts,
                    &disposition_manifest
                        .as_ref()
                        .map(|manifest| {
                            manifest
                                .entries()
                                .filter(|entry| {
                                    entry.disposition.is_implemented()
                                        && entry.semantic_contract.is_none()
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default(),
                )?;
            }
            gate.report(passed);
            Ok(passed)
        }
        "verify" => {
            let mut vendor_artifact = None;
            let mut vendor_inventory = None;
            let mut vendor_companion = None;
            let mut rust_artifact = None;
            let mut rust_companion = None;
            let mut profile_path = None;
            let mut vendor_prefix = "phy_".to_owned();
            let mut rust_prefix = "open_phy_trace_".to_owned();
            let mut gate_name = "completion".to_owned();
            let mut match_floor = None;
            let mut evidence_baseline = None;
            let mut arguments = filtered.into_iter();
            while let Some(argument) = arguments.next() {
                match argument.as_str() {
                    "--vendor-artifact" => {
                        vendor_artifact = Some(PathBuf::from(take_value(
                            &mut arguments,
                            "--vendor-artifact",
                        )?));
                    }
                    "--rust-artifact" => {
                        rust_artifact = Some(PathBuf::from(take_value(
                            &mut arguments,
                            "--rust-artifact",
                        )?));
                    }
                    "--vendor-inventory" => {
                        vendor_inventory = Some(PathBuf::from(take_value(
                            &mut arguments,
                            "--vendor-inventory",
                        )?));
                    }
                    "--vendor-companion" => {
                        vendor_companion = Some(PathBuf::from(take_value(
                            &mut arguments,
                            "--vendor-companion",
                        )?));
                    }
                    "--rust-companion" => {
                        rust_companion = Some(PathBuf::from(take_value(
                            &mut arguments,
                            "--rust-companion",
                        )?));
                    }
                    "--profiles" => {
                        profile_path =
                            Some(PathBuf::from(take_value(&mut arguments, "--profiles")?));
                    }
                    "--vendor-prefix" => {
                        vendor_prefix = take_value(&mut arguments, "--vendor-prefix")?;
                    }
                    "--rust-prefix" => {
                        rust_prefix = take_value(&mut arguments, "--rust-prefix")?;
                    }
                    "--gate" => gate_name = take_value(&mut arguments, "--gate")?,
                    "--match-floor" => {
                        match_floor =
                            Some(take_value(&mut arguments, "--match-floor")?.parse::<usize>()?);
                    }
                    "--evidence-baseline" => {
                        evidence_baseline = Some(PathBuf::from(take_value(
                            &mut arguments,
                            "--evidence-baseline",
                        )?));
                    }
                    _ => return Err(format!("unknown verify option: {argument}").into()),
                }
            }
            let vendor_artifact = vendor_artifact.ok_or("missing --vendor-artifact")?;
            let rust_artifact = rust_artifact.ok_or("missing --rust-artifact")?;
            let gate = VerificationGate::parse(&gate_name, match_floor)?;
            if matches!(gate, VerificationGate::Regression { .. }) && evidence_baseline.is_none() {
                return Err("--gate regression requires --evidence-baseline".into());
            }
            let execution_profiles = profile_path
                .as_deref()
                .map(profiles::load)
                .transpose()?
                .unwrap_or_default();
            let source = VerifySource {
                name: "vendor",
                artifact: &vendor_artifact,
                inventory: vendor_inventory.as_deref(),
                companion: vendor_companion.as_deref(),
                prefix: &vendor_prefix,
            };
            let symbols = vendor_symbols(source)?;
            let mut evidence = EvidenceSet::new();
            let summary = verify_source(
                &svd,
                source,
                &rust_artifact,
                rust_companion.as_deref(),
                &rust_prefix,
                &execution_profiles,
                None,
                &mut evidence,
            )?;
            let orphan_probes =
                orphan_probe_count(&rust_artifact, &rust_prefix, &[(source, &symbols)])?;
            println!(
                "SUMMARY\tvendor-functions={}\tmatch={}\tsymbolic-match={}\tscenario-match={}\tstate-match={}\tcomposition-match={}\tmismatch={}\tincomplete={}\tmissing-rust-probe={}\torphan-rust-probe={orphan_probes}",
                summary.vendor_functions,
                summary.matched,
                summary.symbolic_matches,
                summary.scenario_matches,
                summary.state_matches,
                summary.composition_matches,
                summary.mismatched,
                summary.incomplete,
                summary.missing
            );
            print_evidence(&evidence);
            let evidence_passed = evidence_baseline
                .as_deref()
                .map(load_evidence_baseline)
                .transpose()?
                .is_none_or(|baseline| check_evidence_baseline(&baseline, &evidence));
            let passed = gate.passes(summary, orphan_probes) && evidence_passed;
            gate.report(passed);
            Ok(passed)
        }
        "extract" => {
            let mut input_arguments = filtered.into_iter();
            let input = parse_input(&mut input_arguments, "")?;
            let trace = extract(&input, &svd)?;
            print_trace(&trace);
            Ok(trace.is_exact())
        }
        "compare" => {
            let split = filtered
                .iter()
                .position(|argument| argument == "--right-artifact")
                .ok_or("missing --right-artifact")?;
            let mut left_arguments = filtered[..split].iter().cloned();
            let mut right_arguments = filtered[split..].iter().cloned();
            let left = parse_input(&mut left_arguments, "left")?;
            let right = parse_input(&mut right_arguments, "right")?;
            let left_trace = extract(&left, &svd)?;
            let right_trace = extract(&right, &svd)?;
            print_trace(&left_trace);
            print_trace(&right_trace);
            if !left_trace.is_exact() || !right_trace.is_exact() {
                println!("VERDICT\tINCOMPLETE");
                return Ok(false);
            }
            let equal = traces_equal(&left_trace, &right_trace);
            println!("VERDICT\t{}", if equal { "MATCH" } else { "MISMATCH" });
            Ok(equal)
        }
        _ => Err(format!("unknown command: {command}").into()),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(2),
        Err(error) => {
            usage();
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map() -> SvdMap {
        SvdMap {
            registers: vec![Register {
                address: 0x2010_7030,
                name: "AGC.CONTROL".to_owned(),
            }],
            windows: vec![Window {
                start: 0x2010_0000,
                end: 0x2020_0000,
            }],
        }
    }

    fn indexed_map(base: u32, stride: u32, count: u32, family: &str) -> SvdMap {
        SvdMap {
            registers: (0..count)
                .map(|index| Register {
                    address: base.wrapping_add(index.wrapping_mul(stride)),
                    name: format!("{family}{index}"),
                })
                .collect(),
            windows: vec![Window {
                start: base,
                end: base.wrapping_add(count.wrapping_mul(stride)),
            }],
        }
    }

    #[test]
    fn affine_indexed_mmio_requires_a_contiguous_svd_bank_and_emits_a_guard() {
        let address = Value::expression(
            ExpressionOperation::Add,
            Value::input(0).shift_left(3),
            Value::Constant(0x2010_4004),
        );
        let domain =
            indexed_mmio_domain(&address, &indexed_map(0x2010_4004, 8, 4, "WIFI.BSSID_HIGH"))
                .unwrap();

        assert_eq!(domain.registers.len(), 4);
        assert_eq!(domain.guard.unwrap().maximum, 3);

        let missing_middle = SvdMap {
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
            windows: vec![],
        };
        assert!(indexed_mmio_domain(&address, &missing_middle).is_none());
    }

    #[test]
    fn masked_indexed_mmio_proves_its_entire_domain_without_an_argument_guard() {
        let address = Value::input(1)
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
        let register = Value::RegisterImage {
            read_token: 0,
            address: 0x2010_7030,
            and_mask: u32::MAX,
            or_mask: 0,
        };
        let argument = Value::input(0).and(7);

        for value in [
            register.clone().bitand(argument.clone()),
            register.clone().bitor(argument.clone()),
            register.clone().bitxor(argument.clone()),
        ] {
            assert!(matches!(value, Value::Expression { .. }));
            assert!(value.is_resolved());
        }
        assert_eq!(argument.clone().bitxor(argument), Value::Constant(0));

        let zero_test = Value::input(2).seqz();
        assert!(matches!(zero_test, Value::Expression { .. }));
        assert_eq!(evaluate_for_input(&zero_test, 2, 0), Some(1));
        assert_eq!(evaluate_for_input(&zero_test, 2, 7), Some(0));
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

    fn wifi_osi_tail_symbol(slot_offset: u32) -> binary::BinarySymbol {
        let slot_load = ((slot_offset & 0x0fff) << 20) | (15 << 15) | (2 << 12) | (15 << 7) | 0x03;
        let mut bytes = vec![
            0xb7, 0x07, 0x00, 0x00, // lui a5, %hi(g_osi_funcs_p)
            0x83, 0xa7, 0x07, 0x00, // lw a5, %lo(g_osi_funcs_p)(a5)
        ];
        bytes.extend_from_slice(&slot_load.to_le_bytes());
        bytes.extend_from_slice(&[0x82, 0x87]); // jr a5
        binary::BinarySymbol {
            member: Some("synthetic.o".to_owned()),
            name: "wifi_osi_tail".to_owned(),
            address: 0x1000,
            bytes,
            addresses_resolved: false,
            memory_regions: Vec::new(),
            relocations: vec![binary::SymbolRelocation {
                address: 0x1004,
                kind: binary::RelocationKind::Lo12I,
                symbol: "g_osi_funcs_p".to_owned(),
                addend: 0,
            }],
        }
    }

    #[test]
    fn wifi_osi_rand_tail_call_resolves_from_relocation() {
        let symbol = wifi_osi_tail_symbol(0x0bc);
        let trace =
            trace_binary_symbol(&symbol, &map(), &BTreeMap::new(), &BTreeMap::new(), None).unwrap();

        assert!(trace.is_reference_eligible(), "{trace:#?}");
        assert_eq!(trace.return_value, Value::ExternalResult(0));
        assert!(matches!(
            trace.reference_events.as_slice(),
            [ReferenceEvent::ExternalCall {
                token: 0,
                table: external_abi::Table::Esp32s31WifiOsiV9,
                function: external_abi::Function::Rand,
                ..
            }]
        ));

        let generated = reference_codegen::generate(
            &trace,
            "libpp.a",
            ESP32S31_LIBPP_SHA256,
            Some("hal_mac.o"),
            &[],
        )
        .unwrap();
        assert!(generated.source.contains("pub trait ReferencePlatform"));
        assert!(
            generated
                .source
                .contains("let external_result0 = platform.wifi_osi_rand();")
        );
        assert!(
            generated
                .source
                .contains("assert_eq!(platform.wifi_osi_version(), 0x00000009_u32")
        );
        assert!(
            generated
                .source
                .contains("assert_eq!(platform.wifi_osi_magic(), 0xdeadbeaf_u32")
        );
        assert!(
            generated
                .source
                .contains("assert_eq!(platform.wifi_osi_table_size(), 0x00000200_u32")
        );
        assert!(
            generated
                .source
                .contains("ReferenceOutcome { exit_a0: Some(external_result0) }")
        );
        assert!(generated.source.contains(
            external_abi::table_spec(external_abi::Table::Esp32s31WifiOsiV9).source_sha256
        ));
    }

    #[test]
    fn unknown_wifi_osi_slot_fails_closed() {
        let symbol = wifi_osi_tail_symbol(0x0c0);
        let trace =
            trace_binary_symbol(&symbol, &map(), &BTreeMap::new(), &BTreeMap::new(), None).unwrap();

        assert!(!trace.is_reference_eligible());
        assert!(trace.reference_blockers.iter().any(|blocker| {
            blocker.contains("unregistered-external-abi-slot") && blocker.contains("+0xc0")
        }));
    }

    #[test]
    fn wifi_osi_output_pointer_outside_private_stack_fails_closed() {
        let symbol = wifi_osi_tail_symbol(0x1a8);
        let trace =
            trace_binary_symbol(&symbol, &map(), &BTreeMap::new(), &BTreeMap::new(), None).unwrap();

        assert!(!trace.is_reference_eligible());
        assert!(trace.reference_blockers.iter().any(|blocker| {
            blocker.contains("unsupported-external-output-pointer")
                && blocker.contains("_coex_pti_get")
                && blocker.contains("a1")
        }));
    }

    #[test]
    fn real_libpp_hal_random_resolves_through_wifi_osi_abi() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("phy-trace remains under tools/phy-trace");
        let artifact = root.join("_oracles/libpp.a");
        if !artifact.exists() {
            eprintln!("private libpp fixture is not installed; integration test skipped");
            return;
        }

        let trace = ReferenceCatalog::load(&artifact, &[])
            .unwrap()
            .trace(Some("hal_mac.o"), "hal_random", &map())
            .unwrap();
        assert!(trace.is_reference_eligible(), "{trace:#?}");
        assert_eq!(trace.return_value, Value::ExternalResult(0));
    }

    #[test]
    fn real_libpp_coex_output_bytes_reach_compilable_reference_codegen() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("phy-trace remains under tools/phy-trace");
        let artifact = root.join("_oracles/libpp.a");
        if !artifact.exists() {
            eprintln!("private libpp fixture is not installed; integration test skipped");
            return;
        }
        let svd = SvdMap::load_all(&[
            root.join("svd/esp32s31-radio.svd"),
            root.join("svd/esp32s31-platform-radio-deps.svd"),
        ])
        .unwrap();
        let trace = ReferenceCatalog::load(&artifact, &[])
            .unwrap()
            .trace(Some("hal_coex.o"), "hal_set_ofdma_sequence_pti", &svd)
            .unwrap();

        assert!(trace.is_reference_eligible(), "{trace:#?}");
        assert_eq!(
            trace
                .reference_events
                .iter()
                .filter(|event| matches!(event, ReferenceEvent::ExternalCall { .. }))
                .count(),
            12
        );
        assert_eq!(
            trace.reference_dependencies,
            [
                "hal_set_tb_pti",
                "hal_set_beamf_pti",
                "hal_set_beamf_mt_pti"
            ]
        );
        assert!(trace.reference_events.iter().any(|event| matches!(
            event,
            ReferenceEvent::DiagnosticCall { function, .. } if function == "wifi_log"
        )));
        let generated = reference_codegen::generate(
            &trace,
            "libpp.a",
            ESP32S31_LIBPP_SHA256,
            Some("hal_coex.o"),
            &[],
        )
        .unwrap();
        assert_eq!(
            generated.source.matches("wifi_osi_coex_pti_get(").count(),
            13
        );
        assert_generated_reference_compiles("hal_set_ofdma_sequence_pti", &generated.source);
    }

    #[test]
    fn real_libpp_coex_runtime_leaves_generate_compilable_references() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("phy-trace remains under tools/phy-trace");
        let artifact = root.join("_oracles/libpp.a");
        if !artifact.exists() {
            eprintln!("private libpp fixture is not installed; integration test skipped");
            return;
        }
        let svd = SvdMap::load_all(&[
            root.join("svd/esp32s31-radio.svd"),
            root.join("svd/esp32s31-platform-radio-deps.svd"),
        ])
        .unwrap();
        let catalog = ReferenceCatalog::load(&artifact, &[]).unwrap();

        for symbol in [
            "hal_set_rx_beacon_time",
            "hal_set_rx_beacon_pti",
            "hal_clear_rx_beacon_pti",
            "hal_set_itwt_pti",
            "hal_clr_itwt_pti",
        ] {
            let trace = catalog.trace(Some("hal_coex.o"), symbol, &svd).unwrap();
            assert!(trace.is_reference_eligible(), "{symbol}: {trace:#?}");
            let generated = reference_codegen::generate(
                &trace,
                "libpp.a",
                ESP32S31_LIBPP_SHA256,
                Some("hal_coex.o"),
                &[],
            )
            .unwrap();
            assert_generated_reference_compiles(symbol, &generated.source);
        }
    }

    #[test]
    fn real_libpp_tsf_runtime_leaves_generate_compilable_references() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("phy-trace remains under tools/phy-trace");
        let artifact = root.join("_oracles/libpp.a");
        if !artifact.exists() {
            eprintln!("private libpp fixture is not installed; integration test skipped");
            return;
        }
        let svd = SvdMap::load_all(&[
            root.join("svd/esp32s31-radio.svd"),
            root.join("svd/esp32s31-platform-radio-deps.svd"),
        ])
        .unwrap();
        let catalog = ReferenceCatalog::load(&artifact, &[]).unwrap();

        for (member, symbol) in [
            ("hal_tsf.o", "hal_enable_nan_tsf"),
            ("hal_tsf.o", "hal_disable_nan_tsf"),
            ("hal_tsf.o", "hal_disable_softap_tsf"),
            ("hal_tsf.o", "hal_set_sta_tbtt"),
            ("hal_tsf.o", "hal_set_sta_tbtt_interval"),
            ("hal_tsf.o", "hal_set_sta_light_sleep_wake_ahead_time"),
            ("hal_tsf.o", "hal_is_sta_tsf_active"),
            ("hal_tsf.o", "hal_tsf_clear_soc_wakeup_request"),
            ("hal_mac.o", "hal_enable_sta_btwt_tsf"),
        ] {
            let trace = catalog.trace(Some(member), symbol, &svd).unwrap();
            assert!(
                trace.is_reference_eligible(),
                "{member}::{symbol}: {trace:#?}"
            );
            let generated = reference_codegen::generate(
                &trace,
                "libpp.a",
                ESP32S31_LIBPP_SHA256,
                Some(member),
                &[],
            )
            .unwrap();
            assert_generated_reference_compiles(symbol, &generated.source);
        }
    }

    #[test]
    fn real_libpp_remaining_mmio_leaves_generate_compilable_references() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("phy-trace remains under tools/phy-trace");
        let artifact = root.join("_oracles/libpp.a");
        if !artifact.exists() {
            eprintln!("private libpp fixture is not installed; integration test skipped");
            return;
        }
        let svd = SvdMap::load_all(&[
            root.join("svd/esp32s31-radio.svd"),
            root.join("svd/esp32s31-platform-radio-deps.svd"),
        ])
        .unwrap();
        let catalog = ReferenceCatalog::load(&artifact, &[]).unwrap();

        for (member, symbol) in [
            ("hal_mac.o", "hal_beacon_ie_crc_get"),
            ("hal_mac.o", "hal_enable_sta_beacon_filter"),
            ("hal_mac.o", "hal_disable_sta_beacon_filter"),
            ("hal_mac_ctl.o", "hal_he_set_hw_qos_null_ra_to_trans"),
            ("hal_mac_ctl.o", "hal_mac_interrupt_clr_bsscolor"),
            ("hal_mac_rx.o", "hal_mac_rx_get_end_state"),
            ("hal_mac_rx.o", "hal_mac_rx_get_end_info"),
            ("hal_sniffer.o", "hal_sniffer_rx_clr_statistics"),
        ] {
            let trace = catalog.trace(Some(member), symbol, &svd).unwrap();
            assert!(
                trace.is_reference_eligible(),
                "{member}::{symbol}: {trace:#?}"
            );
            let generated = reference_codegen::generate(
                &trace,
                "libpp.a",
                ESP32S31_LIBPP_SHA256,
                Some(member),
                &[],
            )
            .unwrap();
            assert_generated_reference_compiles(symbol, &generated.source);
        }
    }

    #[test]
    fn real_libpp_timer_update_generates_both_symbolic_cfg_paths() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("phy-trace remains under tools/phy-trace");
        let artifact = root.join("_oracles/libpp.a");
        if !artifact.exists() {
            eprintln!("private libpp fixture is not installed; integration test skipped");
            return;
        }
        let svd = SvdMap::load_all(&[
            root.join("svd/esp32s31-radio.svd"),
            root.join("svd/esp32s31-platform-radio-deps.svd"),
        ])
        .unwrap();

        let trace = ReferenceCatalog::load(&artifact, &[])
            .unwrap()
            .trace(Some("hal_tsf.o"), "hal_timer_update_by_rtc", &svd)
            .unwrap();

        assert!(trace.is_reference_eligible(), "{trace:#?}");
        assert!(!trace.is_exact());
        assert!(trace.reference_flow.is_some());
        let generated = reference_codegen::generate(
            &trace,
            "libpp.a",
            ESP32S31_LIBPP_SHA256,
            Some("hal_tsf.o"),
            &[],
        )
        .unwrap();
        assert!(generated.exit_a0_modeled);
        assert!(generated.source.contains("if (args[0]"));
        assert!(generated.source.contains("0x2010d830_u32"));
        assert!(generated.source.contains("0x2010d878_u32"));
        assert!(generated.source.contains("0x08000000_u32"));
        assert!(generated.source.contains("0x0003ffff_u32"));
    }

    #[test]
    fn real_libpp_indexed_mmio_generates_guarded_compilable_references() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("phy-trace remains under tools/phy-trace");
        let artifact = root.join("_oracles/libpp.a");
        if !artifact.exists() {
            eprintln!("private libpp fixture is not installed; integration test skipped");
            return;
        }
        let svd = SvdMap::load_all(&[
            root.join("svd/esp32s31-radio.svd"),
            root.join("svd/esp32s31-platform-radio-deps.svd"),
        ])
        .unwrap();
        let catalog = ReferenceCatalog::load(&artifact, &[]).unwrap();

        for (member, symbol) in [
            ("hal_mac.o", "hal_mac_is_txq_valid"),
            ("hal_mac.o", "hal_mac_clr_bssid"),
            ("hal_mac_ctl.o", "hal_he_set_ac_muedca_param"),
        ] {
            let trace = catalog.trace(Some(member), symbol, &svd).unwrap();
            assert!(trace.is_reference_eligible(), "{symbol}: {trace:#?}");
            let generated = reference_codegen::generate(
                &trace,
                "libpp.a",
                ESP32S31_LIBPP_SHA256,
                Some(member),
                &[],
            )
            .unwrap();
            if member == "hal_mac.o" {
                assert!(generated.source.contains("let mmio_selector0 = args[0]"));
            }
            assert!(generated.source.contains("assert!(matches!(mmio_address0"));
            assert_generated_reference_compiles(symbol, &generated.source);
        }

        for symbol in [
            "hal_disable_tsf_timer_wakeup",
            "hal_enable_tsf_timer_wakeup",
            "hal_tsf_timer_set_target",
            "hal_tsf_timer_get_target",
            "hal_disable_tsf_timer",
            "hal_enable_tsf_timer",
        ] {
            let trace = catalog.trace(Some("hal_tsf.o"), symbol, &svd).unwrap();
            assert!(trace.is_reference_eligible(), "{symbol}: {trace:#?}");
            let generated = reference_codegen::generate(
                &trace,
                "libpp.a",
                ESP32S31_LIBPP_SHA256,
                Some("hal_tsf.o"),
                &[],
            )
            .unwrap();
            assert!(generated.source.contains("assert!(matches!(mmio_address"));
            assert_generated_reference_compiles(symbol, &generated.source);
        }
    }

    #[test]
    fn real_libpp_caller_memory_accessors_generate_compilable_references() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("phy-trace remains under tools/phy-trace");
        let artifact = root.join("_oracles/libpp.a");
        if !artifact.exists() {
            eprintln!("private libpp fixture is not installed; integration test skipped");
            return;
        }
        let svd = SvdMap::load_all(&[
            root.join("svd/esp32s31-radio.svd"),
            root.join("svd/esp32s31-platform-radio-deps.svd"),
        ])
        .unwrap();
        let catalog = ReferenceCatalog::load(&artifact, &[]).unwrap();

        for (member, name) in [
            ("hal_mac.o", "hal_mac_ftm_get_t3"),
            ("hal_mac_ctl.o", "hal_mac_get_csi_filter"),
        ] {
            let trace = catalog.trace(Some(member), name, &svd).unwrap();
            assert!(
                trace.is_reference_eligible(),
                "{member}::{name}: {trace:#?}"
            );
            assert!(trace.reference_events.iter().any(|event| matches!(
                event,
                ReferenceEvent::Memory { region, .. }
                    if region == "caller-owned ABI argument RAM"
            )));
            let generated = reference_codegen::generate(
                &trace,
                "libpp.a",
                ESP32S31_LIBPP_SHA256,
                Some(member),
                &[],
            )
            .unwrap();
            assert_generated_reference_compiles(name, &generated.source);
        }
    }

    #[test]
    fn real_libpp_relocated_state_accessors_generate_compilable_references() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("phy-trace remains under tools/phy-trace");
        let artifact = root.join("_oracles/libpp.a");
        if !artifact.exists() {
            eprintln!("private libpp fixture is not installed; integration test skipped");
            return;
        }
        let svd = SvdMap::load_all(&[
            root.join("svd/esp32s31-radio.svd"),
            root.join("svd/esp32s31-platform-radio-deps.svd"),
        ])
        .unwrap();
        let catalog = ReferenceCatalog::load(&artifact, &[]).unwrap();

        for (member, name) in [
            ("hal_mac.o", "hal_mac_set_csi"),
            ("hal_tsf.o", "hal_tsf_get_tbttstart"),
        ] {
            let trace = catalog.trace(Some(member), name, &svd).unwrap();
            assert!(
                trace.is_reference_eligible(),
                "{member}::{name}: {trace:#?}"
            );
            let generated = reference_codegen::generate(
                &trace,
                "libpp.a",
                ESP32S31_LIBPP_SHA256,
                Some(member),
                &[],
            )
            .unwrap();
            assert!(generated.source.contains("memory.symbol_address("));
            assert_generated_reference_compiles(name, &generated.source);
        }
    }

    #[test]
    fn every_eligible_real_libpp_hal_trace_reaches_codegen() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("phy-trace remains under tools/phy-trace");
        let artifact = root.join("_oracles/libpp.a");
        if !artifact.exists() {
            eprintln!("private libpp fixture is not installed; integration test skipped");
            return;
        }
        let svd = SvdMap::load_all(&[
            root.join("svd/esp32s31-radio.svd"),
            root.join("svd/esp32s31-platform-radio-deps.svd"),
        ])
        .unwrap();
        let catalog = ReferenceCatalog::load(&artifact, &[]).unwrap();
        let symbols = catalog
            .symbols
            .iter()
            .filter(|symbol| symbol.name.starts_with("hal_"))
            .map(|symbol| (symbol.member.clone(), symbol.name.clone()))
            .collect::<Vec<_>>();
        let mut eligible = 0usize;

        for (member, name) in symbols {
            let trace = catalog.trace(member.as_deref(), &name, &svd).unwrap();
            if trace.is_reference_eligible() {
                eligible += 1;
                let generated = reference_codegen::generate(
                    &trace,
                    "libpp.a",
                    ESP32S31_LIBPP_SHA256,
                    member.as_deref(),
                    &[],
                )
                .unwrap_or_else(|error| panic!("eligible {member:?}::{name} failed: {error}"));
                if trace.reference_indexed_mmio_count() != 0 {
                    assert_generated_reference_compiles(&name, &generated.source);
                }
            }
        }
        assert!(eligible > 0);
    }

    #[test]
    fn real_libpp_mac_delay_names_both_wifi_osi_callbacks() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("phy-trace remains under tools/phy-trace");
        let artifact = root.join("_oracles/libpp.a");
        if !artifact.exists() {
            eprintln!("private libpp fixture is not installed; integration test skipped");
            return;
        }

        let trace = ReferenceCatalog::load(&artifact, &[])
            .unwrap()
            .trace(Some("hal_mac_ctl.o"), "hal_he_set_mac_delay", &map())
            .unwrap();
        let functions = trace
            .reference_events
            .iter()
            .filter_map(|event| match event {
                ReferenceEvent::ExternalCall { function, .. } => Some(*function),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(functions.starts_with(&[
            external_abi::Function::EnvIsChip,
            external_abi::Function::Random,
        ]));
        assert!(trace.reference_blockers.iter().all(|blocker| {
            !blocker.contains("esp32s31-wifi-osi-v9+0x4")
                && !blocker.contains("esp32s31-wifi-osi-v9+0x144")
        }));
    }

    #[test]
    fn linked_rom_catalog_discovers_wifi_osi_pointer_cell_by_symbol() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("phy-trace remains under tools/phy-trace");
        let artifact = root.join("_oracles/esp32s31_rev0_rom.elf");
        if !artifact.exists() {
            eprintln!("private ROM fixture is not installed; integration test skipped");
            return;
        }

        let catalog = ReferenceCatalog::load(&artifact, &[]).unwrap();
        assert_eq!(
            catalog.external_pointer_cells.get(&0x2f07_ff44),
            Some(&external_abi::Table::Esp32s31WifiOsiV9)
        );
    }

    #[test]
    fn wifi_osi_result_survives_direct_call_composition() {
        let parent = binary::BinarySymbol {
            member: None,
            name: "rand_wrapper".to_owned(),
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
        let mut child = wifi_osi_tail_symbol(0x0bc);
        child.address = 0x2000;
        child.relocations[0].address = 0x2004;
        let symbols = BTreeMap::from([(0x2000, child)]);
        let mut visiting = BTreeSet::from([0x1000]);

        let trace = resolve_reference_trace(
            &parent,
            &symbols,
            &BTreeMap::new(),
            &BTreeMap::new(),
            None,
            &map(),
            &mut visiting,
        )
        .unwrap();

        assert!(trace.is_reference_eligible(), "{trace:#?}");
        assert_eq!(trace.reference_dependencies, ["wifi_osi_tail"]);
        assert_eq!(trace.return_value, Value::ExternalResult(0).and(0xff));
        assert!(matches!(
            trace.reference_events.as_slice(),
            [ReferenceEvent::ExternalCall {
                token: 0,
                function: external_abi::Function::Rand,
                ..
            }]
        ));
    }

    #[test]
    fn composite_svd_catalog_resolves_platform_owned_radio_dependencies() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("phy-trace remains under tools/phy-trace");
        let map = SvdMap::load_all(&[
            root.join("svd/esp32s31-radio.svd"),
            root.join("svd/esp32s31-platform-radio-deps.svd"),
        ])
        .unwrap();

        assert_eq!(map.register_name(0x2010_9c18), "MODEM_SYSCON.WIFI_BB_CFG");
        assert_eq!(map.register_name(0x2010_f800), "I2C_ANA_MST.I2C0_CTRL");
        assert_eq!(map.register_name(0x2010_f824), "I2C_ANA_MST.I2C0_CTRL1");
        assert_eq!(map.register_name(0x2010_f828), "I2C_ANA_MST.I2C1_CTRL1");
        assert_eq!(map.register_name(0x2010_f82c), "I2C_ANA_MST.HW_I2C_CTRL");
        assert_eq!(
            map.register_name(0x2070_1068),
            "LP_AON_CLKRST.RTC_SAR2_PWDET_CCT"
        );
        assert_eq!(map.register_name(0x2070_401c), "PMU.HP_ACTIVE_HP_CK_POWER");
        assert_eq!(map.register_name(0x2070_40f0), "PMU.IMM_HP_CK_POWER_0");
        assert_eq!(map.register_name(0x2070_4184), "PMU.RF_PWC");
        assert_eq!(map.register_name(0x2070_4208), "PMU.ANA_PERI_PWR_CTRL");
        assert_eq!(map.register_name(0x2071_0030), "LP_PERICLKRST.TSENS_CTRL");
        assert_eq!(map.register_name(0x2081_8000), "LP_TSENS.CTRL");
        assert_eq!(map.register_name(0x2081_8018), "LP_TSENS.CLK_CONF");
    }

    #[test]
    fn vendor_provenance_requires_the_complete_artifact_digest() {
        assert!(is_pinned_vendor_digest(ESP32S31_LINKED_LIBPHY_SHA256));
        assert!(is_pinned_vendor_digest(ESP32S31_LIBPHY_SHA256));
        assert!(!is_pinned_vendor_digest(
            "0000000000000000000000000000000000000000000000000000000000000000"
        ));
    }

    #[test]
    fn composite_svd_catalog_rejects_same_address_with_different_names() {
        let registers = [
            Register {
                address: 0x2010_0010,
                name: "FIRST.REGISTER".to_owned(),
            },
            Register {
                address: 0x2010_0010,
                name: "SECOND.REGISTER".to_owned(),
            },
        ];
        let error = reject_register_collisions(&registers).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("conflicting SVD register definitions")
        );
    }

    #[test]
    fn source_qualified_probe_names_disambiguate_vendor_sources() {
        assert!(rust_probe_suffix_matches(
            "archive",
            "set_bb_wdg",
            "archive_set_bb_wdg"
        ));
        assert!(!rust_probe_suffix_matches(
            "rom",
            "set_bb_wdg",
            "archive_set_bb_wdg"
        ));
        assert!(rust_probe_suffix_matches(
            "archive",
            "set_bb_wdg",
            "set_bb_wdg"
        ));
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
        let symbol = binary::BinarySymbol {
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
        let trace =
            trace_binary_symbol(&symbol, &map(), &BTreeMap::new(), &BTreeMap::new(), None).unwrap();

        assert!(trace.is_reference_eligible(), "{trace:#?}");
        assert_eq!(trace.reference_events.len(), 2);
        let ReferenceEvent::Memory {
            access: Access::Read,
            address: read_address,
            region,
            ..
        } = &trace.reference_events[0]
        else {
            panic!("expected caller-owned RAM read");
        };
        assert_eq!(region, "caller-owned ABI argument RAM");
        assert!(read_address.canonical().contains("arg0"));
        let ReferenceEvent::Memory {
            access: Access::Write,
            address: write_address,
            value: Some(Value::MemoryImage { read_token: 0, .. }),
            ..
        } = &trace.reference_events[1]
        else {
            panic!("expected caller-owned RAM write of the first read");
        };
        assert!(write_address.canonical().contains("arg0"));

        let generated = reference_codegen::generate(
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
        let symbol = binary::BinarySymbol {
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
        let trace =
            trace_binary_symbol(&symbol, &map(), &BTreeMap::new(), &BTreeMap::new(), None).unwrap();

        assert!(!trace.is_reference_eligible());
        assert_eq!(
            trace
                .reference_events
                .iter()
                .filter(|event| matches!(event, ReferenceEvent::Memory { .. }))
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
        let symbol = binary::BinarySymbol {
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
                binary::SymbolRelocation {
                    address: 0,
                    kind: binary::RelocationKind::Hi20,
                    symbol: "state".to_owned(),
                    addend: 0,
                },
                binary::SymbolRelocation {
                    address: 4,
                    kind: binary::RelocationKind::Lo12I,
                    symbol: "state".to_owned(),
                    addend: 4,
                },
                binary::SymbolRelocation {
                    address: 8,
                    kind: binary::RelocationKind::Lo12S,
                    symbol: "state".to_owned(),
                    addend: 8,
                },
            ],
        };
        let trace =
            trace_binary_symbol(&symbol, &map(), &BTreeMap::new(), &BTreeMap::new(), None).unwrap();

        assert!(trace.is_reference_eligible(), "{trace:#?}");
        assert_eq!(trace.reference_events.len(), 2);
        let ReferenceEvent::Memory {
            access: Access::Read,
            address:
                Value::SymbolAddress {
                    hi_addend: 0,
                    lo_addend: Some(4),
                    ..
                },
            ..
        } = &trace.reference_events[0]
        else {
            panic!("expected relocated symbolic read");
        };
        let ReferenceEvent::Memory {
            access: Access::Write,
            address:
                Value::SymbolAddress {
                    hi_addend: 0,
                    lo_addend: Some(8),
                    ..
                },
            value: Some(Value::MemoryImage { read_token: 0, .. }),
            ..
        } = &trace.reference_events[1]
        else {
            panic!("expected relocated symbolic write");
        };

        let generated = reference_codegen::generate(
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
        let symbol = binary::BinarySymbol {
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
                binary::SymbolRelocation {
                    address: 0,
                    kind: binary::RelocationKind::Hi20,
                    symbol: "first".to_owned(),
                    addend: 0,
                },
                binary::SymbolRelocation {
                    address: 4,
                    kind: binary::RelocationKind::Lo12I,
                    symbol: "second".to_owned(),
                    addend: 0,
                },
            ],
        };
        let trace =
            trace_binary_symbol(&symbol, &map(), &BTreeMap::new(), &BTreeMap::new(), None).unwrap();

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
        let prefix = vec![ReferenceEvent::Observable(Event::Memory {
            access: Access::Read,
            width: 32,
            address: 0x2010_7030,
            register: "AGC.FIRST".to_owned(),
            value: None,
        })];
        let callee = Trace {
            symbol: "child".to_owned(),
            events: Vec::new(),
            reference_events: vec![
                ReferenceEvent::Observable(Event::Memory {
                    access: Access::Read,
                    width: 32,
                    address: 0x2010_7034,
                    register: "AGC.SECOND".to_owned(),
                    value: None,
                }),
                ReferenceEvent::Observable(Event::Memory {
                    access: Access::Write,
                    width: 32,
                    address: 0x2010_7038,
                    register: "AGC.THIRD".to_owned(),
                    value: Some(Value::input(0)),
                }),
            ],
            reference_dependencies: Vec::new(),
            blockers: Vec::new(),
            reference_blockers: Vec::new(),
            return_value: Value::RegisterImage {
                read_token: 0,
                address: 0x2010_7034,
                and_mask: u32::MAX,
                or_mask: 0,
            },
            reference_flow: None,
            unresolved_branch: None,
        };
        let arguments: [Value; 8] = core::array::from_fn(|index| {
            if index == 0 {
                Value::input(1)
            } else {
                Value::Unknown
            }
        });

        let (events, return_value) =
            inline_reference_summary(&prefix, &callee, &arguments).unwrap();

        assert_eq!(events.len(), 3);
        let ReferenceEvent::Observable(Event::Memory {
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
        let prefix = vec![ReferenceEvent::Observable(Event::Memory {
            access: Access::Read,
            width: 32,
            address: 0x2010_7030,
            register: "AGC.FIRST".to_owned(),
            value: None,
        })];
        let callee = Trace {
            symbol: "indexed_child".to_owned(),
            events: Vec::new(),
            reference_events: vec![ReferenceEvent::IndexedMmio {
                access: Access::Read,
                width: 32,
                address: Value::input(0).shift_left(2).add_constant(0x2010_4000),
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
                    selector: Value::input(0),
                    maximum: 1,
                }),
                value: None,
            }],
            reference_dependencies: Vec::new(),
            blockers: Vec::new(),
            reference_blockers: Vec::new(),
            return_value: Value::IndexedRegisterImage {
                read_token: 0,
                and_mask: u32::MAX,
                or_mask: 0,
            },
            reference_flow: None,
            unresolved_branch: None,
        };
        let arguments: [Value; 8] = core::array::from_fn(|index| {
            if index == 0 {
                Value::input(1)
            } else {
                Value::Unknown
            }
        });

        let (events, return_value) =
            inline_reference_summary(&prefix, &callee, &arguments).unwrap();
        let ReferenceEvent::IndexedMmio {
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
        let callee = Trace {
            symbol: "memory_child".to_owned(),
            events: Vec::new(),
            reference_events: vec![ReferenceEvent::Memory {
                access: Access::Read,
                width: 32,
                address: Value::input(0).add_constant(4),
                region: "caller-owned ABI argument RAM".to_owned(),
                value: None,
            }],
            reference_dependencies: Vec::new(),
            blockers: Vec::new(),
            reference_blockers: Vec::new(),
            return_value: Value::MemoryImage {
                read_token: 0,
                and_mask: u32::MAX,
                or_mask: 0,
            },
            reference_flow: None,
            unresolved_branch: None,
        };
        let arguments: [Value; 8] = core::array::from_fn(|index| {
            if index == 0 {
                Value::input(2).add_constant(8)
            } else {
                Value::Unknown
            }
        });

        let (events, return_value) = inline_reference_summary(&[], &callee, &arguments).unwrap();
        let [
            ReferenceEvent::Memory {
                access: Access::Read,
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
            Value::MemoryImage {
                read_token: 0,
                and_mask: u32::MAX,
                or_mask: 0,
            }
        );
    }

    #[test]
    fn private_stack_round_trips_symbolic_values_and_sign_extension() {
        let mut stack = SymbolicStack::default();
        stack.store(-8, 32, &Value::input(2));
        assert_eq!(
            stack.load(-8, 32, false).unwrap().canonical(),
            Value::input(2).canonical()
        );

        stack.store(-1, 8, &Value::Constant(0x80));
        assert_eq!(
            stack.load(-1, 8, true).unwrap(),
            Value::Constant(0xffff_ff80)
        );
        assert!(stack.load(-12, 32, false).is_none());
    }

    #[test]
    fn call_results_are_substituted_into_parent_dataflow() {
        let value = Value::CallResult(7).and(0xff).shift_left(8).or(3);
        let call_results = BTreeMap::from([(7, Value::Constant(0x1234))]);

        let rewritten = value
            .rewrite_call_context(&[], &[], &[], &call_results)
            .unwrap();

        assert_eq!(rewritten, Value::Constant(0x3403));
    }

    #[test]
    fn returning_direct_call_is_flattened_from_binary_symbols() {
        let parent = binary::BinarySymbol {
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
        let child = binary::BinarySymbol {
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
            &BTreeMap::new(),
            None,
            &map(),
            &mut visiting,
        )
        .unwrap();

        assert!(trace.is_reference_eligible(), "{trace:#?}");
        assert_eq!(trace.reference_dependencies, ["child"]);
        assert_eq!(trace.return_value, Value::input(0).and(0xff));
    }

    #[test]
    fn direct_call_to_symbolic_cfg_callee_is_scoped_and_composed() {
        let parent = binary::BinarySymbol {
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
        let child = binary::BinarySymbol {
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
            &BTreeMap::new(),
            None,
            &map(),
            &mut visiting,
        )
        .unwrap();

        assert!(trace.is_reference_eligible(), "{trace:#?}");
        assert!(matches!(
            trace.reference_events.as_slice(),
            [ReferenceEvent::ComposedCall {
                token: 0,
                symbol,
                result_modeled: true,
                ..
            }] if symbol == "branch_child"
        ));
        let generated =
            reference_codegen::generate(&trace, "oracle.elf", "abc123", None, &[]).unwrap();
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
        let grandparent = binary::BinarySymbol {
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
        let parent = binary::BinarySymbol {
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
        let child = binary::BinarySymbol {
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
            &BTreeMap::new(),
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
        let generated =
            reference_codegen::generate(&trace, "oracle.elf", "abc123", None, &[]).unwrap();
        assert_eq!(generated.source.matches("let call_result0 = {").count(), 2);
        assert_generated_reference_compiles("nested-call-scopes", &generated.source);
    }

    #[test]
    fn caller_cfg_can_branch_on_a_composed_callee_result() {
        let parent = binary::BinarySymbol {
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
        let child = binary::BinarySymbol {
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
            &BTreeMap::new(),
            None,
            &map(),
            &mut visiting,
        )
        .unwrap();

        assert!(trace.is_reference_eligible(), "{trace:#?}");
        let generated =
            reference_codegen::generate(&trace, "oracle.elf", "abc123", None, &[]).unwrap();
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
        let parent = binary::BinarySymbol {
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
        let delay = binary::BinarySymbol {
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
            &BTreeMap::new(),
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
        let parent = binary::BinarySymbol {
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
        let delay = binary::BinarySymbol {
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
            &BTreeMap::new(),
            None,
            &map(),
            &mut visiting,
        )
        .unwrap();

        assert!(trace.is_reference_eligible(), "{trace:#?}");
        let generated =
            reference_codegen::generate(&trace, "oracle.elf", "abc123", None, &[]).unwrap();
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
        let parent = binary::BinarySymbol {
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
        let child = binary::BinarySymbol {
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
            &BTreeMap::new(),
            None,
            &map(),
            &mut visiting,
        )
        .unwrap();

        assert!(trace.is_reference_eligible(), "{trace:#?}");
        assert_eq!(trace.reference_dependencies, ["companion_child"]);
        assert_eq!(trace.return_value, Value::input(0).and(0xff));
    }

    #[test]
    fn unresolved_call_relocation_fails_closed() {
        let parent = binary::BinarySymbol {
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

        let trace =
            trace_binary_symbol(&parent, &map(), &relocations, &BTreeMap::new(), None).unwrap();

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
        let symbol = binary::BinarySymbol {
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

        let trace =
            trace_binary_symbol(&symbol, &map(), &BTreeMap::new(), &BTreeMap::new(), None).unwrap();

        assert!(trace.is_reference_eligible(), "{trace:#?}");
        assert_eq!(trace.return_value, Value::input(0));
    }

    #[test]
    fn local_jump_loop_fails_closed() {
        let symbol = binary::BinarySymbol {
            member: None,
            name: "local_loop".to_owned(),
            address: 0x1000,
            bytes: vec![0x6f, 0x00, 0x00, 0x00], // j 0x1000
            addresses_resolved: true,
            memory_regions: Vec::new(),
            relocations: Vec::new(),
        };

        let trace =
            trace_binary_symbol(&symbol, &map(), &BTreeMap::new(), &BTreeMap::new(), None).unwrap();

        assert!(!trace.is_reference_eligible());
        assert!(trace.blockers[0].contains("control-flow loop"));
    }

    #[test]
    fn delay_intrinsic_is_composed_without_decoding_its_rom_body() {
        let parent = binary::BinarySymbol {
            member: None,
            name: "delay_wrapper".to_owned(),
            address: 0x1000,
            bytes: vec![0x6f, 0x10, 0x00, 0x00], // j 0x2000
            addresses_resolved: true,
            memory_regions: Vec::new(),
            relocations: Vec::new(),
        };
        let delay = binary::BinarySymbol {
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
            &BTreeMap::new(),
            None,
            &map(),
            &mut visiting,
        )
        .unwrap();

        assert!(trace.is_reference_eligible(), "{trace:#?}");
        assert_eq!(trace.reference_dependencies, ["ets_delay_us"]);
        assert_eq!(
            trace.reference_events,
            [ReferenceEvent::DelayMicros {
                micros: Value::input(0)
            }]
        );
    }

    #[test]
    fn constant_conditional_branch_follows_only_the_feasible_edge() {
        let symbol = binary::BinarySymbol {
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

        let trace =
            trace_binary_symbol(&symbol, &map(), &BTreeMap::new(), &BTreeMap::new(), None).unwrap();

        assert!(trace.is_reference_eligible(), "{trace:#?}");
    }

    #[test]
    fn symbolic_conditional_branch_fails_closed() {
        let symbol = binary::BinarySymbol {
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

        let trace =
            trace_binary_symbol(&symbol, &map(), &BTreeMap::new(), &BTreeMap::new(), None).unwrap();

        assert!(!trace.is_reference_eligible());
        assert!(trace.blockers[0].contains("input-dependent control-flow"));
    }

    #[test]
    fn bounded_symbolic_cfg_becomes_structured_reference_flow() {
        let symbol = binary::BinarySymbol {
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
            &BTreeMap::new(),
            None,
            &map(),
            &mut visiting,
        )
        .unwrap();

        assert!(trace.is_reference_eligible(), "{trace:#?}");
        assert!(!trace.is_exact());
        let ReferenceTerminator::Branch {
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
            ReferenceTerminator::Return(Value::Constant(2))
        ));
        assert!(matches!(
            not_taken.terminator,
            ReferenceTerminator::Return(Value::Constant(1))
        ));

        let generated =
            reference_codegen::generate(&trace, "oracle.elf", "abc123", None, &[]).unwrap();
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
        let parent = binary::BinarySymbol {
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
        let child = binary::BinarySymbol {
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
            &BTreeMap::new(),
            None,
            &map(),
            &mut visiting,
        )
        .unwrap();

        assert!(trace.is_reference_eligible(), "{trace:#?}");
        assert_eq!(trace.reference_dependencies, ["conditional_child"]);
        assert_eq!(trace.return_value, Value::Constant(0));
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

    #[test]
    fn regression_and_completion_gates_are_independent() {
        let summary = VerifySummary {
            vendor_functions: 466,
            matched: 103,
            symbolic_matches: 57,
            scenario_matches: 34,
            state_matches: 7,
            composition_matches: 5,
            missing: 363,
            ..VerifySummary::default()
        };
        assert!(VerificationGate::Regression { match_floor: 103 }.passes(summary, 0));
        assert!(!VerificationGate::Regression { match_floor: 104 }.passes(summary, 0));
        assert!(!VerificationGate::Completion.passes(summary, 0));

        let regressed = VerifySummary {
            mismatched: 1,
            ..summary
        };
        assert!(!VerificationGate::Regression { match_floor: 103 }.passes(regressed, 0));
        assert!(VerificationGate::parse("regression", None).is_err());
    }

    #[test]
    fn checked_in_evidence_baseline_locks_symbol_and_evidence_identity() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("baselines/esp32s31.evidence");
        let expected = load_evidence_baseline(&path).unwrap();
        assert_eq!(expected.len(), 103);
        assert!(check_evidence_baseline(&expected, &expected));

        let mut downgraded = expected.clone();
        downgraded.insert(
            ("archive".to_owned(), "phy_rf_init".to_owned()),
            "scenario/profile:weaker".to_owned(),
        );
        assert!(!check_evidence_baseline(&expected, &downgraded));

        let mut missing = expected.clone();
        missing.remove(&("rom".to_owned(), "phy_enable_agc".to_owned()));
        assert!(!check_evidence_baseline(&expected, &missing));
    }

    #[test]
    fn profile_evidence_is_bound_to_scenario_contents() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("phy-trace remains under tools/phy-trace");
        let profiles =
            profiles::load(&root.join("tools/phy-trace/profiles/esp32s31.profile")).unwrap();
        let mut modified = profiles[0].clone();
        let original = profile_evidence(&modified);
        modified.scenarios[0].scenario.max_steps =
            modified.scenarios[0].scenario.max_steps.saturating_add(1);
        assert_ne!(profile_evidence(&modified), original);
    }

    #[test]
    fn semantic_evidence_is_bound_to_validator_sources() {
        let original = semantic_contract_digest_from_sources(
            "esp32s31-channel",
            &[("semantic.rs", "footprint-v1"), ("emulator.rs", "strict")],
        );
        let weakened = semantic_contract_digest_from_sources(
            "esp32s31-channel",
            &[
                ("semantic.rs", "footprint-v1"),
                ("emulator.rs", "permissive"),
            ],
        );
        let other_contract = semantic_contract_digest_from_sources(
            "esp32s31-rf-init",
            &[("semantic.rs", "footprint-v1"), ("emulator.rs", "strict")],
        );
        assert_ne!(original, weakened);
        assert_ne!(original, other_contract);
        assert!(
            semantic_contract_evidence("esp32s31-channel")
                .starts_with("composition-state-scenario/esp32s31-channel/sha256:")
        );
    }

    #[test]
    fn verification_json_report_contains_reproducible_inputs() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let path = env::temp_dir().join(format!(
            "open-esp-radio-phy-trace-report-{}.json",
            std::process::id()
        ));
        let mut evidence = EvidenceSet::new();
        record_evidence(&mut evidence, "archive", "symbol", "symbolic").unwrap();
        write_verification_json_report(
            &path,
            VerificationGate::Regression { match_floor: 1 },
            VerifySummary {
                vendor_functions: 1,
                matched: 1,
                symbolic_matches: 1,
                ..VerifySummary::default()
            },
            0,
            true,
            true,
            &evidence,
            &[("manifest", &manifest)],
            &[],
        )
        .unwrap();
        let report = fs::read_to_string(&path).unwrap();
        fs::remove_file(path).unwrap();
        assert!(report.contains("\"schema_version\": 1"));
        assert!(report.contains("\"passed\": true"));
        assert!(report.contains("\"sha256\""));
        assert!(report.contains("\"symbol\": \"symbol\""));
    }
}
