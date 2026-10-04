//! The facts of one crate, written as `<crate>-<hash>.json`.
//!
//! Every function is named by its mangled symbol, as the ELF names it. A
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
    /// Function-pointer types a value reaches by a transmute: their calls
    /// may reach any function.
    pub polluted: BTreeSet<String>,
    /// Each trait's vtable entries, by entry index, by symbol.
    pub vtables: BTreeMap<String, BTreeMap<usize, BTreeSet<String>>>,
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
    use super::fn_pointer_key;

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
