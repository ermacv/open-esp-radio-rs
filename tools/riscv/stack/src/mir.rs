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
//! candidates: the functions made pointers of each call's type and of every
//! type transmuted into it, each call's vtable entry over every type the
//! image makes a `dyn` of the trait, and every leaked function whose ABI fits
//! the call. An instance without facts, or with a call the MIR names no
//! target type for, leaves the site unresolved.
//!
//! A function leaves the candidates of another type's sites only when no
//! address-taking of its type loses the type: the functions of a leaked
//! function-pointer type (and of every type transmuted into one), the
//! functions a constant holds untyped, and every vtable function of a leaked
//! trait are candidates of every site whose ABI they fit. Calling a function
//! through a pointer of another calling convention or argument count is
//! undefined behavior, so those two are what fitting means; a vtable
//! function, whose signature the facts do not give, fits every site. A site
//! of a leaked trait's `dyn` also reaches the same entry of every leaked
//! trait's vtables and every leaked function. A leak whose contents the
//! driver could not enumerate leaks every function made a pointer and every
//! vtable function.
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

/// What the types made a `dyn` of a trait carry.
#[derive(Debug, Default, Deserialize)]
struct Contents {
    keys: BTreeSet<String>,
    traits: BTreeSet<String>,
    unknown: bool,
}

#[derive(Debug, Default, Deserialize)]
struct CrateFacts {
    schema: u32,
    calls: BTreeMap<String, BTreeSet<IndirectCall>>,
    fn_pointers: BTreeMap<String, BTreeSet<String>>,
    vtables: BTreeMap<String, BTreeMap<usize, BTreeSet<String>>>,
    leaked_types: BTreeSet<String>,
    leaked_functions: BTreeMap<String, String>,
    edges: BTreeMap<String, BTreeSet<String>>,
    leaked_traits: BTreeSet<String>,
    trait_contents: BTreeMap<String, Contents>,
    unknown_leak: bool,
}

/// The calling convention and argument count of a function-pointer type's
/// key (`extern "C" fn(u8, bool) -> u32`); `None` when the key does not
/// parse, which fits every site.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Signature<'a> {
    abi: &'a str,
    arguments: usize,
}

fn signature(key: &str) -> Option<Signature<'_>> {
    let (abi, rest) = match key.strip_prefix("extern \"") {
        Some(rest) => {
            let (abi, rest) = rest.split_once("\" ")?;
            (abi, rest)
        }
        None => ("Rust", key),
    };
    let inputs = rest.strip_prefix("fn(")?;
    let (mut depth, mut arguments, mut empty) = (0usize, 0usize, true);
    let mut characters = inputs.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '-' if characters.peek() == Some(&'>') => {
                characters.next();
            }
            '(' | '[' | '<' | '{' => depth += 1,
            ')' if depth == 0 => {
                return Some(Signature {
                    abi,
                    arguments: if empty { 0 } else { arguments + 1 },
                });
            }
            ')' | ']' | '>' | '}' => depth = depth.checked_sub(1)?,
            ',' if depth == 0 => arguments += 1,
            _ => {}
        }
        if !character.is_whitespace() {
            empty = false;
        }
    }
    None
}

/// Whether a leaked function of signature `function` may be called through
/// a pointer of signature `site`.
fn fits(site: Option<Signature>, function: Option<Signature>) -> bool {
    match (site, function) {
        (Some(site), Some(function)) => site == function,
        _ => true,
    }
}

/// The facts of every crate of an image, united.
#[derive(Debug, Default)]
pub struct MirFacts {
    calls: BTreeMap<String, BTreeSet<IndirectCall>>,
    fn_pointers: BTreeMap<String, BTreeSet<String>>,
    vtables: BTreeMap<String, BTreeMap<usize, BTreeSet<String>>>,
    leaked_types: BTreeSet<String>,
    leaked_functions: BTreeMap<String, String>,
    edges: BTreeMap<String, BTreeSet<String>>,
    leaked_traits: BTreeSet<String>,
    trait_contents: BTreeMap<String, Contents>,
    unknown_leak: bool,
    /// Every leaked function with the key of the pointer type it leaked as
    /// (`None`: a vtable function); computed once every crate is read.
    leaked: BTreeSet<(Option<String>, String)>,
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
        facts.close_leaks();
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
        facts.close_leaks();
        Ok(facts)
    }

    fn add(&mut self, crate_facts: CrateFacts) -> Result<()> {
        if crate_facts.schema != 2 {
            return Err(invalid(format!(
                "MIR facts of schema {}, this analysis reads 2",
                crate_facts.schema
            )));
        }
        for (symbol, calls) in crate_facts.calls {
            self.calls.entry(symbol).or_default().extend(calls);
        }
        for (key, functions) in crate_facts.fn_pointers {
            self.fn_pointers.entry(key).or_default().extend(functions);
        }
        for (name, entries) in crate_facts.vtables {
            let into = self.vtables.entry(name).or_default();
            for (entry, functions) in entries {
                into.entry(entry).or_default().extend(functions);
            }
        }
        self.leaked_types.extend(crate_facts.leaked_types);
        self.leaked_functions.extend(crate_facts.leaked_functions);
        for (target, sources) in crate_facts.edges {
            self.edges.entry(target).or_default().extend(sources);
        }
        self.leaked_traits.extend(crate_facts.leaked_traits);
        for (name, contents) in crate_facts.trait_contents {
            let into = self.trait_contents.entry(name).or_default();
            into.keys.extend(contents.keys);
            into.traits.extend(contents.traits);
            into.unknown |= contents.unknown;
        }
        self.unknown_leak |= crate_facts.unknown_leak;
        Ok(())
    }

    /// The leaks of every crate closed over each other: a leaked trait's
    /// implementors' contents leak, and a type transmuted into a leaked type
    /// leaks with it.
    fn close_leaks(&mut self) {
        let mut traits: Vec<String> = self.leaked_traits.iter().cloned().collect();
        while let Some(name) = traits.pop() {
            if let Some(contents) = self.trait_contents.get(&name) {
                self.leaked_types.extend(contents.keys.iter().cloned());
                self.unknown_leak |= contents.unknown;
                for nested in &contents.traits {
                    if self.leaked_traits.insert(nested.clone()) {
                        traits.push(nested.clone());
                    }
                }
            }
        }
        let mut types: Vec<String> = self.leaked_types.iter().cloned().collect();
        while let Some(key) = types.pop() {
            for source in self.edges.get(&key).into_iter().flatten() {
                if self.leaked_types.insert(source.clone()) {
                    types.push(source.clone());
                }
            }
        }
        let mut leaked = BTreeSet::new();
        for (key, functions) in &self.fn_pointers {
            if self.unknown_leak || self.leaked_types.contains(key) {
                leaked.extend(functions.iter().map(|f| (Some(key.clone()), f.clone())));
            }
        }
        for (function, key) in &self.leaked_functions {
            leaked.insert((Some(key.clone()), function.clone()));
        }
        for (name, entries) in &self.vtables {
            if self.unknown_leak || self.leaked_traits.contains(name) {
                leaked.extend(entries.values().flatten().map(|f| (None, f.clone())));
            }
        }
        self.leaked = leaked;
    }

    /// The function-pointer types whose functions a site of type `key`
    /// reaches: itself and every type transmuted into it.
    fn reaching<'a>(&'a self, key: &'a str) -> BTreeSet<&'a str> {
        let mut types = BTreeSet::from([key]);
        let mut work = vec![key];
        while let Some(key) = work.pop() {
            for source in self.edges.get(key).into_iter().flatten() {
                if types.insert(source) {
                    work.push(source);
                }
            }
        }
        types
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
                    for reaching in self.reaching(key) {
                        candidates.extend(
                            self.fn_pointers
                                .get(reaching)
                                .into_iter()
                                .flatten()
                                .map(String::as_str),
                        );
                    }
                    let site = signature(key);
                    candidates.extend(
                        self.leaked
                            .iter()
                            .filter(|(leaked, _)| fits(site, leaked.as_deref().and_then(signature)))
                            .map(|(_, function)| function.as_str()),
                    );
                }
                IndirectCall::Dyn { r#trait, entry } => {
                    candidates.extend(
                        self.vtables
                            .get(r#trait)
                            .and_then(|entries| entries.get(entry))
                            .into_iter()
                            .flatten()
                            .map(String::as_str),
                    );
                    // A leaked `dyn`'s vtable pointer may be any leaked
                    // memory's.
                    if self.unknown_leak || self.leaked_traits.contains(r#trait) {
                        for (name, entries) in &self.vtables {
                            if self.unknown_leak || self.leaked_traits.contains(name) {
                                candidates.extend(
                                    entries.get(entry).into_iter().flatten().map(String::as_str),
                                );
                            }
                        }
                        candidates.extend(self.leaked.iter().map(|(_, f)| f.as_str()));
                    }
                }
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
    use super::{Signature, fits, function_key, signature};

    /// A key's calling convention and argument count, through nested
    /// function types, generics and tuples.
    #[test]
    fn a_signature_counts_the_top_level_arguments() {
        let rust = |arguments| {
            Some(Signature {
                abi: "Rust",
                arguments,
            })
        };
        assert_eq!(signature("fn()"), rust(0));
        assert_eq!(signature("fn() -> u32"), rust(0));
        assert_eq!(signature("fn(u8, bool) -> u32"), rust(2));
        assert_eq!(
            signature("fn(fn(u8, u8) -> u8, core::option::Option<(u8, u16)>, [u8; 4])"),
            rust(3)
        );
        assert_eq!(
            signature("extern \"C\" fn(u32)"),
            Some(Signature {
                abi: "C",
                arguments: 1
            })
        );
        assert_eq!(signature("not a type"), None);
        // A Rust function does not fit a C site, nor one of another arity;
        // an unknown signature fits every site.
        assert!(!fits(
            signature("extern \"C\" fn(u32)"),
            signature("fn(u32)")
        ));
        assert!(!fits(signature("fn(u32)"), signature("fn(u32, u32)")));
        assert!(fits(signature("fn(u32) -> u8"), signature("fn(i8) -> u64")));
        assert!(fits(signature("fn(u32)"), None));
    }

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
