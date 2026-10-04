//! The facts of one crate, written as `<crate>-<hash>.json`.
//!
//! Every function is named by [`function_key`]: its symbol demangled,
//! without crate hashes, so that a precompiled crate's functions match the
//! facts of the same crate compiled apart from its sources. A
//! function-pointer type is named by [`fn_pointer_key`]: its ABI, inputs and
//! output as rustc prints them, without lifetimes or safety, which codegen
//! erases.
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// One indirect call of an instance's MIR.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IndirectCall {
    /// A call through a function pointer of this type.
    FnPointer(String),
    /// A call through a `dyn` vtable: its principal trait and entry index.
    Dyn { r#trait: String, entry: usize },
    /// An indirect call the MIR does not name a target type for: the
    /// instance's indirect sites cannot be resolved by its facts.
    Unknown,
}

#[derive(Debug, Default, Serialize)]
pub struct Facts {
    pub schema: u32,
    pub krate: String,
    /// Each instance with indirect calls, by symbol.
    pub calls: BTreeMap<String, BTreeSet<IndirectCall>>,
    /// The functions made function pointers of each type, by symbol.
    pub fn_pointers: BTreeMap<String, BTreeSet<String>>,
    /// Function-pointer types whose values leave the type somewhere: cast
    /// to a pointer or an integer, transmuted to something else, a union's
    /// field, or read through a pointer of another type. Every function made
    /// a pointer of such a type is a candidate of every site of its ABI.
    pub leaked_types: BTreeSet<String>,
    /// Functions a constant holds where its type gives no function-pointer
    /// type, by the key of their own signature's pointer type.
    pub leaked_functions: BTreeMap<String, String>,
    /// Transmutes between function-pointer types: each target's sources.
    pub edges: BTreeMap<String, BTreeSet<String>>,
    /// Principal traits of `dyn` values that leave their type: every
    /// function of their vtables, and everything their implementors carry
    /// (`trait_contents`), leaks.
    pub leaked_traits: BTreeSet<String>,
    /// The function-pointer type each vtable function is called as.
    pub signatures: BTreeMap<String, String>,
    /// What every type made a `dyn` of each trait carries.
    pub trait_contents: BTreeMap<String, crate::leaks::Contents>,
    /// A value whose contents cannot be enumerated left its type: every
    /// function made a pointer is a candidate of every site of its ABI.
    pub unknown_leak: bool,
    /// Each trait's vtable entries, by entry index, by symbol.
    pub vtables: BTreeMap<String, BTreeMap<usize, BTreeSet<String>>>,
}

/// The key of a function's mangled `symbol`: demangled without the crate
/// hashes a compilation gives it. Two functions of one path (two versions
/// of a crate) share a key, which only unites their facts.
pub fn function_key(symbol: &str) -> String {
    let demangled = match rustc_demangle::try_demangle(symbol) {
        Ok(demangled) => format!("{demangled:#}"),
        Err(_) => symbol.to_owned(),
    };
    let mut key = String::with_capacity(demangled.len());
    let mut depth = 0_u32;
    for c in demangled.chars() {
        match c {
            '[' => depth += 1,
            ']' => depth = depth.saturating_sub(1),
            _ if depth == 0 => key.push(c),
            _ => {}
        }
    }
    key
}

/// The key of a function-pointer type printed `ty`: without lifetimes,
/// higher-ranked binders or `unsafe`.
pub fn fn_pointer_key(printed: &str) -> String {
    let mut out = String::with_capacity(printed.len());
    let mut chars = printed.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\'' {
            // A lifetime: skip its name and the space after a `&'a `.
            while chars
                .peek()
                .is_some_and(|c| c.is_alphanumeric() || *c == '_')
            {
                chars.next();
            }
            if chars.peek() == Some(&' ') {
                chars.next();
            }
            continue;
        }
        out.push(c);
    }
    let mut key = out.replace("unsafe ", "");
    // `for<> fn(..)` once its lifetimes are gone.
    while let Some(at) = key.find("for<") {
        let Some(close) = key[at..].find("> ") else {
            break;
        };
        key.replace_range(at..at + close + 2, "");
    }
    key.replace("<, ", "<")
        .replace(", >", ">")
        .replace("<>", "")
}

pub fn write(directory: &Path, facts: &Facts) -> std::io::Result<()> {
    std::fs::create_dir_all(directory)?;
    let text = serde_json::to_string(facts).map_err(std::io::Error::other)?;
    // Two compilations of one crate (a test harness, a check) differ in
    // their arguments; the hash of the facts keeps both.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
    }
    std::fs::write(
        directory.join(format!("{}-{hash:016x}.json", facts.krate)),
        text,
    )
}

#[cfg(test)]
mod tests {
    use super::{fn_pointer_key, function_key};

    #[test]
    fn a_function_key_drops_the_crate_hashes() {
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

    #[test]
    fn a_key_drops_lifetimes_binders_and_safety() {
        assert_eq!(
            fn_pointer_key("for<'a> fn(&'a u8, &'a mut core::fmt::Formatter<'_>) -> u32"),
            "fn(&u8, &mut core::fmt::Formatter) -> u32"
        );
        assert_eq!(
            fn_pointer_key("unsafe extern \"C\" fn(u32)"),
            "extern \"C\" fn(u32)"
        );
        assert_eq!(fn_pointer_key("fn(u32) -> u32"), "fn(u32) -> u32");
    }
}
