use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
};

use object::{Object, ObjectSection, ObjectSymbol, SectionFlags, SectionKind, SymbolKind};
use rustc_demangle::try_demangle;
use serde::Serialize;

use crate::{
    ConsumerScope, Error, MemoryPolicy, PlacementReason, PlacementRequirement, RegionKind, Result,
    policy::wildcard_matches,
};

#[derive(Clone, Debug, Serialize)]
pub struct MemoryReport {
    pub schema: u32,
    pub elf: PathBuf,
    pub regions: Vec<RegionReport>,
    pub consumers: Vec<ConsumerReport>,
    pub largest_unclassified: Vec<Allocation>,
    pub audit: AuditReport,
}

#[derive(Clone, Debug, Serialize)]
pub struct RegionReport {
    pub id: String,
    pub kind: RegionKind,
    pub start: u64,
    pub end: u64,
    pub capacity: u64,
    pub allocated: u64,
    pub executable: u64,
    pub read_only: u64,
    pub mutable: u64,
    pub policy_attributed: u64,
    pub mutable_unattributed: u64,
    pub reserved: u64,
    pub free: u64,
    pub sections: Vec<SectionReport>,
    pub reservations: Vec<ReservationReport>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SectionReport {
    pub name: String,
    pub address: u64,
    pub size: u64,
    pub class: SectionClass,
}

#[derive(Clone, Copy, Debug, Serialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum SectionClass {
    Executable,
    ReadOnly,
    Mutable,
}

#[derive(Clone, Debug, Serialize)]
pub struct ReservationReport {
    pub id: String,
    pub start: u64,
    pub end: u64,
    pub size: u64,
    pub reason: PlacementReason,
}

#[derive(Clone, Debug, Serialize)]
pub struct ConsumerReport {
    pub owner: String,
    pub scope: ConsumerScope,
    pub reason: PlacementReason,
    pub placement: PlacementRequirement,
    pub region: String,
    pub bytes: u64,
    pub symbols: u64,
    pub declared_count: Option<u64>,
    pub element_capacity: Option<u64>,
    pub optimization: Option<String>,
    pub allocations: Vec<Allocation>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Allocation {
    pub symbol: String,
    pub demangled: String,
    pub section: String,
    pub region: Option<String>,
    pub address: u64,
    pub size: u64,
    #[serde(skip)]
    rule_index: Option<usize>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct AuditReport {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

pub fn analyze(elf_path: &Path, policy: &MemoryPolicy) -> Result<MemoryReport> {
    let bytes = fs::read(elf_path).map_err(|source| Error::Read {
        path: elf_path.to_owned(),
        source,
    })?;
    let elf = object::File::parse(bytes.as_slice()).map_err(|error| Error::Elf {
        path: elf_path.to_owned(),
        message: error.to_string(),
    })?;

    let symbol_values = elf
        .symbols()
        .chain(elf.dynamic_symbols())
        .filter_map(|symbol| {
            symbol
                .name()
                .ok()
                .map(|name| (name.to_owned(), symbol.address()))
        })
        .collect::<HashMap<_, _>>();
    let mut audit = AuditReport::default();
    let reservations = resolve_reservations(policy, &symbol_values, &mut audit);

    let mut sections = Vec::new();
    let mut section_names = HashMap::new();
    for section in elf.sections() {
        let name = section
            .name()
            .unwrap_or("<invalid-section-name>")
            .to_owned();
        section_names.insert(section.index(), name.clone());
        if section.size() == 0 || !is_allocated(&section.flags(), section.address()) {
            continue;
        }
        let Some(region) = region_for_range(policy, section.address(), section.size()) else {
            audit.warnings.push(format!(
                "allocated section {name:?} at {:#x}..{:#x} is outside configured regions",
                section.address(),
                section.address().saturating_add(section.size())
            ));
            continue;
        };
        sections.push((
            region.id.clone(),
            SectionReport {
                name,
                address: section.address(),
                size: section.size(),
                class: section_class(section.kind()),
            },
        ));
    }

    let mut rule_matches = vec![0_u64; policy.rules.len()];
    let mut allocations = Vec::new();
    for symbol in elf.symbols() {
        if !symbol.is_definition()
            || symbol.size() == 0
            || symbol.address() == 0
            || symbol.kind() == SymbolKind::Text
        {
            continue;
        }
        let Ok(raw_name) = symbol.name() else {
            continue;
        };
        let demangled = try_demangle(raw_name)
            .map(|name| name.to_string())
            .unwrap_or_else(|_| raw_name.to_owned());
        let rule_index = policy.rules.iter().position(|rule| {
            wildcard_matches(&rule.symbol, raw_name)
                || (demangled != raw_name && wildcard_matches(&rule.symbol, &demangled))
        });
        if let Some(index) = rule_index {
            rule_matches[index] = rule_matches[index].saturating_add(1);
        }
        let region = region_for_range(policy, symbol.address(), symbol.size())
            .map(|region| region.id.clone());
        let section = symbol
            .section_index()
            .and_then(|index| section_names.get(&index))
            .cloned()
            .unwrap_or_else(|| "<absolute-or-unknown>".into());
        allocations.push(Allocation {
            symbol: raw_name.to_owned(),
            demangled,
            section,
            region,
            address: symbol.address(),
            size: symbol.size(),
            rule_index,
        });
    }

    validate_rules(policy, &allocations, &rule_matches, &mut audit);
    validate_attribution_overlap(&allocations, &mut audit);
    let consumers = build_consumers(policy, &allocations);
    let regions = build_regions(policy, &sections, &reservations, &allocations);
    let mut largest_unclassified = allocations
        .iter()
        .filter(|allocation| allocation.rule_index.is_none() && allocation.region.is_some())
        .cloned()
        .collect::<Vec<_>>();
    largest_unclassified.sort_by_key(|allocation| std::cmp::Reverse(allocation.size));
    largest_unclassified.truncate(20);

    Ok(MemoryReport {
        schema: 1,
        elf: elf_path.to_owned(),
        regions,
        consumers,
        largest_unclassified,
        audit,
    })
}

pub fn audit(report: &MemoryReport) -> Result<()> {
    if report.audit.errors.is_empty() {
        return Ok(());
    }
    Err(Error::Audit(report.audit.errors.join("\n")))
}

fn is_allocated(flags: &SectionFlags, address: u64) -> bool {
    match flags {
        SectionFlags::Elf { sh_flags } => sh_flags & u64::from(object::elf::SHF_ALLOC) != 0,
        _ => address != 0,
    }
}

fn section_class(kind: SectionKind) -> SectionClass {
    match kind {
        SectionKind::Text => SectionClass::Executable,
        SectionKind::ReadOnlyData | SectionKind::ReadOnlyString => SectionClass::ReadOnly,
        _ => SectionClass::Mutable,
    }
}

fn region_for_range(
    policy: &MemoryPolicy,
    address: u64,
    size: u64,
) -> Option<&crate::RegionPolicy> {
    let end = address.checked_add(size)?;
    policy
        .regions
        .iter()
        .find(|region| address >= region.start && end <= region.end)
}

fn resolve_reservations(
    policy: &MemoryPolicy,
    symbols: &HashMap<String, u64>,
    audit: &mut AuditReport,
) -> BTreeMap<String, Vec<ReservationReport>> {
    let mut result = BTreeMap::<String, Vec<ReservationReport>>::new();
    for reserve in &policy.reserves {
        let Some(&start) = symbols.get(&reserve.start_symbol) else {
            audit.errors.push(format!(
                "reservation {:?} start symbol {:?} is missing",
                reserve.id, reserve.start_symbol
            ));
            continue;
        };
        let Some(&end) = symbols.get(&reserve.end_symbol) else {
            audit.errors.push(format!(
                "reservation {:?} end symbol {:?} is missing",
                reserve.id, reserve.end_symbol
            ));
            continue;
        };
        let Some(region) = policy
            .regions
            .iter()
            .find(|region| region.id == reserve.region)
        else {
            continue;
        };
        if start >= end || start < region.start || end > region.end {
            audit.errors.push(format!(
                "reservation {:?} range {start:#x}..{end:#x} is invalid for region {:?}",
                reserve.id, reserve.region
            ));
            continue;
        }
        result
            .entry(reserve.region.clone())
            .or_default()
            .push(ReservationReport {
                id: reserve.id.clone(),
                start,
                end,
                size: end - start,
                reason: reserve.reason,
            });
    }
    result
}

fn validate_rules(
    policy: &MemoryPolicy,
    allocations: &[Allocation],
    matches: &[u64],
    audit: &mut AuditReport,
) {
    for (index, rule) in policy.rules.iter().enumerate() {
        if matches[index] == 0 && !rule.optional {
            audit.errors.push(format!(
                "required consumer rule {:?} ({}) matched no ELF symbol",
                rule.symbol, rule.owner
            ));
        }
    }
    for allocation in allocations {
        let Some(index) = allocation.rule_index else {
            continue;
        };
        let rule = &policy.rules[index];
        if allocation.region.as_deref() != Some(rule.region.as_str()) {
            audit.errors.push(format!(
                "{} expected in region {:?}, found {:?} at {:#x}",
                rule.owner, rule.region, allocation.region, allocation.address
            ));
            continue;
        }
        let actual_kind = policy
            .regions
            .iter()
            .find(|region| region.id == rule.region)
            .map(|region| region.kind);
        let required = match rule.placement {
            PlacementRequirement::RequiredSram => Some(RegionKind::Sram),
            PlacementRequirement::RequiredPsram => Some(RegionKind::Psram),
            _ => None,
        };
        if let Some(required) = required
            && actual_kind != Some(required)
        {
            audit.errors.push(format!(
                "{} requires {required:?}, but region {:?} is {actual_kind:?}",
                rule.owner, rule.region
            ));
        }
        let preferred = match rule.placement {
            PlacementRequirement::PreferredSram => Some(RegionKind::Sram),
            PlacementRequirement::PreferredPsram => Some(RegionKind::Psram),
            _ => None,
        };
        if let Some(preferred) = preferred
            && actual_kind != Some(preferred)
        {
            audit.warnings.push(format!(
                "{} prefers {preferred:?}, but region {:?} is {actual_kind:?}",
                rule.owner, rule.region
            ));
        }
    }
}

fn validate_attribution_overlap(allocations: &[Allocation], audit: &mut AuditReport) {
    let mut attributed = allocations
        .iter()
        .filter(|allocation| allocation.rule_index.is_some() && allocation.region.is_some())
        .collect::<Vec<_>>();
    attributed.sort_by_key(|allocation| (allocation.region.as_deref(), allocation.address));
    for pair in attributed.windows(2) {
        let [left, right] = pair else { continue };
        if left.region == right.region && left.address.saturating_add(left.size) > right.address {
            audit.errors.push(format!(
                "policy-attributed symbols overlap: {} and {}",
                left.demangled, right.demangled
            ));
        }
    }
}

fn build_consumers(policy: &MemoryPolicy, allocations: &[Allocation]) -> Vec<ConsumerReport> {
    let mut consumers = BTreeMap::<String, ConsumerReport>::new();
    for allocation in allocations {
        let Some(index) = allocation.rule_index else {
            continue;
        };
        let rule = &policy.rules[index];
        let consumer = consumers
            .entry(rule.owner.clone())
            .or_insert_with(|| ConsumerReport {
                owner: rule.owner.clone(),
                scope: rule.scope,
                reason: rule.reason,
                placement: rule.placement,
                region: rule.region.clone(),
                bytes: 0,
                symbols: 0,
                declared_count: rule.count,
                element_capacity: rule.element_capacity,
                optimization: rule.optimization.clone(),
                allocations: Vec::new(),
            });
        consumer.bytes = consumer.bytes.saturating_add(allocation.size);
        consumer.symbols = consumer.symbols.saturating_add(1);
        consumer.allocations.push(allocation.clone());
    }
    let mut result = consumers.into_values().collect::<Vec<_>>();
    result.sort_by_key(|consumer| std::cmp::Reverse(consumer.bytes));
    result
}

fn build_regions(
    policy: &MemoryPolicy,
    sections: &[(String, SectionReport)],
    reservations: &BTreeMap<String, Vec<ReservationReport>>,
    allocations: &[Allocation],
) -> Vec<RegionReport> {
    policy
        .regions
        .iter()
        .map(|region| {
            let region_sections = sections
                .iter()
                .filter(|(id, _)| id == &region.id)
                .map(|(_, section)| section.clone())
                .collect::<Vec<_>>();
            let reservation_list = reservations.get(&region.id).cloned().unwrap_or_default();
            let section_ranges = region_sections
                .iter()
                .map(|section| (section.address, section.address + section.size))
                .collect::<Vec<_>>();
            let reserve_ranges = reservation_list
                .iter()
                .map(|reserve| (reserve.start, reserve.end))
                .collect::<Vec<_>>();
            let mut occupied_ranges = section_ranges.clone();
            occupied_ranges.extend(reserve_ranges.iter().copied());
            let allocated = union_length(&section_ranges);
            let reserved = union_length(&reserve_ranges);
            let occupied = union_length(&occupied_ranges);
            let executable = region_sections
                .iter()
                .filter(|section| section.class == SectionClass::Executable)
                .map(|section| section.size)
                .sum();
            let read_only = region_sections
                .iter()
                .filter(|section| section.class == SectionClass::ReadOnly)
                .map(|section| section.size)
                .sum();
            let mutable = region_sections
                .iter()
                .filter(|section| section.class == SectionClass::Mutable)
                .map(|section| section.size)
                .sum();
            let attributed_ranges = allocations
                .iter()
                .filter(|allocation| {
                    allocation.rule_index.is_some()
                        && allocation.region.as_deref() == Some(region.id.as_str())
                })
                .map(|allocation| (allocation.address, allocation.address + allocation.size))
                .collect::<Vec<_>>();
            let policy_attributed = union_length(&attributed_ranges);
            RegionReport {
                id: region.id.clone(),
                kind: region.kind,
                start: region.start,
                end: region.end,
                capacity: region.end - region.start,
                allocated,
                executable,
                read_only,
                mutable,
                policy_attributed,
                mutable_unattributed: mutable.saturating_sub(policy_attributed),
                reserved,
                free: (region.end - region.start).saturating_sub(occupied),
                sections: region_sections,
                reservations: reservation_list,
            }
        })
        .collect()
}

fn union_length(ranges: &[(u64, u64)]) -> u64 {
    let mut ranges = ranges
        .iter()
        .copied()
        .filter(|(start, end)| start < end)
        .collect::<Vec<_>>();
    ranges.sort_unstable();
    let mut total = 0_u64;
    let mut current: Option<(u64, u64)> = None;
    for (start, end) in ranges {
        match current {
            Some((current_start, current_end)) if start <= current_end => {
                current = Some((current_start, current_end.max(end)));
            }
            Some((current_start, current_end)) => {
                total = total.saturating_add(current_end - current_start);
                current = Some((start, end));
            }
            None => current = Some((start, end)),
        }
    }
    if let Some((start, end)) = current {
        total = total.saturating_add(end - start);
    }
    total
}

#[cfg(test)]
mod tests {
    use super::{analyze, union_length};
    use crate::{
        ConsumerRule, ConsumerScope, MemoryPolicy, PlacementReason, PlacementRequirement,
        RegionKind, RegionPolicy,
    };

    #[test]
    fn interval_union_ignores_overlap_and_adjacency() {
        assert_eq!(union_length(&[(10, 20), (15, 25), (25, 30), (40, 42)]), 22);
    }

    #[test]
    fn native_elf_analysis_uses_actual_sections() {
        let executable = std::env::current_exe().unwrap();
        let policy = broad_native_policy(Vec::new());
        let report = analyze(&executable, &policy).unwrap();
        assert!(report.regions[0].allocated > 0);
        assert!(report.audit.errors.is_empty());
    }

    #[test]
    fn required_rule_that_matches_nothing_fails_closed() {
        let executable = std::env::current_exe().unwrap();
        let policy = broad_native_policy(vec![ConsumerRule {
            symbol: "__open_radio_symbol_that_cannot_exist__".into(),
            owner: "test.missing".into(),
            scope: ConsumerScope::Runtime,
            reason: PlacementReason::Other,
            placement: PlacementRequirement::Neutral,
            region: "native".into(),
            optional: false,
            count: None,
            element_capacity: None,
            optimization: None,
        }]);
        let report = analyze(&executable, &policy).unwrap();
        assert_eq!(report.audit.errors.len(), 1);
        assert!(report.audit.errors[0].contains("matched no ELF symbol"));
    }

    fn broad_native_policy(rules: Vec<ConsumerRule>) -> MemoryPolicy {
        MemoryPolicy {
            schema: 1,
            regions: vec![RegionPolicy {
                id: "native".into(),
                kind: RegionKind::Other,
                start: 1,
                end: u64::MAX,
            }],
            reserves: Vec::new(),
            rules,
        }
    }
}
