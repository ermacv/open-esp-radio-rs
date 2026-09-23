//! Frontend projections of the shared, source-preserving register inventory.

use super::{ProjectSession, push_error};
use crate::application::{model::*, register_inventory};
use std::collections::BTreeSet;

pub(super) fn collect(
    resolved: &ProjectSession,
    diagnostics: &mut Vec<DiagnosticRecord>,
) -> RegisterWorkspaceReport {
    let (inventory, publication) = match resolved.register_query() {
        Ok(query) => {
            let publication = match &query.publication.summary {
                Ok(summary) => *summary,
                Err(reason) => {
                    push_error(
                        diagnostics,
                        "registers",
                        crate::Error::invalid(reason.clone()),
                        None,
                    );
                    None
                }
            };
            (
                RegisterInventoryState::Available {
                    snapshot: query.snapshot.clone(),
                },
                publication,
            )
        }
        Err(error) => {
            let reason = error.to_string();
            push_error(diagnostics, "register-inventory", error, None);
            (RegisterInventoryState::Failed { reason }, None)
        }
    };
    let graph = inventory.snapshot().map(|snapshot| snapshot.inventory());
    RegisterWorkspaceReport {
        configured: resolved.project.registers.is_some(),
        model: resolved
            .project
            .registers
            .as_ref()
            .map(|paths| paths.model.clone()),
        ranges: graph.map_or(0, |graph| graph.regions.len()),
        observed: graph.map_or(0, |graph| {
            graph
                .registers
                .values()
                .filter(|register| !register.access_widths.is_empty())
                .count()
        }),
        fields: graph.map_or(0, |graph| {
            graph
                .registers
                .values()
                .map(|register| register.fields.len())
                .sum()
        }),
        publication,
        inventory,
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
    session: &crate::application::ProjectSession,
    selector: &register_inventory::RegisterSelector,
) -> crate::Result<Option<RegisterDetailSummary>> {
    let query = session.register_query()?;
    detail_from_capture(
        &session.project,
        query.snapshot.inventory(),
        &query.publication,
        query.snapshot.id(),
        selector,
    )
}

#[cfg(test)]
pub(crate) fn detail_from_inventory(
    project: &crate::ProjectSpec,
    inventory: register_inventory::RegisterInventory,
    selector: &register_inventory::RegisterSelector,
) -> crate::Result<Option<RegisterDetailSummary>> {
    let snapshot = crate::RegisterInventorySnapshot::new(inventory)?;
    let facts = project
        .registers
        .as_ref()
        .filter(|paths| paths.facts.is_file())
        .map(|paths| crate::registers::RegisterFacts::load(&paths.facts))
        .transpose()
        .map_err(|error| error.to_string());
    let publication =
        crate::application::register_query::PublicationInputs::capture(project, facts);
    detail_from_capture(
        project,
        snapshot.inventory(),
        &publication,
        snapshot.id(),
        selector,
    )
}

fn detail_from_capture(
    project: &crate::ProjectSpec,
    inventory: &register_inventory::RegisterInventory,
    publication: &crate::application::register_query::PublicationInputs,
    snapshot_id: &str,
    selector: &register_inventory::RegisterSelector,
) -> crate::Result<Option<RegisterDetailSummary>> {
    let subjects = inventory
        .resolve_selector(selector)
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    if subjects.is_empty() {
        return Ok(None);
    }
    let address = match selector {
        register_inventory::RegisterSelector::Address(address) => *address,
        register_inventory::RegisterSelector::Subject(_) => subjects[0].subject.address,
    };
    let width = (subjects.len() == 1).then(|| subjects[0].width()).flatten();
    let physical_address = if subjects.len() == 1 {
        subjects[0].subject.address
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
        .flat_map(|subject| subject.evidence_ids().into_iter().cloned())
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
                    address,
                    width: number(&effect["width"]) as u32,
                    function: function.to_owned(),
                    pc: number(&effect["site"]),
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
                address: number(&payload["address"]),
                width: number(&payload["width"]) as u32,
                function: function.to_owned(),
                pc: number(&payload["site"]),
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
                    address: number(&fact["address"]),
                    width: number(&fact["width"]) as u32,
                    function: site["function"].as_str().unwrap_or("unknown").to_owned(),
                    pc: number(&site["pc"]),
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
        .flat_map(|subject| subject.fields.values().map(move |field| (subject, field)))
        .map(|(subject, field)| {
            let linked = field
                .evidence_ids()
                .into_iter()
                .filter_map(|id| inventory.evidence.get(id))
                .filter(|record| record.kind == "linked-register")
                .flat_map(|record| {
                    record.payload["field_candidates"]
                        .as_array()
                        .into_iter()
                        .flatten()
                })
                .filter(|candidate| {
                    field
                        .mask
                        .is_some_and(|mask| number(&candidate["mask"]) == u64::from(mask))
                })
                .collect::<Vec<_>>();
            let union = |key: &str| {
                linked
                    .iter()
                    .flat_map(|candidate| strings(&candidate[key]))
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>()
            };
            RegisterFieldSummary {
                subject: subject.id.clone(),
                field: field.clone(),
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
            }
        })
        .collect();
    let mut coverage_gaps = inventory.gaps.iter().cloned().collect::<Vec<_>>();
    let mut review_sources = Vec::new();
    let mut review_classification = None;
    let mut reviewed = false;
    let mut ownership_unknown = true;
    let mut external = false;
    let mut project_location = false;
    if let Some(paths) = &project.registers {
        let review = (|| -> crate::Result<()> {
            let model = publication.model()?;
            let [register] = subjects.as_slice() else {
                return Err(crate::Error::invalid(
                    "address query matches multiple physical subjects; select an exact subject for publication status",
                ));
            };
            let location = &register.subject;
            if location.chip != model.chip()
                || location.address_space != model.address_space()
                || location.route != "mmio"
                || location.bank.is_some()
            {
                return Err(crate::Error::invalid(
                    "selected physical subject is outside the register model's chip/address-space/MMIO domain",
                ));
            }
            project_location = true;
            if let Some(width) = width {
                let maps = crate::registers::register_identity_maps(model)?;
                let key = (physical_address, width);
                reviewed = maps.reviewed.contains_key(&key);
                if let Some(annotation) = maps.annotations.get(&key) {
                    review_sources = annotation.sources.clone();
                    review_classification = Some(format!(
                        "provenance={:?}, accuracy={:?}, completeness={:?}",
                        annotation.provenance, annotation.accuracy, annotation.completeness
                    ));
                }
            }
            let address = u32::try_from(physical_address).map_err(|_| crate::Error::invalid(
                "publication ownership facts support only 32-bit addresses; ownership is unknown for this subject"))?;
            let width = width.and_then(|width| u8::try_from(width).ok()).ok_or_else(|| crate::Error::invalid(
                "publication ownership requires known geometry representable by discovery facts"))?;
            let facts = publication.facts()?.ok_or_else(|| {
                crate::Error::invalid(
                    "MMIO discovery facts are unavailable in this register snapshot",
                )
            })?;
            external = !crate::registers::classify_register_publication(
                facts,
                &paths.owned_ranges,
                address,
                width,
            )?
            .is_owned();
            ownership_unknown = false;
            Ok(())
        })();
        if let Err(error) = review {
            coverage_gaps.push(
                open_radio_vendor_contracts::register_inventory::CoverageGap {
                    source: "register-workspace".to_owned(),
                    scope: "publication-status".to_owned(),
                    reason: error.to_string(),
                },
            );
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
    let regions = inventory
        .regions
        .iter()
        .filter(|region| {
            region.start <= address
                && address < region.end_exclusive
                && subjects.iter().any(|register| {
                    register.subject.address_space == region.address_space
                        && register.subject.route == "mmio"
                        && register.subject.bank.is_none()
                })
        })
        .cloned()
        .collect();
    let mut publication_scopes = Vec::new();
    if project.review.is_some() && project_location {
        match publication.scopes() {
            Ok(Some(report)) => {
                publication_scopes = report
                    .scopes
                    .iter()
                    .filter(|scope| {
                        scope.publication
                            && scope.mmio.iter().any(|item| {
                                u64::from(item.address) == address
                                    || u64::from(item.address) == physical_address
                            })
                    })
                    .map(|scope| scope.id.clone())
                    .collect()
            }
            Ok(None) => {}
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
        inventory_snapshot: snapshot_id.to_owned(),
        selection: selector.clone(),
        address: physical_address,
        width,
        regions,
        name,
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
        sources: inventory.sources.clone(),
        coverage_gaps,
    }))
}
