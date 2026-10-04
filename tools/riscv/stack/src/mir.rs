//! Indirect sites resolved by the facts `oer-mir-facts` records from each
//! crate's MIR (`tools/mir-facts`): which functions a call through a
//! function pointer of a type, or through an entry of a trait's vtables, can
//! reach.
//!
//! Functions are named by [`function_key`], their symbol demangled without
//! crate hashes, as the facts name them: the precompiled `core` then matches
//! the facts of `core` compiled apart from the toolchain's sources. A site's
//! MIR instance is the innermost function the DWARF inlines at it, by its
//! linkage name. The site is
//! one of that instance's indirect calls, so it reaches the union of their
//! candidates: the functions made pointers of each call's type, and each
//! call's vtable entry over every type the image makes a `dyn` of the trait.
//! An instance without facts, with a call the MIR names no target type for,
//! or with a call through a type a transmute produces, leaves the site
//! unresolved.
use crate::{Analysis, Dwarf, Fact, Resolutions, TransferKind};
use object::{Object, ObjectSymbol};
use oer_riscv_model::{Error, ErrorCode, Result};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(rename_all = "snake_case")]
enum IndirectCall {
    FnPointer(String),
    Dyn { r#trait: String, entry: usize },
    Unknown,
}

#[derive(Debug, Default, Deserialize)]
struct CrateFacts {
    schema: u32,
    calls: BTreeMap<String, BTreeSet<IndirectCall>>,
    fn_pointers: BTreeMap<String, BTreeSet<String>>,
    polluted: BTreeSet<String>,
    vtables: BTreeMap<String, BTreeMap<usize, BTreeSet<String>>>,
}

/// The facts of every crate of an image, united.
#[derive(Debug, Default)]
pub struct MirFacts {
    calls: BTreeMap<String, BTreeSet<IndirectCall>>,
    fn_pointers: BTreeMap<String, BTreeSet<String>>,
    polluted: BTreeSet<String>,
    vtables: BTreeMap<String, BTreeMap<usize, BTreeSet<String>>>,
}

/// The key of a function's mangled `symbol`: demangled without the crate
/// hashes a compilation gives it, as `oer-mir-facts` names functions.
pub fn function_key(symbol: &str) -> String {
    crate::dwarf::plain(&addr2line::demangle_auto(symbol.into(), None))
}

fn invalid(message: String) -> Error {
    Error::new(ErrorCode::Integrity, message)
}

impl MirFacts {
    /// Read every crate's facts file in each of `directories`: the image's
    /// and the toolchain library's.
    pub fn read(directories: &[&Path]) -> Result<Self> {
        let mut facts = Self::default();
        for directory in directories {
            facts.read_directory(directory)?;
        }
        Ok(facts)
    }

    fn read_directory(&mut self, directory: &Path) -> Result<()> {
        let facts = self;
        let entries = std::fs::read_dir(directory)
            .map_err(|error| invalid(format!("{}: {error}", directory.display())))?;
        for entry in entries {
            let path = entry
                .map_err(|error| invalid(format!("{}: {error}", directory.display())))?
                .path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            let text = std::fs::read(&path)
                .map_err(|error| invalid(format!("{}: {error}", path.display())))?;
            facts.add(
                serde_json::from_slice(&text)
                    .map_err(|error| invalid(format!("{}: {error}", path.display())))?,
            )?;
        }
        Ok(())
    }

    /// The facts of one crate's JSON text, for a test.
    pub fn from_json(texts: &[&str]) -> Result<Self> {
        let mut facts = Self::default();
        for text in texts {
            facts.add(
                serde_json::from_str(text).map_err(|error| invalid(format!("facts: {error}")))?,
            )?;
        }
        Ok(facts)
    }

    fn add(&mut self, crate_facts: CrateFacts) -> Result<()> {
        if crate_facts.schema != 1 {
            return Err(invalid(format!(
                "MIR facts of schema {}, this analysis reads 1",
                crate_facts.schema
            )));
        }
        for (symbol, calls) in crate_facts.calls {
            self.calls.entry(symbol).or_default().extend(calls);
        }
        for (key, functions) in crate_facts.fn_pointers {
            self.fn_pointers.entry(key).or_default().extend(functions);
        }
        self.polluted.extend(crate_facts.polluted);
        for (name, entries) in crate_facts.vtables {
            let into = self.vtables.entry(name).or_default();
            for (entry, functions) in entries {
                into.entry(entry).or_default().extend(functions);
            }
        }
        Ok(())
    }

    /// The symbols the indirect calls of `instance` can reach; `None` when
    /// its facts cannot bound them.
    fn candidates(&self, instance: &str) -> Option<BTreeSet<&str>> {
        let calls = self.calls.get(instance)?;
        let mut candidates = BTreeSet::new();
        for call in calls {
            match call {
                IndirectCall::Unknown => return None,
                IndirectCall::FnPointer(key) => {
                    if self.polluted.contains(key) {
                        return None;
                    }
                    candidates.extend(
                        self.fn_pointers
                            .get(key)
                            .into_iter()
                            .flatten()
                            .map(String::as_str),
                    );
                }
                IndirectCall::Dyn { r#trait, entry } => candidates.extend(
                    self.vtables
                        .get(r#trait)
                        .and_then(|entries| entries.get(entry))
                        .into_iter()
                        .flatten()
                        .map(String::as_str),
                ),
            }
        }
        Some(candidates)
    }
}

/// The targets the MIR facts give the indirect sites the analysis left
/// unresolved in `elf`.
pub fn mir_resolutions(
    elf: &[u8],
    analysis: &Analysis,
    dwarf: &Dwarf,
    facts: &MirFacts,
) -> Result<Resolutions> {
    let file = object::File::parse(elf).map_err(|_| invalid("invalid ELF".into()))?;
    // Functions by key: a key may name several (two versions of a crate).
    let mut addresses: BTreeMap<String, BTreeSet<u32>> = BTreeMap::new();
    for symbol in file.symbols() {
        if symbol.kind() == object::SymbolKind::Text
            && let (Ok(name), Ok(address)) = (symbol.name(), u32::try_from(symbol.address()))
        {
            addresses
                .entry(function_key(name))
                .or_default()
                .insert(address & !1);
        }
    }
    let mut resolutions = Resolutions::new();
    for facts_of in analysis.functions.values() {
        for transfer in &facts_of.transfers {
            if transfer.target.is_some()
                || !matches!(transfer.kind, TransferKind::Call | TransferKind::Tail)
            {
                continue;
            }
            let Some(instance) = dwarf.innermost_symbol(transfer.site)? else {
                continue;
            };
            let Some(candidates) = facts.candidates(&function_key(&instance)) else {
                continue;
            };
            // A candidate the image does not link is never called.
            resolutions.add(
                transfer.site,
                Fact::Mir,
                candidates
                    .into_iter()
                    .filter_map(|key| addresses.get(key))
                    .flatten()
                    .copied(),
            );
        }
    }
    Ok(resolutions)
}

#[cfg(test)]
mod tests {
    use super::function_key;

    /// The keys `oer-mir-facts` gives the same symbols.
    #[test]
    fn a_function_key_drops_the_crate_hashes_as_the_facts_do() {
        assert_eq!(
            function_key("_RNvCs4YwXaCc3kOe_6sample8call_dyn"),
            "sample::call_dyn"
        );
        assert_eq!(
            function_key("_ZN4core3fmt5write17h0123456789abcdefE"),
            "core::fmt::write"
        );
        assert_eq!(function_key("_start"), "_start");
    }
}
