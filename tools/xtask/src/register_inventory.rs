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
//! Every run analyzes the inputs again, in about half a minute. The accesses
//! and the report are generated outputs under
//! `target/register-inventory/<chip>/`; nothing is tracked. Addresses computed at run time
//! (queue strides, pointer tables) are not constants and stay outside the
//! inventory.
use crate::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(test)]
mod tests;

/// Bits of the word the inventory compares; register-model registers are
/// split into words of this size.
const WORD_BITS: u32 = 32;
/// Bytes of one compared word.
const WORD_BYTES: u32 = WORD_BITS / 8;
/// Wall-clock budget of one Blobray operation. Analyzing every pinned
/// binary takes most of it; the host's default budget is shorter.
const OPERATION_TIMEOUT_SECS: u64 = 3600;
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

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
struct PublicationManifest {
    model: PathBuf,
    memory: PathBuf,
    ownership: PathBuf,
}

#[derive(Deserialize)]
struct Memory {
    regions: Vec<MemoryRegion>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
struct MemoryRegion {
    name: String,
    kind: String,
    start: u32,
    end_exclusive: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
struct Ownership {
    owned_ranges: Vec<String>,
}

/// The chip's register model and owned MMIO ranges, through its
/// publication manifest.
fn model(ctx: &Context, chip: &str) -> Result<(Vec<DeclaredRegister>, Vec<Region>)> {
    let manifest_path = ctx
        .root
        .join(format!("registers/{chip}/publication/registers.toml"));
    let base = manifest_path.parent().ok_or("publication manifest path")?;
    let manifest: PublicationManifest = toml::from_str(&fs::read_to_string(&manifest_path)?)?;
    let memory: Memory = toml::from_str(&fs::read_to_string(base.join(&manifest.memory))?)?;
    let ownership: Ownership =
        toml::from_str(&fs::read_to_string(base.join(&manifest.ownership))?)?;
    let mut regions = vec![];
    for name in &ownership.owned_ranges {
        let region = memory
            .regions
            .iter()
            .find(|r| &r.name == name && r.kind == "mmio")
            .ok_or_else(|| format!("owned range {name} is not an MMIO region of the memory map"))?;
        regions.push(Region {
            name: region.name.clone(),
            start: region.start,
            end: region.end_exclusive,
        });
    }
    let model = oer_register_model::RegisterModel::load(&base.join(&manifest.model))?;
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

/// One Blobray input: a pinned vendor binary.
struct Input {
    id: String,
    path: PathBuf,
    sha256: String,
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Every pinned vendor binary of `chip`, in pin order. Local builds are
/// applications linked from these binaries, and sources are not code.
fn inputs(ctx: &Context, chip: &str) -> Result<Vec<Input>> {
    let mut inputs = vec![];
    for pinned in crate::vendor_fetch::pinned(ctx, chip)? {
        if pinned.local {
            continue;
        }
        let bytes = fs::read(&pinned.path)?;
        if !crate::vendor_fingerprint::is_binary(&bytes) {
            continue;
        }
        inputs.push(Input {
            id: pinned.id,
            sha256: sha256(&bytes),
            path: pinned.path,
        });
    }
    if inputs.is_empty() {
        return Err(format!("no pinned vendor binary for {chip}").into());
    }
    Ok(inputs)
}

/// Blobray's register accesses of every function of `inputs` inside
/// `regions`, written to `registers.json` in `directory`.
fn accesses(
    ctx: &Context,
    directory: &Path,
    host: &Path,
    inputs: &[Input],
    regions: &[Region],
) -> Result<PathBuf> {
    let ranges: Vec<String> = regions
        .iter()
        .map(|r| format!("{:#x}:{:#x}", r.start, r.end - r.start))
        .collect();
    let accesses = directory.join("registers.json");
    eprintln!(
        "register-inventory: analyzing every function of {} vendor binaries",
        inputs.len()
    );
    let mut command = ctx.command(host);
    command
        .args(["--format", "json", "register-accesses"])
        .args(["--timeout-secs", &OPERATION_TIMEOUT_SECS.to_string()]);
    for input in inputs {
        command
            .arg("--input")
            .arg(format!("{}={}", input.id, input.path.display()));
    }
    for range in &ranges {
        command.args(["--range", range]);
    }
    // The document is complete only once renamed into place.
    let partial = directory.join("registers.json.partial");
    let status = command.stdout(fs::File::create(&partial)?).status()?;
    if !status.success() {
        return Err(format!(
            "blobray register-accesses failed; its partial output is {}",
            partial.display()
        )
        .into());
    }
    fs::rename(&partial, &accesses)?;
    Ok(accesses)
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
    name: Option<Vec<u8>>,
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

/// The resolved accesses of named functions in the `register-accesses`
/// document `accesses`; a function names its input by position in `inputs`.
fn observations(accesses: &str, inputs: &[Input]) -> Result<Vec<Observation>> {
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
            function: (
                input.id.clone(),
                String::from_utf8_lossy(&name).into_owned(),
            ),
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

#[derive(Serialize)]
struct Report<'a> {
    chip: &'a str,
    inputs: Vec<(&'a str, &'a str)>,
    words: &'a [WordReport],
}

fn render(chip: &str, words: &[WordReport]) -> String {
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

/// Build the inventory of `chip` into `output`, by default
/// `target/register-inventory/<chip>`.
pub fn run(ctx: &Context, chip: &str, output: Option<PathBuf>) -> Result<()> {
    crate::chips::Chip::new(&ctx.root, chip)?;
    let directory = output.unwrap_or_else(|| ctx.root.join("target/register-inventory").join(chip));
    fs::create_dir_all(&directory)?;
    let inputs = inputs(ctx, chip)?;
    let (registers, regions) = model(ctx, chip)?;
    crate::process::run(crate::blobray::cargo(ctx, "build").args([
        "--profile",
        "blobray",
        "-p",
        "blobray-cli",
        "--bin",
        "blobray",
    ]))?;
    let host = crate::blobray::binary(ctx, "blobray");
    let accesses = accesses(ctx, &directory, &host, &inputs, &regions)?;
    let observations = observations(&fs::read_to_string(&accesses)?, &inputs)?;
    let cited: BTreeSet<Function> = crate::vendor_provenance::registered(ctx, chip)?
        .into_iter()
        .map(|e| (e.artifact, e.symbol))
        .collect();
    let mut words = classify(&regions, &declared_words(&registers), &observations, &cited);
    rank(&mut words);

    let text = render(chip, &words);
    fs::write(directory.join("report.txt"), &text)?;
    fs::write(
        directory.join("report.json"),
        serde_json::to_string_pretty(&Report {
            chip,
            inputs: inputs
                .iter()
                .map(|i| (i.id.as_str(), i.sha256.as_str()))
                .collect(),
            words: &words,
        })?,
    )?;
    print!(
        "{}",
        text.lines()
            .take(5)
            .map(|l| format!("{l}\n"))
            .collect::<String>()
    );
    println!("report: {}", directory.join("report.txt").display());
    Ok(())
}
