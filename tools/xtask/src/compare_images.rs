//! Compare two linked RISC-V images function by function, modulo placement.
//!
//! Moving code between crates or adding a crate changes symbol hashes and,
//! under fat LTO, the order functions are laid out in, so the bytes of every
//! section differ even when no function's code changed. This comparison
//! replaces every address an instruction forms (branch and jump targets,
//! `auipc`/`lui` pairs with their use, `.word` data and pointers) by the
//! containing symbol and offset, drops legacy mangling hashes, and compares
//! each function's normalized instruction list by name. Identical-code-folded
//! functions, which keep either of their names, are unified through reviewed
//! aliases; reviewed scheduling ties are listed explicitly. It is the gate for
//! pure code moves between crates.
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
};

use crate::{Context, Result};
use oer_process as process;

/// One named symbol of an image.
#[derive(Clone, Debug)]
struct Symbol {
    address: u64,
    size: u64,
    name: String,
}

/// Allocated section contents by start address.
struct Sections(Vec<(u64, Vec<u8>)>);

impl Sections {
    fn read(elf: &Path) -> Result<Self> {
        use object::{Object as _, ObjectSection as _, SectionFlags, SectionKind};
        let bytes = std::fs::read(elf)?;
        let file = object::File::parse(&*bytes)?;
        let mut sections = Vec::new();
        for section in file.sections() {
            let allocated = match section.flags() {
                SectionFlags::Elf { sh_flags } => sh_flags & u64::from(object::elf::SHF_ALLOC) != 0,
                _ => false,
            };
            if allocated && section.kind() != SectionKind::UninitializedData {
                sections.push((section.address(), section.data()?.to_vec()));
            }
        }
        Ok(Self(sections))
    }

    fn contains(&self, address: u64) -> bool {
        self.0
            .iter()
            .any(|(start, data)| *start <= address && address < start + data.len() as u64)
    }
}

/// What a reviewer supplies to a comparison.
#[derive(Clone, Debug, Default)]
pub struct Review {
    pub aliases: Aliases,
    /// Functions whose code may differ (reviewed scheduling ties).
    pub allowed: BTreeSet<String>,
    /// Print the instruction diff of every differing or one-sided function
    /// whose name contains one of these.
    pub show: Vec<String>,
}

/// Reviewed renames applied to every symbol name before comparison.
#[derive(Clone, Debug, Default)]
pub struct Aliases(pub Vec<(String, String)>);

fn clean(name: &str, aliases: &Aliases) -> String {
    let mut name = name.to_owned();
    if let Some(index) = name.rfind("::h")
        && name.len() == index + 19
        && name[index + 3..].bytes().all(|b| b.is_ascii_hexdigit())
    {
        name.truncate(index);
    }
    // LLVM numbers local clones (`.123`, `.llvm.456`) per build; the
    // numbering is not part of the code, so clones share a name and are
    // compared as a sorted set of bodies.
    // The demangler shows an unrecognized suffix as `name (.123)`.
    while let Some(stem) = name.strip_suffix(')')
        && let Some((stem, suffix)) = stem.rsplit_once(" (.")
        && suffix.split('.').all(|part| {
            part == "llvm" || (!part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
        })
    {
        name.truncate(stem.len());
    }
    while let Some((stem, number)) = name.rsplit_once('.')
        && !number.is_empty()
        && number.bytes().all(|b| b.is_ascii_digit())
    {
        name.truncate(stem.strip_suffix(".llvm").unwrap_or(stem).len());
    }
    for (from, to) in &aliases.0 {
        name = name.replace(from.as_str(), to);
    }
    name
}

struct Image {
    symbols: Vec<Symbol>,
    starts: Vec<u64>,
    sections: Sections,
}

impl Image {
    fn load(tools: &Tools, elf: &Path, aliases: &Aliases) -> Result<Self> {
        let output = process::capture(
            std::process::Command::new(&tools.nm)
                .args(["-S", "-C", "--defined-only"])
                .arg(elf),
        )?;
        let mut symbols = String::from_utf8(output.stdout)?
            .lines()
            .filter_map(|line| {
                let mut parts = line.splitn(4, ' ');
                let address = u64::from_str_radix(parts.next()?, 16).ok()?;
                let size = u64::from_str_radix(parts.next()?, 16).ok()?;
                let _kind = parts.next()?;
                let name = parts.next()?;
                (!name.starts_with(".L")).then(|| Symbol {
                    address,
                    size,
                    name: clean(name, aliases),
                })
            })
            .collect::<Vec<_>>();
        symbols.sort_by(|a, b| a.address.cmp(&b.address).then(a.name.cmp(&b.name)));
        // Folded functions share an address; the smallest name stands for all.
        let mut canonical: HashMap<u64, String> = HashMap::new();
        for symbol in &symbols {
            canonical
                .entry(symbol.address)
                .or_insert_with(|| symbol.name.clone());
        }
        for symbol in &mut symbols {
            symbol.name = canonical[&symbol.address].clone();
        }
        let starts = symbols.iter().map(|s| s.address).collect();
        Ok(Self {
            symbols,
            starts,
            sections: Sections::read(elf)?,
        })
    }

    fn locate(&self, address: u64) -> String {
        let mut index = self.starts.partition_point(|start| *start <= address);
        while index > 0 {
            index -= 1;
            let symbol = &self.symbols[index];
            if symbol.address <= address && address < symbol.address + symbol.size.max(1) {
                return format!("{}+{:#x}", symbol.name, address - symbol.address);
            }
            if symbol.size != 0 {
                break;
            }
        }
        if self.sections.contains(address) {
            "data".into()
        } else {
            format!("?{address:#x}")
        }
    }
}

struct Tools {
    nm: PathBuf,
    objdump: PathBuf,
}

impl Tools {
    fn find(ctx: &Context) -> Result<Self> {
        let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        let sysroot = String::from_utf8(
            process::capture(ctx.command(&rustc).args(["--print", "sysroot"]))?.stdout,
        )?;
        let version = String::from_utf8(process::capture(ctx.command(&rustc).arg("-vV"))?.stdout)?;
        let host = version
            .lines()
            .find_map(|line| line.strip_prefix("host: "))
            .ok_or("rustc -vV has no host")?;
        let bin = Path::new(sysroot.trim())
            .join("lib/rustlib")
            .join(host)
            .join("bin");
        let tool = |name: &str| -> Result<PathBuf> {
            let path = bin.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
            if path.is_file() {
                Ok(path)
            } else {
                Err(format!(
                    "active Rust toolchain lacks {}; install llvm-tools",
                    path.display()
                )
                .into())
            }
        };
        Ok(Self {
            nm: tool("llvm-nm")?,
            objdump: tool("llvm-objdump")?,
        })
    }
}

const BRANCHES: &[&str] = &[
    "jal", "j", "beq", "bne", "blt", "bge", "bltu", "bgeu", "beqz", "bnez", "bltz", "bgez", "blez",
    "bgtz", "c.j", "c.jal", "c.beqz", "c.bnez",
];
const CALLS: &[&str] = &[
    "jal", "jalr", "c.jal", "c.jalr", "call", "tail", "jr", "c.jr", "j", "ret",
];
const STORES: &[&str] = &["sw", "sh", "sb", "sd", "c.sw", "c.sd", "fsw", "c.fsw"];

fn hex(text: &str) -> Option<u64> {
    text.strip_prefix("0x")
        .and_then(|digits| u64::from_str_radix(digits, 16).ok())
}

fn immediate(text: &str) -> Option<i64> {
    match text.strip_prefix('-') {
        Some(rest) => immediate(rest).map(|value| -value),
        None => hex(text).map(|v| v as i64).or_else(|| text.parse().ok()),
    }
}

/// Normalized instruction lists of every function, by name. A name that
/// occurs in several places keeps each body.
fn functions(
    tools: &Tools,
    elf: &Path,
    image: &Image,
) -> Result<BTreeMap<String, Vec<Vec<String>>>> {
    let output = process::capture(
        std::process::Command::new(&tools.objdump)
            .args([
                "-d",
                "-C",
                "--no-show-raw-insn",
                "--mattr=+c,+m,+a,+zba,+zbb,+zbs,+zcb,+zcmp,+f",
            ])
            .arg(elf),
    )?;
    Ok(normalize(&String::from_utf8(output.stdout)?, image))
}

/// Function bodies of an `objdump -d` listing, with every address replaced
/// by the symbol that contains it.
fn normalize(text: &str, image: &Image) -> BTreeMap<String, Vec<Vec<String>>> {
    let mut functions: BTreeMap<String, Vec<Vec<String>>> = BTreeMap::new();
    let mut current: Option<(String, Vec<String>)> = None;
    let mut pending: HashMap<String, u64> = HashMap::new();
    for line in text.lines() {
        if let Some(label) = line.strip_suffix(">:")
            && let Some((address, _)) = label.split_once(" <")
            && let Ok(address) = u64::from_str_radix(address, 16)
        {
            if let Some((name, body)) = current.take() {
                functions.entry(name).or_default().push(body);
            }
            let name = image.locate(address);
            current = Some((name.trim_end_matches("+0x0").to_owned(), Vec::new()));
            pending.clear();
            continue;
        }
        let Some((_, body)) = current.as_mut() else {
            continue;
        };
        let Some((pc, instruction)) = line.trim_start().split_once(':') else {
            continue;
        };
        let Ok(pc) = u64::from_str_radix(pc, 16) else {
            continue;
        };
        let instruction = instruction.trim();
        let instruction = instruction.split(" <").next().unwrap_or(instruction);
        let (op, arguments) = instruction
            .split_once(char::is_whitespace)
            .map_or((instruction, ""), |(op, rest)| (op, rest.trim()));
        let mut args = if arguments.is_empty() {
            Vec::new()
        } else {
            arguments
                .split(',')
                .map(|a| a.trim().to_owned())
                .collect::<Vec<_>>()
        };
        let normalized = if op == ".word" {
            let value = args.first().and_then(|a| hex(a)).unwrap_or_default();
            if image.sections.contains(value) {
                format!(".word {}", image.locate(value))
            } else {
                format!(".word {value:#x}")
            }
        } else if BRANCHES.contains(&op) && args.last().and_then(|a| hex(a)).is_some() {
            let target = hex(args.last().unwrap()).unwrap();
            let kept = args[..args.len() - 1].join(",");
            format!("{op} {kept} {}", image.locate(target))
        } else if op == "auipc" && args.len() == 2 {
            let high = immediate(&args[1]).unwrap_or_default();
            pending.insert(args[0].clone(), pc.wrapping_add((high << 12) as u64));
            format!("auipc {} <pair>", args[0])
        } else if op == "lui" && args.len() == 2 {
            let high = immediate(&args[1]).unwrap_or_default();
            pending.insert(args[0].clone(), ((high << 12) as u64) & 0xffff_ffff);
            format!("lui {} <pair>", args[0])
        } else {
            // `addi rd,rs,0` prints as `mv rd,rs`: one instruction, one form.
            let op = if op == "mv" && args.len() == 2 && pending.contains_key(&args[1]) {
                args = vec![args[0].clone(), args[1].clone(), "0".into()];
                "addi"
            } else {
                op
            };
            // An indirect jump with a zero offset prints as `jalr rs`; the
            // offset of the same call differs with placement.
            if matches!(op, "jalr" | "jr" | "c.jalr" | "c.jr")
                && args.len() == 1
                && !args[0].contains('(')
            {
                args = vec![format!("0({})", args[0])];
            }
            let memory = args.last().and_then(|last| {
                let (offset, base) = last.strip_suffix(')')?.split_once('(')?;
                Some((immediate(offset)?, base.to_owned()))
            });
            let (base, offset) = match &memory {
                Some((offset, base)) => (Some(base.clone()), *offset),
                None if matches!(op, "addi" | "mv" | "jalr") && args.len() >= 3 => (
                    Some(args[1].clone()),
                    immediate(&args[2]).unwrap_or_default(),
                ),
                None => (None, 0),
            };
            let resolved = base
                .as_ref()
                .and_then(|base| pending.get(base))
                .map(|high| (*high as i64).wrapping_add(offset) as u64 & 0xffff_ffff);
            let text = match resolved {
                Some(target) => {
                    let kept = if memory.is_some() {
                        args[..args.len() - 1].join(",")
                    } else {
                        args[..2].join(",")
                    };
                    format!("{op} {kept} {}", image.locate(target))
                }
                None => format!("{op} {}", args.join(",")),
            };
            if let Some(destination) = args.first()
                && !STORES.contains(&op)
            {
                // An address computed from a tracked base is itself a base
                // (`addi s2,s2,lo`, `mv a0,s2`); anything else a register
                // receives, including a load through it, is not.
                match resolved {
                    Some(target) if memory.is_none() => {
                        pending.insert(destination.clone(), target);
                    }
                    _ => {
                        pending.remove(destination);
                    }
                }
            }
            text
        };
        if CALLS.contains(&op) {
            for register in std::iter::once("ra".to_owned())
                .chain((0..8).map(|i| format!("a{i}")))
                .chain((0..7).map(|i| format!("t{i}")))
            {
                pending.remove(&register);
            }
        }
        body.push(normalized);
    }
    if let Some((name, body)) = current {
        functions.entry(name).or_default().push(body);
    }
    for bodies in functions.values_mut() {
        bodies.sort();
    }
    functions
}

/// The outcome of one comparison.
#[derive(Debug, Default)]
pub struct Comparison {
    pub common: usize,
    pub differing: Vec<String>,
    pub only_old: Vec<String>,
    pub only_new: Vec<String>,
    /// One-image names whose bodies match a one-image name of the other side.
    pub paired_by_body: usize,
}

impl Comparison {
    pub fn equivalent(&self, allowed: &BTreeSet<String>) -> bool {
        self.differing.iter().all(|name| allowed.contains(name))
            && self.only_old.len() == self.paired_by_body
            && self.only_new.len() == self.paired_by_body
    }
}

fn compare_functions(
    old: &BTreeMap<String, Vec<Vec<String>>>,
    new: &BTreeMap<String, Vec<Vec<String>>>,
) -> Comparison {
    let names_old = old.keys().cloned().collect::<BTreeSet<_>>();
    let names_new = new.keys().cloned().collect::<BTreeSet<_>>();
    let common = names_old
        .intersection(&names_new)
        .cloned()
        .collect::<Vec<_>>();
    let only_old = names_old
        .difference(&names_new)
        .cloned()
        .collect::<Vec<_>>();
    let only_new = names_new
        .difference(&names_old)
        .cloned()
        .collect::<Vec<_>>();
    let differing = common
        .iter()
        .filter(|name| old[*name] != new[*name])
        .cloned()
        .collect();
    let mut left = only_old.iter().map(|n| &old[n]).collect::<Vec<_>>();
    let mut right = only_new.iter().map(|n| &new[n]).collect::<Vec<_>>();
    left.sort();
    right.sort();
    let paired_by_body = left.iter().zip(&right).filter(|(a, b)| a == b).count();
    Comparison {
        common: common.len(),
        differing,
        only_old,
        only_new,
        paired_by_body,
    }
}

/// Compare two ELF images and print the result; fails unless every function
/// is equivalent or listed in `review.allowed`.
pub fn compare_elf(ctx: &Context, old: &Path, new: &Path, review: &Review) -> Result<Comparison> {
    let tools = Tools::find(ctx)?;
    let image_old = Image::load(&tools, old, &review.aliases)?;
    let image_new = Image::load(&tools, new, &review.aliases)?;
    let old_functions = functions(&tools, old, &image_old)?;
    let new_functions = functions(&tools, new, &image_new)?;
    let comparison = compare_functions(&old_functions, &new_functions);
    println!(
        "{}: {} common functions, {} differ, {} only old, {} only new ({} paired by identical body)",
        new.display(),
        comparison.common,
        comparison.differing.len(),
        comparison.only_old.len(),
        comparison.only_new.len(),
        comparison.paired_by_body
    );
    for name in comparison.differing.iter().take(40) {
        let reviewed = if review.allowed.contains(name) {
            " (reviewed tie)"
        } else {
            ""
        };
        println!("  differs: {name}{reviewed}");
    }
    for name in comparison.only_old.iter().take(10) {
        println!("  only old: {name}");
    }
    for name in comparison.only_new.iter().take(10) {
        println!("  only new: {name}");
    }
    let shown = comparison
        .differing
        .iter()
        .chain(&comparison.only_old)
        .chain(&comparison.only_new)
        .filter(|name| review.show.iter().any(|pattern| name.contains(pattern)))
        .collect::<BTreeSet<_>>();
    let none = Vec::new();
    for name in shown {
        println!("--- {name}");
        let old = old_functions.get(name).unwrap_or(&none);
        let new = new_functions.get(name).unwrap_or(&none);
        for index in 0..old.len().max(new.len()) {
            let empty = Vec::new();
            let (old, new) = (
                old.get(index).unwrap_or(&empty),
                new.get(index).unwrap_or(&empty),
            );
            if old == new {
                continue;
            }
            if old.len().max(new.len()) > 1 {
                println!("  body {index}:");
            }
            for (mark, line) in diff(old, new) {
                println!("  {mark} {line}");
            }
        }
    }
    Ok(comparison)
}

/// A line diff by longest common subsequence: ' ' kept, '-' old, '+' new.
fn diff<'a>(old: &'a [String], new: &'a [String]) -> Vec<(char, &'a str)> {
    let mut common = vec![vec![0u32; new.len() + 1]; old.len() + 1];
    for i in (0..old.len()).rev() {
        for j in (0..new.len()).rev() {
            common[i][j] = if old[i] == new[j] {
                common[i + 1][j + 1] + 1
            } else {
                common[i + 1][j].max(common[i][j + 1])
            };
        }
    }
    let (mut i, mut j, mut lines) = (0, 0, Vec::new());
    while i < old.len() || j < new.len() {
        if i < old.len() && j < new.len() && old[i] == new[j] {
            lines.push((' ', old[i].as_str()));
            i += 1;
            j += 1;
        } else if j < new.len() && (i == old.len() || common[i][j + 1] >= common[i + 1][j]) {
            lines.push(('+', new[j].as_str()));
            j += 1;
        } else {
            lines.push(('-', old[i].as_str()));
            i += 1;
        }
    }
    lines
}

/// The runtime ELF of the newest build of `class` in the checkout at `root`.
fn image_elf(root: &Path, class: &str) -> Result<PathBuf> {
    let suffix = format!("-{class}-owned-xarxa");
    std::fs::read_dir(root.join("target/hil/esp32s31"))?
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(&suffix))
        .map(|entry| entry.path().join("runtime.elf"))
        .filter(|elf| elf.is_file())
        .max_by_key(|elf| std::fs::metadata(elf).and_then(|m| m.modified()).ok())
        .ok_or_else(|| format!("no {class} runtime.elf under {}", root.display()).into())
}

/// Build `classes` at `base` in a detached worktree and in this checkout,
/// then compare each pair; fails unless every pair is equivalent.
pub fn compare_images(
    ctx: &Context,
    base: &str,
    classes: &[String],
    review: &Review,
) -> Result<()> {
    let worktree = ctx.root.join("target/compare/base");
    if worktree.exists() {
        let _ = process::run(
            ctx.command("git")
                .args(["worktree", "remove", "--force"])
                .arg(&worktree),
        );
    }
    process::run(
        ctx.command("git")
            .args(["worktree", "add", "--detach", "--force"])
            .arg(&worktree)
            .arg(base),
    )?;
    let mut equivalent = true;
    for class in classes {
        for root in [worktree.as_path(), ctx.root.as_path()] {
            process::run(
                ctx.cargo()
                    .current_dir(root)
                    .args(["hil", "image", "build", class]),
            )?;
        }
        let comparison = compare_elf(
            ctx,
            &image_elf(&worktree, class)?,
            &image_elf(&ctx.root, class)?,
            review,
        )?;
        equivalent &= comparison.equivalent(&review.allowed);
    }
    if equivalent {
        println!("images equivalent modulo placement against {base}");
        Ok(())
    } else {
        Err(format!("images differ from {base} beyond placement").into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(lines: &[&str]) -> Vec<Vec<String>> {
        vec![lines.iter().map(|l| (*l).to_owned()).collect()]
    }

    #[test]
    fn legacy_hashes_are_dropped_and_aliases_applied() {
        let aliases = Aliases(vec![("oer_esp32s31_pac::Mac".into(), "Mac".into())]);
        assert_eq!(
            clean("oer_esp32s31_pac::Mac::set::h0123456789abcdef", &aliases),
            "Mac::set"
        );
        assert_eq!(clean("plain::h0123", &aliases), "plain::h0123");
    }

    #[test]
    fn llvm_clone_numbers_are_dropped() {
        let none = Aliases::default();
        assert_eq!(
            clean("<u8 as Debug>::fmt.1234", &none),
            "<u8 as Debug>::fmt"
        );
        assert_eq!(clean("inner.llvm.98765.12", &none), "inner");
        assert_eq!(clean("v1.2::name", &none), "v1.2::name");
        assert_eq!(clean("log::LOGGER (.4696)", &none), "log::LOGGER");
        assert_eq!(
            clean("descriptor_head (.llvm.12)", &none),
            "descriptor_head"
        );
        assert_eq!(clean("f (.cold)", &none), "f (.cold)");
    }

    #[test]
    fn a_base_advanced_in_place_resolves_to_the_advanced_address() {
        let symbol = |address, size, name: &str| Symbol {
            address,
            size,
            name: name.into(),
        };
        let symbols = vec![
            symbol(0x4000_0000, 0x20, "f"),
            symbol(0x4080_1000, 0x100, "POOL"),
            symbol(0x4080_1100, 0x10, "WAKER"),
        ];
        let image = Image {
            starts: symbols.iter().map(|s| s.address).collect(),
            symbols,
            sections: Sections(Vec::new()),
        };
        let listing = "\
40000000 <f>:
40000000: lui s2, 0x40801
40000004: addi s2, s2, 0x100
40000008: addi a1, s2, 0x4
4000000c: lw s2, 0x0(s2)
40000010: addi a2, s2, 0x8
";
        let body = &normalize(listing, &image)["f"][0];
        assert_eq!(body[1], "addi s2,s2 WAKER+0x0");
        assert_eq!(body[2], "addi a1,s2 WAKER+0x4");
        assert_eq!(body[4], "addi a2,s2,0x8", "a loaded value is no address");
    }

    #[test]
    fn placement_dependent_spellings_of_one_instruction_normalize_alike() {
        let symbols = vec![
            Symbol {
                address: 0x4000_0000,
                size: 0x40,
                name: "f".into(),
            },
            Symbol {
                address: 0x4080_0000,
                size: 0x10,
                name: "callee".into(),
            },
            Symbol {
                address: 0x4090_0000,
                size: 0x10,
                name: "DATA".into(),
            },
        ];
        let image = Image {
            starts: symbols.iter().map(|s| s.address).collect(),
            symbols,
            sections: Sections(Vec::new()),
        };
        let listing = "\
40000000 <f>:
40000000: auipc ra, 0x800
40000004: jalr ra
40000008: auipc ra, 0x800
4000000c: jalr -0x8(ra)
40000010: lui a0, 0x40900
40000014: mv a1, a0
40000018: lui a0, 0x40900
4000001c: addi a1, a0, 0x0
";
        let body = &normalize(listing, &image)["f"][0];
        assert_eq!(body[1], "jalr  callee+0x0");
        assert_eq!(body[3], "jalr  callee+0x0");
        assert_eq!(body[5], body[7]);
    }

    #[test]
    fn a_diff_keeps_common_lines_and_marks_the_rest() {
        let lines = |text: &[&str]| text.iter().map(|l| (*l).to_owned()).collect::<Vec<_>>();
        let (old, new) = (lines(&["a", "b", "c"]), lines(&["a", "x", "c"]));
        assert_eq!(
            diff(&old, &new),
            [(' ', "a"), ('+', "x"), ('-', "b"), (' ', "c")]
        );
    }

    #[test]
    fn equivalence_allows_reviewed_ties_and_body_paired_renames() {
        let old = BTreeMap::from([
            ("a".to_owned(), body(&["addi a0,a0,1"])),
            ("tie".to_owned(), body(&["addi s8,s8,1", "addi s9,s9,-1"])),
            ("old_name".to_owned(), body(&["ret"])),
        ]);
        let new = BTreeMap::from([
            ("a".to_owned(), body(&["addi a0,a0,1"])),
            ("tie".to_owned(), body(&["addi s9,s9,-1", "addi s8,s8,1"])),
            ("new_name".to_owned(), body(&["ret"])),
        ]);
        let comparison = compare_functions(&old, &new);
        assert_eq!(comparison.differing, ["tie"]);
        assert_eq!(comparison.paired_by_body, 1);
        assert!(!comparison.equivalent(&BTreeSet::new()));
        assert!(comparison.equivalent(&BTreeSet::from(["tie".to_owned()])));
    }

    #[test]
    fn a_changed_body_is_not_equivalent() {
        let old = BTreeMap::from([("f".to_owned(), body(&["li a0,1"]))]);
        let new = BTreeMap::from([("f".to_owned(), body(&["li a0,2"]))]);
        assert!(!compare_functions(&old, &new).equivalent(&BTreeSet::new()));
    }
}
