//! Standalone inspection of a chip's pinned vendor code.
//!
//! Every archive member and ELF image of the chip's artifact manifest is
//! decoded with Blobray's RISC-V decoder. A forward pass over each function
//! folds constants and follows the bits of a loaded word through masks, so a
//! store reports which bits it clears, sets to a constant or takes from a
//! value the pass does not know, such as an argument. Two queries use it:
//! [`xref`] lists every read and write to an address range, and [`show`]
//! prints one function, from every artifact that defines it, with its folded
//! constants, relocation targets and string literals.
//!
//! The pass is intraprocedural and forgets what it knows where control can
//! enter from elsewhere, so an access through a pointer computed in another
//! function, or after a join, is not attributed.
use crate::harness::{Result, invalid};
use crate::registers::Registers;
use crate::triage::{CALLEE_SAVED, PARCEL, REGISTERS, ZERO};
use blobray_backend_riscv::RiscvDecoder;
use blobray_domain::{
    FunctionDecoder, FunctionSemantics, InstructionFlow, IntegerOp, MemoryKind, Operand, SemanticOp,
};
use object::{
    Object, ObjectSection, ObjectSymbol, RelocationFlags, RelocationTarget, SectionIndex,
    SymbolKind,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

/// Leading bytes of a Unix `ar` archive.
const ARCHIVE_MAGIC: &[u8] = b"!<arch>\n";
/// Artifact kinds the manifest pins as code: static archives and ELF images.
const CODE_EXTENSIONS: [&str; 2] = [".a", ".elf"];
/// Longest string literal a reference shows.
const STRING_LIMIT: usize = 160;
/// Width of a full word access, in bytes.
const WORD_BYTES: u8 = 4;
/// All bits of a word.
const ALL: u32 = u32::MAX;

/// One vendor function of a pinned artifact.
pub struct Function {
    /// `artifact` for an ELF image, `artifact[member]` for an archive member.
    pub origin: String,
    pub name: String,
    /// Address of the first byte: linked for an ELF image, relative to its
    /// section for an archive member.
    pub entry: u32,
    pub code: Vec<u8>,
    /// Relocations of the function's code, by offset from its entry.
    references: BTreeMap<u32, Reference>,
}

/// What a relocation at one instruction refers to.
#[derive(Clone, Debug)]
struct Reference {
    /// ELF relocation type.
    kind: u32,
    target: String,
    addend: i64,
    /// The NUL-terminated string the target addresses, when it is one.
    literal: Option<String>,
}

impl Reference {
    fn is_call(&self) -> bool {
        matches!(
            self.kind,
            object::elf::R_RISCV_CALL | object::elf::R_RISCV_CALL_PLT | object::elf::R_RISCV_JAL
        )
    }

    fn describe(&self) -> String {
        let mut text = self.target.clone();
        if self.addend != 0 {
            let _ = write!(text, "{:+#x}", self.addend);
        }
        if let Some(literal) = &self.literal {
            let _ = write!(text, " {literal:?}");
        }
        text
    }
}

/// The code functions of every pinned artifact of the installed chip.
pub struct Corpus {
    pub functions: Vec<Function>,
    /// Function names by origin and address, for calls resolved by address.
    names: BTreeMap<(String, u32), String>,
    registers: Registers,
    /// Code artifacts the manifest pins but this checkout does not hold.
    pub missing: Vec<String>,
}

impl Corpus {
    /// Load every code artifact of the chip's manifest below the repository
    /// `root`, failing on one whose bytes differ from its pinned digest.
    pub fn load(root: &Path) -> Result<Self> {
        let mut corpus = Self {
            functions: vec![],
            names: BTreeMap::new(),
            registers: Registers::load(&root.join(crate::chip().registers))?,
            missing: vec![],
        };
        for artifact in &crate::artifacts::manifest().artifact {
            if !CODE_EXTENSIONS
                .iter()
                .any(|extension| artifact.path.ends_with(extension))
            {
                continue;
            }
            let path = crate::artifacts::path(root, &artifact.id);
            let Ok(bytes) = std::fs::read(&path) else {
                corpus.missing.push(artifact.id.clone());
                continue;
            };
            let digest = format!("{:x}", Sha256::digest(&bytes));
            if digest != artifact.sha256 {
                return Err(invalid(format!(
                    "{} at {} has sha256 {digest}, not the pinned {}",
                    artifact.id,
                    path.display(),
                    artifact.sha256
                )));
            }
            corpus.add_artifact(&artifact.id, &bytes)?;
        }
        Ok(corpus)
    }

    /// A corpus of the given objects, for tests: (origin, ELF or archive).
    pub fn of(objects: &[(&str, &[u8])], registers: Registers) -> Result<Self> {
        let mut corpus = Self {
            functions: vec![],
            names: BTreeMap::new(),
            registers,
            missing: vec![],
        };
        for (origin, bytes) in objects {
            corpus.add_artifact(origin, bytes)?;
        }
        Ok(corpus)
    }

    fn add_artifact(&mut self, id: &str, bytes: &[u8]) -> Result<()> {
        if !bytes.starts_with(ARCHIVE_MAGIC) {
            self.add_object(id, bytes);
            return Ok(());
        }
        let archive = object::read::archive::ArchiveFile::parse(bytes)?;
        for member in archive.members() {
            let member = member?;
            let name = String::from_utf8_lossy(member.name()).into_owned();
            self.add_object(&format!("{id}[{name}]"), member.data(bytes)?);
        }
        Ok(())
    }

    /// Add the text symbols of one ELF object with their relocations; bytes
    /// that are not an ELF object, such as an archive symbol table, add
    /// nothing.
    fn add_object(&mut self, origin: &str, bytes: &[u8]) {
        let Ok(file) = object::File::parse(bytes) else {
            return;
        };
        let mut references: BTreeMap<(usize, u64), Reference> = BTreeMap::new();
        for section in file.sections() {
            for (offset, relocation) in section.relocations() {
                let RelocationFlags::Elf { r_type } = relocation.flags() else {
                    continue;
                };
                if r_type == object::elf::R_RISCV_RELAX {
                    continue;
                }
                let RelocationTarget::Symbol(index) = relocation.target() else {
                    continue;
                };
                let Ok(symbol) = file.symbol_by_index(index) else {
                    continue;
                };
                let target_section = symbol.section_index();
                let name = match symbol.kind() {
                    SymbolKind::Section => target_section
                        .and_then(|s| file.section_by_index(s).ok())
                        .and_then(|s| s.name().ok().map(str::to_owned))
                        .unwrap_or_default(),
                    _ => symbol.name().unwrap_or_default().to_owned(),
                };
                let literal = target_section.and_then(|s| {
                    literal(
                        &file,
                        s,
                        symbol.address().wrapping_add_signed(relocation.addend()),
                    )
                });
                references.insert(
                    (section.index().0, offset),
                    Reference {
                        kind: r_type,
                        target: name,
                        addend: relocation.addend(),
                        literal,
                    },
                );
            }
        }
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
            let Ok(data) = section.data() else {
                continue;
            };
            let start = symbol.address() - section.address();
            let Some(code) = data.get(start as usize..(start + symbol.size()) as usize) else {
                continue;
            };
            let Ok(entry) = u32::try_from(symbol.address()) else {
                continue;
            };
            let references = references
                .range((index.0, start)..(index.0, start + symbol.size()))
                .map(|((_, offset), reference)| ((offset - start) as u32, reference.clone()))
                .collect();
            self.names
                .entry((origin.to_owned(), entry))
                .or_insert_with(|| name.to_owned());
            self.functions.push(Function {
                origin: origin.to_owned(),
                name: name.to_owned(),
                entry,
                code: code.to_vec(),
                references,
            });
        }
    }

    /// The decoded, annotated instructions of `function`.
    fn walk(&self, function: &Function) -> Vec<Step> {
        let mut decoded = vec![];
        let mut offset = 0usize;
        while offset + PARCEL <= function.code.len() {
            match RiscvDecoder.decode(&function.code[offset..]) {
                Some(op) => {
                    let length = usize::from(op.length);
                    decoded.push((offset, length, Some(op)));
                    offset += length;
                }
                None => {
                    decoded.push((offset, PARCEL, None));
                    offset += PARCEL;
                }
            }
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
        let mut state = State::new();
        let mut steps = vec![];
        for (at, length, op) in decoded {
            if targets.contains(&at) {
                state = State::new();
            }
            let Some(op) = op else {
                state = State::new();
                steps.push(Step {
                    offset: at as u32,
                    text: ".half".into(),
                    notes: vec![],
                    access: None,
                });
                continue;
            };
            let raw = &function.code[at..at + length];
            let reference = function.references.get(&(at as u32));
            let address = function.entry.wrapping_add(at as u32);
            let mut notes = vec![];
            if let Some(reference) = reference {
                notes.push(reference.describe());
            }
            let call = matches!(
                op.flow,
                InstructionFlow::Jump { link: true, .. }
                    | InstructionFlow::Indirect { link: true, .. }
            );
            // A linked image names its callees by address; an archive member
            // names them through the call's relocation, noted above.
            if call && reference.is_none_or(|r| !r.is_call()) {
                let target = match op.flow {
                    InstructionFlow::Jump { displacement, .. } => {
                        Some(address.wrapping_add_signed(displacement))
                    }
                    InstructionFlow::Indirect { base, offset, .. } => match state.get(base) {
                        Value::Constant(value) => Some(value.wrapping_add_signed(offset)),
                        _ => None,
                    },
                    _ => None,
                };
                if let Some(name) =
                    target.and_then(|t| self.names.get(&(function.origin.clone(), t)))
                {
                    notes.push(format!("<{name}>"));
                }
            }
            let access = state.step(
                RiscvDecoder.lift(raw),
                address,
                reference.is_some(),
                &mut notes,
                &self.registers,
            );
            if call {
                state.call();
            } else if matches!(op.flow, InstructionFlow::Indirect { link: true, .. }) {
                state = State::new();
            }
            steps.push(Step {
                offset: at as u32,
                text: op.text,
                notes,
                access,
            });
        }
        steps
    }
}

/// The NUL-terminated printable string at `address` of `section`, if any.
fn literal(file: &object::File<'_>, section: SectionIndex, address: u64) -> Option<String> {
    let section = file.section_by_index(section).ok()?;
    let data = section.data().ok()?;
    let start = usize::try_from(address.checked_sub(section.address())?).ok()?;
    let bytes = data.get(start..)?;
    let end = bytes.iter().position(|b| *b == 0)?;
    let text = std::str::from_utf8(&bytes[..end]).ok()?;
    if text.is_empty()
        || !text
            .chars()
            .all(|c| !c.is_control() || c == '\n' || c == '\t')
    {
        return None;
    }
    Some(text.chars().take(STRING_LIMIT).collect())
}

/// One decoded instruction and what the pass learned from it.
struct Step {
    offset: u32,
    text: String,
    notes: Vec<String>,
    access: Option<Access>,
}

/// A load or store to an address the pass resolved.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Access {
    pub address: u32,
    pub width: u8,
    pub effect: Effect,
}

/// What an access does to its location.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Effect {
    Read,
    /// A store of the word read from the same address with `clear` bits
    /// forced to zero, `set` bits forced to one and `value` bits taken from a
    /// value the pass does not know; every other bit keeps its old value.
    Modify {
        clear: u32,
        set: u32,
        value: u32,
    },
    /// A store of a constant.
    Constant(u32),
    /// A store of a value the pass does not know, nonzero only in `bits`.
    Unknown {
        bits: u32,
    },
}

/// A register's value where the pass knows something about it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Value {
    Constant(u32),
    /// The word read from `address` with only its `keep` bits, `set` bits
    /// forced to one and `value` bits from a value the pass does not know.
    Word {
        address: u32,
        keep: u32,
        set: u32,
        value: u32,
    },
    /// A value the pass does not know, nonzero only in `bits`.
    Unknown {
        bits: u32,
    },
}

const UNKNOWN: Value = Value::Unknown { bits: ALL };

struct State([Value; REGISTERS]);

impl State {
    fn new() -> Self {
        Self([UNKNOWN; REGISTERS])
    }

    fn get(&self, register: u8) -> Value {
        if register == ZERO {
            Value::Constant(0)
        } else {
            self.0[usize::from(register)]
        }
    }

    fn operand(&self, operand: Operand) -> Value {
        match operand {
            Operand::Immediate(value) => Value::Constant(value),
            Operand::Register(register) => self.get(register),
        }
    }

    fn set(&mut self, register: u8, value: Value) {
        if register != ZERO {
            self.0[usize::from(register)] = value;
        }
    }

    /// A call leaves only the callee-saved registers known.
    fn call(&mut self) {
        for (register, value) in self.0.iter_mut().enumerate() {
            if !CALLEE_SAVED.contains(&(register as u8)) {
                *value = UNKNOWN;
            }
        }
    }

    /// Advance over one lifted instruction at `address`; the access it makes.
    /// A relocated instruction's immediate is a link-time value the pass
    /// does not know.
    fn step(
        &mut self,
        op: SemanticOp,
        address: u32,
        relocated: bool,
        notes: &mut Vec<String>,
        registers: &Registers,
    ) -> Option<Access> {
        match op {
            SemanticOp::Upper {
                dest,
                value,
                pc_relative,
            } => {
                let result = if relocated {
                    UNKNOWN
                } else if pc_relative {
                    Value::Constant(address.wrapping_add(value))
                } else {
                    Value::Constant(value)
                };
                if let Value::Constant(value) = result {
                    notes.push(format!("= {value:#x}"));
                }
                self.set(dest, result);
                None
            }
            SemanticOp::Integer {
                op,
                dest,
                left,
                right,
            } => {
                let result = if relocated {
                    UNKNOWN
                } else {
                    combine(op, self.operand(left), self.operand(right))
                };
                if let Value::Constant(value) = result
                    && matches!(right, Operand::Immediate(_))
                {
                    notes.push(format!("= {value:#x}"));
                }
                self.set(dest, result);
                None
            }
            SemanticOp::Memory {
                kind,
                base,
                displacement,
                width,
                dest,
                source,
                ..
            } => {
                let target = match self.get(base) {
                    Value::Constant(base) if !relocated => {
                        Some(base.wrapping_add_signed(displacement))
                    }
                    _ => None,
                };
                if let Some(target) = target {
                    notes.push(format!("[{}]", registers.name(target)));
                }
                let access = target.map(|target| {
                    let effect = match kind {
                        MemoryKind::Load => Effect::Read,
                        _ => match source.map(|s| self.get(s)) {
                            Some(Value::Word {
                                address,
                                keep,
                                set,
                                value,
                            }) if address == target && width == WORD_BYTES => Effect::Modify {
                                clear: !(keep | set | value),
                                set,
                                value,
                            },
                            Some(Value::Constant(value)) => Effect::Constant(value),
                            Some(Value::Unknown { bits }) => Effect::Unknown { bits },
                            _ => Effect::Unknown { bits: ALL },
                        },
                    };
                    Access {
                        address: target,
                        width,
                        effect,
                    }
                });
                if let Some(dest) = dest {
                    let loaded = match target {
                        Some(address)
                            if kind == MemoryKind::Load
                                && width == WORD_BYTES
                                && address % u32::from(WORD_BYTES) == 0 =>
                        {
                            Value::Word {
                                address,
                                keep: ALL,
                                set: 0,
                                value: 0,
                            }
                        }
                        _ => UNKNOWN,
                    };
                    self.set(dest, loaded);
                }
                access
            }
            SemanticOp::Link { dest } => {
                self.set(dest, UNKNOWN);
                None
            }
            SemanticOp::Unsupported => {
                *self = Self::new();
                None
            }
            SemanticOp::Fence { .. } | SemanticOp::None => None,
        }
    }
}

/// `op` over two values, keeping what the pass can say about the result.
fn combine(op: IntegerOp, left: Value, right: Value) -> Value {
    use IntegerOp::*;
    use Value::{Constant, Unknown, Word};
    match (op, left, right) {
        (_, Constant(a), Constant(b)) => Constant(op.evaluate(a, b)),
        (Add | Or | Xor | Sub | Shl | Shr | Sar, value, Constant(0))
        | (Add | Or | Xor, Constant(0), value) => value,
        (And, Word { .. }, Constant(mask)) | (And, Constant(mask), Word { .. }) => {
            let word = if matches!(left, Word { .. }) {
                left
            } else {
                right
            };
            modify(word, |keep, set, value| {
                (keep & mask, set & mask, value & mask)
            })
        }
        (AndNot, Word { .. }, Constant(mask)) => modify(left, |keep, set, value| {
            (keep & !mask, set & !mask, value & !mask)
        }),
        (Or, Word { .. }, Constant(bits)) | (Or, Constant(bits), Word { .. }) => {
            let word = if matches!(left, Word { .. }) {
                left
            } else {
                right
            };
            modify(word, |keep, set, value| {
                (keep & !bits, set | bits, value & !bits)
            })
        }
        (BitSet, Word { .. }, Constant(bit)) => {
            let bits = 1u32 << (bit % u32::BITS);
            modify(left, |keep, set, value| {
                (keep & !bits, set | bits, value & !bits)
            })
        }
        (BitClear, Word { .. }, Constant(bit)) => {
            let bits = 1u32 << (bit % u32::BITS);
            modify(left, |keep, set, value| {
                (keep & !bits, set & !bits, value & !bits)
            })
        }
        (Or, Word { .. }, Unknown { bits }) | (Or, Unknown { bits }, Word { .. }) => {
            let word = if matches!(left, Word { .. }) {
                left
            } else {
                right
            };
            modify(word, |keep, set, value| {
                (keep & !bits, set & !bits, value | bits)
            })
        }
        (And, Unknown { bits }, Constant(mask)) | (And, Constant(mask), Unknown { bits }) => {
            Unknown { bits: bits & mask }
        }
        (Or, Unknown { bits }, Constant(more)) | (Or, Constant(more), Unknown { bits }) => {
            Unknown { bits: bits | more }
        }
        (Or, Unknown { bits: a }, Unknown { bits: b }) => Unknown { bits: a | b },
        (And, Unknown { bits: a }, Unknown { bits: b }) => Unknown { bits: a & b },
        (Shl, Unknown { bits }, Constant(shift)) => Unknown {
            bits: bits.checked_shl(shift).unwrap_or(0),
        },
        (Shr, Unknown { bits }, Constant(shift)) => Unknown {
            bits: bits.checked_shr(shift).unwrap_or(0),
        },
        (Shl | Shr | Sar | And, Word { .. }, _) => {
            // An extracted field is a value like any other from here on.
            match (op, right) {
                (Shl, Constant(shift)) => Unknown {
                    bits: ALL.checked_shl(shift).unwrap_or(0),
                },
                (Shr, Constant(shift)) => Unknown {
                    bits: ALL.checked_shr(shift).unwrap_or(0),
                },
                _ => UNKNOWN,
            }
        }
        _ => UNKNOWN,
    }
}

fn modify(word: Value, change: impl FnOnce(u32, u32, u32) -> (u32, u32, u32)) -> Value {
    match word {
        Value::Word {
            address,
            keep,
            set,
            value,
        } => {
            let (keep, set, value) = change(keep, set, value);
            Value::Word {
                address,
                keep,
                set,
                value,
            }
        }
        other => other,
    }
}

/// The address range an `xref` of one address covers: its word.
pub const WORD: u32 = WORD_BYTES as u32;

/// `text` as an address: hexadecimal with a `0x` prefix, or decimal.
pub fn parse_address(text: &str) -> std::result::Result<u32, String> {
    let parsed = match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u32::from_str_radix(&hex.replace('_', ""), 16),
        None => text.replace('_', "").parse(),
    };
    parsed.map_err(|error| format!("{text}: {error}"))
}

/// The installed chip's corpus from this checkout. A pinned code artifact
/// the checkout does not hold is reported and skipped.
pub fn load() -> Result<Corpus> {
    let corpus = Corpus::load(&crate::observation::root()?)?;
    for id in &corpus.missing {
        eprintln!("{id}: not in this checkout, skipped");
    }
    Ok(corpus)
}

/// Every resolved access of the corpus to `start..end`, one line each:
/// address and register, R or W, origin, function and offset, and for a
/// store the bits it changes named by the published fields.
pub fn xref(corpus: &Corpus, start: u32, end: u32) -> String {
    let mut lines = BTreeSet::new();
    for function in &corpus.functions {
        for step in corpus.walk(function) {
            let Some(access) = step.access else {
                continue;
            };
            if !(start..end).contains(&access.address) {
                continue;
            }
            let word = access.address & !(u32::from(WORD_BYTES) - 1);
            let bits = |mask: u32| corpus.registers.bits(word, mask);
            let (mode, detail) = match access.effect {
                Effect::Read => ("R", String::new()),
                Effect::Modify { clear, set, value } => {
                    let mut parts = vec![];
                    if clear != 0 {
                        parts.push(format!("clears {}", bits(clear)));
                    }
                    if set != 0 {
                        parts.push(format!("sets {}", bits(set)));
                    }
                    if value != 0 {
                        parts.push(format!("from a value {}", bits(value)));
                    }
                    if parts.is_empty() {
                        parts.push("rewrites the word read".into());
                    }
                    ("W", parts.join("; "))
                }
                Effect::Constant(value) => ("W", format!("= {value:#x}")),
                Effect::Unknown { bits: mask } => ("W", format!("a value {}", bits(mask))),
            };
            let width = if access.width == WORD_BYTES {
                String::new()
            } else {
                format!(" {}-byte", access.width)
            };
            lines.insert((
                access.address,
                function.origin.clone(),
                function.name.clone(),
                step.offset,
                format!(
                    "{} {mode}{width} {}::{}+{:#x}  {detail}",
                    corpus.registers.name(access.address),
                    function.origin,
                    function.name,
                    step.offset
                ),
            ));
        }
    }
    let mut text = String::new();
    for (.., line) in lines {
        let _ = writeln!(text, "{}", line.trim_end());
    }
    text
}

/// Every definition of `name` in the corpus, annotated.
pub fn show(corpus: &Corpus, name: &str) -> Option<String> {
    let mut text = String::new();
    for function in corpus.functions.iter().filter(|f| f.name == name) {
        let _ = writeln!(
            text,
            "== {}::{} at {:#x}, {:#x} bytes",
            function.origin,
            function.name,
            function.entry,
            function.code.len()
        );
        for step in corpus.walk(function) {
            let _ = write!(text, "  +{:04x}  {}", step.offset, step.text);
            if !step.notes.is_empty() {
                let _ = write!(text, "    ; {}", step.notes.join(", "));
            }
            let _ = writeln!(text);
        }
    }
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests;
