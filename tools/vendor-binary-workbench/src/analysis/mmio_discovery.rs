//! Best-effort artifact-wide discovery of statically addressed MMIO accesses.
//!
//! Unlike reference generation, discovery retains partial function results
//! when later control flow is unsupported. Findings are therefore analysis
//! evidence, not a completeness claim.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::{Path, PathBuf},
};

use crate::{
    BitSource, FunctionAnalysis, MemoryAccess, MmioMap, MmioRegion, ObservableEvent, Result,
    StructuralPointerContext, SymbolicValue, artifact, direct,
};

const MAX_DISCOVERY_STATES: usize = 127;
const MAX_DISCOVERY_BRANCH_DECISIONS: usize = 12;

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
    pub(crate) write_patterns: Vec<WritePatternFinding>,
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
    pub(crate) functions_with_mmio: usize,
    pub(crate) functions_with_diagnostics: usize,
    pub(crate) explored_states: usize,
    pub(crate) terminal_paths: usize,
    pub(crate) branch_sites: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MmioDiscoveryReport {
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
    write_patterns: BTreeMap<WriteBitPattern, (usize, BTreeSet<DiscoveryFunction>)>,
}

#[derive(Default)]
struct FunctionExploration {
    /// One representative observable plus its maximum multiplicity on any
    /// explored path. ObservableEvent does not retain the instruction site,
    /// so summing paths would duplicate their common prefix.
    events: Vec<(ObservableEvent, usize)>,
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
    event: &ObservableEvent,
    occurrences: usize,
) -> bool {
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
        }
        MemoryAccess::Write => {
            entry.write_count += occurrences;
            entry.write_functions.insert(function.clone());
            let pattern = classify_write_bits(value.as_ref(), *address, *width);
            let (pattern_occurrences, functions) = entry.write_patterns.entry(pattern).or_default();
            *pattern_occurrences += occurrences;
            functions.insert(function.clone());
        }
    }
    true
}

fn merge_path_events(merged: &mut Vec<(ObservableEvent, usize)>, path_events: &[ObservableEvent]) {
    let mut path_counts = Vec::<(ObservableEvent, usize)>::new();
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
        } else {
            merged.push((event, count));
        }
    }
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
    let mut queue = VecDeque::from([BTreeMap::<u32, bool>::new()]);
    let mut queued = BTreeSet::from([BTreeMap::<u32, bool>::new()]);

    while let Some(forced_branches) = queue.pop_front() {
        if result.explored_states >= MAX_DISCOVERY_STATES {
            result.diagnostics.insert((
                "exploration",
                format!(
                    "symbolic CFG exceeds the discovery limit of {MAX_DISCOVERY_STATES} states"
                ),
            ));
            break;
        }
        result.explored_states += 1;
        let trace = match direct::trace_binary_symbol_with_branches(
            symbol,
            map,
            relocated_calls,
            pointer_context,
            None,
            &forced_branches,
        ) {
            Ok(trace) => trace,
            Err(error) => {
                result.diagnostics.insert(("decode", error.to_string()));
                continue;
            }
        };
        merge_path_events(&mut result.events, &trace.events);
        collect_trace_diagnostics(&trace, &mut result.diagnostics);

        let Some(branch) = trace.unresolved_branch else {
            result.terminal_paths += 1;
            continue;
        };
        result.branch_sites.insert(branch.site);
        if forced_branches.len() >= MAX_DISCOVERY_BRANCH_DECISIONS {
            result.diagnostics.insert((
                "exploration",
                format!(
                    "symbolic CFG exceeds the discovery limit of {MAX_DISCOVERY_BRANCH_DECISIONS} branch decisions per path at {:#010x}",
                    branch.site
                ),
            ));
            continue;
        }
        for taken in [false, true] {
            let mut next = forced_branches.clone();
            if next.insert(branch.site, taken).is_some() {
                result.diagnostics.insert((
                    "exploration",
                    format!(
                        "symbolic CFG revisits branch {:#010x}; discovery stopped that path",
                        branch.site
                    ),
                ));
            } else if queued.insert(next.clone()) {
                queue.push_back(next);
            }
        }
    }

    result
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
    fields(artifacts = artifacts.len(), ranges = ranges.len(), symbol_prefix)
)]
pub(crate) fn discover_mmio(
    artifacts: &[(String, PathBuf)],
    ranges: &[DiscoveryRange],
    symbol_prefix: &str,
    svd: &MmioMap,
) -> Result<MmioDiscoveryReport> {
    let map = discovery_map(svd, ranges);
    let relocated_calls = BTreeMap::new();
    let pointer_context = StructuralPointerContext::default();
    let mut accumulators = BTreeMap::<(u32, u8), RegisterAccumulator>::new();
    let mut diagnostics = Vec::new();
    let mut artifact_summaries = Vec::new();

    for (source, path) in artifacts {
        let symbols = artifact::load_symbols(Path::new(path), symbol_prefix)?;
        let mut functions_with_mmio = 0usize;
        let mut functions_with_diagnostics = 0usize;
        let mut explored_states = 0usize;
        let mut terminal_paths = 0usize;
        let mut branch_sites = 0usize;
        for symbol in &symbols {
            let function = DiscoveryFunction {
                source: source.clone(),
                member: symbol.member.clone(),
                symbol: symbol.name.clone(),
            };
            let exploration = explore_symbol(symbol, &map, &relocated_calls, &pointer_context);
            let mut found = false;
            for (event, occurrences) in &exploration.events {
                found |= record_event(&mut accumulators, ranges, &function, event, *occurrences);
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
        }
        artifact_summaries.push(ArtifactDiscoverySummary {
            source: source.clone(),
            path: path.clone(),
            functions: symbols.len(),
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

    Ok(MmioDiscoveryReport {
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
        let event = ObservableEvent::Memory {
            access: MemoryAccess::Read,
            width: 32,
            address: 0x2010_7030,
            register: "AGC.CONTROL".to_owned(),
            value: None,
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
            memory_regions: Vec::new(),
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
            &BTreeMap::new(),
            &StructuralPointerContext::default(),
        );
        let addresses = exploration
            .events
            .iter()
            .filter_map(|(event, _)| match event {
                ObservableEvent::Memory { address, .. } => Some(*address),
                ObservableEvent::Fence { .. } => None,
            })
            .collect::<BTreeSet<_>>();

        assert_eq!(addresses, BTreeSet::from([0x2010_7030, 0x2010_7034]));
        assert_eq!(exploration.explored_states, 3);
        assert_eq!(exploration.terminal_paths, 2);
        assert_eq!(exploration.branch_sites, BTreeSet::from([0x1000]));
        assert!(
            exploration.diagnostics.is_empty(),
            "{:#?}",
            exploration.diagnostics
        );
    }
}
