//! PHY archive policy over compiled symbols. ELF is read natively; LLVM
//! bitcode members use the llvm-nm shipped with the active Rust toolchain.

use crate::{Context, Result, process};
use object::{Object, ObjectSymbol, SymbolKind};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[cfg(test)]
mod tests;

#[derive(Default)]
struct Symbols {
    defined: BTreeSet<String>,
    undefined: BTreeSet<String>,
}

pub(super) fn audit_phy(ctx: &Context, path: &Path) -> Result<()> {
    let bytes = std::fs::read(path)
        .map_err(|error| format!("cannot read PHY archive {}: {error}", path.display()))?;
    let symbols = archive_symbols(ctx, &bytes)?;
    check_symbols(&symbols)?;
    println!("source-only PHY archive symbols passed: {}", path.display());
    Ok(())
}

fn archive_symbols(ctx: &Context, bytes: &[u8]) -> Result<Symbols> {
    let archive = object::read::archive::ArchiveFile::parse(bytes)?;
    if archive.is_thin() {
        return Err(
            "PHY archive must contain its own members; thin archives are unsupported".into(),
        );
    }
    let mut symbols = Symbols::default();
    let mut code_members = 0;
    let mut llvm_nm = None;
    let temporary = tempfile::tempdir()?;
    for (index, member) in archive.members().enumerate() {
        let member = member?;
        let name = std::str::from_utf8(member.name())?;
        let data = member.data(bytes)?;
        let metadata = matches!(name, "lib.rmeta" | "lib.rmeta-link");
        if data.starts_with(b"\x7fELF") {
            elf_symbols(data, &mut symbols, !metadata)
                .map_err(|error| format!("invalid ELF member {name}: {error}"))?;
        } else if is_bitcode(data) {
            let tool = match &llvm_nm {
                Some(tool) => tool,
                None => llvm_nm.insert(matched_llvm_nm(ctx)?),
            };
            // Never interpret archive names as extraction paths.
            let input = temporary.path().join(format!("member-{index}.bc"));
            std::fs::write(&input, data)?;
            bitcode_symbols(ctx, tool, &input, &mut symbols)
                .map_err(|error| format!("cannot inspect bitcode member {name}: {error}"))?;
        } else {
            return Err(format!(
                "unrecognized PHY archive member {name}; no symbols were assumed absent"
            )
            .into());
        }
        if !metadata {
            code_members += 1;
        }
    }
    if code_members == 0 {
        return Err("PHY archive contains no compiled code members".into());
    }
    Ok(symbols)
}

fn elf_symbols(bytes: &[u8], symbols: &mut Symbols, require_table: bool) -> Result<()> {
    let file = object::File::parse(bytes)?;
    if file.kind() != object::ObjectKind::Relocatable {
        return Err("expected a relocatable ELF archive member".into());
    }
    if require_table && file.symbol_table().is_none() {
        return Err(
            "compiled ELF member has no symbol table; external references cannot be audited".into(),
        );
    }
    for symbol in file.symbols() {
        if matches!(symbol.kind(), SymbolKind::File | SymbolKind::Section) {
            continue;
        }
        let name = symbol.name()?;
        if name.is_empty() {
            continue;
        }
        if symbol.is_undefined() {
            symbols.undefined.insert(name.to_owned());
        } else {
            symbols.defined.insert(name.to_owned());
        }
    }
    Ok(())
}

fn is_bitcode(bytes: &[u8]) -> bool {
    // LLVM raw bitcode and the documented bitcode wrapper magic.
    bytes.starts_with(b"BC\xc0\xde") || bytes.starts_with(&[0xde, 0xc0, 0x17, 0x0b])
}

fn matched_llvm_nm(ctx: &Context) -> Result<PathBuf> {
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let sysroot = process::capture(ctx.command(&rustc).args(["--print", "sysroot"]))?;
    let sysroot = String::from_utf8(sysroot.stdout)?;
    let version = process::capture(ctx.command(&rustc).arg("-vV"))?;
    let version = String::from_utf8(version.stdout)?;
    let host = required_field(&version, "host: ")?;
    let tool = Path::new(sysroot.trim())
        .join("lib/rustlib")
        .join(host)
        .join("bin")
        .join(format!("llvm-nm{}", std::env::consts::EXE_SUFFIX));
    if !tool.is_file() {
        return Err(format!(
            "active Rust toolchain lacks {}; install its llvm-tools-preview component to inspect LLVM bitcode (PATH llvm-nm is not a compatible substitute)",
            tool.display()
        ).into());
    }
    Ok(tool)
}

fn required_field<'a>(text: &'a str, prefix: &str) -> Result<&'a str> {
    let values: Vec<_> = text
        .lines()
        .filter_map(|line| line.strip_prefix(prefix))
        .collect();
    match values.as_slice() {
        [value] if !value.is_empty() => Ok(value),
        _ => Err(format!("rustc -vV must identify exactly one {prefix}field").into()),
    }
}

fn bitcode_symbols(ctx: &Context, tool: &Path, input: &Path, symbols: &mut Symbols) -> Result<()> {
    for (mode, destination) in [
        ("--defined-only", &mut symbols.defined),
        ("--undefined-only", &mut symbols.undefined),
    ] {
        let output = process::capture(
            ctx.command(tool)
                .args([mode, "--just-symbol-name", "--no-demangle"])
                .arg(input),
        )?;
        // llvm-nm can emit a diagnostic even with a successful status. Do not
        // accept incomplete observations or silently discard unknown warnings.
        if !output.stderr.is_empty() {
            return Err(format!(
                "llvm-nm reported: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        parse_nm_names(&String::from_utf8(output.stdout)?, destination)?;
    }
    Ok(())
}

fn parse_nm_names(text: &str, destination: &mut BTreeSet<String>) -> Result<()> {
    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        if line.chars().any(char::is_whitespace) || line.ends_with(':') {
            return Err(format!("malformed llvm-nm symbol record: {line:?}").into());
        }
        destination.insert(line.to_owned());
    }
    Ok(())
}

fn check_symbols(symbols: &Symbols) -> Result<()> {
    for raw in symbols.defined.union(&symbols.undefined) {
        if forbidden_radio_symbol(raw) {
            return Err(
                format!("radio ROM/vendor ABI symbol survived source-only build: {raw}").into(),
            );
        }
    }
    for raw in symbols.undefined.difference(&symbols.defined) {
        let demangled = format!("{:#}", rustc_demangle::demangle(raw));
        if !allowed_external(&demangled) {
            return Err(format!(
                "unexpected external symbol in source-only radio rlib: {demangled}"
            )
            .into());
        }
    }
    Ok(())
}

fn source_namespace(symbol: &str) -> bool {
    [
        "open_esp_radio_esp32s31_hal::",
        "open_esp_radio_esp32s31_pac::",
        "open_esp_radio_esp32s31_pac_raw::",
        "core::fmt::",
    ]
    .iter()
    .any(|prefix| symbol.starts_with(prefix))
}

fn allowed_external(symbol: &str) -> bool {
    if source_namespace(symbol)
        || matches!(
            symbol,
            "__divdi3" | "__udivdi3" | "memcmp" | "memcpy" | "memmove" | "memset"
        )
    {
        return true;
    }
    if let Some(subject) = symbol.strip_prefix('<') {
        let subject = subject
            .split_once(">::")
            .map_or(subject, |(subject, _)| subject);
        let trait_name = subject
            .rsplit_once(" as ")
            .map_or(subject, |(_, name)| name);
        if source_namespace(subject) || source_namespace(trait_name) {
            return true;
        }
    }
    symbol.starts_with("core::")
        && symbol
            .rsplit("::")
            .next()
            .is_some_and(|leaf| leaf.starts_with("panic") || leaf.starts_with("len_mismatch_fail"))
}

fn forbidden_radio_symbol(symbol: &str) -> bool {
    matches!(
        symbol,
        "phy_wifi_get_tx_gain" | "register_chipv7_phy" | "g_phyFuns" | "phy_param"
    ) || ["esp_wifi_", "pp_", "net80211_"]
        .iter()
        .any(|prefix| symbol.starts_with(prefix))
}
