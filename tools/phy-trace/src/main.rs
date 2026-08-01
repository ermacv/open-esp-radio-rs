//! SVD-aware extraction of direct MMIO traces from compiled RISC-V code.
//!
//! This is deliberately an instruction/ELF tool, not a source-text policy
//! checker. It handles straight-line leaves exactly and fails closed when a
//! symbol contains control flow, calls or unresolved MMIO addressing.

mod binary;
mod dispositions;
mod emulator;
mod profiles;
mod semantic;

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
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
            | ESP32S31_REV0_ROM_LOCAL_SHA256
            | ESP32S31_REV0_ROM_CANONICAL_SHA256
            | ESP32S31_LINKED_LIBPHY_SHA256
    )
}

fn pinned_vendor_digest(path: &Path) -> Result<String> {
    let bytes = fs::read(path)?;
    let digest = format!("{:x}", Sha256::digest(&bytes));
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
    RegisterImage {
        read_token: u32,
        address: u32,
        and_mask: u32,
        or_mask: u32,
    },
    Bits(Box<[BitSource; 32]>),
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
            Self::Bits(bits) => **bits,
        }
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
        Self::Bits(Box::new(bits))
    }

    fn and(self, constant: u32) -> Self {
        Self::from_bits(core::array::from_fn(|bit| {
            if constant & (1 << bit) == 0 {
                BitSource::Constant(false)
            } else {
                self.bits()[bit]
            }
        }))
    }

    fn or(self, constant: u32) -> Self {
        Self::from_bits(core::array::from_fn(|bit| {
            if constant & (1 << bit) != 0 {
                BitSource::Constant(true)
            } else {
                self.bits()[bit]
            }
        }))
    }

    fn bitand(self, other: Self) -> Self {
        let left = self.bits();
        let right = other.bits();
        Self::from_bits(core::array::from_fn(|bit| match (left[bit], right[bit]) {
            (BitSource::Constant(false), _) | (_, BitSource::Constant(false)) => {
                BitSource::Constant(false)
            }
            (BitSource::Constant(true), source) | (source, BitSource::Constant(true)) => source,
            (left, right) if left == right => left,
            _ => BitSource::Unknown,
        }))
    }

    fn bitor(self, other: Self) -> Self {
        let left = self.bits();
        let right = other.bits();
        Self::from_bits(core::array::from_fn(|bit| match (left[bit], right[bit]) {
            (BitSource::Constant(true), _) | (_, BitSource::Constant(true)) => {
                BitSource::Constant(true)
            }
            (BitSource::Constant(false), source) | (source, BitSource::Constant(false)) => source,
            (left, right) if left == right => left,
            _ => BitSource::Unknown,
        }))
    }

    fn shift_left(self, amount: u32) -> Self {
        let source = self.bits();
        Self::from_bits(core::array::from_fn(|bit| {
            bit.checked_sub(amount as usize)
                .map_or(BitSource::Constant(false), |source_bit| source[source_bit])
        }))
    }

    fn shift_right(self, amount: u32) -> Self {
        let source = self.bits();
        Self::from_bits(core::array::from_fn(|bit| {
            source
                .get(bit + amount as usize)
                .copied()
                .unwrap_or(BitSource::Constant(false))
        }))
    }

    fn add_constant(self, constant: u32) -> Self {
        let source = self.bits();
        let mut result = [BitSource::Unknown; 32];
        let mut carry = false;
        let mut carry_unknown = false;
        for bit in 0..32 {
            if carry_unknown {
                result[bit] = BitSource::Unknown;
                continue;
            }
            let add = constant & (1 << bit) != 0;
            match source[bit] {
                BitSource::Constant(value) => {
                    result[bit] = BitSource::Constant(value ^ add ^ carry);
                    carry = (value && add) || (value && carry) || (add && carry);
                }
                symbolic if !add && !carry => {
                    result[bit] = symbolic;
                }
                _ => {
                    result[bit] = BitSource::Unknown;
                    carry_unknown = true;
                }
            }
        }
        Self::from_bits(result)
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
            BitSource::Unknown => BitSource::Unknown,
        }))
    }

    fn xor(self, constant: u32) -> Self {
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
                    BitSource::Unknown => BitSource::Unknown,
                }
            }
        }))
    }

    fn bitxor(self, other: Self) -> Self {
        match (self, other) {
            (Self::Constant(constant), value) | (value, Self::Constant(constant)) => {
                value.xor(constant)
            }
            _ => Self::Unknown,
        }
    }

    fn as_constant(&self) -> Option<u32> {
        match self {
            Self::Constant(value) => Some(*value),
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
            return Self::Bits(Box::new(core::array::from_fn(|bit| {
                if bit == 0 {
                    BitSource::Unknown
                } else {
                    BitSource::Constant(false)
                }
            })));
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
        !matches!(self, Self::Unknown) && !self.bits().contains(&BitSource::Unknown)
    }

    fn canonical(&self) -> String {
        match self {
            Self::Unknown => "unknown".to_owned(),
            Self::Constant(value) => format!("const:{value:#010x}"),
            Self::RegisterImage {
                read_token,
                address,
                and_mask,
                or_mask,
            } => format!("rmw:read{read_token}[{address:#010x}]&{and_mask:#010x}|{or_mask:#010x}"),
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
        value: String,
    },
    Fence {
        fm: u8,
        predecessor: u8,
        successor: u8,
    },
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
    fn memory_value(&self) -> Option<&str> {
        match self {
            Self::Memory { value, .. } => Some(value),
            Self::Fence { .. } => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Trace {
    symbol: String,
    events: Vec<Event>,
    blockers: Vec<String>,
    return_value: Value,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ArtifactSymbol {
    member: Option<String>,
    name: String,
}

impl Trace {
    fn is_exact(&self) -> bool {
        self.blockers.is_empty()
            && self
                .events
                .iter()
                .all(|event| event.unmapped_address().is_none())
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
    let mut blockers = Vec::new();
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
                    events.push(Event::Memory {
                        access: Access::Read,
                        width,
                        address,
                        register: svd.register_name(address),
                        value: "-".to_owned(),
                    });
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
                    events.push(Event::Memory {
                        access: Access::Write,
                        width: width_for(mnemonic).unwrap(),
                        address,
                        register: svd.register_name(address),
                        value: value.canonical(),
                    });
                }
            }
            "ret" => {
                return_value = values.get("a0").cloned().unwrap_or(Value::Unknown);
            }
            "fence" if operands.len() == 2 => {
                match (parse_fence_set(operands[0]), parse_fence_set(operands[1])) {
                    (Some(predecessor), Some(successor)) => events.push(Event::Fence {
                        fm: 0,
                        predecessor,
                        successor,
                    }),
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
        blockers,
        return_value,
    }
}

fn structural_effective_address(values: &[Value; 32], base: Reg, offset: i32) -> Option<u32> {
    values[usize::from(base.0)]
        .as_constant()
        .map(|base| base.wrapping_add(offset as u32))
}

fn structural_set(values: &mut [Value; 32], register: Reg, value: Value) {
    if register != Reg::ZERO {
        values[usize::from(register.0)] = value;
    }
}

fn trace_binary_symbol(symbol: &binary::BinarySymbol, svd: &SvdMap) -> Result<Trace> {
    let mut values: [Value; 32] = core::array::from_fn(|_| Value::Unknown);
    values[0] = Value::Constant(0);
    for index in 0..8 {
        values[10 + index] = Value::input(index as u8);
    }
    let mut events = Vec::new();
    let mut blockers = Vec::new();
    let mut return_value = Value::Unknown;
    let mut next_mmio_read_token = 0_u32;

    for decoded in binary::decode_symbol(symbol)? {
        let pc = decoded.address;
        let width = decoded.width;
        let instruction = decoded.instruction;
        match instruction {
            Inst::Lui { uimm, dest } => {
                structural_set(&mut values, dest, Value::Constant(uimm.as_u32()));
            }
            Inst::Auipc { uimm, dest } => {
                structural_set(
                    &mut values,
                    dest,
                    Value::Constant((pc as u32).wrapping_add(uimm.as_u32())),
                );
            }
            Inst::Addi { imm, dest, src1 } => {
                let value = values[usize::from(src1.0)]
                    .clone()
                    .add_constant(imm.as_u32());
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
                let value = values[usize::from(src1.0)]
                    .as_constant()
                    .map(|value| Value::Constant(((value as i32) >> imm.as_u32()) as u32))
                    .unwrap_or(Value::Unknown);
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
                    _ => Value::Unknown,
                };
                structural_set(&mut values, dest, value);
            }
            Inst::Sub { dest, src1, src2 } => {
                let value = match (
                    values[usize::from(src1.0)].as_constant(),
                    values[usize::from(src2.0)].as_constant(),
                ) {
                    (Some(left), Some(right)) => Value::Constant(left.wrapping_sub(right)),
                    _ => Value::Unknown,
                };
                structural_set(&mut values, dest, value);
            }
            Inst::Sll { dest, src1, src2 }
            | Inst::Srl { dest, src1, src2 }
            | Inst::Sra { dest, src1, src2 } => {
                let source = values[usize::from(src1.0)].clone();
                let amount = values[usize::from(src2.0)]
                    .as_constant()
                    .map(|value| value & 31);
                let value = match (instruction, amount) {
                    (Inst::Sll { .. }, Some(amount)) => source.shift_left(amount),
                    (Inst::Srl { .. }, Some(amount)) => source.shift_right(amount),
                    (Inst::Sra { .. }, Some(amount)) => source
                        .as_constant()
                        .map(|value| Value::Constant(((value as i32) >> amount) as u32))
                        .unwrap_or(Value::Unknown),
                    _ => Value::Unknown,
                };
                structural_set(&mut values, dest, value);
            }
            Inst::Mul { dest, src1, src2 }
            | Inst::Div { dest, src1, src2 }
            | Inst::Divu { dest, src1, src2 }
            | Inst::Rem { dest, src1, src2 }
            | Inst::Remu { dest, src1, src2 } => {
                let left = values[usize::from(src1.0)].as_constant();
                let right = values[usize::from(src2.0)].as_constant();
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
                    _ => Value::Unknown,
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
                let address = structural_effective_address(&values, base, offset.as_i32());
                let value =
                    if let Some(address) = address.filter(|address| svd.contains_mmio(*address)) {
                        let read_token = next_mmio_read_token;
                        next_mmio_read_token += 1;
                        events.push(Event::Memory {
                            access: Access::Read,
                            width,
                            address,
                            register: svd.register_name(address),
                            value: "-".to_owned(),
                        });
                        if width == 32 {
                            Value::RegisterImage {
                                read_token,
                                address,
                                and_mask: u32::MAX,
                                or_mask: 0,
                            }
                        } else {
                            Value::Unknown
                        }
                    } else {
                        Value::Unknown
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
                if let Some(address) = structural_effective_address(&values, base, offset.as_i32())
                    .filter(|address| svd.contains_mmio(*address))
                {
                    let value = values[usize::from(src.0)].clone();
                    if !value.is_resolved() {
                        blockers.push(format!(
                            "unresolved MMIO write value at {pc:#x}: {instruction}"
                        ));
                    }
                    events.push(Event::Memory {
                        access: Access::Write,
                        width,
                        address,
                        register: svd.register_name(address),
                        value: value.canonical(),
                    });
                }
            }
            Inst::Beq { .. }
            | Inst::Bne { .. }
            | Inst::Blt { .. }
            | Inst::Bge { .. }
            | Inst::Bltu { .. }
            | Inst::Bgeu { .. } => {
                blockers.push(format!(
                    "control-flow instruction at {pc:#x}: {instruction}"
                ));
            }
            Inst::Jal { .. } => {
                blockers.push(format!("call/jump instruction at {pc:#x}: {instruction}"));
            }
            Inst::Jalr { offset, base, dest }
                if dest == Reg::ZERO && base == Reg::RA && offset.as_u32() == 0 =>
            {
                return_value = values[usize::from(Reg::A0.0)].clone();
            }
            Inst::Jalr { .. } => {
                blockers.push(format!("call/jump instruction at {pc:#x}: {instruction}"));
            }
            Inst::Fence { fence } => events.push(Event::Fence {
                fm: fence.fm,
                predecessor: encode_fence_set(fence.pred),
                successor: encode_fence_set(fence.succ),
            }),
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
    }

    Ok(Trace {
        symbol: symbol.name.clone(),
        events,
        blockers,
        return_value,
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
    trace_binary_symbol(symbol, svd)
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
        "usage:\n  open-esp-radio-phy-trace execute --svd PATH [--svd PATH]... --artifact PATH [--companion PATH] --symbol NAME [--concrete-only] [--timeline] [--arg VALUE] [--mmio ADDRESS=VALUE] [--read ADDRESS=VALUE] [--ram ADDRESS=VALUE] [--observe ADDRESS=LENGTH] [--max-steps COUNT]\n  open-esp-radio-phy-trace execute-compare --svd PATH [--svd PATH]... --vendor-artifact PATH [--vendor-companion PATH] --vendor-symbol NAME --rust-artifact PATH [--rust-companion PATH] --rust-symbol NAME [--compare-return] [--case NAME [--arg VALUE] [--mmio ADDRESS=VALUE] [--read ADDRESS=VALUE] [--ram ADDRESS=VALUE] [--vendor-ram-symbol ADDRESS=SYMBOL] [--rust-ram-symbol ADDRESS=SYMBOL] [--observe ADDRESS=LENGTH] [--max-steps COUNT]]...\n  open-esp-radio-phy-trace qualify-esp32s31-channel --svd PATH [--svd PATH]... --vendor-artifact PATH --vendor-companion PATH\n  open-esp-radio-phy-trace qualify-esp32s31-rf-init --svd PATH [--svd PATH]... --vendor-artifact PATH --vendor-companion PATH\n  open-esp-radio-phy-trace verify-profiles --svd PATH [--svd PATH]... --profiles PATH --vendor-artifact PATH [--vendor-companion PATH] --rust-artifact PATH [--rust-companion PATH]\n  open-esp-radio-phy-trace analyze --svd PATH [--svd PATH]... --artifact PATH [--symbol-prefix PREFIX]\n  open-esp-radio-phy-trace verify --svd PATH [--svd PATH]... --vendor-artifact PATH [--vendor-inventory PATH] --rust-artifact PATH [--profiles PATH] [--vendor-companion PATH] [--rust-companion PATH] [--vendor-prefix PREFIX] [--rust-prefix PREFIX] [--gate completion|regression] [--match-floor COUNT] [--evidence-baseline PATH]\n  open-esp-radio-phy-trace verify-all --svd PATH [--svd PATH]... --rom-artifact PATH --archive-artifact PATH --archive-inventory PATH --rust-artifact PATH [--profiles PATH] [--dispositions PATH] [--rom-companion PATH] [--archive-companion PATH] [--rust-companion PATH] [--rom-prefix PREFIX] [--archive-prefix PREFIX] [--rust-prefix PREFIX] [--gate completion|regression] [--match-floor COUNT] [--evidence-baseline PATH] [--json-report PATH]\n  open-esp-radio-phy-trace extract --svd PATH [--svd PATH]... --artifact PATH [--member NAME] --symbol NAME\n  open-esp-radio-phy-trace compare --svd PATH [--svd PATH]... --left-artifact PATH [--left-member NAME] --left-symbol NAME --right-artifact PATH [--right-member NAME] --right-symbol NAME"
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
        "analyze" => {
            let mut artifact = None;
            let mut prefix = "phy_".to_owned();
            let mut arguments = filtered.into_iter();
            while let Some(argument) = arguments.next() {
                match argument.as_str() {
                    "--artifact" => {
                        artifact = Some(PathBuf::from(take_value(&mut arguments, "--artifact")?));
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

            let mut exact = 0usize;
            let mut incomplete = 0usize;
            let mut reasons = BTreeMap::<String, usize>::new();
            for symbol in &symbols {
                let input = Input {
                    artifact: artifact.clone(),
                    member: symbol.member.clone(),
                    symbol: symbol.name.clone(),
                };
                let trace = extract(&input, &svd)?;
                let owner = symbol.member.as_deref().unwrap_or("-");
                if trace.is_exact() {
                    exact += 1;
                    println!(
                        "FUNCTION\t{}\t{owner}\tDIRECT-TRACE-EXACT\tevents={}",
                        symbol.name,
                        trace.events.len()
                    );
                } else {
                    incomplete += 1;
                    println!(
                        "FUNCTION\t{}\t{owner}\tINCOMPLETE\tevents={}\tuncovered={}",
                        symbol.name,
                        trace.events.len(),
                        trace.blockers.len()
                            + trace
                                .events
                                .iter()
                                .filter_map(Event::unmapped_address)
                                .count()
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
            }
            println!(
                "SUMMARY\tfunctions={}\tdirect_trace_exact={exact}\tincomplete={incomplete}",
                symbols.len()
            );
            for (reason, count) in reasons {
                println!("SUMMARY-UNCOVERED\t{reason}\t{count}");
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
            trace.events[1].memory_value(),
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
            vendor.events[2].memory_value(),
            Some("rmw:read0[0x20107030]&0xffffffff|0x00000000")
        );
        assert_eq!(
            rust.events[2].memory_value(),
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
