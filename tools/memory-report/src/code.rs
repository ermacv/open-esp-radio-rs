//! Surviving linked text, independent of RAM policy and compiler estimates.

use crate::{Error, Result};
use object::{Object, ObjectSection, ObjectSymbol, SectionKind, SymbolKind};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

/// Aliases at one linked address are one range, never additive allocations.
#[derive(Debug, Serialize)]
pub struct CodeSymbol {
    pub address: u64,
    pub bytes: u64,
    pub aliases: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct CodeSection {
    pub name: String,
    pub address: u64,
    pub bytes: u64,
    /// Union of sized text-symbol ranges; overlapping symbols count once.
    pub symbol_covered_bytes: u64,
    /// Includes padding and code with no sized surviving symbol, not "unused" RAM.
    pub unattributed_bytes: u64,
    pub symbols: Vec<CodeSymbol>,
}

#[derive(Debug, Serialize)]
pub struct CodeReport {
    pub schema: u32,
    pub elf: PathBuf,
    pub text_bytes: u64,
    pub symbol_covered_bytes: u64,
    pub sections: Vec<CodeSection>,
}

#[derive(Debug, Serialize)]
pub struct CodeDiff {
    pub before_elf: PathBuf,
    pub after_elf: PathBuf,
    pub before_text_bytes: u64,
    pub after_text_bytes: u64,
    pub text_delta_bytes: i128,
    pub section_delta_bytes: BTreeMap<String, i128>,
    /// Diagnostic symbol ranges, not additive attribution. Matching uses the
    /// complete alias set and section, never a demangled substring. A changed
    /// alias set is reported as removed/added, not guessed to be the same code.
    pub symbols: Vec<CodeSymbolDiff>,
}

#[derive(Debug, Serialize)]
pub struct CodeSymbolDiff {
    pub section: String,
    pub aliases: Vec<String>,
    pub before: Vec<CodeSymbolRange>,
    pub after: Vec<CodeSymbolRange>,
    /// Sum of this identity's ranges only; not additive across identities that
    /// overlap. Section deltas remain the linked-image size authority.
    pub range_delta_bytes: i128,
}

#[derive(Debug, Serialize)]
pub struct CodeSymbolRange {
    pub address: u64,
    pub bytes: u64,
}

/// Exact section deltas, not guessed ownership of inlined generic code.
pub fn diff_code(before: &CodeReport, after: &CodeReport) -> CodeDiff {
    let mut sections = BTreeMap::new();
    let mut symbols = BTreeMap::<(String, Vec<String>), CodeSymbolDiff>::new();
    for (report, sign) in [(before, -1i128), (after, 1)] {
        for section in &report.sections {
            *sections.entry(section.name.clone()).or_default() += sign * i128::from(section.bytes);
            for symbol in &section.symbols {
                let mut aliases = symbol.aliases.clone();
                aliases.sort();
                aliases.dedup();
                let diff = symbols
                    .entry((section.name.clone(), aliases.clone()))
                    .or_insert_with(|| CodeSymbolDiff {
                        section: section.name.clone(),
                        aliases,
                        before: Vec::new(),
                        after: Vec::new(),
                        range_delta_bytes: 0,
                    });
                let ranges = if sign < 0 {
                    &mut diff.before
                } else {
                    &mut diff.after
                };
                ranges.push(CodeSymbolRange {
                    address: symbol.address,
                    bytes: symbol.bytes,
                });
                diff.range_delta_bytes += sign * i128::from(symbol.bytes);
            }
        }
    }
    let mut symbols: Vec<_> = symbols.into_values().collect();
    symbols.sort_by(|a, b| {
        b.range_delta_bytes
            .abs()
            .cmp(&a.range_delta_bytes.abs())
            .then(a.section.cmp(&b.section))
            .then(a.aliases.cmp(&b.aliases))
    });
    CodeDiff {
        before_elf: before.elf.clone(),
        after_elf: after.elf.clone(),
        before_text_bytes: before.text_bytes,
        after_text_bytes: after.text_bytes,
        text_delta_bytes: i128::from(after.text_bytes) - i128::from(before.text_bytes),
        section_delta_bytes: sections,
        symbols,
    }
}

pub fn render_code_diff(report: &CodeDiff) -> String {
    use std::fmt::Write;
    let mut out = format!(
        "Linked text: {} -> {} bytes ({:+})\n{:?}\nSymbol ranges below are diagnostic and are NOT additive generic attribution.\n",
        report.before_text_bytes,
        report.after_text_bytes,
        report.text_delta_bytes,
        report.section_delta_bytes,
    );
    for symbol in report
        .symbols
        .iter()
        .filter(|s| s.range_delta_bytes != 0)
        .take(40)
    {
        writeln!(
            out,
            "  {:+} bytes {} {} ({} -> {} ranges)",
            symbol.range_delta_bytes,
            symbol.section,
            symbol.aliases.join(" | "),
            symbol.before.len(),
            symbol.after.len()
        )
        .unwrap();
    }
    out
}

/// Inspect the supplied final image without rebuilding it or changing flags.
/// Inlined/merged code cannot be attributed uniquely to its generic definition.
pub fn analyze_code(path: &Path) -> Result<CodeReport> {
    let data = fs::read(path).map_err(|source| Error::Read {
        path: path.into(),
        source,
    })?;
    let error = |message: String| Error::Elf {
        path: path.into(),
        message,
    };
    let elf = object::File::parse(&*data).map_err(|e| error(e.to_string()))?;
    if !matches!(
        elf.kind(),
        object::ObjectKind::Executable | object::ObjectKind::Dynamic
    ) {
        return Err(error(
            "code analysis requires a linked image, not a relocatable object".into(),
        ));
    }
    let mut sections = Vec::new();
    for section in elf.sections().filter(|s| s.kind() == SectionKind::Text) {
        let start = section.address();
        let end = start
            .checked_add(section.size())
            .ok_or_else(|| error("text range overflow".into()))?;
        let mut symbols = BTreeMap::<u64, CodeSymbol>::new();
        for symbol in elf.symbols().filter(|s| {
            s.is_definition()
                && s.kind() == SymbolKind::Text
                && s.section_index() == Some(section.index())
        }) {
            let address = symbol.address();
            let symbol_end = address
                .checked_add(symbol.size())
                .ok_or_else(|| error("symbol range overflow".into()))?;
            if address < start || symbol_end > end {
                return Err(error(format!(
                    "text symbol outside section {:?}",
                    section.name()
                )));
            }
            let entry = symbols.entry(address).or_insert_with(|| CodeSymbol {
                address,
                bytes: 0,
                aliases: Vec::new(),
            });
            entry.bytes = entry.bytes.max(symbol.size());
            let name = symbol.name().map_err(|e| error(e.to_string()))?;
            entry.aliases.push(
                rustc_demangle::try_demangle(name)
                    .map(|n| n.to_string())
                    .unwrap_or_else(|_| name.into()),
            );
        }
        let mut symbols: Vec<_> = symbols.into_values().collect();
        for symbol in &mut symbols {
            symbol.aliases.sort();
            symbol.aliases.dedup();
        }
        let covered = range_union(&symbols);
        sections.push(CodeSection {
            name: section.name().map_err(|e| error(e.to_string()))?.into(),
            address: start,
            bytes: section.size(),
            symbol_covered_bytes: covered,
            unattributed_bytes: section.size() - covered,
            symbols,
        });
    }
    if sections.is_empty() {
        return Err(error("image has no text sections".into()));
    }
    Ok(CodeReport {
        schema: 1,
        elf: path.into(),
        text_bytes: sections.iter().map(|s| s.bytes).sum(),
        symbol_covered_bytes: sections.iter().map(|s| s.symbol_covered_bytes).sum(),
        sections,
    })
}

fn range_union(symbols: &[CodeSymbol]) -> u64 {
    let mut end = 0;
    let mut bytes = 0;
    for symbol in symbols {
        let next = symbol.address + symbol.bytes;
        bytes += next.saturating_sub(end.max(symbol.address));
        end = end.max(next);
    }
    bytes
}

pub fn render_code_report(report: &CodeReport) -> String {
    use std::fmt::Write;
    let mut out = format!(
        "Linked text: {} bytes; symbol-covered union: {} bytes\nNot a per-generic size attribution.\n",
        report.text_bytes, report.symbol_covered_bytes
    );
    for section in &report.sections {
        writeln!(
            out,
            "{}: {} bytes, {} unattributed",
            section.name, section.bytes, section.unattributed_bytes
        )
        .unwrap();
        let mut largest: Vec<_> = section.symbols.iter().collect();
        largest.sort_by_key(|s| (std::cmp::Reverse(s.bytes), s.address));
        for symbol in largest.into_iter().take(20) {
            writeln!(
                out,
                "  {:#x} {} bytes {}",
                symbol.address,
                symbol.bytes,
                symbol.aliases.join(" | ")
            )
            .unwrap();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn diff_retains_alias_groups_added_removed_and_ambiguous_names() {
        let report = |symbols: Vec<CodeSymbol>| CodeReport {
            schema: 1,
            elf: "fixture.elf".into(),
            text_bytes: 100,
            symbol_covered_bytes: 100,
            sections: vec![CodeSection {
                name: ".text".into(),
                address: 100,
                bytes: 100,
                symbol_covered_bytes: 100,
                unattributed_bytes: 0,
                symbols,
            }],
        };
        let symbol = |address, bytes, names: &[&str]| CodeSymbol {
            address,
            bytes,
            aliases: names.iter().map(|s| (*s).into()).collect(),
        };
        let before = report(vec![
            symbol(100, 20, &["a", "alias"]),
            symbol(130, 10, &["duplicate"]),
            symbol(150, 5, &["duplicate"]),
            symbol(160, 4, &["gone"]),
        ]);
        let after = report(vec![
            symbol(110, 25, &["alias", "a"]),
            symbol(140, 12, &["duplicate"]),
            symbol(170, 9, &["new"]),
        ]);
        let diff = diff_code(&before, &after);
        assert_eq!(diff.text_delta_bytes, 0); // Symbol deltas are not section deltas.
        let aliases = diff
            .symbols
            .iter()
            .find(|s| s.aliases == ["a", "alias"])
            .unwrap();
        assert_eq!(aliases.range_delta_bytes, 5);
        assert_eq!(aliases.before.len(), 1); // alias is not charged twice
        let duplicate = diff
            .symbols
            .iter()
            .find(|s| s.aliases == ["duplicate"])
            .unwrap();
        assert_eq!(duplicate.before.len(), 2); // no arbitrary first match
        assert_eq!(duplicate.range_delta_bytes, -3);
        assert!(
            diff.symbols
                .iter()
                .any(|s| s.aliases == ["gone"] && s.after.is_empty())
        );
        assert!(
            diff.symbols
                .iter()
                .any(|s| s.aliases == ["new"] && s.before.is_empty())
        );
    }
    #[test]
    fn actual_linked_test_binary_is_accounted_without_double_counting() {
        let report = analyze_code(&std::env::current_exe().unwrap()).unwrap();
        assert!(report.text_bytes > 0);
        assert!(report.symbol_covered_bytes > 0);
        assert!(report.symbol_covered_bytes <= report.text_bytes);
        for section in &report.sections {
            assert_eq!(
                section.symbol_covered_bytes + section.unattributed_bytes,
                section.bytes
            );
            assert!(
                section
                    .symbols
                    .windows(2)
                    .all(|pair| pair[0].address < pair[1].address)
            );
        }
        let delta = diff_code(&report, &report);
        assert_eq!(delta.text_delta_bytes, 0);
        assert!(delta.section_delta_bytes.values().all(|v| *v == 0));
    }
    #[test]
    fn aliases_overlaps_and_unsized_symbols_are_not_additive() {
        let symbols =
            [(100, 20), (110, 20), (120, 0), (140, 10)].map(|(address, bytes)| CodeSymbol {
                address,
                bytes,
                aliases: vec!["first".into(), "alias".into()],
            });
        assert_eq!(range_union(&symbols), 40);
    }
    #[test]
    fn contained_range_cannot_shrink_the_union_frontier() {
        let symbols = [(100, 50), (110, 5), (140, 20)].map(|(address, bytes)| CodeSymbol {
            address,
            bytes,
            aliases: Vec::new(),
        });
        assert_eq!(range_union(&symbols), 60);
    }
}
