//! Best-effort artifact-wide discovery of statically addressed MMIO accesses.
//!
//! Unlike reference generation, discovery retains partial function results
//! when later control flow is unsupported. Findings are therefore analysis
//! evidence, not a completeness claim.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc::sync_channel,
    },
    thread,
};

use tracing_indicatif::span_ext::IndicatifSpanExt;

use crate::{
    BitSource, FunctionAnalysis, LocatedObservableEvent, MemoryAccess, MmioMap, MmioRegion,
    ObservableEvent, Result, StructuralPointerContext, SymbolicValue, artifact, direct,
};

// Artifact-wide discovery remains bounded, but these are analysis-coverage
// limits rather than performance knobs. Symbolic-value size, trace steps and
// retained events independently bound resource use; do not lower path
// coverage without a measured completeness comparison on a real project.
const MAX_DISCOVERY_STATES: usize = 127;
const MAX_DISCOVERY_BRANCH_DECISIONS: usize = 12;
const MAX_DISCOVERY_JOBS: usize = 8;
const MAX_DISCOVERY_INSTRUCTION_STEPS_PER_TRACE: usize = 4_096;
const MAX_DISCOVERY_EVENTS_PER_TRACE: usize = 1_024;
const MAX_DISCOVERY_EVENTS_PER_FUNCTION: usize = 2_048;
// Symbolic expressions are recursive trees. A whole-ROM function can build a
// much deeper (still bounded) value than the operating system's small default
// worker stack can drop safely. Keep the stack explicit so `--jobs` does not
// make an artifact fail when the same analysis succeeds on the main thread.
const DISCOVERY_WORKER_STACK_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct MmioDiscoveryOptions {
    /// The caller supplies an explicit worker count; zero is normalized to one.
    pub(crate) jobs: usize,
}

impl MmioDiscoveryOptions {
    fn worker_count(self, functions: usize) -> usize {
        let available = thread::available_parallelism().map_or(1, usize::from);
        let requested = self.jobs.clamp(1, MAX_DISCOVERY_JOBS).min(available);
        requested.max(1).min(functions.max(1))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DiscoveryRange {
    pub(crate) name: String,
    pub(crate) start: u32,
    pub(crate) end: u32,
}

impl DiscoveryRange {
    fn contains(&self, address: u32) -> bool {
        address >= self.start && address < self.end
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct DiscoveryFunction {
    pub(crate) source: String,
    pub(crate) member: Option<String>,
    pub(crate) symbol: String,
}

impl DiscoveryFunction {
    pub(crate) fn canonical(&self) -> String {
        self.member.as_deref().map_or_else(
            || format!("{}:{}", self.source, self.symbol),
            |member| format!("{}:{}:{}", self.source, member, self.symbol),
        )
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct WriteBitPattern {
    /// Output bits copied unchanged from the same bit of the same register.
    pub(crate) preserved_mask: u32,
    /// Output bits containing the inversion of the same source bit.
    pub(crate) inverted_mask: u32,
    pub(crate) forced_zero_mask: u32,
    pub(crate) forced_one_mask: u32,
    /// Output bits derived from a different bit of this register, or from an
    /// indexed register read whose exact address is not retained by BitSource.
    pub(crate) read_derived_mask: u32,
    /// Output bits derived from arguments, RAM, calls, another register, or an
    /// expression whose bit provenance could not be retained.
    pub(crate) dynamic_mask: u32,
}

impl WriteBitPattern {
    pub(crate) fn modified_mask(&self, width: u8) -> u32 {
        width_mask(width) & !self.preserved_mask
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WritePatternFinding {
    pub(crate) pattern: WriteBitPattern,
    pub(crate) occurrences: usize,
    pub(crate) functions: BTreeSet<DiscoveryFunction>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RegisterFinding {
    pub(crate) address: u32,
    pub(crate) width: u8,
    pub(crate) name: String,
    pub(crate) read_count: usize,
    pub(crate) write_count: usize,
    pub(crate) read_functions: BTreeSet<DiscoveryFunction>,
    pub(crate) write_functions: BTreeSet<DiscoveryFunction>,
    pub(crate) read_sites: BTreeSet<DiscoveryAccessSite>,
    pub(crate) write_sites: BTreeSet<DiscoveryAccessSite>,
    pub(crate) write_patterns: Vec<WritePatternFinding>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct DiscoveryAccessSite {
    pub(crate) function: DiscoveryFunction,
    pub(crate) site: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FunctionDiagnostic {
    pub(crate) function: DiscoveryFunction,
    pub(crate) scope: &'static str,
    pub(crate) message: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ArtifactDiscoverySummary {
    pub(crate) source: String,
    pub(crate) path: PathBuf,
    pub(crate) functions: usize,
    pub(crate) reviewed_boundaries: usize,
    pub(crate) functions_with_mmio: usize,
    pub(crate) functions_with_diagnostics: usize,
    pub(crate) explored_states: usize,
    pub(crate) terminal_paths: usize,
    pub(crate) branch_sites: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MmioDiscoveryReport {
    pub(crate) code_symbol_selection: artifact::CodeSymbolSelection,
    pub(crate) symbol_prefix: String,
    pub(crate) ranges: Vec<DiscoveryRange>,
    pub(crate) artifacts: Vec<ArtifactDiscoverySummary>,
    pub(crate) registers: Vec<RegisterFinding>,
    pub(crate) diagnostics: Vec<FunctionDiagnostic>,
}

#[derive(Default)]
struct RegisterAccumulator {
    name: String,
    read_count: usize,
    write_count: usize,
    read_functions: BTreeSet<DiscoveryFunction>,
    write_functions: BTreeSet<DiscoveryFunction>,
    read_sites: BTreeSet<DiscoveryAccessSite>,
    write_sites: BTreeSet<DiscoveryAccessSite>,
    write_patterns: BTreeMap<WriteBitPattern, (usize, BTreeSet<DiscoveryFunction>)>,
}

#[derive(Debug, Default, Eq, PartialEq)]
struct FunctionExploration {
    /// One instruction-local observable plus its maximum multiplicity on any
    /// explored path. Summing paths would duplicate their common prefix.
    events: Vec<(LocatedObservableEvent, usize)>,
    diagnostics: BTreeSet<(&'static str, String)>,
    explored_states: usize,
    terminal_paths: usize,
    branch_sites: BTreeSet<u32>,
}

fn width_mask(width: u8) -> u32 {
    match width {
        8 => 0xff,
        16 => 0xffff,
        32 => u32::MAX,
        _ => 0,
    }
}

pub(crate) fn classify_write_bits(
    value: Option<&SymbolicValue>,
    address: u32,
    width: u8,
) -> WriteBitPattern {
    let mask = width_mask(width);
    let Some(value) = value else {
        return WriteBitPattern {
            preserved_mask: 0,
            inverted_mask: 0,
            forced_zero_mask: 0,
            forced_one_mask: 0,
            read_derived_mask: 0,
            dynamic_mask: mask,
        };
    };
    let bits = value.bits();
    let mut pattern = WriteBitPattern {
        preserved_mask: 0,
        inverted_mask: 0,
        forced_zero_mask: 0,
        forced_one_mask: 0,
        read_derived_mask: 0,
        dynamic_mask: 0,
    };
    for (output_bit, source) in bits.iter().enumerate().take(usize::from(width)) {
        let output_mask = 1_u32 << output_bit;
        match source {
            BitSource::Constant(false) => pattern.forced_zero_mask |= output_mask,
            BitSource::Constant(true) => pattern.forced_one_mask |= output_mask,
            BitSource::Register {
                address: source_address,
                bit,
                inverted,
                ..
            } if *source_address == address && usize::from(*bit) == output_bit => {
                if *inverted {
                    pattern.inverted_mask |= output_mask;
                } else {
                    pattern.preserved_mask |= output_mask;
                }
            }
            BitSource::Register {
                address: source_address,
                ..
            } if *source_address == address => pattern.read_derived_mask |= output_mask,
            BitSource::IndexedRegister { .. } => pattern.read_derived_mask |= output_mask,
            _ => pattern.dynamic_mask |= output_mask,
        }
    }
    debug_assert_eq!(
        pattern.preserved_mask
            | pattern.inverted_mask
            | pattern.forced_zero_mask
            | pattern.forced_one_mask
            | pattern.read_derived_mask
            | pattern.dynamic_mask,
        mask
    );
    pattern
}

fn candidate_name(ranges: &[DiscoveryRange], svd_name: &str, address: u32) -> String {
    if svd_name != "UNMAPPED" {
        return svd_name.to_owned();
    }
    let range = ranges
        .iter()
        .find(|range| range.contains(address))
        .expect("the event was filtered through one requested range");
    format!("{}.REG_{address:08X}", range.name)
}

fn record_event(
    accumulators: &mut BTreeMap<(u32, u8), RegisterAccumulator>,
    ranges: &[DiscoveryRange],
    function: &DiscoveryFunction,
    located: &LocatedObservableEvent,
    occurrences: usize,
) -> bool {
    let event = &located.event;
    let ObservableEvent::Memory {
        access,
        width,
        address,
        register,
        value,
    } = event
    else {
        return false;
    };
    if !ranges.iter().any(|range| range.contains(*address)) {
        return false;
    }
    let name = candidate_name(ranges, register, *address);
    let entry = accumulators.entry((*address, *width)).or_default();
    if entry.name.is_empty() {
        entry.name = name;
    }
    match access {
        MemoryAccess::Read => {
            entry.read_count += occurrences;
            entry.read_functions.insert(function.clone());
            entry.read_sites.insert(DiscoveryAccessSite {
                function: function.clone(),
                site: located.site,
            });
        }
        MemoryAccess::Write => {
            entry.write_count += occurrences;
            entry.write_functions.insert(function.clone());
            entry.write_sites.insert(DiscoveryAccessSite {
                function: function.clone(),
                site: located.site,
            });
            let pattern = classify_write_bits(value.as_ref(), *address, *width);
            let (pattern_occurrences, functions) = entry.write_patterns.entry(pattern).or_default();
            *pattern_occurrences += occurrences;
            functions.insert(function.clone());
        }
    }
    true
}

fn merge_path_events(
    merged: &mut Vec<(LocatedObservableEvent, usize)>,
    path_events: &[LocatedObservableEvent],
) -> bool {
    let mut truncated = false;
    let mut path_counts = Vec::<(LocatedObservableEvent, usize)>::new();
    for event in path_events {
        if let Some((_, count)) = path_counts
            .iter_mut()
            .find(|(candidate, _)| candidate == event)
        {
            *count += 1;
        } else {
            path_counts.push((event.clone(), 1));
        }
    }
    for (event, count) in path_counts {
        if let Some((_, merged_count)) =
            merged.iter_mut().find(|(candidate, _)| candidate == &event)
        {
            *merged_count = (*merged_count).max(count);
        } else if merged.len() >= MAX_DISCOVERY_EVENTS_PER_FUNCTION {
            truncated = true;
        } else {
            merged.push((event, count));
        }
    }
    truncated
}

fn collect_trace_diagnostics(
    trace: &FunctionAnalysis,
    diagnostics: &mut BTreeSet<(&'static str, String)>,
) {
    for blocker in &trace.blockers {
        // An input-dependent branch is expected here: the explorer will run
        // both decisions. Other blockers on the path remain visible.
        if trace.unresolved_branch.is_some() && blocker.starts_with("input-dependent control-flow")
        {
            continue;
        }
        diagnostics.insert(("direct", blocker.clone()));
    }
    diagnostics.extend(
        trace
            .reference_blockers
            .iter()
            .cloned()
            .map(|message| ("reference", message)),
    );
}

fn explore_symbol(
    symbol: &artifact::ArtifactSymbolDefinition,
    map: &MmioMap,
    relocated_calls: &direct::StructuralRelocatedCalls,
    pointer_context: &StructuralPointerContext,
) -> FunctionExploration {
    let mut result = FunctionExploration::default();
    let program = match direct::StructuralProgram::decode(symbol) {
        Ok(program) => program,
        Err(error) => {
            result.diagnostics.insert(("decode", error.to_string()));
            return result;
        }
    };
    let summary = direct::explore_structural_program_bounded(
        symbol,
        &program,
        map,
        relocated_calls,
        pointer_context,
        None,
        direct::StructuralTraceBudget {
            max_instruction_steps: MAX_DISCOVERY_INSTRUCTION_STEPS_PER_TRACE,
            max_events: MAX_DISCOVERY_EVENTS_PER_TRACE,
        },
        MAX_DISCOVERY_STATES,
        MAX_DISCOVERY_BRANCH_DECISIONS,
        |trace| {
            let trace = match trace {
                Ok(trace) => trace,
                Err(error) => {
                    result.diagnostics.insert(("decode", error.to_string()));
                    return;
                }
            };
            if merge_path_events(&mut result.events, &trace.located_events) {
                result.diagnostics.insert((
                    "exploration",
                    format!(
                        "function exceeds the discovery limit of {MAX_DISCOVERY_EVENTS_PER_FUNCTION} distinct observable events"
                    ),
                ));
            }
            collect_trace_diagnostics(&trace, &mut result.diagnostics);
            if let Some(branch) = trace.unresolved_branch {
                result.branch_sites.insert(branch.site);
            } else {
                result.terminal_paths += 1;
            }
        },
    );
    result.explored_states = summary.explored_states;
    for limit in summary.limits {
        let message = match limit {
            direct::StructuralExplorationLimit::States { maximum } => {
                format!("symbolic CFG exceeds the discovery limit of {maximum} states")
            }
            direct::StructuralExplorationLimit::BranchDecisions { site, maximum } => format!(
                "symbolic CFG exceeds the discovery limit of {maximum} branch decisions per path at {site:#010x}"
            ),
            direct::StructuralExplorationLimit::RevisitedBranch { site } => {
                format!("symbolic CFG revisits branch {site:#010x}; discovery stopped that path")
            }
        };
        result.diagnostics.insert(("exploration", message));
    }

    result
}

fn artifact_progress_span(source: &str, path: &Path, functions: usize) -> tracing::Span {
    let span = tracing::info_span!(
        "mmio_artifact",
        indicatif.pb_show = tracing::field::Empty,
        source,
        artifact = %path.display(),
        functions,
    );
    crate::progress::determinate_span(span, functions, &format!("{source}: loading functions"))
}

fn explore_symbols(
    source: &str,
    symbols: &[artifact::ArtifactSymbolDefinition],
    map: &MmioMap,
    relocated_calls: &direct::StructuralRelocatedCalls,
    pointer_context: &StructuralPointerContext,
    jobs: usize,
    mut consume: impl FnMut(DiscoveryFunction, FunctionExploration),
) {
    let next = AtomicUsize::new(0);
    // A bounded channel prevents fast workers from retaining an entire
    // artifact's symbolic results while the deterministic accumulator merges
    // earlier completions.
    let (sender, receiver) = sync_channel::<(DiscoveryFunction, FunctionExploration)>(jobs * 2);
    thread::scope(|scope| {
        for worker in 0..jobs {
            let sender = sender.clone();
            let next = &next;
            thread::Builder::new()
                .name(format!("mmio-discovery-{worker}"))
                .stack_size(DISCOVERY_WORKER_STACK_BYTES)
                .spawn_scoped(scope, move || {
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(symbol) = symbols.get(index) else {
                            break;
                        };
                        let function = DiscoveryFunction {
                            source: source.to_owned(),
                            member: symbol.member.clone(),
                            symbol: symbol.name.clone(),
                        };
                        let exploration =
                            explore_symbol(symbol, map, relocated_calls, pointer_context);
                        if sender.send((function, exploration)).is_err() {
                            break;
                        }
                    }
                })
                .expect("spawning a bounded MMIO discovery worker");
        }
        drop(sender);
        for (function, exploration) in receiver {
            consume(function, exploration);
        }
    });
}

fn discovery_map(svd: &MmioMap, ranges: &[DiscoveryRange]) -> MmioMap {
    let mut map = svd.clone();
    map.regions.extend(ranges.iter().map(|range| MmioRegion {
        name: range.name.clone(),
        start: range.start,
        end: range.end,
        readable: true,
        writable: true,
    }));
    map.regions
        .sort_by_key(|region| (region.start, region.end, region.name.clone()));
    map.regions.dedup();
    map
}

#[tracing::instrument(
    name = "discover_mmio",
    skip(artifacts, ranges, svd),
    fields(
        artifacts = artifacts.len(),
        ranges = ranges.len(),
        symbol_prefix,
        code_symbols = code_symbol_selection.label()
    )
)]
pub(crate) fn discover_mmio(
    artifacts: &[(String, PathBuf)],
    ranges: &[DiscoveryRange],
    symbol_prefix: &str,
    code_symbol_selection: artifact::CodeSymbolSelection,
    svd: &MmioMap,
    effective_code: Option<&super::EffectiveCodeCatalog>,
    options: MmioDiscoveryOptions,
) -> Result<MmioDiscoveryReport> {
    let map = discovery_map(svd, ranges);
    let relocated_calls = direct::StructuralRelocatedCalls::default();
    let pointer_context = StructuralPointerContext::default();
    let mut accumulators = BTreeMap::<(u32, u8), RegisterAccumulator>::new();
    let mut diagnostics = Vec::new();
    let mut artifact_summaries = Vec::new();

    for (source, path) in artifacts {
        let (symbols, reviewed_boundaries) = match effective_code {
            Some(catalog) => {
                let loaded = catalog.load_symbols(
                    source,
                    Path::new(path),
                    symbol_prefix,
                    code_symbol_selection,
                )?;
                (loaded.symbols, loaded.reviewed_boundaries)
            }
            None => (
                artifact::load_code_symbols(Path::new(path), symbol_prefix, code_symbol_selection)?,
                0,
            ),
        };
        let jobs = options.worker_count(symbols.len());
        let progress = artifact_progress_span(source, path, symbols.len());
        progress.pb_set_message(&format!(
            "{source}: analyzing {} functions with {jobs} worker{}",
            symbols.len(),
            if jobs == 1 { "" } else { "s" }
        ));
        tracing::info!(
            source,
            artifact = %path.display(),
            functions = symbols.len(),
            jobs,
            max_states_per_function = MAX_DISCOVERY_STATES,
            "starting artifact MMIO analysis"
        );
        let mut functions_with_mmio = 0usize;
        let mut functions_with_diagnostics = 0usize;
        let mut explored_states = 0usize;
        let mut terminal_paths = 0usize;
        let mut branch_sites = 0usize;
        explore_symbols(
            source,
            &symbols,
            &map,
            &relocated_calls,
            &pointer_context,
            jobs,
            |function, exploration| {
                let mut found = false;
                for (event, occurrences) in &exploration.events {
                    found |=
                        record_event(&mut accumulators, ranges, &function, event, *occurrences);
                }
                functions_with_mmio += usize::from(found);
                functions_with_diagnostics += usize::from(!exploration.diagnostics.is_empty());
                explored_states += exploration.explored_states;
                terminal_paths += exploration.terminal_paths;
                branch_sites += exploration.branch_sites.len();
                diagnostics.extend(exploration.diagnostics.into_iter().map(|(scope, message)| {
                    FunctionDiagnostic {
                        function: function.clone(),
                        scope,
                        message,
                    }
                }));
                progress.pb_inc(1);
                progress.pb_set_message(&format!("{source}: completed {}", function.canonical()));
            },
        );
        progress.pb_set_finish_message(&format!("{source}: analyzed {} functions", symbols.len()));
        artifact_summaries.push(ArtifactDiscoverySummary {
            source: source.clone(),
            path: path.clone(),
            functions: symbols.len(),
            reviewed_boundaries,
            functions_with_mmio,
            functions_with_diagnostics,
            explored_states,
            terminal_paths,
            branch_sites,
        });
    }

    let registers = accumulators
        .into_iter()
        .map(|((address, width), accumulator)| RegisterFinding {
            address,
            width,
            name: accumulator.name,
            read_count: accumulator.read_count,
            write_count: accumulator.write_count,
            read_functions: accumulator.read_functions,
            write_functions: accumulator.write_functions,
            read_sites: accumulator.read_sites,
            write_sites: accumulator.write_sites,
            write_patterns: accumulator
                .write_patterns
                .into_iter()
                .map(|(pattern, (occurrences, functions))| WritePatternFinding {
                    pattern,
                    occurrences,
                    functions,
                })
                .collect(),
        })
        .collect();
    diagnostics.sort_by(|left, right| {
        (&left.function, left.scope, &left.message).cmp(&(
            &right.function,
            right.scope,
            &right.message,
        ))
    });

    Ok(MmioDiscoveryReport {
        code_symbol_selection,
        symbol_prefix: symbol_prefix.to_owned(),
        ranges: ranges.to_vec(),
        artifacts: artifact_summaries,
        registers,
        diagnostics,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_a_register_read_modify_write_by_output_bit() {
        let address = 0x2010_7030;
        let value = SymbolicValue::register_read(0, address, 32, false)
            .and(0xffff_ff0f)
            .or(0x50);
        let pattern = classify_write_bits(Some(&value), address, 32);

        assert_eq!(pattern.preserved_mask, 0xffff_ff0f);
        assert_eq!(pattern.forced_one_mask, 0x50);
        assert_eq!(pattern.forced_zero_mask, 0xa0);
        assert_eq!(pattern.modified_mask(32), 0xf0);
        assert_eq!(pattern.dynamic_mask, 0);
    }

    #[test]
    fn classifies_an_argument_write_as_dynamic_bits() {
        let pattern = classify_write_bits(Some(&SymbolicValue::input(0)), 0x2000_0000, 16);
        assert_eq!(pattern.dynamic_mask, 0xffff);
        assert_eq!(pattern.modified_mask(16), 0xffff);
    }

    #[test]
    fn path_merge_does_not_double_count_a_common_prefix() {
        let event = LocatedObservableEvent {
            site: 0x1004,
            event: ObservableEvent::Memory {
                access: MemoryAccess::Read,
                width: 32,
                address: 0x2010_7030,
                register: "AGC.CONTROL".to_owned(),
                value: None,
            },
        };
        let mut merged = Vec::new();

        merge_path_events(&mut merged, std::slice::from_ref(&event));
        merge_path_events(&mut merged, std::slice::from_ref(&event));
        assert_eq!(merged, [(event.clone(), 1)]);

        merge_path_events(&mut merged, &[event.clone(), event.clone()]);
        assert_eq!(merged, [(event, 2)]);
    }

    #[test]
    fn explores_mmio_on_both_sides_of_an_input_dependent_branch() {
        let symbol = artifact::ArtifactSymbolDefinition {
            member: None,
            name: "branched_mmio".to_owned(),
            address: 0x1000,
            bytes: vec![
                0x63, 0x08, 0x05, 0x00, // beq a0, zero, 0x1010
                0xb7, 0x75, 0x10, 0x20, // lui a1, 0x20107
                0x23, 0xa8, 0xc5, 0x02, // sw a2, 0x30(a1)
                0x67, 0x80, 0x00, 0x00, // ret
                0xb7, 0x75, 0x10, 0x20, // lui a1, 0x20107
                0x23, 0xaa, 0xd5, 0x02, // sw a3, 0x34(a1)
                0x67, 0x80, 0x00, 0x00, // ret
            ],
            addresses_resolved: true,
            memory_regions: Default::default(),
            relocations: Vec::new(),
        };
        let map = MmioMap {
            registers: Vec::new(),
            regions: vec![MmioRegion {
                name: "radio".to_owned(),
                start: 0x2010_7000,
                end: 0x2010_7100,
                readable: true,
                writable: true,
            }],
        };

        let exploration = explore_symbol(
            &symbol,
            &map,
            &direct::StructuralRelocatedCalls::new(),
            &StructuralPointerContext::default(),
        );
        let addresses = exploration
            .events
            .iter()
            .filter_map(|(event, _)| match &event.event {
                ObservableEvent::Memory { address, .. } => Some(*address),
                ObservableEvent::Fence { .. } => None,
            })
            .collect::<BTreeSet<_>>();
        let sites = exploration
            .events
            .iter()
            .filter_map(|(located, _)| match &located.event {
                ObservableEvent::Memory { address, .. } => Some((*address, located.site)),
                ObservableEvent::Fence { .. } => None,
            })
            .collect::<BTreeMap<_, _>>();

        assert_eq!(addresses, BTreeSet::from([0x2010_7030, 0x2010_7034]));
        assert_eq!(sites[&0x2010_7030], 0x1008);
        assert_eq!(sites[&0x2010_7034], 0x1014);
        assert_eq!(exploration.explored_states, 3);
        assert_eq!(exploration.terminal_paths, 2);
        assert_eq!(exploration.branch_sites, BTreeSet::from([0x1000]));
        assert!(
            exploration.diagnostics.is_empty(),
            "{:#?}",
            exploration.diagnostics
        );
    }

    #[test]
    fn parallel_function_exploration_matches_serial_results() {
        let template = artifact::ArtifactSymbolDefinition {
            member: None,
            name: String::new(),
            address: 0x1000,
            bytes: vec![
                0x63, 0x08, 0x05, 0x00, // beq a0, zero, 0x1010
                0xb7, 0x75, 0x10, 0x20, // lui a1, 0x20107
                0x23, 0xa8, 0xc5, 0x02, // sw a2, 0x30(a1)
                0x67, 0x80, 0x00, 0x00, // ret
                0xb7, 0x75, 0x10, 0x20, // lui a1, 0x20107
                0x23, 0xaa, 0xd5, 0x02, // sw a3, 0x34(a1)
                0x67, 0x80, 0x00, 0x00, // ret
            ],
            addresses_resolved: true,
            memory_regions: Default::default(),
            relocations: Vec::new(),
        };
        let symbols = (0..4)
            .map(|index| artifact::ArtifactSymbolDefinition {
                name: format!("branched_mmio_{index}"),
                ..template.clone()
            })
            .collect::<Vec<_>>();
        let map = MmioMap {
            registers: Vec::new(),
            regions: vec![MmioRegion {
                name: "radio".to_owned(),
                start: 0x2010_7000,
                end: 0x2010_7100,
                readable: true,
                writable: true,
            }],
        };
        let relocated_calls = direct::StructuralRelocatedCalls::default();
        let pointer_context = StructuralPointerContext::default();
        let collect = |jobs| {
            let mut results = BTreeMap::new();
            explore_symbols(
                "fixture",
                &symbols,
                &map,
                &relocated_calls,
                &pointer_context,
                jobs,
                |function, exploration| {
                    results.insert(function, exploration);
                },
            );
            results
        };

        assert_eq!(collect(1), collect(2));
    }
}
