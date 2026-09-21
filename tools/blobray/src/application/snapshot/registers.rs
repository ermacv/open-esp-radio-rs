//! Frontend projections of the shared, source-preserving register inventory.

use super::{ProjectSession, push_error};
use crate::application::{model::*, register_inventory};
use std::collections::BTreeSet;

pub(super) fn collect(
    resolved: &ProjectSession,
    diagnostics: &mut Vec<DiagnosticRecord>,
) -> RegisterWorkspaceReport {
    let summary = match resolved
        .register_workspace()
        .and_then(|workspace| workspace.map(|workspace| workspace.summary()).transpose())
    {
        Ok(summary) => summary,
        Err(error) => {
            push_error(diagnostics, "registers", error, None);
            None
        }
    };
    let registers = match register_inventory::load(&resolved.project, &resolved.mmio) {
        Ok(inventory) => inventory
            .registers
            .values()
            .filter_map(|register| {
                u32::try_from(register.subject.address)
                    .ok()
                    .map(|address| RegisterSummary {
                        address,
                        name: register.label(),
                    })
            })
            .collect(),
        Err(error) => {
            push_error(diagnostics, "register-inventory", error, None);
            Vec::new()
        }
    };
    RegisterWorkspaceReport {
        configured: resolved.project.registers.is_some(),
        model: resolved
            .project
            .registers
            .as_ref()
            .map(|paths| paths.model.clone()),
        ranges: summary.map_or(0, |s| s.ranges),
        observed: summary.map_or(0, |s| s.observed),
        reviewed: summary.map_or(0, |s| s.reviewed),
        ignored: summary.map_or(0, |s| s.ignored),
        non_operational: summary.map_or(0, |s| s.non_operational),
        manual: summary.map_or(0, |s| s.manual),
        unreviewed: summary.map_or(0, |s| s.unreviewed),
        fields: summary.map_or(0, |s| s.fields),
        registers,
    }
}

fn number(value: &serde_json::Value) -> u64 {
    value
        .as_u64()
        .or_else(|| {
            value.as_str().and_then(|s| {
                s.strip_prefix("0x")
                    .map_or_else(|| s.parse().ok(), |s| u64::from_str_radix(s, 16).ok())
            })
        })
        .unwrap_or(0)
}
fn strings(value: &serde_json::Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item.as_str().map(str::to_owned))
        .collect()
}

pub(crate) fn detail(
    project: &crate::ProjectSpec,
    catalog: &crate::MmioMap,
    address: u32,
) -> crate::Result<Option<RegisterDetailSummary>> {
    detail_from_inventory(
        project,
        register_inventory::load(project, catalog)?,
        address,
    )
}

pub(crate) fn detail_from_inventory(
    project: &crate::ProjectSpec,
    inventory: register_inventory::RegisterInventory,
    address: u32,
) -> crate::Result<Option<RegisterDetailSummary>> {
    let subjects = inventory
        .at_address(u64::from(address))
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    if subjects.is_empty() {
        return Ok(None);
    }
    let width = if subjects.len() == 1 {
        subjects[0]
            .width()
            .and_then(|width| u8::try_from(width).ok())
    } else {
        None
    };
    let physical_address = if subjects.len() == 1 {
        u32::try_from(subjects[0].subject.address).unwrap_or(address)
    } else {
        address
    };
    let names = subjects
        .iter()
        .flat_map(|subject| subject.names.values().into_iter().cloned())
        .collect::<BTreeSet<_>>();
    let name = if names.is_empty() {
        format!("UNKNOWN@{address:#010x}")
    } else {
        names.into_iter().collect::<Vec<_>>().join(" | ")
    };
    let ids = subjects
        .iter()
        .flat_map(|subject| subject.evidence.iter().cloned())
        .collect::<BTreeSet<_>>();
    let evidence = ids
        .iter()
        .filter_map(|id| inventory.evidence.get(id).cloned())
        .collect::<Vec<_>>();

    let mut read_functions = BTreeSet::new();
    let mut write_functions = BTreeSet::new();
    let mut read_sites = Vec::new();
    let mut write_sites = Vec::new();
    let mut write_patterns = Vec::new();
    for item in &evidence {
        if item.kind == "function-use" {
            let function = item.payload["identity"].as_str().unwrap_or("unknown");
            for effect in item.payload["instruction_effects"]
                .as_array()
                .into_iter()
                .flatten()
            {
                if effect["kind"].as_str() != Some("mmio") {
                    continue;
                }
                let address = number(&effect["address"]);
                if !subjects.iter().any(|subject| {
                    subject.subject.address == address
                        || subject.width().is_some_and(|width| {
                            subject.subject.address < address
                                && address
                                    < subject
                                        .subject
                                        .address
                                        .saturating_add(u64::from(width).div_ceil(8))
                        })
                }) {
                    continue;
                }
                let (functions, sites) = match effect["access"].as_str() {
                    Some("read") => (&mut read_functions, &mut read_sites),
                    Some("write") => (&mut write_functions, &mut write_sites),
                    _ => continue,
                };
                functions.insert(function.to_owned());
                sites.push(RegisterAccessSiteSummary {
                    evidence: BTreeSet::from([item.id.clone()]),
                    source_identity: item.identity.to_string(),
                    address: address as u32,
                    width: number(&effect["width"]) as u8,
                    function: function.to_owned(),
                    pc: number(&effect["site"]) as u32,
                });
            }
            continue;
        }
        if item.kind == "instruction-access" {
            let payload = &item.payload;
            let function = payload["function"].as_str().unwrap_or("unknown");
            let (functions, sites) = match payload["access"].as_str() {
                Some("Read") => (&mut read_functions, &mut read_sites),
                Some("Write") => (&mut write_functions, &mut write_sites),
                _ => continue,
            };
            functions.insert(function.to_owned());
            sites.push(RegisterAccessSiteSummary {
                evidence: BTreeSet::from([item.id.clone()]),
                source_identity: item.identity.to_string(),
                address: number(&payload["address"]) as u32,
                width: number(&payload["width"]) as u8,
                function: function.to_owned(),
                pc: number(&payload["site"]) as u32,
            });
            continue;
        }
        if item.kind != "discovery-access" {
            continue;
        }
        let fact = &item.payload;

        read_functions.extend(strings(&fact["read_functions"]));
        write_functions.extend(strings(&fact["write_functions"]));
        for (key, sites) in [
            ("read_sites", &mut read_sites),
            ("write_sites", &mut write_sites),
        ] {
            for site in fact[key].as_array().into_iter().flatten() {
                sites.push(RegisterAccessSiteSummary {
                    evidence: BTreeSet::from([item.id.clone()]),
                    source_identity: item.identity.to_string(),
                    address: number(&fact["address"]) as u32,
                    width: number(&fact["width"]) as u8,
                    function: site["function"].as_str().unwrap_or("unknown").to_owned(),
                    pc: number(&site["pc"]) as u32,
                });
            }
        }
        for pattern in fact["write_patterns"].as_array().into_iter().flatten() {
            write_patterns.push(RegisterWritePatternSummary {
                occurrences: number(&pattern["occurrences"]) as usize,
                modified_mask: number(&pattern["modified_mask"]) as u32,
                preserved_mask: number(&pattern["preserved_mask"]) as u32,
                inverted_mask: number(&pattern["inverted_mask"]) as u32,
                forced_zero_mask: number(&pattern["forced_zero_mask"]) as u32,
                forced_one_mask: number(&pattern["forced_one_mask"]) as u32,
                read_derived_mask: number(&pattern["read_derived_mask"]) as u32,
                dynamic_mask: number(&pattern["dynamic_mask"]) as u32,
                functions: strings(&pattern["functions"]),
            });
        }
    }
    fn deduplicate(sites: Vec<RegisterAccessSiteSummary>) -> Vec<RegisterAccessSiteSummary> {
        let mut unique = std::collections::BTreeMap::<_, RegisterAccessSiteSummary>::new();
        for site in sites {
            let key = (
                site.source_identity.clone(),
                site.address,
                site.width,
                site.function.clone(),
                site.pc,
            );
            if let Some(existing) = unique.get_mut(&key) {
                existing.evidence.extend(site.evidence);
            } else {
                unique.insert(key, site);
            }
        }
        unique.into_values().collect()
    }
    let read_sites = deduplicate(read_sites);
    let write_sites = deduplicate(write_sites);
    let reads = read_sites.len();
    let writes = write_sites.len();
    let read_modify_writes = write_patterns
        .iter()
        .filter(|pattern| {
            pattern.preserved_mask | pattern.inverted_mask | pattern.read_derived_mask != 0
        })
        .map(|pattern| pattern.occurrences)
        .sum();
    let functions = subjects
        .iter()
        .flat_map(|subject| subject.functions.iter().cloned())
        .collect::<BTreeSet<_>>();
    let accessed = read_functions
        .union(&write_functions)
        .cloned()
        .collect::<BTreeSet<_>>();
    let configured = project
        .registers
        .as_ref()
        .map(|paths| {
            paths
                .non_operational_functions
                .iter()
                .cloned()
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    let fields: Vec<_> = subjects
        .iter()
        .flat_map(|subject| subject.fields.values())
        .filter_map(|field| {
            let linked = field
                .evidence
                .iter()
                .filter_map(|id| inventory.evidence.get(id))
                .filter(|record| record.kind == "linked-register")
                .flat_map(|record| {
                    record.payload["field_candidates"]
                        .as_array()
                        .into_iter()
                        .flatten()
                })
                .filter(|candidate| number(&candidate["mask"]) as u32 == field.mask.unwrap_or(0))
                .collect::<Vec<_>>();
            let union = |key: &str| {
                linked
                    .iter()
                    .flat_map(|candidate| strings(&candidate[key]))
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>()
            };
            Some(RegisterFieldSummary {
                mask: field.mask?,
                names: field.names.values().into_iter().cloned().collect(),
                kind: field.kind.clone(),
                evidence: field.evidence.iter().cloned().collect(),
                least_significant_bit: field.offset as u8,
                most_significant_bit: (field.offset + field.width - 1) as u8,
                write_shapes: linked
                    .iter()
                    .map(|candidate| number(&candidate["write_shapes"]) as usize)
                    .max()
                    .unwrap_or(0),
                predicate_shapes: linked
                    .iter()
                    .map(|candidate| number(&candidate["predicate_shapes"]) as usize)
                    .max()
                    .unwrap_or(0),
                poll_shapes: linked
                    .iter()
                    .map(|candidate| number(&candidate["poll_shapes"]) as usize)
                    .max()
                    .unwrap_or(0),
                functions: union("functions"),
                predicate_functions: union("predicate_functions"),
                semantic_operations: union("semantic_operations"),
                semantic_roots: union("semantic_roots"),
                predicates: linked
                    .iter()
                    .flat_map(|candidate| {
                        candidate["predicate_evidence"]
                            .as_array()
                            .into_iter()
                            .flatten()
                    })
                    .map(|predicate| RegisterPredicateSummary {
                        kind: predicate["kind"].as_str().unwrap_or("unknown").to_owned(),
                        function: predicate["function"]
                            .as_str()
                            .unwrap_or("unknown")
                            .to_owned(),
                        producer_path: strings(&predicate["producer_path"]),
                        condition: predicate["condition"]
                            .as_str()
                            .unwrap_or("unknown")
                            .to_owned(),
                        effective_operation: predicate["effective_operation"]
                            .as_str()
                            .map(str::to_owned),
                        register_comparison_value: predicate["register_comparison_value"]
                            .as_u64()
                            .map(|n| n as u32),
                        transitive: strings(&predicate["producer_path"]).len() > 1,
                    })
                    .collect(),
            })
        })
        .collect();
    let mut review_sources = Vec::new();
    let mut review_classification = None;
    let mut reviewed = false;
    if let Some(paths) = &project.registers
        && let Ok(model) = crate::registers::load_effective_register_model(paths)
    {
        let maps = crate::registers::register_identity_maps(&model)?;
        if let Some(width) = width {
            let key = (u64::from(physical_address), u32::from(width));
            reviewed = maps.reviewed.contains_key(&key);
            if let Some(annotation) = maps.annotations.get(&key) {
                review_sources = annotation.sources.clone();
                review_classification = Some(format!(
                    "provenance={:?}, accuracy={:?}, completeness={:?}",
                    annotation.provenance, annotation.accuracy, annotation.completeness
                ));
            }
        }
    }

    let mut ownership_unknown = false;
    let mut external = false;
    if let Some(paths) = &project.registers {
        if let Some(width) = width {
            match crate::registers::RegisterFacts::load(&paths.facts).and_then(|facts| {
                crate::registers::classify_register_publication(
                    &facts,
                    &paths.owned_ranges,
                    physical_address,
                    width,
                )
                .map(|ownership| !ownership.is_owned())
            }) {
                Ok(is_external) => external = is_external,
                Err(_) => ownership_unknown = true,
            }
        } else {
            ownership_unknown = true;
        }
    }
    let non_operational = !accessed.is_empty() && accessed.is_subset(&configured);
    let review_status = if reviewed {
        if accessed.is_empty() {
            RegisterReviewState::Manual
        } else {
            RegisterReviewState::Reviewed
        }
    } else if external {
        RegisterReviewState::Ignored
    } else if non_operational {
        RegisterReviewState::NonOperational
    } else {
        RegisterReviewState::Unreviewed
    };
    let range = inventory
        .regions
        .iter()
        .find(|region| {
            region.start <= u64::from(address) && u64::from(address) < region.end_exclusive
        })
        .map(|region| region.name.clone());
    let mut coverage_gaps = inventory.gaps.into_iter().collect::<Vec<_>>();
    let mut publication_scopes = Vec::new();
    if project.review.is_some() {
        match crate::review_scopes::load_for_project(project) {
            Ok(report) => {
                publication_scopes = report
                    .scopes
                    .into_iter()
                    .filter(|scope| {
                        scope.publication
                            && scope.mmio.iter().any(|item| {
                                item.address == address || item.address == physical_address
                            })
                    })
                    .map(|scope| scope.id)
                    .collect()
            }
            Err(error) => coverage_gaps.push(
                open_radio_vendor_contracts::register_inventory::CoverageGap {
                    source: "review-scopes".to_owned(),
                    scope: "publication-status".to_owned(),
                    reason: error.to_string(),
                },
            ),
        }
    }
    Ok(Some(RegisterDetailSummary {
        address: physical_address,
        width,
        range,
        name,
        name_source: if evidence.iter().any(|item| item.kind == "model-geometry") {
            RegisterNameSource::Model
        } else {
            RegisterNameSource::Address
        },
        review_status,
        publication_debt: if ownership_unknown
            || coverage_gaps
                .iter()
                .any(|gap| gap.scope == "publication-status")
        {
            None
        } else {
            Some(!publication_scopes.is_empty() && !reviewed && !non_operational && !external)
        },
        publication_scopes,
        review_classification,
        review_sources,
        access_count_mode:
            "distinct-static-sites; write-pattern occurrences are per-source maximum-per-path"
                .to_owned(),
        reads,
        writes,
        read_modify_writes,
        read_functions: read_functions.into_iter().collect(),
        write_functions: write_functions.into_iter().collect(),
        operational_functions: accessed.difference(&configured).cloned().collect(),
        non_operational_functions: accessed.intersection(&configured).cloned().collect(),
        related_functions: functions.difference(&accessed).cloned().collect(),
        functions: functions.into_iter().collect(),
        read_sites,
        write_sites,
        write_patterns,
        semantic_operations: fields
            .iter()
            .flat_map(|field| field.semantic_operations.iter().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        fields,
        subjects,
        evidence,
        sources: inventory.sources,
        coverage_gaps,
    }))
}
