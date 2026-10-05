//! Inventory of the vendor's radio MMIO accesses against the register model.
//!
//! Blobray's `register-accesses` analyzes every function of every pinned
//! vendor binary in one process and reports each statically resolved access
//! inside the publication's owned MMIO ranges, with the bits a masked read
//! selects or a read-modify-write replaces. Each touched 32-bit word is
//! compared with the register model: words no register declares, bits the
//! vendor selects or replaces outside every declared field, and declared
//! opaque bits the vendor touches. Words touched by a function the provenance
//! registry names, one production, the register model or a verification
//! decision cites, rank first.
//!
//! This module is the comparison: the publication's owned regions and
//! declared words ([`model`]), Blobray's access records ([`observations`]),
//! [`classify`], [`rank`] and the report; [`run()`] runs Blobray over the
//! pinned binaries (`cargo xtask register-inventory`) and writes the report under
//! `target/register-inventory/<chip>/`; nothing is tracked. Addresses
//! computed at run time (queue strides, pointer tables) are not constants
//! and stay outside the inventory.
use crate::Result;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

mod run;
#[cfg(test)]
mod tests;

pub use run::run;

/// Bits of the word the inventory compares; register-model registers are
/// split into words of this size.
const WORD_BITS: u32 = 32;
/// Bytes of one compared word.
const WORD_BYTES: u32 = WORD_BITS / 8;
/// Kind of a Blobray fact composed from a callee: the callee's own analysis
/// records the same access, so it is attributed there.
const CALLEE_EFFECT: &str = "callee-effect";

/// A register as the model declares it, independent of its SVD form.
#[derive(Clone, Debug)]
pub struct DeclaredRegister {
    pub address: u64,
    pub width_bits: u32,
    pub name: String,
    pub fields: Vec<DeclaredField>,
}

#[derive(Clone, Debug)]
pub struct DeclaredField {
    pub name: String,
    pub offset: u32,
    pub width: u32,
}

/// What the model declares about one word.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeclaredWord {
    /// Names of the registers overlapping the word.
    pub registers: BTreeSet<String>,
    /// Bits of declared fields.
    pub fields: u32,
    /// Declared bits whose name, or whose register's name, is opaque.
    pub opaque: u32,
}

/// Word-granular view of the declared registers.
pub fn declared_words(registers: &[DeclaredRegister]) -> BTreeMap<u32, DeclaredWord> {
    let mut words: BTreeMap<u32, DeclaredWord> = BTreeMap::new();
    let opaque = |name: &str| name.ends_with(oer_register_model::OPAQUE_SUFFIX);
    for register in registers {
        let Ok(address) = u32::try_from(register.address) else {
            continue;
        };
        let lane = (address % WORD_BYTES) * 8;
        let base = address - address % WORD_BYTES;
        let span = lane + register.width_bits;
        for word in 0..span.div_ceil(WORD_BITS) {
            words
                .entry(base + word * WORD_BYTES)
                .or_default()
                .registers
                .insert(register.name.clone());
        }
        for field in &register.fields {
            for bit in lane + field.offset..lane + field.offset + field.width {
                let word = words
                    .entry(base + bit / WORD_BITS * WORD_BYTES)
                    .or_default();
                word.fields |= 1 << (bit % WORD_BITS);
                if opaque(&register.name) || opaque(&field.name) {
                    word.opaque |= 1 << (bit % WORD_BITS);
                }
            }
        }
    }
    words
}

/// A vendor function: its artifact id and symbol.
pub type Function = (String, String);

/// Kind of one resolved access.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Load,
    Store,
    /// A masked expression over an earlier load.
    Expression,
}

/// One statically resolved access to a constant address.
#[derive(Clone, Debug)]
pub struct Observation {
    pub function: Function,
    pub address: u32,
    pub access: Access,
    /// Bits a masked read selects or a read-modify-write replaces, in the
    /// accessed value.
    pub bits: Option<u32>,
}

/// Status of one touched word.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    /// No register of the model overlaps the word.
    Unmodeled,
    /// The vendor selects or replaces bits outside every declared field.
    UndeclaredBits,
    /// The vendor touches a word with opaque declared bits.
    Opaque,
    /// Every touched bit lies in a named field.
    Declared,
}

#[derive(Clone, Debug, Serialize)]
pub struct WordReport {
    pub address: u32,
    pub region: String,
    pub status: Status,
    /// Some touching function is cited by production or the model.
    pub cited: bool,
    pub registers: Vec<String>,
    pub loads: usize,
    pub stores: usize,
    /// Bits masked reads select and read-modify-writes replace.
    pub field_bits: u32,
    /// Whether an access selects or replaces the whole word.
    pub whole_word: bool,
    pub declared_bits: u32,
    pub undeclared_bits: u32,
    pub opaque_bits: u32,
    pub functions: Vec<String>,
}

/// Compare the accesses inside `regions` with the declared words.
pub fn classify(
    regions: &[Region],
    declared: &BTreeMap<u32, DeclaredWord>,
    observations: &[Observation],
    cited: &BTreeSet<Function>,
) -> Vec<WordReport> {
    #[derive(Default)]
    struct Touch {
        functions: BTreeSet<Function>,
        loads: usize,
        stores: usize,
        bits: u32,
        whole: bool,
    }
    let mut touched: BTreeMap<u32, Touch> = BTreeMap::new();
    for observation in observations {
        if region_of(regions, observation.address).is_none() {
            continue;
        }
        let word = observation.address - observation.address % WORD_BYTES;
        let touch = touched.entry(word).or_default();
        touch.functions.insert(observation.function.clone());
        match observation.access {
            Access::Load => touch.loads += 1,
            Access::Store => touch.stores += 1,
            Access::Expression => {}
        }
        match observation.bits {
            Some(u32::MAX) => touch.whole = true,
            Some(bits) => {
                let lane = (observation.address % WORD_BYTES) * 8;
                touch.bits |= bits.checked_shl(lane).unwrap_or(0);
            }
            None => {}
        }
    }
    let empty = DeclaredWord::default();
    touched
        .into_iter()
        .map(|(address, touch)| {
            let word = declared.get(&address);
            let declared_word = word.unwrap_or(&empty);
            let undeclared_bits = touch.bits & !declared_word.fields;
            let opaque_bits = declared_word.opaque;
            let status = if word.is_none() {
                Status::Unmodeled
            } else if undeclared_bits != 0 {
                Status::UndeclaredBits
            } else if opaque_bits != 0 {
                Status::Opaque
            } else {
                Status::Declared
            };
            WordReport {
                address,
                region: region_of(regions, address).unwrap_or_default().to_owned(),
                status,
                cited: touch.functions.iter().any(|f| cited.contains(f)),
                registers: declared_word.registers.iter().cloned().collect(),
                loads: touch.loads,
                stores: touch.stores,
                field_bits: touch.bits,
                whole_word: touch.whole,
                declared_bits: declared_word.fields,
                undeclared_bits,
                opaque_bits,
                functions: touch
                    .functions
                    .iter()
                    .map(|(artifact, symbol)| format!("{artifact}::{symbol}"))
                    .collect(),
            }
        })
        .collect()
}

/// One owned MMIO range of the publication.
#[derive(Clone, Debug)]
pub struct Region {
    pub name: String,
    pub start: u32,
    pub end: u32,
}

fn region_of(regions: &[Region], address: u32) -> Option<&str> {
    regions
        .iter()
        .find(|r| r.start <= address && address < r.end)
        .map(|r| r.name.as_str())
}

/// The words to review first: cited before uncited, then by status, then by
/// how many functions touch them.
pub fn rank(words: &mut [WordReport]) {
    words.sort_by(|a, b| {
        (
            !a.cited,
            a.status,
            std::cmp::Reverse(a.functions.len()),
            a.address,
        )
            .cmp(&(
                !b.cited,
                b.status,
                std::cmp::Reverse(b.functions.len()),
                b.address,
            ))
    });
}

/// The chip's register model and owned MMIO ranges, through its
/// publication manifest `registers/<chip>/publication/registers.toml`.
pub fn model(root: &Path, chip: &str) -> Result<(Vec<DeclaredRegister>, Vec<Region>)> {
    let manifest = root.join(format!("registers/{chip}/publication/registers.toml"));
    let publication = crate::ChipSources::load(&manifest)?;
    let regions = publication
        .owned_mmio()?
        .into_iter()
        .map(|(name, start, end)| -> Result<Region> {
            Ok(Region {
                name,
                start: u32::try_from(start).map_err(|_| "owned range above 4 GiB")?,
                end: u32::try_from(end).map_err(|_| "owned range above 4 GiB")?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let model = oer_register_model::RegisterModel::load(&publication.model)?;
    let registers = model
        .register_geometry()?
        .into_iter()
        .map(|r| DeclaredRegister {
            address: r.address,
            width_bits: r.width.unwrap_or(WORD_BITS),
            name: r.name,
            fields: r
                .fields
                .into_iter()
                .map(|f| DeclaredField {
                    name: f.name,
                    offset: f.offset,
                    width: f.width,
                })
                .collect(),
        })
        .collect();
    Ok((registers, regions))
}

#[derive(Deserialize)]
struct Document {
    records: Vec<AccessRecord>,
}

#[derive(Deserialize)]
struct AccessRecord {
    kind: String,
    function: Option<LibraryFunction>,
    fact: Option<Fact>,
    address: Option<u32>,
    mask: Option<Mask>,
}

#[derive(Deserialize)]
struct LibraryFunction {
    input: usize,
    name: Option<String>,
}

#[derive(Deserialize)]
struct Fact {
    kind: String,
    access: Option<String>,
}

#[derive(Deserialize)]
struct Mask {
    bits: u32,
}

/// The resolved accesses of named functions in Blobray's
/// `register-accesses` document `accesses`; a function names its input by
/// position in `inputs`, the artifact ids passed to Blobray in order.
pub fn observations(accesses: &str, inputs: &[String]) -> Result<Vec<Observation>> {
    let document: Document = serde_json::from_str(accesses)?;
    let mut observations = vec![];
    for access in document.records {
        if access.kind != "observation" {
            continue;
        }
        let (Some(function), Some(fact), Some(address)) =
            (access.function, access.fact, access.address)
        else {
            continue;
        };
        if fact.kind == CALLEE_EFFECT {
            continue;
        }
        let name = function.name.ok_or("observation of an unnamed function")?;
        let input = inputs
            .get(function.input)
            .ok_or("function of an unknown input")?;
        observations.push(Observation {
            function: (input.clone(), name),
            address,
            access: match fact.access.as_deref() {
                Some("load") => Access::Load,
                Some("store") => Access::Store,
                _ => Access::Expression,
            },
            bits: access.mask.map(|m| m.bits),
        });
    }
    Ok(observations)
}

/// The JSON report: the chip, each input with its SHA-256, and every word.
#[derive(Serialize)]
pub struct Report<'a> {
    pub chip: &'a str,
    pub inputs: Vec<(&'a str, &'a str)>,
    pub words: &'a [WordReport],
}

/// The text report: counts by status, then every word not fully declared.
pub fn render(chip: &str, words: &[WordReport]) -> String {
    let count = |status: Status, cited: bool| {
        words
            .iter()
            .filter(|w| w.status == status && (!cited || w.cited))
            .count()
    };
    let mut text = format!(
        "Register inventory of {chip}: {} vendor-touched words in owned MMIO ranges\n",
        words.len()
    );
    for (status, label) in [
        (Status::Unmodeled, "not declared by any register"),
        (
            Status::UndeclaredBits,
            "touched bits outside declared fields",
        ),
        (Status::Opaque, "opaque declared bits"),
        (Status::Declared, "fully declared"),
    ] {
        text.push_str(&format!(
            "  {:5} {label} ({} touched by a cited function)\n",
            count(status, false),
            count(status, true)
        ));
    }
    for word in words.iter().filter(|w| w.status != Status::Declared) {
        text.push_str(&format!(
            "\n0x{:08x} {:?}{} {}\n  registers: {}\n  loads {} stores {} field bits 0x{:08x}{} declared 0x{:08x} undeclared 0x{:08x} opaque 0x{:08x}\n  functions ({}): {}\n",
            word.address,
            word.status,
            if word.cited { " cited" } else { "" },
            word.region,
            if word.registers.is_empty() { "-".into() } else { word.registers.join(", ") },
            word.loads,
            word.stores,
            word.field_bits,
            if word.whole_word { " (and whole-word)" } else { "" },
            word.declared_bits,
            word.undeclared_bits,
            word.opaque_bits,
            word.functions.len(),
            word.functions.join(" "),
        ));
    }
    text
}
