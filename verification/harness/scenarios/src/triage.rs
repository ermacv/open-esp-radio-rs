//! Triage aid for untriaged vendor locations.
//!
//! For every location the scenarios leave untriaged, the report shows the
//! instructions around it, the in-function definitions of the registers the
//! instruction at the location reads, and whether the block it opens only
//! calls diagnostic functions and stores nothing outside the stack. These are
//! proposals for a reviewer: exclusions stay reviewed decisions.
use crate::session::evidence_index::{Location, LocationKind};
use object::{Object, ObjectSymbol, SymbolKind};
use rv_asm::{Inst, Xlen};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Instructions shown before and after a location.
const BEFORE: usize = 10;
const AFTER: usize = 3;
/// Definitions followed back from a location's operands.
const DEPTH: usize = 2;
/// Callee name fragments of functions that only log or print.
const DIAGNOSTIC: &[&str] = &["log", "printf", "puts", "putchar", "dump", "dbg_"];

/// One decoded instruction of a function.
#[derive(Clone, Debug)]
pub struct Line {
    pub offset: u32,
    pub text: String,
}

/// A function's code, by name, from the first ELF that defines it.
pub struct Code {
    functions: BTreeMap<String, (u32, Vec<u8>)>,
    names: BTreeMap<u32, String>,
}

impl Code {
    /// The code functions of `elfs`; an earlier ELF wins a name.
    pub fn of(elfs: &[&[u8]]) -> Self {
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
        Self { functions, names }
    }

    /// The linear decoding of `function`.
    pub fn lines(&self, function: &str) -> Option<Vec<Line>> {
        let (entry, bytes) = self.functions.get(function)?;
        let mut lines = vec![];
        let mut offset = 0usize;
        // The address and upper immediate of the previous `auipc ra`.
        let mut auipc: Option<(u32, i64)> = None;
        while offset + 2 <= bytes.len() {
            let half = u16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
            let (text, length) = if half & 3 == 3 {
                let Some(word) = bytes.get(offset..offset + 4) else {
                    break;
                };
                let word = u32::from_le_bytes([word[0], word[1], word[2], word[3]]);
                let text = Inst::decode_normal(word, Xlen::Rv32)
                    .map_or_else(|_| format!(".word {word:#010x}"), |i| i.to_string());
                (text, 4)
            } else {
                let text = Inst::decode_compressed(half, Xlen::Rv32)
                    .map_or_else(|_| format!(".half {half:#06x}"), |i| i.to_string());
                (text, 2)
            };
            let address = entry + offset as u32;
            let mut text = self.annotate(address, &text);
            if let (Some((base, upper)), Some(low)) = (auipc, call_offset(&text)) {
                let target = (i64::from(base) + (upper << 12) + low) as u32;
                if let Some(name) = self.names.get(&target) {
                    text = format!("{text}    <{name}>");
                }
            }
            auipc = text
                .strip_prefix("auipc ra, ")
                .and_then(|upper| upper.trim().parse::<i64>().ok())
                .map(|upper| (address, upper));
            lines.push(Line {
                offset: offset as u32,
                text,
            });
            offset += length;
        }
        Some(lines)
    }

    /// `text` of the instruction at `address`, with the name of a direct
    /// jump target.
    fn annotate(&self, address: u32, text: &str) -> String {
        let target = text
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter_map(|token| token.parse::<i64>().ok())
            .next_back();
        match (text.split_whitespace().next(), target) {
            (Some("jal"), Some(offset)) => {
                let target = (i64::from(address) + offset) as u32;
                match self.names.get(&target) {
                    Some(name) => format!("{text}    <{name}>"),
                    None => text.to_owned(),
                }
            }
            _ => text.to_owned(),
        }
    }
}

/// The offset of a `jalr ra, N(ra)` call through `auipc ra`.
fn call_offset(text: &str) -> Option<i64> {
    text.strip_prefix("jalr ra, ")?
        .strip_suffix("(ra)")?
        .parse()
        .ok()
}

/// Register operands of an instruction's text, first the written one when
/// the mnemonic writes one.
fn registers(text: &str) -> (Option<String>, Vec<String>) {
    let mut words = text.split(|c: char| c == ',' || c == '(' || c == ')' || c.is_whitespace());
    let mnemonic = words.next().unwrap_or_default();
    let operands: Vec<String> = words
        .filter(|w| is_register(w))
        .map(str::to_owned)
        .collect();
    let writes = !(mnemonic.starts_with('b') && mnemonic != "bclr" && mnemonic != "bset"
        || mnemonic.starts_with('s') && matches!(mnemonic, "sb" | "sh" | "sw" | "c.sw" | "c.swsp")
        || mnemonic.starts_with("fence")
        || mnemonic == "ret"
        || mnemonic == "j");
    if writes && !operands.is_empty() {
        (Some(operands[0].clone()), operands[1..].to_vec())
    } else {
        (None, operands)
    }
}

fn is_register(word: &str) -> bool {
    matches!(word, "zero" | "ra" | "sp" | "gp" | "tp" | "fp")
        || (word.len() >= 2
            && matches!(&word[..1], "a" | "s" | "t")
            && word[1..].chars().all(|c| c.is_ascii_digit()))
}

/// The in-function definitions of the registers the instruction at `index`
/// reads, followed `DEPTH` steps back.
fn definitions(lines: &[Line], index: usize) -> Vec<String> {
    let mut found = vec![];
    let mut wanted: Vec<(String, usize, usize)> = registers(&lines[index].text)
        .1
        .into_iter()
        .filter(|r| r != "zero" && r != "sp")
        .map(|r| (r, index, 0))
        .collect();
    let mut seen = BTreeSet::new();
    while let Some((register, from, depth)) = wanted.pop() {
        let Some(at) = (0..from)
            .rev()
            .find(|i| registers(&lines[*i].text).0.as_deref() == Some(&register))
        else {
            found.push(format!(
                "{register}: an argument or not defined in this function"
            ));
            continue;
        };
        if !seen.insert(at) {
            continue;
        }
        found.push(format!(
            "{register} <- +{:#x}: {}",
            lines[at].offset, lines[at].text
        ));
        if depth + 1 < DEPTH {
            wanted.extend(
                registers(&lines[at].text)
                    .1
                    .into_iter()
                    .filter(|r| r != "zero" && r != "sp")
                    .map(|r| (r, at, depth + 1)),
            );
        }
    }
    found
}

/// Whether the straight-line code from `index` to the next branch or return
/// only calls diagnostic functions and stores nothing outside the stack.
fn diagnostic_only(lines: &[Line], index: usize) -> bool {
    let mut calls = 0;
    for line in &lines[index..] {
        let mnemonic = line.text.split_whitespace().next().unwrap_or_default();
        if line.text.starts_with('b') && !line.text.starts_with("bclr") || mnemonic == "ret" {
            break;
        }
        if matches!(mnemonic, "sb" | "sh" | "sw") && !line.text.contains("(sp)") {
            return false;
        }
        if matches!(mnemonic, "jal" | "jalr") {
            // A call names its callee; an unnamed indirect transfer is not a
            // diagnostic call.
            calls += 1;
            let callee = line.text.rsplit_once('<').map(|(_, name)| name);
            if !callee.is_some_and(|name| DIAGNOSTIC.iter().any(|d| name.contains(d))) {
                return false;
            }
        }
    }
    calls > 0
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
            let candidate =
                matches!(location.kind, LocationKind::Block) && diagnostic_only(&lines, index);
            text.push_str(&format!(
                "\n  +{:#x} {:?}{}\n",
                location.offset,
                location.kind,
                if candidate {
                    "  [candidate: diagnostic calls only, no store outside the stack]"
                } else {
                    ""
                }
            ));
            let start = index.saturating_sub(BEFORE);
            for (i, line) in lines
                .iter()
                .enumerate()
                .take((index + AFTER + 1).min(lines.len()))
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

    fn line(offset: u32, text: &str) -> Line {
        Line {
            offset,
            text: text.into(),
        }
    }

    #[test]
    fn operands_trace_back_to_their_definitions() {
        let lines = [
            line(0, "lui a5, 32772"),
            line(4, "lw a4, 32(a5)"),
            line(8, "andi a4, a4, 1"),
            line(12, "beq a4, zero, 8"),
        ];
        let found = definitions(&lines, 3);
        assert!(found[0].contains("a4 <- +0x8: andi a4, a4, 1"), "{found:?}");
        assert!(
            found
                .iter()
                .any(|f| f.contains("a4 <- +0x4: lw a4, 32(a5)")),
            "{found:?}"
        );
    }

    #[test]
    fn a_logging_block_is_a_candidate_and_a_store_is_not() {
        let logging = [
            line(0, "addi a0, zero, 6"),
            line(4, "jal ra, 64    <wifi_log>"),
            line(8, "sw a0, 12(sp)"),
            line(12, "beq a0, zero, 8"),
        ];
        assert!(diagnostic_only(&logging, 0));
        let storing = [
            line(0, "jal ra, 64    <wifi_log>"),
            line(4, "sw a0, 0(a5)"),
            line(8, "ret"),
        ];
        assert!(!diagnostic_only(&storing, 0));
        let calling = [line(0, "jal ra, 64    <mac_tx_set_len>"), line(4, "ret")];
        assert!(!diagnostic_only(&calling, 0));
        let far = [
            line(0, "auipc ra, 0"),
            line(4, "jalr ra, -166(ra)    <wifi_log>"),
            line(8, "ret"),
        ];
        assert!(diagnostic_only(&far, 0));
        assert_eq!(call_offset("jalr ra, -166(ra)"), Some(-166));
    }
}
