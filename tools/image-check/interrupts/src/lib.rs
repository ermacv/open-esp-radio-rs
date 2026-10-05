//! The interrupt-stack gate: each hart's worst-case interrupt stack, bounded
//! from the image's machine code, its interrupt table and the chip's ROM ELF
//! (`oer-riscv-stack`), must leave the platform's margin below the usable
//! interrupt stack. An unknown bound fails, naming what is unresolved.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

use oer_riscv_stack::{
    Analysis, Dwarf, Field, HartStack, Stacks, TableLayout, interrupt_table, parse_summaries,
    trap_entry, vector_table,
};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

use oer_image_check_stack::{ADMITTED, Image, contract::Contract};

/// The interrupt stacks' report in an image build's output directory.
pub const INTERRUPT_REPORT: &str = "interrupt-stack.txt";

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
    /// Copies of a CLIC level or route writer outside the functions it may
    /// run inside (the contract's `level-writers`), or a writer of which the
    /// DWARF shows no copy: where a source may take a level other than its
    /// table entry's. Empty when the table levels are checked.
    pub level_writers: Vec<String>,
    /// What the interrupt handlers' code does that interrupt context should
    /// not, over what is resolved.
    pub handler_code: HandlerCode,
    /// The chip's address map and interrupt contract the bounds are held to.
    pub contract: Contract,
}

/// What the code every interrupt level reaches (its handlers and what they
/// call, over what is resolved; the exception's panic path apart) does that
/// interrupt context should not. A hole may hide more; what is listed is
/// there.
#[derive(Debug, Default)]
pub struct HandlerCode {
    /// Floating-point instructions and floating-point CSR accesses, by
    /// function and site: the handlers run with `mstatus.FS` off, so each
    /// raises an illegal-instruction exception.
    pub float: Vec<(u32, u32)>,
    /// Calls into the panic machinery (`core::panicking`), by caller and
    /// callee: an interrupt that can panic.
    pub panics: Vec<(u32, u32)>,
    /// Functions in a cached region (flash or PSRAM): code that cannot run
    /// while the cache is off.
    pub cached: Vec<u32>,
}

/// Sites of each kind the report lists.
const SHOWN: usize = 10;

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
            self.contract
                .memory
                .sram
                .contains_range(u64::from(address), u64::from(address) + 4)
        };
        // A floating-point instruction in a handler traps: the handlers run
        // with the FPU off.
        if let Some((function, site)) = self.handler_code.float.first() {
            return Err(format!(
                "interrupt context runs floating point at {site:#010x} in {}, with the FPU off",
                self.name(*function)
            )
            .into());
        }
        // The sum over levels needs each source at its table level.
        if !self.level_writers.is_empty() {
            return Err(format!(
                "the table levels are not checked: {}",
                self.level_writers.join("; ")
            )
            .into());
        }
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
            let percent = self.contract.interrupts.irq_stack_margin_percent;
            let usable = self.contract.irq_stack_usable_bytes();
            if with_margin(bytes, percent) > u64::from(usable) {
                return Err(format!(
                    "hart {}'s interrupt stack needs {bytes} bytes and a {}% margin, more than \
                     its usable {} bytes:\n{}",
                    hart.core,
                    percent,
                    usable,
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
        if self.level_writers.is_empty() {
            let _ = writeln!(
                out,
                "levels: checked, every CLIC level and route writer runs in start-up or the table"
            );
        } else {
            let _ = writeln!(out, "levels: not checked");
            for problem in &self.level_writers {
                let _ = writeln!(out, "  {problem}");
            }
        }
        let code = &self.handler_code;
        let _ = writeln!(
            out,
            "handler code: {} floating-point sites, {} calls into the panic machinery, {} \
             functions in cached memory",
            code.float.len(),
            code.panics.len(),
            code.cached.len()
        );
        for (function, site) in code.float.iter().take(SHOWN) {
            let _ = writeln!(
                out,
                "  floating point at {site:#010x} in {}",
                name(*function)
            );
        }
        for (caller, callee) in code.panics.iter().take(SHOWN) {
            let _ = writeln!(out, "  {} calls {}", name(*caller), name(*callee));
        }
        for function in code.cached.iter().take(SHOWN) {
            let _ = writeln!(out, "  in cached memory: {}", name(*function));
        }
        for hart in &self.harts {
            let _ = writeln!(
                out,
                "hart {}: {} of {} usable bytes, {}",
                hart.core,
                hart.bytes
                    .map_or(format!("{} + ?", hart.partial), |bytes| bytes.to_string()),
                self.contract.irq_stack_usable_bytes(),
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
fn with_margin(bytes: u64, percent: u32) -> u64 {
    bytes + bytes * u64::from(percent) / 100
}

/// The names of the reviewed ROM summaries of `profile`'s chip; none for a
/// chip that reviews none.
pub fn rom_summaries(root: &Path, profile: &oer_chip_profile::Profile) -> Result<Vec<String>> {
    let Some(path) = profile.rom.as_ref().and_then(|rom| rom.summaries.as_ref()) else {
        return Ok(Vec::new());
    };
    let summaries = parse_summaries(&std::fs::read_to_string(root.join(path))?)?;
    Ok(summaries.into_iter().map(|summary| summary.name).collect())
}

/// Bound every hart's interrupt stack of the runtime ELF `elf`, with the
/// pinned ROM of the repository at `root`.
pub fn interrupt_stacks(
    root: &Path,
    profile: &oer_chip_profile::Profile,
    elf: &Path,
) -> Result<InterruptStacks> {
    interrupt_stacks_of(&Image::read(root, profile, elf)?)
}

/// [`interrupt_stacks`] of an analysed runtime image.
pub fn interrupt_stacks_of(image: &Image) -> Result<InterruptStacks> {
    let Image {
        elf,
        analysis,
        functions,
        names,
        dwarf,
        resolutions,
        contract,
    } = image;
    let contract = contract
        .as_ref()
        .ok_or("the image's chip names no [memory] map or [interrupts] contract")?;
    let interrupts = &contract.interrupts;
    let symbol = |name: &str| -> Result<u32> {
        image
            .symbol(name)
            .ok_or_else(|| format!("the runtime has no `{name}`").into())
    };
    let field = |(offset, size): (u32, u32)| Field { offset, size };
    let table = interrupt_table(
        elf,
        &interrupts.table_symbol,
        &TableLayout {
            entry: interrupts.table_entry_bytes,
            source: field(interrupts.table_source.into()),
            level: field(interrupts.table_level.into()),
            core: field(interrupts.table_core.into()),
            handler: interrupts.table_handler,
        },
    )?;
    let vectors = vector_table(elf, &interrupts.vector_table_symbol)?
        .into_iter()
        .flatten()
        .map(|entry| trap_entry(elf, functions, entry))
        .collect::<oer_riscv_model::Result<Vec<_>>>()?;
    let exception = trap_entry(elf, functions, symbol(&interrupts.exception_entry_symbol)?)?;
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
    let level_writers = level_writers(elf, dwarf, interrupts)?;

    let sources = source_table(elf, &interrupts.source_table_symbol)?;
    let harts = oer_riscv_stack::interrupt_stacks(
        analysis,
        &Stacks {
            table: &table,
            cores: &interrupts.harts,
            always: &interrupts.always_levels,
            vectors: &vectors,
            exception,
            sources,
            resolutions,
        },
    )?;
    let reached: BTreeSet<u32> = harts
        .iter()
        .flat_map(|hart| hart.levels.iter())
        .flat_map(|level| level.bound.reached.iter().copied())
        .collect();
    let handler_code = handler_code(analysis, &reached, names, &contract.memory);
    Ok(InterruptStacks {
        harts,
        handlers,
        summaries: analysis.summaries.clone(),
        names: names.clone(),
        level_writers,
        handler_code,
        contract: contract.clone(),
    })
}

/// What the functions in `reached` do that interrupt context should not.
fn handler_code(
    analysis: &Analysis,
    reached: &BTreeSet<u32>,
    names: &BTreeMap<u32, String>,
    memory: &oer_chip_profile::Memory,
) -> HandlerCode {
    let panicking = |address: u32| {
        names
            .get(&address)
            .is_some_and(|name| name.starts_with("core::panicking::"))
    };
    let mut code = HandlerCode::default();
    for &function in reached {
        let Some(facts) = analysis.functions.get(&function) else {
            continue;
        };
        code.float
            .extend(facts.float_sites.iter().map(|&site| (function, site)));
        if !panicking(function) {
            code.panics.extend(
                facts
                    .transfers
                    .iter()
                    .filter_map(|transfer| transfer.target)
                    .filter(|&target| panicking(target))
                    .map(|target| (function, target)),
            );
        }
        let at = (u64::from(function), u64::from(function) + 4);
        if memory
            .cached()
            .iter()
            .any(|region| region.contains_range(at.0, at.1))
        {
            code.cached.push(function);
        }
    }
    code.panics.sort_unstable();
    code.panics.dedup();
    code
}

/// Where a CLIC level or route writer runs outside the functions it may run
/// inside, or a writer of which the DWARF shows no copy. Each copy's inline
/// chain must hold one of the writer's contexts.
fn level_writers(
    elf: &[u8],
    dwarf: &Dwarf,
    interrupts: &oer_chip_profile::InterruptContract,
) -> Result<Vec<String>> {
    let writers: Vec<&str> = interrupts
        .level_writers
        .iter()
        .map(|writer| writer.writer.as_str())
        .collect();
    let copies = oer_riscv_stack::instances(elf, &writers)?;
    let mut problems = Vec::new();
    for level_writer in &interrupts.level_writers {
        let (writer, contexts) = (&level_writer.writer, &level_writer.contexts);
        let starts = &copies[writer.as_str()];
        if starts.is_empty() {
            problems.push(format!("the DWARF shows no copy of `{writer}`"));
        }
        for &start in starts {
            let chain = dwarf.inline_chain(start)?;
            if !chain.iter().any(|function| contexts.contains(function)) {
                problems.push(format!(
                    "`{writer}` at {start:#010x} runs inside {}",
                    chain.last().map_or("no function", String::as_str)
                ));
            }
        }
    }
    Ok(problems)
}

/// The address of esp-hal's per-source handler table.
fn source_table(elf: &[u8], symbol: &str) -> Result<u32> {
    oer_elf::Elf::parse(elf)?
        .address(symbol)
        .map(|address| address as u32)
        .ok_or_else(|| format!("the runtime has no `{symbol}`").into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_riscv_stack::Assumption;

    fn contract() -> Contract {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        oer_chip_profile::Profile::all(&root)
            .unwrap()
            .iter()
            .find_map(Contract::of)
            .expect("a chip names an interrupt contract")
    }

    /// The `TIMER` slot's handler: in the contract's SRAM.
    fn timer() -> u32 {
        contract().memory.sram.origin + 0x1000
    }

    fn stacks(handlers: Vec<(u32, Vec<u32>)>) -> InterruptStacks {
        InterruptStacks {
            harts: Vec::new(),
            summaries: BTreeSet::new(),
            handlers,
            names: BTreeMap::from([(timer(), "TIMER".to_owned())]),
            level_writers: Vec::new(),
            handler_code: HandlerCode::default(),
            contract: contract(),
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
            reached: BTreeSet::new(),
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
            level_writers: Vec::new(),
            handler_code: HandlerCode::default(),
            contract: contract(),
        }
    }

    #[test]
    fn a_handler_called_from_outside_sram_fails_the_gate() {
        let sram = timer();
        let cached = contract().memory.flash_xip.origin;
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
        let error = stacks(vec![(sram, vec![cached])])
            .check(Required::Proven)
            .unwrap_err()
            .to_string();
        assert!(error.contains("`TIMER`"), "{error}");
        assert!(
            stacks(vec![(cached, Vec::new())])
                .check(Required::Proven)
                .is_err()
        );
    }

    #[test]
    fn a_partial_bound_passes_only_where_the_policy_allows_it_and_its_part_fits() {
        assert!(partial(1000).check(Required::Proven).is_err());
        let warnings = partial(1000).check(Required::Partial).unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings.iter().any(|warning| warning.contains("1000 + ?")),
            "{warnings:?}"
        );
        // The proven part alone must fit the usable stack with the margin.
        let over = u64::from(contract().irq_stack_usable_bytes());
        assert!(partial(over).check(Required::Partial).is_err());
    }

    #[test]
    fn a_bound_on_an_admitted_assumption_passes_conditional() {
        let mut stacks = partial(1000);
        stacks.harts[0].bytes = Some(1000);
        assert!(stacks.render().contains(", proven"));
        stacks.harts[0]
            .exception
            .bound
            .assumptions
            .insert(Assumption::ExecutorInvariant);
        // The gate names it in a warning and the report.
        assert!(stacks.render().contains("conditional"));
        let warnings = stacks.check(Required::Proven).unwrap();
        assert_eq!(warnings.len(), 1);
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
    fn a_level_writer_outside_its_contexts_fails_every_policy() {
        let mut stacks = partial(1000);
        stacks.harts[0].bytes = Some(1000);
        assert!(stacks.render().contains("levels: checked"));
        stacks.level_writers =
            vec!["`esp_hal::interrupt::map_raw` at 0x2f001000 runs inside main".into()];
        for required in [Required::Proven, Required::Partial] {
            let error = stacks.check(required).unwrap_err().to_string();
            assert!(error.contains("table levels are not checked"), "{error}");
        }
        assert!(stacks.render().contains("levels: not checked"));
    }

    #[test]
    fn floating_point_in_a_handler_fails_and_the_rest_is_reported() {
        let mut stacks = partial(1000);
        stacks.harts[0].bytes = Some(1000);
        stacks.handler_code.panics = vec![(0x2f00_1000, 0x4000_0000)];
        stacks.handler_code.cached = vec![0x4000_0000];
        assert!(stacks.check(Required::Proven).is_ok());
        let report = stacks.render();
        assert!(
            report.contains("1 calls into the panic machinery"),
            "{report}"
        );
        assert!(report.contains("1 functions in cached memory"), "{report}");
        stacks.handler_code.float = vec![(0x2f00_1000, 0x2f00_1004)];
        let error = stacks.check(Required::Partial).unwrap_err().to_string();
        assert!(error.contains("floating point at 0x2f001004"), "{error}");
    }

    #[test]
    fn the_margin_is_a_share_of_the_bound() {
        assert_eq!(
            with_margin(1000, contract().interrupts.irq_stack_margin_percent),
            1000 + 1000 * u64::from(contract().interrupts.irq_stack_margin_percent) / 100
        );
    }
}
