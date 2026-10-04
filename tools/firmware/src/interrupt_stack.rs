//! The interrupt-stack gate: each hart's worst-case interrupt stack, bounded
//! from the image's machine code, its interrupt table and the chip's ROM ELF
//! (`oer-riscv-stack`), must leave the platform's margin below the usable
//! interrupt stack. An unknown bound fails, naming what is unresolved.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use oer_esp32s31_platform_layout::interrupts as contract;
use oer_riscv_stack::{
    Analysis, Dwarf, Field, HartStack, Stacks, TableLayout, address_taken, analyze, functions,
    interrupt_table, parse_summaries, trap_entry, vector_table, waker_resolutions, waker_vtables,
};

use crate::Result;

/// The chip's vendor manifest, which pins the ROM ELF.
const ARTIFACTS: &str = "verification/esp32s31/artifacts.toml";
/// The reviewed summaries of ROM functions.
const ROM_SUMMARIES: &str = "platform/esp32s31/linker/rom/functions.toml";

/// The pinned ROM ELF in `root`'s vendor store, checked against its pin.
pub fn rom_elf(root: &Path) -> Result<PathBuf> {
    use sha2::Digest as _;
    let manifest: toml::Table = toml::from_str(&std::fs::read_to_string(root.join(ARTIFACTS))?)?;
    let tables = |key: &str| -> Vec<toml::Table> {
        manifest
            .get(key)
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|value| value.as_table().cloned())
            .collect()
    };
    let text = |table: &toml::Table, key: &str| {
        table
            .get(key)
            .and_then(toml::Value::as_str)
            .map(str::to_owned)
    };
    let artifact = tables("artifact")
        .into_iter()
        .find(|artifact| text(artifact, "id").as_deref() == Some("rom"))
        .ok_or("artifacts.toml pins no `rom` artifact")?;
    let source_id = text(&artifact, "source").ok_or("the `rom` artifact names no source")?;
    let source = tables("source")
        .into_iter()
        .find(|source| text(source, "id").as_deref() == Some(source_id.as_str()))
        .ok_or("the `rom` artifact's source is not pinned")?;
    let revision = text(&source, "revision").ok_or("the ROM's source pins no revision")?;
    let path = text(&artifact, "path").ok_or("the `rom` artifact has no path")?;
    let sha256 = text(&artifact, "sha256").ok_or("the `rom` artifact has no sha256")?;
    let elf = root
        .join("target/vendor")
        .join(&source_id)
        .join(&revision)
        .join(&path);
    let bytes = std::fs::read(&elf).map_err(|_| {
        format!(
            "the pinned ROM ELF {} is missing: run `cargo xtask vendor-fetch esp32s31 --artifact rom`",
            elf.display()
        )
    })?;
    let digest = hex(&sha2::Sha256::digest(&bytes));
    if digest != sha256 {
        return Err(format!("{} differs from its pin {sha256}", elf.display()).into());
    }
    Ok(elf)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

/// Each hart's interrupt stack of the runtime ELF `elf`.
pub struct InterruptStacks {
    pub harts: Vec<HartStack>,
    /// Function names by address, for the report.
    names: BTreeMap<u32, String>,
}

impl InterruptStacks {
    /// The gate: every hart's bound is known and, with the margin, fits the
    /// usable interrupt stack.
    pub fn check(&self) -> Result<()> {
        for hart in &self.harts {
            let Some(bytes) = hart.bytes else {
                return Err(format!(
                    "hart {}'s interrupt stack has no bound:\n{}",
                    hart.core,
                    self.render()
                )
                .into());
            };
            if with_margin(bytes) > u64::from(contract::IRQ_STACK_USABLE_BYTES) {
                return Err(format!(
                    "hart {}'s interrupt stack needs {bytes} bytes and a {}% margin, more than \
                     its usable {} bytes:\n{}",
                    hart.core,
                    contract::IRQ_STACK_MARGIN_PERCENT,
                    contract::IRQ_STACK_USABLE_BYTES,
                    self.render()
                )
                .into());
            }
        }
        Ok(())
    }

    /// Each hart's bound, its levels and their critical paths.
    pub fn render(&self) -> String {
        let name = |address: u32| {
            self.names
                .get(&address)
                .cloned()
                .unwrap_or_else(|| format!("{address:#010x}"))
        };
        let mut out = String::new();
        for hart in &self.harts {
            let _ = writeln!(
                out,
                "hart {}: {} of {} usable bytes",
                hart.core,
                hart.bytes
                    .map_or("unknown".to_owned(), |bytes| bytes.to_string()),
                contract::IRQ_STACK_USABLE_BYTES
            );
            for level in hart.levels.iter().chain([&hart.exception]) {
                let label = if level.level == 0 {
                    "exception".to_owned()
                } else {
                    format!("level {}", level.level)
                };
                let _ = writeln!(
                    out,
                    "  {label}: {} (entry frame {})",
                    level
                        .bytes
                        .map_or("unknown".to_owned(), |bytes| bytes.to_string()),
                    level.frame
                );
                for (function, frame) in &level.bound.path {
                    let _ = writeln!(out, "    {frame:>6} {}", name(*function));
                }
                for (site, reason) in &level.bound.unresolved {
                    let _ = writeln!(out, "    unresolved {reason} at {site:#010x}");
                }
            }
        }
        out
    }
}

/// `bytes` with the platform's margin.
fn with_margin(bytes: u64) -> u64 {
    bytes + bytes * u64::from(contract::IRQ_STACK_MARGIN_PERCENT) / 100
}

/// Bound every hart's interrupt stack of the runtime ELF `elf`, with the
/// pinned ROM of the repository at `root`.
pub fn interrupt_stacks(root: &Path, elf: &Path) -> Result<InterruptStacks> {
    let elf = std::fs::read(elf)?;
    let rom = std::fs::read(rom_elf(root)?)?;
    let summaries = parse_summaries(&std::fs::read_to_string(root.join(ROM_SUMMARIES))?)?;
    let analysis = analyze(&elf, &[&rom], &summaries)?;
    let functions = functions(&elf)?;
    let mut names = BTreeMap::new();
    for function in &functions {
        if let Some(name) = function.names.first() {
            names.insert(
                function.address,
                format!("{:#}", rustc_demangle::demangle(name)),
            );
        }
    }
    let address_of = |path: &str| -> Vec<u32> {
        names
            .iter()
            .filter(|(_, name)| name.as_str() == path)
            .map(|(&address, _)| address)
            .collect()
    };
    let symbol = |name: &str| -> Result<u32> {
        functions
            .iter()
            .find(|function| function.names.iter().any(|candidate| candidate == name))
            .map(|function| function.address)
            .ok_or_else(|| format!("the runtime has no `{name}`").into())
    };
    let field = |(offset, size): (u32, u32)| Field { offset, size };
    let table = interrupt_table(
        &elf,
        contract::TABLE_SYMBOL,
        &TableLayout {
            entry: contract::TABLE_ENTRY_BYTES,
            source: field(contract::TABLE_SOURCE),
            level: field(contract::TABLE_LEVEL),
            core: field(contract::TABLE_CORE),
            handler: contract::TABLE_HANDLER,
        },
    )?;
    let vectors = vector_table(&elf, contract::VECTOR_TABLE_SYMBOL)?
        .into_iter()
        .flatten()
        .map(|entry| trap_entry(&elf, &functions, entry))
        .collect::<oer_riscv_model::Result<Vec<_>>>()?;
    let exception = trap_entry(&elf, &functions, symbol(contract::EXCEPTION_ENTRY_SYMBOL)?)?;
    let dwarf = Dwarf::read(&elf)?;
    let vtables = waker_vtables(&elf, &dwarf, &analysis)?;
    let mut resolutions = waker_resolutions(&analysis, &dwarf, &vtables)?;
    resolutions.extend(ipc_resolutions(&elf, &analysis, &address_of)?);
    let sources = source_table(&elf)?;
    let harts = oer_riscv_stack::interrupt_stacks(
        &analysis,
        &Stacks {
            table: &table,
            cores: &contract::HARTS,
            always: &contract::ALWAYS_LEVELS,
            vectors: &vectors,
            exception,
            sources,
            resolutions: &resolutions,
        },
    )?;
    Ok(InterruptStacks { harts, names })
}

/// The targets of the IPC dispatch's call of the posted callback: the
/// `handler` argument of every direct call of the only function that posts
/// one; none when the image posts none.
fn ipc_resolutions(
    elf: &[u8],
    analysis: &Analysis,
    address_of: &dyn Fn(&str) -> Vec<u32>,
) -> Result<oer_riscv_stack::Resolutions> {
    let mut targets = BTreeSet::new();
    for post in address_of(contract::IPC_POST) {
        if address_taken(elf, post)? {
            return Err(format!("`{}` is called through a pointer", contract::IPC_POST).into());
        }
        targets.extend(analysis.constant_arguments(post, contract::IPC_POST_HANDLER_ARGUMENT)?);
    }
    let mut resolutions = oer_riscv_stack::Resolutions::new();
    for dispatch in contract::IPC_DISPATCH {
        for function in address_of(dispatch) {
            for transfer in &analysis.functions[&function].transfers {
                if transfer.target.is_none() {
                    resolutions.insert(transfer.site, targets.clone());
                }
            }
        }
    }
    Ok(resolutions)
}

/// The address of esp-hal's per-source handler table.
fn source_table(elf: &[u8]) -> Result<u32> {
    use object::{Object, ObjectSymbol};
    let file = object::File::parse(elf)?;
    file.symbols()
        .find(|symbol| symbol.name() == Ok(contract::SOURCE_TABLE_SYMBOL))
        .map(|symbol| symbol.address() as u32)
        .ok_or_else(|| format!("the runtime has no `{}`", contract::SOURCE_TABLE_SYMBOL).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_margin_is_a_share_of_the_bound() {
        assert_eq!(
            with_margin(1000),
            1000 + 1000 * u64::from(contract::IRQ_STACK_MARGIN_PERCENT) / 100
        );
    }

    #[test]
    fn the_rom_pin_resolves_into_the_vendor_store() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        match rom_elf(&root) {
            Ok(path) => assert!(path.ends_with("esp32s31_rev0_rom.elf")),
            // A checkout without the vendor store names the fetch command.
            Err(error) => assert!(
                error
                    .to_string()
                    .contains("vendor-fetch esp32s31 --artifact rom")
            ),
        }
    }
}
