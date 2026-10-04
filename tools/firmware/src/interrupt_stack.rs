//! The interrupt-stack gate: each hart's worst-case interrupt stack, bounded
//! from the image's machine code, its interrupt table and the chip's ROM ELF
//! (`oer-riscv-stack`), must leave the platform's margin below the usable
//! interrupt stack. An unknown bound fails, naming what is unresolved.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use oer_esp32s31_platform_layout::interrupts as contract;
use oer_riscv_stack::{
    Analysis, Assumption, Dwarf, Field, HartStack, Stacks, TableLayout, TypeFacts, address_taken,
    analyze, function_pointer_resolutions, functions, interrupt_table, parse_summaries,
    taken_addresses, trap_entry, vector_table, waker_resolutions, waker_vtables,
};

use crate::Result;

/// The chip's vendor manifest, which pins the ROM ELF.
const ARTIFACTS: &str = "verification/esp32s31/artifacts.toml";
/// The reviewed summaries of ROM functions.
const ROM_SUMMARIES: &str = "platform/esp32s31/linker/rom/functions.toml";

/// Overrides the host-wide store of fetched vendor artifacts.
pub const VENDOR_STORE_ENV: &str = "OER_VENDOR_CACHE";

/// The host-wide store of fetched vendor artifacts, which every checkout and
/// source snapshot shares: `OER_VENDOR_CACHE`, else
/// `$XDG_CACHE_HOME/open-esp-radio/vendor`, else `~/.cache/open-esp-radio/vendor`.
pub fn vendor_store() -> Result<PathBuf> {
    match std::env::var_os(VENDOR_STORE_ENV).filter(|value| !value.is_empty()) {
        Some(store) => Ok(PathBuf::from(store)),
        None => Ok(std::env::var_os("XDG_CACHE_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
            .ok_or("HOME is required to locate the vendor artifact store")?
            .join("open-esp-radio/vendor")),
    }
}

/// The pinned ROM ELF of the repository at `root` in the vendor store,
/// checked against its pin.
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
    let elf = vendor_store()?.join(&source_id).join(&revision).join(&path);
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
    /// The ROM summaries the image's analysis applied: those of the ROM
    /// functions it reaches.
    pub summaries: BTreeSet<String>,
    /// Each table entry's vector-slot symbol and the functions it calls
    /// itself (the handler, unless inlined into it).
    pub handlers: Vec<(u32, Vec<u32>)>,
    /// Function names by address, for the report.
    names: BTreeMap<u32, String>,
}

/// The assumptions the gate admits in a bound, each by name: a bound resting
/// on one is conditional and passes with a warning; any other fails. Each
/// leaves this list in the pull request that proves it (#119).
const ADMITTED: &[Assumption] = &[Assumption::TableLevels, Assumption::ExecutorInvariant];

/// What an image's interrupt stacks must reach: the gate's policy, apart
/// from the analysis, which always reports all it can.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Required {
    /// Every hart's bound is proven: product images.
    Proven,
    /// A hart may be `partial + ?` with its holes named, as long as the part
    /// it proves fits: images whose observers the product does not carry.
    Partial,
}

impl InterruptStacks {
    /// The gate under `required`: every entry's handler runs from SRAM, and
    /// each hart's bound, or under [`Required::Partial`] its proven part, fits
    /// the usable interrupt stack with the margin. Returns a warning for each
    /// hart left `partial + ?`.
    pub fn check(&self, required: Required) -> Result<Vec<String>> {
        let in_sram = |address: u32| {
            oer_esp32s31_platform_layout::memory::SRAM
                .contains_range(u64::from(address), u64::from(address) + 4)
        };
        for (slot, calls) in &self.handlers {
            if let Some(outside) = std::iter::once(slot)
                .chain(calls)
                .find(|address| !in_sram(**address))
            {
                return Err(format!(
                    "the interrupt handler `{}` runs {} from outside SRAM",
                    self.name(*slot),
                    self.name(*outside)
                )
                .into());
            }
        }
        let mut warnings = Vec::new();
        for hart in &self.harts {
            // The sum over levels holds only where nothing lowers the level.
            let drops = hart.level_drops();
            if !drops.is_empty() {
                match required {
                    Required::Proven => {
                        return Err(format!(
                            "hart {}'s interrupt stack is conditional: its handlers can lower \
                             the interrupt level, so one level may nest in itself:\n{}",
                            hart.core,
                            self.render()
                        )
                        .into());
                    }
                    Required::Partial => warnings.push(format!(
                        "hart {}'s interrupt stack is conditional on the nesting rule, which {} \
                         sites may break: they are in the report",
                        hart.core,
                        drops.len()
                    )),
                }
            }
            let assumptions = hart.assumptions();
            if let Some(assumption) = assumptions
                .iter()
                .find(|assumption| !ADMITTED.contains(assumption))
            {
                return Err(format!(
                    "hart {}'s interrupt stack assumes {assumption}, which the gate does not \
                     admit:\n{}",
                    hart.core,
                    self.render()
                )
                .into());
            }
            for assumption in &assumptions {
                warnings.push(format!(
                    "hart {}'s interrupt stack is conditional on {assumption}",
                    hart.core
                ));
            }
            let bytes = match (hart.bytes, required) {
                (Some(bytes), _) => bytes,
                (None, Required::Proven) => {
                    return Err(format!(
                        "hart {}'s interrupt stack has no bound:\n{}",
                        hart.core,
                        self.render()
                    )
                    .into());
                }
                (None, Required::Partial) => {
                    warnings.push(format!(
                        "hart {}'s interrupt stack is {} + ? bytes: its holes are in the report",
                        hart.core, hart.partial
                    ));
                    hart.partial
                }
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
        Ok(warnings)
    }

    fn name(&self, address: u32) -> String {
        self.names
            .get(&address)
            .cloned()
            .unwrap_or_else(|| format!("{address:#010x}"))
    }

    /// Each hart's bound, its levels and their critical paths.
    pub fn render(&self) -> String {
        let name = |address: u32| self.name(address);
        let mut out = String::new();
        for hart in &self.harts {
            let _ = writeln!(
                out,
                "hart {}: {} of {} usable bytes, {}",
                hart.core,
                hart.bytes
                    .map_or(format!("{} + ?", hart.partial), |bytes| bytes.to_string()),
                contract::IRQ_STACK_USABLE_BYTES,
                match hart.bytes {
                    None => "partial",
                    Some(_) if hart.assumptions().is_empty() && hart.level_drops().is_empty() => {
                        "proven"
                    }
                    Some(_) => "conditional",
                }
            );
            for assumption in hart.assumptions() {
                let _ = writeln!(out, "  assumes {assumption}");
            }
            let drops = hart.level_drops();
            if drops.is_empty() {
                let _ = writeln!(
                    out,
                    "  nesting: checked, no handler can lower the interrupt level"
                );
            } else {
                let _ = writeln!(
                    out,
                    "  nesting: conditional, a level may nest in itself where a handler lowers \
                     the interrupt level:"
                );
                for (site, drop) in drops {
                    let _ = writeln!(out, "    {drop} at {site:#010x}");
                }
            }
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
                        .map_or(format!("{} + ?", level.partial), |bytes| bytes.to_string()),
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
/// The names of the reviewed ROM summaries.
pub fn rom_summaries(root: &Path) -> Result<Vec<String>> {
    let summaries = parse_summaries(&std::fs::read_to_string(root.join(ROM_SUMMARIES))?)?;
    Ok(summaries.into_iter().map(|summary| summary.name).collect())
}

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
    // Calls through a static's function pointer reach the taken functions of
    // its type: the diagnostic observers' `OnceCell<fn(..)>`.
    let types = TypeFacts::read(&elf)?;
    let taken = taken_addresses(&elf)?;
    resolutions.extend(function_pointer_resolutions(&analysis, &types, &taken));
    // The handler a slot calls is a direct call of the slot's own code: a
    // call site no inlined function owns. Calls from a handler inlined into
    // the slot are the handler's.
    let handlers = table
        .iter()
        .filter_map(|entry| entry.handler)
        .map(|slot| {
            let calls = analysis
                .functions
                .get(&slot)
                .map(|facts| {
                    facts
                        .transfers
                        .iter()
                        .filter_map(|transfer| {
                            let target = transfer.target?;
                            let own = dwarf
                                .inline_chain(transfer.site)
                                .map_or(true, |chain| chain.len() <= 1);
                            own.then_some(target)
                        })
                        .collect()
                })
                .unwrap_or_default();
            (slot, calls)
        })
        .collect();
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
    Ok(InterruptStacks {
        harts,
        handlers,
        summaries: analysis.summaries.clone(),
        names,
    })
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
                    resolutions.add(
                        transfer.site,
                        oer_riscv_stack::Fact::IpcPosts,
                        targets.iter().copied(),
                    );
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

    fn stacks(handlers: Vec<(u32, Vec<u32>)>) -> InterruptStacks {
        InterruptStacks {
            harts: Vec::new(),
            summaries: BTreeSet::new(),
            handlers,
            names: BTreeMap::from([(0x2f00_1000, "TIMER".to_owned())]),
        }
    }

    fn partial(partial: u64) -> InterruptStacks {
        let bound = oer_riscv_stack::Bound {
            root: 0,
            bytes: None,
            partial,
            unresolved: Vec::new(),
            path: Vec::new(),
            level_drops: Vec::new(),
            assumptions: BTreeSet::new(),
        };
        let level = oer_riscv_stack::LevelStack {
            level: 0,
            bytes: None,
            partial,
            frame: 0,
            bound,
        };
        InterruptStacks {
            harts: vec![HartStack {
                core: 0,
                levels: Vec::new(),
                exception: level,
                bytes: None,
                partial,
            }],
            summaries: BTreeSet::new(),
            handlers: Vec::new(),
            names: BTreeMap::new(),
        }
    }

    #[test]
    fn a_handler_called_from_outside_sram_fails_the_gate() {
        let sram = oer_esp32s31_platform_layout::memory::SRAM.origin + 0x1000;
        let psram = oer_esp32s31_platform_layout::memory::PSRAM.origin;
        // A slot whose handler was inlined into it calls nothing itself.
        assert!(
            stacks(vec![(sram, Vec::new())])
                .check(Required::Proven)
                .is_ok()
        );
        assert!(
            stacks(vec![(sram, vec![sram + 0x40])])
                .check(Required::Proven)
                .is_ok()
        );
        let error = stacks(vec![(sram, vec![psram])])
            .check(Required::Proven)
            .unwrap_err()
            .to_string();
        assert!(error.contains("`TIMER`"), "{error}");
        assert!(
            stacks(vec![(psram, Vec::new())])
                .check(Required::Proven)
                .is_err()
        );
    }

    #[test]
    fn a_partial_bound_passes_only_where_the_policy_allows_it_and_its_part_fits() {
        assert!(partial(1000).check(Required::Proven).is_err());
        let warnings = partial(1000).check(Required::Partial).unwrap();
        assert_eq!(warnings.len(), 2);
        assert!(
            warnings.iter().any(|warning| warning.contains("1000 + ?")),
            "{warnings:?}"
        );
        // The proven part alone must fit the usable stack with the margin.
        let over = u64::from(contract::IRQ_STACK_USABLE_BYTES);
        assert!(partial(over).check(Required::Partial).is_err());
    }

    #[test]
    fn a_bound_on_an_admitted_assumption_passes_conditional() {
        // Every hart assumes the table levels, which no check covers yet.
        let mut stacks = partial(1000);
        stacks.harts[0].bytes = Some(1000);
        assert!(stacks.render().contains("1000 of"));
        assert!(stacks.render().contains("conditional"));
        assert!(stacks.render().contains("assumes the table levels"));
        stacks.harts[0]
            .exception
            .bound
            .assumptions
            .insert(Assumption::ExecutorInvariant);
        // The gate names each in a warning and the report.
        let warnings = stacks.check(Required::Proven).unwrap();
        assert_eq!(warnings.len(), 2);
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("executor invariant")),
            "{warnings:?}"
        );
        assert!(stacks.render().contains("assumes the executor invariant"));
    }

    #[test]
    fn a_handler_that_can_lower_the_level_leaves_its_hart_conditional() {
        // A bound with no instruction that lowers the level is checked.
        let mut stacks = partial(1000);
        stacks.harts[0].bytes = Some(1000);
        assert!(stacks.check(Required::Proven).is_ok());
        assert!(stacks.render().contains("nesting: checked"));
        // One such instruction breaks the sum over levels: no proof, and a
        // warning under the partial policy.
        stacks.harts[0].exception.bound.level_drops =
            vec![(0x2f00_1000, oer_riscv_stack::LevelDrop::Return)];
        let error = stacks.check(Required::Proven).unwrap_err().to_string();
        assert!(error.contains("conditional"), "{error}");
        let warnings = stacks.check(Required::Partial).unwrap();
        assert!(warnings[0].contains("nesting rule"), "{warnings:?}");
        assert!(stacks.render().contains("a trap return at 0x2f001000"));
    }

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
