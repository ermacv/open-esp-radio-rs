//! Triage aid for untriaged vendor locations.
//!
//! For every location the scenarios leave untriaged, the report shows the
//! instructions around it, the in-function definitions of the registers the
//! instruction at the location reads, and whether the block it opens only
//! calls reviewed diagnostic functions and stores nothing outside the stack.
//! Instructions are decoded and lifted by Blobray's RISC-V decoder; a forward
//! pass over each function folds constants, so the report shows the
//! addresses, register names and fields an instruction touches. These are
//! proposals for a reviewer: exclusions stay reviewed decisions.
use crate::registers::Registers;
use crate::session::evidence_index::{Location, LocationKind};
use blobray_backend_riscv::RiscvDecoder;
use blobray_domain::{
    FunctionDecoder, FunctionSemantics, InstructionFlow, IntegerOp, MemoryKind, Operand, SemanticOp,
};
use object::{Object, ObjectSymbol, SymbolKind};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Instructions shown before and after a location.
const BEFORE: usize = 10;
const AFTER: usize = 3;
/// Definitions followed back from a location's operands.
const DEPTH: usize = 2;
/// Architectural registers, and the ones a call preserves.
const REGISTERS: usize = 32;
const ZERO: u8 = 0;
const STACK: u8 = 2;
const CALLEE_SAVED: [u8; 13] = [2, 8, 9, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27];
/// Smallest instruction, the step of the undecodable-bytes fallback.
const PARCEL: usize = 2;

/// A register's value where the forward pass knows it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Value {
    Constant(u32),
    /// Read from the word at `address`, shifted right by `shift`.
    Loaded {
        address: u32,
        shift: u32,
    },
}

/// One decoded instruction of a function.
#[derive(Clone, Debug)]
pub struct Line {
    pub offset: u32,
    pub text: String,
    op: SemanticOp,
    flow: InstructionFlow,
    /// Registers a conditional branch compares.
    branch: Vec<u8>,
    /// A call's resolved target.
    callee: Option<String>,
}

/// A function's code, by name, from the first ELF that defines it.
pub struct Code {
    functions: BTreeMap<String, (u32, Vec<u8>)>,
    names: BTreeMap<u32, String>,
    registers: Registers,
    diagnostic: BTreeSet<String>,
}

impl Code {
    /// The code functions of `elfs`, an earlier ELF winning a name; the
    /// chip's published `registers`; the functions reviewed decisions name
    /// as `diagnostic` output.
    pub fn of(elfs: &[&[u8]], registers: Registers, diagnostic: BTreeSet<String>) -> Self {
        let mut functions = BTreeMap::new();
        let mut names = BTreeMap::new();
        for bytes in elfs {
            let Ok(file) = object::File::parse(*bytes) else {
                continue;
            };
            for symbol in file.symbols() {
                let (Ok(name), Some(index)) = (symbol.name(), symbol.section_index()) else {
                    continue;
                };
                if symbol.kind() != SymbolKind::Text || symbol.size() == 0 {
                    continue;
                }
                let Ok(section) = file.section_by_index(index) else {
                    continue;
                };
                let Ok(data) = object::ObjectSection::data(&section) else {
                    continue;
                };
                let start = symbol.address() - object::ObjectSection::address(&section);
                let Some(code) = data.get(start as usize..(start + symbol.size()) as usize) else {
                    continue;
                };
                let Ok(address) = u32::try_from(symbol.address()) else {
                    continue;
                };
                names.entry(address).or_insert_with(|| name.to_owned());
                functions
                    .entry(name.to_owned())
                    .or_insert_with(|| (address, code.to_vec()));
            }
        }
        Self {
            functions,
            names,
            registers,
            diagnostic,
        }
    }

    /// The decoding of `function`, each line annotated with the values the
    /// forward pass resolves.
    pub fn lines(&self, function: &str) -> Option<Vec<Line>> {
        let (entry, bytes) = self.functions.get(function)?;
        let mut decoded = vec![];
        let mut offset = 0usize;
        while offset + PARCEL <= bytes.len() {
            let rest = &bytes[offset..];
            let Some(op) = RiscvDecoder.decode(rest) else {
                decoded.push((offset, PARCEL, None));
                offset += PARCEL;
                continue;
            };
            let length = usize::from(op.length);
            decoded.push((offset, length, Some(op)));
            offset += length;
        }
        // Knowledge ends where control can enter from elsewhere.
        let targets: BTreeSet<usize> = decoded
            .iter()
            .filter_map(|(at, _, op)| match op.as_ref()?.flow {
                InstructionFlow::Branch { displacement }
                | InstructionFlow::Jump {
                    displacement,
                    link: false,
                } => usize::try_from(*at as i64 + i64::from(displacement)).ok(),
                _ => None,
            })
            .collect();
        let mut values: [Option<Value>; REGISTERS] = [None; REGISTERS];
        let mut lines = vec![];
        for (at, length, op) in decoded {
            if targets.contains(&at) {
                values = [None; REGISTERS];
            }
            let address = entry + at as u32;
            let Some(op) = op else {
                let text = bytes[at..at + length]
                    .iter()
                    .rev()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>();
                values = [None; REGISTERS];
                lines.push(Line {
                    offset: at as u32,
                    text: format!(".half 0x{text}"),
                    op: SemanticOp::Unsupported,
                    flow: InstructionFlow::Stop,
                    branch: vec![],
                    callee: None,
                });
                continue;
            };
            let raw = &bytes[at..at + length];
            let lifted = RiscvDecoder.lift(raw);
            let branch = RiscvDecoder
                .branch(raw)
                .map(|(_, a, b)| {
                    [a, b]
                        .into_iter()
                        .filter_map(|o| match o {
                            Operand::Register(r) => Some(r),
                            Operand::Immediate(_) => None,
                        })
                        .collect()
                })
                .unwrap_or_default();
            let callee = match op.flow {
                InstructionFlow::Jump {
                    displacement,
                    link: true,
                } => Some(address.wrapping_add_signed(displacement)),
                InstructionFlow::Indirect {
                    base,
                    offset,
                    link: true,
                } => match values[usize::from(base)] {
                    Some(Value::Constant(value)) => Some(value.wrapping_add_signed(offset)),
                    _ => None,
                },
                _ => None,
            }
            .map(|target| {
                self.names
                    .get(&target)
                    .cloned()
                    .unwrap_or_else(|| format!("{target:#010x}"))
            });
            let note = self.step(&mut values, address, lifted, op.flow, callee.is_some());
            let mut text = op.text;
            if let Some(callee) = &callee {
                text = format!("{text}    <{callee}>");
            }
            if let Some(note) = note {
                text = format!("{text}    ; {note}");
            }
            lines.push(Line {
                offset: at as u32,
                text,
                op: lifted,
                flow: op.flow,
                branch,
                callee,
            });
        }
        Some(lines)
    }

    /// Advance `values` over one instruction at `address`; what it resolves.
    fn step(
        &self,
        values: &mut [Option<Value>; REGISTERS],
        address: u32,
        op: SemanticOp,
        flow: InstructionFlow,
        call: bool,
    ) -> Option<String> {
        let read = |values: &[Option<Value>; REGISTERS], operand: Operand| match operand {
            Operand::Immediate(value) => Some(Value::Constant(value)),
            Operand::Register(ZERO) => Some(Value::Constant(0)),
            Operand::Register(r) => values[usize::from(r)],
        };
        let mut note = None;
        let written = match op {
            SemanticOp::Upper {
                dest,
                value,
                pc_relative,
            } => {
                let value = if pc_relative {
                    address.wrapping_add(value)
                } else {
                    value
                };
                note = Some(format!("= {}", self.constant(value)));
                Some((dest, Some(Value::Constant(value))))
            }
            SemanticOp::Integer {
                op,
                dest,
                left,
                right,
            } => {
                let result = match (read(values, left), read(values, right)) {
                    (Some(Value::Constant(a)), Some(Value::Constant(b))) => {
                        let value = op.evaluate(a, b);
                        note = Some(format!("= {}", self.constant(value)));
                        Some(Value::Constant(value))
                    }
                    (Some(Value::Loaded { address, shift }), Some(Value::Constant(b))) => {
                        self.field(op, address, shift, b, &mut note)
                    }
                    _ => None,
                };
                Some((dest, result))
            }
            SemanticOp::Memory {
                kind,
                base,
                displacement,
                dest,
                ..
            } => {
                let target = match read(values, Operand::Register(base)) {
                    Some(Value::Constant(base)) if base != 0 || kind != MemoryKind::Load => {
                        Some(base.wrapping_add_signed(displacement))
                    }
                    _ => None,
                };
                if let Some(target) = target {
                    note = Some(format!("[{}]", self.registers.name(target)));
                }
                dest.map(|dest| {
                    let loaded = target.filter(|_| kind == MemoryKind::Load).map(|target| {
                        let word = target & !3;
                        Value::Loaded {
                            address: word,
                            shift: (target - word) * 8,
                        }
                    });
                    (dest, loaded)
                })
            }
            SemanticOp::Link { dest } => Some((dest, None)),
            SemanticOp::Unsupported => {
                *values = [None; REGISTERS];
                None
            }
            SemanticOp::Fence { .. } | SemanticOp::None => None,
        };
        if let Some((dest, value)) = written
            && dest != ZERO
        {
            values[usize::from(dest)] = value;
        }
        if call {
            for (register, value) in values.iter_mut().enumerate() {
                if !CALLEE_SAVED.contains(&(register as u8)) {
                    *value = None;
                }
            }
        } else if matches!(flow, InstructionFlow::Indirect { link: true, .. }) {
            *values = [None; REGISTERS];
        }
        note
    }

    /// The value of a register read from `address` after `op` with `operand`,
    /// naming the bits a mask or bit operation selects.
    fn field(
        &self,
        op: IntegerOp,
        address: u32,
        shift: u32,
        operand: u32,
        note: &mut Option<String>,
    ) -> Option<Value> {
        let bits = |mask: u32| {
            self.registers
                .bits(address, mask.checked_shl(shift).unwrap_or(0))
        };
        match op {
            IntegerOp::And | IntegerOp::Or | IntegerOp::Xor | IntegerOp::AndNot => {
                *note = Some(bits(operand));
                Some(Value::Loaded { address, shift })
            }
            IntegerOp::BitSet | IntegerOp::BitClear | IntegerOp::BitInvert => {
                *note = Some(bits(1 << (operand % u32::BITS)));
                Some(Value::Loaded { address, shift })
            }
            IntegerOp::BitExtract => {
                *note = Some(bits(1 << (operand % u32::BITS)));
                None
            }
            IntegerOp::Shr | IntegerOp::Sar => Some(Value::Loaded {
                address,
                shift: shift + operand % u32::BITS,
            }),
            _ => None,
        }
    }

    /// `value`, with the function or register it addresses.
    fn constant(&self, value: u32) -> String {
        match (self.names.get(&value), self.registers.at(value)) {
            (Some(name), _) => format!("{value:#x} <{name}>"),
            (None, Some(_)) => self.registers.name(value),
            (None, None) => format!("{value:#x}"),
        }
    }

    /// How the straight-line code from `index` ends, when it only calls
    /// reviewed diagnostic functions, stores nothing outside the stack, and
    /// reads nothing outside the stack after its last call: work after the
    /// output is behavior, not diagnostics.
    fn diagnostic_only(&self, lines: &[Line], index: usize) -> Option<Ending> {
        let mut calls = 0;
        for line in &lines[index..] {
            if let SemanticOp::Memory {
                kind, base, dest, ..
            } = line.op
            {
                let stack_store = base == STACK && dest.is_none() && kind == MemoryKind::Store;
                let late_read = kind == MemoryKind::Load && base != STACK && calls > 0;
                if (kind != MemoryKind::Load && !stack_store) || late_read {
                    return None;
                }
            }
            match line.flow {
                InstructionFlow::Jump { link: true, .. }
                | InstructionFlow::Indirect { link: true, .. } => {
                    calls += 1;
                    if !line
                        .callee
                        .as_ref()
                        .is_some_and(|name| self.diagnostic.contains(name))
                    {
                        return None;
                    }
                }
                InstructionFlow::Next => {}
                InstructionFlow::Jump {
                    displacement: 0,
                    link: false,
                } => return (calls > 0).then_some(Ending::Spins),
                InstructionFlow::Indirect { link: false, .. } | InstructionFlow::Stop => {
                    return (calls > 0).then_some(Ending::Returns);
                }
                InstructionFlow::Branch { .. } | InstructionFlow::Jump { .. } => {
                    return (calls > 0).then_some(Ending::Continues);
                }
            }
        }
        None
    }
}

/// How a diagnostic-only block ends.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Ending {
    /// Jumps to itself: an assertion that logs and stops the core.
    Spins,
    /// Returns from its function after the output.
    Returns,
    /// Continues on another path after the output.
    Continues,
}

impl Ending {
    fn label(self) -> &'static str {
        match self {
            Ending::Spins => "assertion: reviewed diagnostic output, then the core spins",
            Ending::Returns => {
                "reviewed diagnostic output, then a return: the early return is behavior"
            }
            Ending::Continues => "reviewed diagnostic output, then another path",
        }
    }
}

/// Registers `line` writes and reads.
fn operands(line: &Line) -> (Option<u8>, Vec<u8>) {
    let register = |operand: Operand| match operand {
        Operand::Register(r) => Some(r),
        Operand::Immediate(_) => None,
    };
    match line.op {
        SemanticOp::Integer {
            dest, left, right, ..
        } => (
            Some(dest),
            [left, right].into_iter().filter_map(register).collect(),
        ),
        SemanticOp::Upper { dest, .. } | SemanticOp::Link { dest } => (Some(dest), vec![]),
        SemanticOp::Memory {
            base, dest, source, ..
        } => (dest, [Some(base), source].into_iter().flatten().collect()),
        SemanticOp::Fence { .. } | SemanticOp::None | SemanticOp::Unsupported => {
            (None, line.branch.clone())
        }
    }
}

fn name(register: u8) -> String {
    const ABI: [&str; REGISTERS] = [
        "zero", "ra", "sp", "gp", "tp", "t0", "t1", "t2", "s0", "s1", "a0", "a1", "a2", "a3", "a4",
        "a5", "a6", "a7", "s2", "s3", "s4", "s5", "s6", "s7", "s8", "s9", "s10", "s11", "t3", "t4",
        "t5", "t6",
    ];
    ABI.get(usize::from(register))
        .map_or_else(|| format!("x{register}"), |name| (*name).to_owned())
}

/// The in-function definitions of the registers the instruction at `index`
/// reads, followed `DEPTH` steps back.
fn definitions(lines: &[Line], index: usize) -> Vec<String> {
    let wanted_of = |at: usize, depth: usize| {
        operands(&lines[at])
            .1
            .into_iter()
            .filter(|r| *r != ZERO && *r != STACK)
            .map(move |r| (r, at, depth))
            .collect::<Vec<_>>()
    };
    let mut found = vec![];
    let mut wanted = wanted_of(index, 0);
    let mut seen = BTreeSet::new();
    while let Some((register, from, depth)) = wanted.pop() {
        let Some(at) = (0..from)
            .rev()
            .find(|i| operands(&lines[*i]).0 == Some(register))
        else {
            found.push(format!(
                "{}: an argument or not defined in this function",
                name(register)
            ));
            continue;
        };
        if !seen.insert(at) {
            continue;
        }
        found.push(format!(
            "{} <- +{:#x}: {}",
            name(register),
            lines[at].offset,
            lines[at].text
        ));
        if depth + 1 < DEPTH {
            wanted.extend(wanted_of(at, depth + 1));
        }
    }
    found
}

/// The index of the transfer that ends the block opened at `index`: a
/// branch, jump, return or other stop; calls do not end it.
fn block_end(lines: &[Line], index: usize) -> usize {
    lines[index..]
        .iter()
        .position(|line| {
            !matches!(
                line.flow,
                InstructionFlow::Next
                    | InstructionFlow::Jump { link: true, .. }
                    | InstructionFlow::Indirect { link: true, .. }
            )
        })
        .map_or(lines.len().saturating_sub(1), |offset| index + offset)
}

/// The report of `untriaged` over `code`.
pub fn report(code: &Code, untriaged: &BTreeSet<Location>) -> String {
    let mut text = String::new();
    let mut by_function: BTreeMap<&str, Vec<&Location>> = BTreeMap::new();
    for location in untriaged {
        by_function
            .entry(&location.function)
            .or_default()
            .push(location);
    }
    for (function, locations) in by_function {
        text.push_str(&format!("\n== {function}: {} locations\n", locations.len()));
        let Some(lines) = code.lines(function) else {
            text.push_str("  (no code in the linked image or ROM)\n");
            continue;
        };
        for location in locations {
            let Some(index) = lines.iter().position(|l| l.offset >= location.offset) else {
                continue;
            };
            let ending = matches!(location.kind, LocationKind::Block)
                .then(|| code.diagnostic_only(&lines, index))
                .flatten();
            let candidate = ending.is_some();
            text.push_str(&format!(
                "\n  +{:#x} {:?}{}\n",
                location.offset,
                location.kind,
                ending.map_or_else(String::new, |e| format!("  [candidate: {}]", e.label()))
            ));
            let start = index.saturating_sub(BEFORE);
            // A candidate shows its whole block through the transfer that
            // ends it: whether it rejoins the other path or returns is the
            // reviewer's question.
            let end = if candidate {
                block_end(&lines, index) + AFTER
            } else {
                index + AFTER
            };
            for (i, line) in lines
                .iter()
                .enumerate()
                .take((end + 1).min(lines.len()))
                .skip(start)
            {
                let mark = if i == index { ">" } else { " " };
                text.push_str(&format!("   {mark} +{:04x}  {}\n", line.offset, line.text));
            }
            for definition in definitions(&lines, index) {
                text.push_str(&format!("      {definition}\n"));
            }
        }
    }
    text
}

/// Every function of `untriaged` in full, each instruction marked by the
/// locations at its offset: `U` untriaged, `E` excluded by a reviewed
/// decision (its reason follows the listing, by number), `C` reached only
/// through excluded functions; unmarked code is covered.
pub fn functions(
    code: &Code,
    untriaged: &BTreeSet<Location>,
    uncovered: &BTreeSet<Location>,
    consequential: &BTreeSet<Location>,
    reason: impl Fn(&Location) -> Option<&'static str>,
) -> String {
    let mut text = String::new();
    let names: BTreeSet<&str> = untriaged.iter().map(|l| l.function.as_str()).collect();
    for function in names {
        let Some(lines) = code.lines(function) else {
            continue;
        };
        let mut reasons: Vec<&'static str> = vec![];
        let at = |offset: u32| {
            uncovered
                .iter()
                .filter(move |l| l.function == function && l.offset == offset)
        };
        text.push_str(&format!("\n== {function}\n"));
        for line in &lines {
            let mut marks = vec![];
            for location in at(line.offset) {
                let kind = match location.kind {
                    LocationKind::Block => "block",
                    LocationKind::Taken => "taken",
                    LocationKind::Fallthrough => "fallthrough",
                    LocationKind::Followed => "followed",
                    LocationKind::Unresolved => "unresolved",
                };
                let mark = if untriaged.contains(location) {
                    format!("U {kind}")
                } else if consequential.contains(location) {
                    format!("C {kind}")
                } else if let Some(reason) = reason(location) {
                    let number = reasons
                        .iter()
                        .position(|r| *r == reason)
                        .unwrap_or_else(|| {
                            reasons.push(reason);
                            reasons.len() - 1
                        });
                    format!("E{} {kind}", number + 1)
                } else {
                    format!("? {kind}")
                };
                marks.push(mark);
            }
            text.push_str(&format!(
                "  {:<24} +{:04x}  {}\n",
                marks.join(", "),
                line.offset,
                line.text
            ));
        }
        for (number, reason) in reasons.iter().enumerate() {
            text.push_str(&format!("  E{}: {reason}\n", number + 1));
        }
    }
    text
}

/// Write the report of `untriaged` below `run`; its path.
pub fn write(
    run: &Path,
    scenario: &str,
    code: &Code,
    untriaged: &BTreeSet<Location>,
) -> std::io::Result<PathBuf> {
    let path = run.join(format!("untriaged-{scenario}.txt"));
    std::fs::write(&path, report(code, untriaged))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `lui a5, 0x20104`; `lw a4, 32(a5)`; `andi a4, a4, 1`;
    /// `beqz a4, +8`; `jal ra, +8`; `sw a0, 12(sp)`; `ret`; `ret`;
    /// `sw a0, 0(a5)`; `ret`.
    const CODE: [u32; 10] = [
        0x2010_47b7,
        0x0207_a703,
        0x0017_7713,
        0x0007_0463,
        0x0080_00ef,
        0x00a1_2623,
        0x0000_8067,
        0x0000_8067,
        0x00a7_a023,
        0x0000_8067,
    ];

    fn code(diagnostic: &[&str]) -> Code {
        let bytes = CODE.iter().flat_map(|w| w.to_le_bytes()).collect();
        let registers = Registers::parse(
            "[[registers]]\naddress = 0x20104020\nidentity = \"MAC.TX_CONFIG\"\n\
             [[registers.fields]]\nsvd-name = \"ENABLE\"\nbit-offset = 0\nbit-width = 1\n",
        )
        .unwrap();
        let mut functions = BTreeMap::new();
        functions.insert("probe".to_owned(), (0x4000_0000, bytes));
        let mut names = BTreeMap::new();
        names.insert(0x4000_0000, "probe".to_owned());
        names.insert(0x4000_0018, "wifi_log".to_owned());
        Code {
            functions,
            names,
            registers,
            diagnostic: diagnostic.iter().map(|d| (*d).to_owned()).collect(),
        }
    }

    #[test]
    fn constants_fold_into_named_registers_and_fields() {
        let lines = code(&[]).lines("probe").unwrap();
        assert!(
            lines[0].text.ends_with("; = 0x20104000"),
            "{}",
            lines[0].text
        );
        assert!(
            lines[1].text.ends_with("; [0x20104020 MAC.TX_CONFIG]"),
            "{}",
            lines[1].text
        );
        assert!(
            lines[2].text.ends_with("; MAC.TX_CONFIG: ENABLE"),
            "{}",
            lines[2].text
        );
        let found = definitions(&lines, 3);
        assert!(found[0].starts_with("a4 <- +0x8: "), "{found:?}");
        assert!(
            found.iter().any(|f| f.starts_with("a4 <- +0x4: ")),
            "{found:?}"
        );
    }

    #[test]
    fn a_block_ends_at_its_first_transfer_other_than_a_call() {
        let lines = code(&[]).lines("probe").unwrap();
        // The call at +0x10 continues the block; the return at +0x18 ends it.
        assert_eq!(block_end(&lines, 4), 6);
        assert_eq!(block_end(&lines, 0), 3);
    }

    #[test]
    fn only_reviewed_diagnostic_calls_make_a_candidate() {
        let unreviewed = code(&[]);
        let lines = unreviewed.lines("probe").unwrap();
        assert_eq!(lines[4].callee.as_deref(), Some("wifi_log"));
        // A callee no decision names as diagnostic output is never one.
        assert_eq!(unreviewed.diagnostic_only(&lines, 4), None);
        let reviewed = code(&["wifi_log"]);
        let lines = reviewed.lines("probe").unwrap();
        assert_eq!(reviewed.diagnostic_only(&lines, 4), Some(Ending::Returns));
        // A store outside the stack is never diagnostic.
        assert_eq!(reviewed.diagnostic_only(&lines, 8), None);
        // An assertion logs, then jumps to itself.
        let line = |op, flow, callee: Option<&str>| Line {
            offset: 0,
            text: String::new(),
            op,
            flow,
            branch: vec![],
            callee: callee.map(str::to_owned),
        };
        let call = || {
            line(
                SemanticOp::Link { dest: 1 },
                InstructionFlow::Jump {
                    displacement: 8,
                    link: true,
                },
                Some("wifi_log"),
            )
        };
        let spin = line(
            SemanticOp::None,
            InstructionFlow::Jump {
                displacement: 0,
                link: false,
            },
            None,
        );
        assert_eq!(
            reviewed.diagnostic_only(&[call(), spin.clone()], 0),
            Some(Ending::Spins)
        );
        // Reading device or object state after the output is behavior.
        let read = line(
            SemanticOp::Memory {
                kind: MemoryKind::Load,
                base: 10,
                displacement: 4,
                width: 4,
                dest: Some(15),
                source: None,
                swap: false,
                signed: false,
            },
            InstructionFlow::Next,
            None,
        );
        assert_eq!(
            reviewed.diagnostic_only(&[call(), read.clone(), spin.clone()], 0),
            None
        );
        assert_eq!(
            reviewed.diagnostic_only(&[read, call(), spin], 0),
            Some(Ending::Spins)
        );
    }

    #[test]
    fn the_function_view_marks_every_location_by_its_triage() {
        let at = |offset, kind| Location {
            function: "probe".into(),
            offset,
            kind,
        };
        let untriaged = BTreeSet::from([at(0xc, LocationKind::Taken)]);
        let uncovered = BTreeSet::from([
            at(0xc, LocationKind::Taken),
            at(0x10, LocationKind::Block),
            at(0x20, LocationKind::Block),
        ]);
        let consequential = BTreeSet::from([at(0x20, LocationKind::Block)]);
        let view = functions(&code(&[]), &untriaged, &uncovered, &consequential, |l| {
            (l.offset == 0x10).then_some("reviewed path")
        });
        let line = |offset: &str| {
            view.lines()
                .find(|l| l.contains(offset))
                .unwrap()
                .to_owned()
        };
        assert!(line("+000c").contains("U taken"), "{view}");
        assert!(line("+0010").contains("E1 block"), "{view}");
        assert!(line("+0020").contains("C block"), "{view}");
        assert!(!line("+0000").contains("U "), "{view}");
        assert!(view.contains("E1: reviewed path"), "{view}");
    }
}
