//! Register-workspace projection for the read-only workspace snapshot.

use std::collections::BTreeSet;

use super::{ProjectSession, push_error};
use crate::{
    application::model::{
        DiagnosticRecord, RegisterAccessSiteSummary, RegisterDetailSummary, RegisterFieldSummary,
        RegisterNameSource, RegisterPredicateSummary, RegisterReviewState, RegisterSummary,
        RegisterWorkspaceReport, RegisterWritePatternSummary,
    },
    registers::{RegisterFacts, RegisterModel, RegisterReviewIr},
};

pub(super) fn collect(
    resolved: &ProjectSession,
    diagnostics: &mut Vec<DiagnosticRecord>,
) -> RegisterWorkspaceReport {
    let configured = resolved.project.registers.is_some();
    let model = resolved
        .project
        .registers
        .as_ref()
        .map(|paths| paths.model.clone());
    let summary = resolved.project.registers.as_ref().and_then(|paths| {
        if !paths.model.is_file() {
            return None;
        }
        match resolved.register_workspace().and_then(|workspace| {
            workspace.map_or_else(|| Ok(None), |workspace| workspace.summary().map(Some))
        }) {
            Ok(summary) => summary,
            Err(error) => {
                push_error(diagnostics, "registers", error, Some(paths.model.clone()));
                None
            }
        }
    });
    RegisterWorkspaceReport {
        configured,
        model,
        ranges: summary.map_or(0, |summary| summary.ranges),
        observed: summary.map_or(0, |summary| summary.observed),
        reviewed: summary.map_or(0, |summary| summary.reviewed),
        ignored: summary.map_or(0, |summary| summary.ignored),
        non_operational: summary.map_or(0, |summary| summary.non_operational),
        manual: summary.map_or(0, |summary| summary.manual),
        unreviewed: summary.map_or(0, |summary| summary.unreviewed),
        fields: summary.map_or(0, |summary| summary.fields),
        registers: resolved
            .mmio
            .registers
            .iter()
            .map(|register| RegisterSummary {
                address: register.address,
                name: register.name.clone(),
            })
            .collect(),
    }
}

pub(super) fn detail(
    resolved: &ProjectSession,
    address: u32,
) -> crate::Result<Option<RegisterDetailSummary>> {
    let catalog = resolved.mmio.register(address);
    let Some(paths) = resolved.project.registers.as_ref() else {
        return Ok(catalog.map(|register| catalog_only_detail(address, register.name.clone())));
    };

    let facts = paths
        .facts
        .is_file()
        .then(|| RegisterFacts::load(&paths.facts))
        .transpose()?;
    let model = paths
        .model
        .is_file()
        .then(|| RegisterModel::load(&paths.model))
        .transpose()?;
    let identities = model
        .as_ref()
        .map(RegisterModel::register_identities)
        .transpose()?
        .unwrap_or_default();
    let ir_paths = paths
        .review_ir_reports
        .iter()
        .filter(|path| path.is_dir())
        .cloned()
        .collect::<Vec<_>>();
    let ir = RegisterReviewIr::load_all(&ir_paths)?;

    let width = facts
        .as_ref()
        .into_iter()
        .flat_map(|facts| &facts.registers)
        .find(|fact| fact.address == address)
        .map(|fact| fact.width)
        .or_else(|| {
            identities
                .keys()
                .find(|(candidate, _)| *candidate == u64::from(address))
                .and_then(|(_, width)| u8::try_from(*width).ok())
        })
        .or_else(|| {
            ir.registers
                .keys()
                .find(|(candidate, _)| *candidate == address)
                .map(|(_, width)| *width)
        });
    let fact = facts.as_ref().and_then(|facts| {
        facts
            .registers
            .iter()
            .find(|fact| fact.address == address && width.is_none_or(|width| fact.width == width))
    });
    let identity = identities
        .iter()
        .find(|((candidate, candidate_width), _)| {
            *candidate == u64::from(address)
                && width.is_none_or(|width| u32::from(width) == *candidate_width)
        })
        .map(|(_, identity)| identity);
    let ir_register = width
        .and_then(|width| ir.register(address, width))
        .or_else(|| {
            ir.registers
                .iter()
                .find(|((candidate, _), _)| *candidate == address)
                .map(|(_, register)| register)
        });

    if catalog.is_none() && fact.is_none() && identity.is_none() && ir_register.is_none() {
        return Ok(None);
    }

    let (name, name_source) = if let Some(identity) = identity {
        (identity.clone(), RegisterNameSource::Model)
    } else if let Some(register) = catalog {
        (register.name.clone(), RegisterNameSource::Catalog)
    } else if let Some(fact) = fact {
        (fact.catalog_name.clone(), RegisterNameSource::Discovery)
    } else {
        (format!("MMIO_{address:08X}"), RegisterNameSource::Address)
    };
    let annotation = identity.and_then(|identity| {
        model
            .as_ref()
            .and_then(|model| model.review().iter().find(|item| item.entity == *identity))
    });
    let outside_publication_scope = fact.is_some_and(|fact| {
        facts.as_ref().is_some_and(|facts| {
            facts.ranges.iter().any(|range| {
                range.contains(fact.address) && !paths.owned_ranges.contains(&range.name)
            })
        })
    });
    let non_operational_only = fact.is_some_and(|fact| {
        let configured = paths
            .non_operational_functions
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        crate::registers::fact_is_non_operational(fact, &configured)
    });
    let review_status = match (
        outside_publication_scope,
        identity.is_some(),
        non_operational_only,
        fact.is_some(),
    ) {
        (true, _, _, _) => RegisterReviewState::Ignored,
        (false, true, _, true) => RegisterReviewState::Reviewed,
        (false, true, _, false) => RegisterReviewState::Manual,
        (false, false, true, true) => RegisterReviewState::NonOperational,
        (false, false, _, _) => RegisterReviewState::Unreviewed,
    };

    let mut functions = BTreeSet::new();
    if let Some(fact) = fact {
        functions.extend(fact.read_functions.iter().cloned());
        functions.extend(fact.write_functions.iter().cloned());
    }
    if let Some(register) = ir_register {
        functions.extend(register.functions.iter().cloned());
    }
    let mut fields = ir_register
        .into_iter()
        .flat_map(|register| register.fields.values())
        .map(|field| RegisterFieldSummary {
            least_significant_bit: field.least_significant_bit,
            most_significant_bit: field.most_significant_bit,
            mask: field.mask,
            write_shapes: field.write_shapes,
            predicate_shapes: field.predicate_shapes,
            poll_shapes: field.poll_shapes,
            functions: field.functions.iter().cloned().collect(),
            predicate_functions: field.predicate_functions.iter().cloned().collect(),
            semantic_operations: field.semantic_operations.iter().cloned().collect(),
            semantic_roots: field.semantic_roots.iter().cloned().collect(),
            predicates: field
                .predicate_evidence
                .iter()
                .map(|predicate| RegisterPredicateSummary {
                    kind: predicate.kind.clone(),
                    function: predicate.function.clone(),
                    producer_path: predicate.producer_path.clone(),
                    condition: predicate.condition.clone(),
                    effective_operation: predicate.effective_operation.clone(),
                    register_comparison_value: predicate.register_comparison_value,
                    transitive: predicate.producer_path.len() > 1,
                })
                .collect(),
        })
        .collect::<Vec<_>>();
    if fields.is_empty()
        && let Some(fact) = fact
    {
        fields.extend(
            fact.candidate_masks
                .iter()
                .filter(|mask| **mask != 0)
                .map(|mask| RegisterFieldSummary {
                    least_significant_bit: mask.trailing_zeros() as u8,
                    most_significant_bit: (31 - mask.leading_zeros()) as u8,
                    mask: *mask,
                    write_shapes: 0,
                    predicate_shapes: 0,
                    poll_shapes: 0,
                    functions: fact.write_functions.iter().cloned().collect(),
                    predicate_functions: Vec::new(),
                    semantic_operations: Vec::new(),
                    semantic_roots: Vec::new(),
                    predicates: Vec::new(),
                }),
        );
    }
    let semantic_operations = fields
        .iter()
        .flat_map(|field| field.semantic_operations.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let write_patterns = fact
        .into_iter()
        .flat_map(|fact| &fact.write_patterns)
        .map(|pattern| RegisterWritePatternSummary {
            occurrences: pattern.occurrences,
            modified_mask: pattern.modified_mask,
            preserved_mask: pattern.preserved_mask,
            inverted_mask: pattern.inverted_mask,
            forced_zero_mask: pattern.forced_zero_mask,
            forced_one_mask: pattern.forced_one_mask,
            read_derived_mask: pattern.read_derived_mask,
            dynamic_mask: pattern.dynamic_mask,
            functions: pattern.functions.iter().cloned().collect(),
        })
        .collect::<Vec<_>>();
    let read_modify_writes = write_patterns
        .iter()
        .filter(|pattern| pattern.read_derived_mask != 0 || pattern.preserved_mask != 0)
        .map(|pattern| pattern.occurrences)
        .sum();

    Ok(Some(RegisterDetailSummary {
        address,
        width,
        name,
        name_source,
        review_status,
        review_confidence: annotation.and_then(|item| item.confidence.clone()),
        review_sources: annotation.map_or_else(Vec::new, |item| item.sources.clone()),
        reads: fact.map_or(0, |fact| fact.reads),
        writes: fact.map_or(0, |fact| fact.writes),
        read_modify_writes,
        read_functions: fact.map_or_else(Vec::new, |fact| {
            fact.read_functions.iter().cloned().collect()
        }),
        write_functions: fact.map_or_else(Vec::new, |fact| {
            fact.write_functions.iter().cloned().collect()
        }),
        read_sites: fact.map_or_else(Vec::new, |fact| {
            fact.read_sites
                .iter()
                .map(|site| RegisterAccessSiteSummary {
                    function: site.function.clone(),
                    pc: site.pc,
                })
                .collect()
        }),
        write_sites: fact.map_or_else(Vec::new, |fact| {
            fact.write_sites
                .iter()
                .map(|site| RegisterAccessSiteSummary {
                    function: site.function.clone(),
                    pc: site.pc,
                })
                .collect()
        }),
        functions: functions.into_iter().collect(),
        write_patterns,
        fields,
        semantic_operations,
    }))
}

fn catalog_only_detail(address: u32, name: String) -> RegisterDetailSummary {
    RegisterDetailSummary {
        address,
        width: None,
        name,
        name_source: RegisterNameSource::Catalog,
        review_status: RegisterReviewState::Unreviewed,
        review_confidence: None,
        review_sources: Vec::new(),
        reads: 0,
        writes: 0,
        read_modify_writes: 0,
        read_functions: Vec::new(),
        write_functions: Vec::new(),
        read_sites: Vec::new(),
        write_sites: Vec::new(),
        functions: Vec::new(),
        write_patterns: Vec::new(),
        fields: Vec::new(),
        semantic_operations: Vec::new(),
    }
}
