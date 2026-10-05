//! The stack gate of an image build. One analysis of each ELF
//! (`oer-riscv-stack`, with the pinned ROM ELF and its reviewed summaries)
//! bounds every stack the image runs on:
//!
//! - the runtime's interrupt stacks ([`crate::interrupt_stack`]);
//! - each task stack the policy names, from the function that runs on it
//!   (its root, by symbol): the root's worst-case call chain must leave the
//!   stack's storage the policy's minimum free bytes;
//! - the bootstrap image's stack, from its root likewise.
//!
//! A task stack's bound may be `partial + ?`: the root reaches indirect sites
//! no fact resolves (an executor's task polls, trait objects). The gate then
//! accepts it, holds the part it proves to the budget and lists every
//! unresolved site with its reason; the runtime's stack painting stays the
//! check of the exercised call chains. A root symbol the image lacks fails,
//! as does an image without compiler frame records or a linked function
//! without a record that the coverage review does not explain.

mod coverage;
mod policy;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

use oer_esp32s31_platform_layout::interrupts as contract;
use oer_riscv_stack::{
    Analysis, Bound, Dwarf, Function, Reason, Resolutions, Summary, TypeFacts, address_taken,
    analyze, function_pointer_resolutions, functions, parse_summaries, stack_sizes,
    taken_addresses, waker_resolutions, waker_vtables,
};

pub use coverage::{Coverage, CoverageCategory, CoveragePolicy, CoverageReview};
pub use policy::{InterruptStacks, SCHEMA, StackPolicy, Stacks, Storage, TaskStack};

use crate::Result;
use crate::interrupt_stack::{self, ADMITTED, ROM_SUMMARIES, Required};

/// The interrupt stacks' report in an image build's output directory.
pub const INTERRUPT_REPORT: &str = "interrupt-stack.txt";
/// The runtime task stacks' report.
pub const RUNTIME_REPORT: &str = "runtime-stack.txt";
/// The bootstrap stack's report.
pub const BOOTSTRAP_REPORT: &str = "bootstrap-stack.txt";

/// What the runtime's stack gate passed with.
pub struct RuntimeAudit {
    /// The ROM summaries the runtime's analysis applied.
    pub summaries: BTreeSet<String>,
    /// Bounds that passed `partial + ?` or conditional on an assumption.
    pub warnings: Vec<String>,
}

/// Bound the runtime ELF `elf`'s interrupt and task stacks, write
/// [`INTERRUPT_REPORT`] and [`RUNTIME_REPORT`] into `output`, then check
/// them: the interrupt stacks under `required`, the task stacks and the
/// frame coverage under `policy`.
pub fn audit_runtime_stacks(
    root: &Path,
    elf: &Path,
    policy: &StackPolicy,
    required: Required,
    output: &Path,
) -> Result<RuntimeAudit> {
    let image = Image::read(root, elf)?;
    let interrupts = interrupt_stack::interrupt_stacks_of(&image)?;
    std::fs::write(output.join(INTERRUPT_REPORT), interrupts.render())?;
    let tasks = StackAudit::of(&image, policy, &policy.runtime_task_stacks())?;
    std::fs::write(output.join(RUNTIME_REPORT), tasks.render())?;
    let mut warnings = interrupts.check(required)?;
    warnings.extend(tasks.check()?);
    Ok(RuntimeAudit {
        summaries: interrupts.summaries.clone(),
        warnings,
    })
}

/// Bound the bootstrap ELF `elf`'s stack, write [`BOOTSTRAP_REPORT`] into
/// `output`, then check it under `policy`. Returns the warnings it passed
/// with.
pub fn audit_bootstrap_stack(
    root: &Path,
    elf: &Path,
    policy: &StackPolicy,
    output: &Path,
) -> Result<Vec<String>> {
    let image = Image::read(root, elf)?;
    let audit = StackAudit::of(
        &image,
        policy,
        &[("bootstrap stack", policy.stacks()?.bootstrap_stack)],
    )?;
    std::fs::write(output.join(BOOTSTRAP_REPORT), audit.render())?;
    audit.check()
}

/// One image's analysis with its pinned ROM, and the facts outside the
/// machine code every stack bound resolves indirect sites with.
pub(crate) struct Image {
    pub(crate) elf: Vec<u8>,
    pub(crate) analysis: Analysis,
    /// The image's own functions.
    pub(crate) functions: Vec<Function>,
    /// The image's function names, demangled, by address.
    pub(crate) names: BTreeMap<u32, String>,
    pub(crate) dwarf: Dwarf,
    pub(crate) resolutions: Resolutions,
}

impl Image {
    pub(crate) fn read(root: &Path, elf: &Path) -> Result<Self> {
        let elf = std::fs::read(elf)
            .map_err(|error| format!("cannot read {}: {error}", elf.display()))?;
        let rom = std::fs::read(oer_vendor_artifacts::fetched(
            root,
            crate::staged::CHIP,
            "rom",
        )?)?;
        let summaries = parse_summaries(&std::fs::read_to_string(root.join(ROM_SUMMARIES))?)?;
        Self::new(elf, &[&rom], &summaries)
    }

    /// The analysis of `elf` with its `companions` and their `summaries`.
    fn new(elf: Vec<u8>, companions: &[&[u8]], summaries: &[Summary]) -> Result<Self> {
        let analysis = analyze(&elf, companions, summaries)?;
        let functions = functions(&elf)?;
        let names = functions
            .iter()
            .filter_map(|function| {
                let name = function.names.first()?;
                Some((function.address, oer_elf::demangle(name)))
            })
            .collect();
        let dwarf = Dwarf::read(&elf)?;
        let mut image = Self {
            elf,
            analysis,
            functions,
            names,
            dwarf,
            resolutions: Resolutions::new(),
        };
        let vtables = waker_vtables(&image.elf, &image.dwarf, &image.analysis)?;
        let mut resolutions = waker_resolutions(&image.analysis, &image.dwarf, &vtables)?;
        resolutions.extend(image.ipc_resolutions()?);
        // Calls through a static's function pointer reach the taken functions
        // of its type: the diagnostic observers' `OnceCell<fn(..)>`.
        let types = TypeFacts::read(&image.elf)?;
        let taken = taken_addresses(&image.elf)?;
        resolutions.extend(function_pointer_resolutions(
            &image.analysis,
            &types,
            &taken,
        ));
        image.resolutions = resolutions;
        Ok(image)
    }

    /// The image's function that carries the symbol `name`.
    pub(crate) fn symbol(&self, name: &str) -> Option<u32> {
        self.functions
            .iter()
            .find(|function| function.names.iter().any(|candidate| candidate == name))
            .map(|function| function.address)
    }

    /// The image's functions whose demangled name is `path`.
    pub(crate) fn address_of(&self, path: &str) -> Vec<u32> {
        self.names
            .iter()
            .filter(|(_, name)| name.as_str() == path)
            .map(|(&address, _)| address)
            .collect()
    }

    /// The name of the function (of the image or a companion) that holds
    /// `address`.
    fn function_at(&self, address: u32) -> String {
        let Some((&start, facts)) = self.analysis.functions.range(..=address).next_back() else {
            return format!("{address:#010x}");
        };
        if address - start >= facts.function.size {
            return format!("{address:#010x}");
        }
        let name = self
            .names
            .get(&start)
            .cloned()
            .unwrap_or_else(|| oer_elf::demangle(&facts.function.label()));
        if address == start {
            name
        } else {
            format!("{name}+{:#x}", address - start)
        }
    }

    /// The capacity of `storage` in bytes, or `None` when the image links
    /// none of its symbols.
    fn capacity(&self, storage: Storage<'_>) -> Result<Option<u64>> {
        let file = oer_elf::Elf::parse(self.elf.as_slice())?;
        let defined = |name: &str| file.symbol(name);
        let address = |name: &str| file.address(name);
        match storage {
            Storage::Symbol(name) => match defined(name) {
                None => Ok(None),
                Some(symbol) if symbol.size == 0 => {
                    Err(format!("the stack storage `{name}` has no size").into())
                }
                Some(symbol) => Ok(Some(symbol.size)),
            },
            Storage::Range { bottom, top } => match (address(bottom), address(top)) {
                (None, None) => Ok(None),
                (Some(low), Some(high)) if high > low => Ok(Some(high - low)),
                (Some(low), Some(high)) => Err(format!(
                    "the stack `{bottom}`..`{top}` is empty or grows up: {low:#x}..{high:#x}"
                )
                .into()),
                _ => Err(format!("the image links only one of `{bottom}` and `{top}`").into()),
            },
        }
    }

    /// The targets of the IPC dispatch's call of the posted callback: the
    /// `handler` argument of every direct call of the only function that
    /// posts one; none when the image posts none.
    fn ipc_resolutions(&self) -> Result<Resolutions> {
        let mut targets = BTreeSet::new();
        for post in self.address_of(contract::IPC_POST) {
            if address_taken(&self.elf, post)? {
                return Err(format!("`{}` is called through a pointer", contract::IPC_POST).into());
            }
            targets.extend(
                self.analysis
                    .constant_arguments(post, contract::IPC_POST_HANDLER_ARGUMENT)?,
            );
        }
        let mut resolutions = Resolutions::new();
        for dispatch in contract::IPC_DISPATCH {
            for function in self.address_of(dispatch) {
                for transfer in &self.analysis.functions[&function].transfers {
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
}

/// The frame coverage and task stack bounds of one image.
#[derive(Debug)]
pub struct StackAudit {
    pub coverage: Coverage,
    pub stacks: Vec<StackBound>,
}

/// One stack's budget and its root's bound.
#[derive(Debug)]
pub struct StackBound {
    pub name: &'static str,
    pub root: String,
    /// The storage symbol, or the bottom and top symbols.
    pub storage: String,
    pub minimum_free_bytes: u64,
    /// The stack's bytes and its root's bound; `None` for an optional stack
    /// the image does not link.
    pub linked: Option<(u64, Bound)>,
    /// The deepest path, root first: each function's frame and name.
    pub path: Vec<(u64, String)>,
    /// Each unresolved site (or function) the root reaches: its reason, its
    /// address and the function that holds it.
    pub unresolved: Vec<(Reason, u32, String)>,
}

impl StackAudit {
    fn of(
        image: &Image,
        policy: &StackPolicy,
        stacks: &[(&'static str, &TaskStack)],
    ) -> Result<Self> {
        let coverage = CoveragePolicy::load(policy.stacks()?.coverage_policy)?
            .coverage(&image.functions, &stack_sizes(&image.elf)?)?;
        let mut bounds = Vec::new();
        for &(name, stack) in stacks {
            let storage = match stack.storage() {
                Storage::Symbol(symbol) => format!("`{symbol}`"),
                Storage::Range { bottom, top } => format!("`{bottom}`..`{top}`"),
            };
            let mut bound = StackBound {
                name,
                root: stack.root.clone(),
                storage,
                minimum_free_bytes: u64::from(stack.minimum_free_bytes),
                linked: None,
                path: Vec::new(),
                unresolved: Vec::new(),
            };
            let Some(capacity) = image.capacity(stack.storage())? else {
                if !stack.optional {
                    return Err(format!("the image links no {name} ({})", bound.storage).into());
                }
                bounds.push(bound);
                continue;
            };
            let root = image.symbol(&stack.root).ok_or_else(|| {
                format!(
                    "the image has no `{}`, the root the policy names for its {name}",
                    stack.root
                )
            })?;
            let walked = image.analysis.bound_with(root, &image.resolutions)?;
            bound.path = walked
                .path
                .iter()
                .map(|&(function, frame)| (frame, image.function_at(function)))
                .collect();
            bound.unresolved = walked
                .unresolved
                .iter()
                .map(|&(site, reason)| (reason, site, image.function_at(site)))
                .collect();
            bound.unresolved.sort();
            bound.linked = Some((capacity, walked));
            bounds.push(bound);
        }
        Ok(Self {
            coverage,
            stacks: bounds,
        })
    }

    /// The gate: full frame coverage, and each linked stack's bound, or the
    /// part a `partial + ?` bound proves, within its storage less its
    /// minimum free bytes, on no assumption the gate does not admit. Returns
    /// a warning for each bound left `partial + ?` or conditional.
    pub fn check(&self) -> Result<Vec<String>> {
        if !self.coverage.unreviewed.is_empty() {
            return Err(format!(
                "linked functions without a compiler frame record need exactly one coverage \
                 review:\n  {}",
                self.coverage.unreviewed.join("\n  ")
            )
            .into());
        }
        let mut warnings = Vec::new();
        for stack in &self.stacks {
            let Some((capacity, bound)) = &stack.linked else {
                continue;
            };
            let budget = capacity
                .checked_sub(stack.minimum_free_bytes)
                .filter(|budget| *budget != 0)
                .ok_or_else(|| {
                    format!(
                        "the {} has {capacity} bytes ({}) and cannot keep {} bytes free",
                        stack.name, stack.storage, stack.minimum_free_bytes
                    )
                })?;
            if let Some(assumption) = bound
                .assumptions
                .iter()
                .find(|assumption| !ADMITTED.contains(assumption))
            {
                return Err(format!(
                    "the {}'s bound assumes {assumption}, which the gate does not admit",
                    stack.name
                )
                .into());
            }
            for assumption in &bound.assumptions {
                warnings.push(format!(
                    "the {}'s bound is conditional on {assumption}",
                    stack.name
                ));
            }
            let (bytes, shown) = match bound.bytes {
                Some(bytes) => (bytes, bytes.to_string()),
                None => (bound.partial, format!("{} + ?", bound.partial)),
            };
            if bytes > budget {
                return Err(format!(
                    "the {} needs {shown} bytes from `{}`, more than its {budget}-byte budget \
                     ({capacity} bytes of {} less {} bytes free):\n{}",
                    stack.name,
                    stack.root,
                    stack.storage,
                    stack.minimum_free_bytes,
                    stack.render()
                )
                .into());
            }
            if bound.bytes.is_none() {
                warnings.push(format!(
                    "the {} is {shown} bytes from `{}`: its {} unresolved sites are in the report",
                    stack.name,
                    stack.root,
                    stack.unresolved.len()
                ));
            }
        }
        Ok(warnings)
    }

    /// The coverage and each stack's bound, budget, headroom, deepest path
    /// and unresolved sites.
    pub fn render(&self) -> String {
        let coverage = &self.coverage;
        let mut out = format!(
            "frame coverage: {} of {} functions have a compiler frame record; {} reviewed, {} \
             unreviewed\n",
            coverage.measured,
            coverage.functions,
            coverage.reviewed.len(),
            coverage.unreviewed.len()
        );
        for (address, name, category) in &coverage.reviewed {
            let _ = writeln!(out, "  reviewed {category:?} {address:#010x} {name}");
        }
        for problem in &coverage.unreviewed {
            let _ = writeln!(out, "  unreviewed {problem}");
        }
        for stack in &self.stacks {
            out.push_str(&stack.render());
        }
        out
    }
}

impl StackBound {
    /// This stack's section of the report.
    pub fn render(&self) -> String {
        let mut out = String::new();
        let Some((capacity, bound)) = &self.linked else {
            let _ = writeln!(
                out,
                "{}: not linked ({}); the image does not run it",
                self.name, self.storage
            );
            return out;
        };
        let budget = capacity.saturating_sub(self.minimum_free_bytes);
        let (bytes, state) = match bound.bytes {
            Some(bytes) if bound.assumptions.is_empty() => (bytes.to_string(), "proven"),
            Some(bytes) => (bytes.to_string(), "conditional"),
            None => (format!("{} + ?", bound.partial), "partial"),
        };
        let used = bound.bytes.unwrap_or(bound.partial);
        let headroom = if used <= budget {
            format!("{} bytes of headroom", budget - used)
        } else {
            format!("{} bytes over", used - budget)
        };
        let _ = writeln!(
            out,
            "{}: `{}` needs {bytes} bytes, {state}; budget {budget} bytes ({capacity} bytes of {} \
             less {} bytes free), {headroom}",
            self.name, self.root, self.storage, self.minimum_free_bytes
        );
        for assumption in &bound.assumptions {
            let _ = writeln!(out, "  assumes {assumption}");
        }
        let _ = writeln!(out, "  deepest path:");
        for (frame, function) in &self.path {
            let _ = writeln!(out, "    {frame:>6} {function}");
        }
        if !self.unresolved.is_empty() {
            let reasons = bound
                .reasons()
                .iter()
                .map(|(reason, count)| format!("{count} {reason}"))
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(
                out,
                "  unresolved: {} ({reasons}); a hole may reach deeper",
                self.unresolved.len()
            );
            for (reason, site, function) in &self.unresolved {
                let _ = writeln!(
                    out,
                    "    {reason} at {site:#010x} in {function} (closed by {})",
                    reason.closed_by()
                );
            }
        }
        out
    }
}

#[cfg(test)]
mod tests;
