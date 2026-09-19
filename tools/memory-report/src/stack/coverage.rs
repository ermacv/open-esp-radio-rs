//! Linked symbol inventory, independent of frame-size policy. A Rust symbol
//! is an observable naming fact, not proof that a function contains no assembly.
//! Absolute symbols (including ROM declarations) are reported separately from
//! linked code; neither they nor unclassified code receive invented frame sizes.

use super::*;
use object::SectionKind;

/// Observable symbol origin, not a compiler or assembly coverage exemption.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum StackCoverageOrigin {
    LinkedRustSymbol,
    LinkedOtherText,
    AbsoluteText,
    OtherTextDefinition,
}

/// Metadata coverage of linked text address groups, independent of budgets.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum StackCoverageStatus {
    Complete,
    Incomplete,
    Unavailable,
}

/// Defined text aliases at one address without a measured frame.
#[derive(Clone, Debug, Serialize)]
pub struct StackCoverageFunction {
    pub address: u64,
    pub functions: Vec<String>,
    pub origin: StackCoverageOrigin,
}

/// Coverage refers only to defined linked text symbols, grouped by address.
/// Aliases are counted once. Stripped symbols, inline code and indirect call
/// chains prevent this inventory from proving complete runtime stack usage.
#[derive(Clone, Debug, Serialize)]
pub struct StackCoverage {
    pub linked_text_status: StackCoverageStatus,
    pub linked_text_addresses: usize,
    pub measured_linked_text_addresses: usize,
    pub unmeasured_functions: Vec<StackCoverageFunction>,
    pub metadata_without_text_symbol: Vec<u64>,
}

pub(super) fn analyze(elf: &object::File<'_>, frames: &BTreeMap<u64, StackFrame>) -> StackCoverage {
    let mut functions = BTreeMap::<u64, StackCoverageFunction>::new();
    for symbol in elf.symbols() {
        if (!symbol.is_definition() && symbol.section() != object::SymbolSection::Absolute)
            || symbol.kind() != SymbolKind::Text
            || symbol.address() == 0
        {
            continue;
        }
        let raw = symbol.name().unwrap_or("<unnamed text symbol>");
        let rust = try_demangle(raw).ok();
        let linked = symbol
            .section_index()
            .and_then(|index| elf.section_by_index(index).ok())
            .is_some_and(|section| section.kind() == SectionKind::Text);
        let origin = if linked {
            if rust.is_some() {
                StackCoverageOrigin::LinkedRustSymbol
            } else {
                StackCoverageOrigin::LinkedOtherText
            }
        } else if symbol.section() == object::SymbolSection::Absolute {
            StackCoverageOrigin::AbsoluteText
        } else {
            StackCoverageOrigin::OtherTextDefinition
        };
        let entry = functions
            .entry(symbol.address())
            .or_insert_with(|| StackCoverageFunction {
                address: symbol.address(),
                functions: Vec::new(),
                origin,
            });
        // A linked alias is stronger evidence than an absolute declaration at
        // that address; one demangled alias identifies the group as Rust.
        if origin_priority(origin) > origin_priority(entry.origin) {
            entry.origin = origin;
        }
        entry.functions.push(
            rust.map(|name| name.to_string())
                .unwrap_or_else(|| raw.to_owned()),
        );
    }
    let mut result = StackCoverage {
        linked_text_status: StackCoverageStatus::Unavailable,
        linked_text_addresses: 0,
        measured_linked_text_addresses: 0,
        metadata_without_text_symbol: frames
            .keys()
            .filter(|address| !functions.contains_key(address))
            .copied()
            .collect(),
        unmeasured_functions: Vec::new(),
    };
    for (_, mut function) in functions {
        function.functions.sort();
        function.functions.dedup();
        if matches!(
            function.origin,
            StackCoverageOrigin::LinkedRustSymbol | StackCoverageOrigin::LinkedOtherText
        ) {
            result.linked_text_addresses += 1;
            result.measured_linked_text_addresses +=
                usize::from(frames.contains_key(&function.address));
        }
        if !frames.contains_key(&function.address) {
            result.unmeasured_functions.push(function);
        }
    }
    if result.linked_text_addresses != 0 {
        result.linked_text_status =
            if result.linked_text_addresses == result.measured_linked_text_addresses {
                StackCoverageStatus::Complete
            } else {
                StackCoverageStatus::Incomplete
            };
    }
    result
}

fn origin_priority(origin: StackCoverageOrigin) -> u8 {
    match origin {
        StackCoverageOrigin::AbsoluteText => 0,
        StackCoverageOrigin::OtherTextDefinition => 1,
        StackCoverageOrigin::LinkedOtherText => 2,
        StackCoverageOrigin::LinkedRustSymbol => 3,
    }
}
