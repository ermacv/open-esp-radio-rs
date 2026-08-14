//! Human-oriented bridge from immutable MMIO facts to the reviewed model.
//!
//! The reviewed model owns accepted assertions and evidence links, never the
//! underlying immutable observations.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    path::{Path, PathBuf},
};

use super::{
    RegisterFacts, RegisterModel, ReviewAnnotation,
    review_draft::{candidate_fields, inferred_access, write_draft},
    review_ir::RegisterReviewIr,
    review_ir_markdown::write_ir_evidence,
};
use crate::Result;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RegisterReviewSummary {
    pub(crate) observed: usize,
    pub(crate) reviewed: usize,
    pub(crate) ignored: usize,
    pub(crate) non_operational: usize,
    pub(crate) unreviewed: usize,
    pub(crate) model_only: usize,
    pub(crate) field_candidates: usize,
    pub(crate) ir_reports: usize,
    pub(crate) ir_registers: usize,
    pub(crate) ir_only_registers: usize,
    pub(crate) ir_field_candidates: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RegisterReviewIrSummary {
    pub(crate) reports: usize,
    pub(crate) registers: usize,
    pub(crate) fields: usize,
}

pub(crate) fn inspect_register_review_ir(paths: &[PathBuf]) -> Result<RegisterReviewIrSummary> {
    let ir = RegisterReviewIr::load_all(paths)?;
    Ok(RegisterReviewIrSummary {
        reports: ir.reports.len(),
        registers: ir.registers.len(),
        fields: ir.field_count(),
    })
}

pub(crate) fn render_register_review(
    facts: &RegisterFacts,
    model: &RegisterModel,
    ir_paths: &[PathBuf],
    owned_ranges: &[String],
    non_operational_functions: &[String],
    facts_path: &Path,
    model_path: &Path,
) -> Result<(String, RegisterReviewSummary)> {
    let ir = RegisterReviewIr::load_all(ir_paths)?;
    let identities = model.register_identities()?;
    render_report(
        facts,
        RegisterReviewContext {
            identities: &identities,
            annotations: model.review(),
            ir: &ir,
            owned_ranges,
            non_operational_functions,
            facts_path,
            model_path,
        },
    )
}

struct RegisterReviewContext<'a> {
    identities: &'a BTreeMap<(u64, u32), String>,
    annotations: &'a [ReviewAnnotation],
    ir: &'a RegisterReviewIr,
    owned_ranges: &'a [String],
    non_operational_functions: &'a [String],
    facts_path: &'a Path,
    model_path: &'a Path,
}

fn render_report(
    facts: &RegisterFacts,
    context: RegisterReviewContext<'_>,
) -> Result<(String, RegisterReviewSummary)> {
    let RegisterReviewContext {
        identities,
        annotations,
        ir,
        owned_ranges,
        non_operational_functions,
        facts_path,
        model_path,
    } = context;
    let fact_keys = facts
        .registers
        .iter()
        .map(|fact| (u64::from(fact.address), u32::from(fact.width)))
        .collect::<BTreeSet<_>>();
    let model_keys = identities.keys().copied().collect::<BTreeSet<_>>();
    let owned_ranges = owned_ranges
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let non_operational_functions = non_operational_functions
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    super::workspace::validate_non_operational_functions(facts, &non_operational_functions)?;
    let owned_fact_keys = facts
        .registers
        .iter()
        .filter(|fact| {
            facts.ranges.iter().any(|range| {
                range.contains(fact.address) && owned_ranges.contains(range.name.as_str())
            })
        })
        .map(|fact| (u64::from(fact.address), u32::from(fact.width)))
        .collect::<BTreeSet<_>>();
    let non_operational_fact_keys = facts
        .registers
        .iter()
        .filter(|fact| owned_fact_keys.contains(&(u64::from(fact.address), u32::from(fact.width))))
        .filter(|fact| super::workspace::fact_is_non_operational(fact, &non_operational_functions))
        .map(|fact| (u64::from(fact.address), u32::from(fact.width)))
        .collect::<BTreeSet<_>>();
    let reviewed = owned_fact_keys
        .iter()
        .filter(|identity| identities.contains_key(identity))
        .count();
    let model_only = identities
        .keys()
        .filter(|identity| !fact_keys.contains(identity))
        .count();
    let field_candidates = facts
        .registers
        .iter()
        .filter(|fact| owned_fact_keys.contains(&(u64::from(fact.address), u32::from(fact.width))))
        .filter(|fact| {
            !non_operational_fact_keys.contains(&(u64::from(fact.address), u32::from(fact.width)))
        })
        .map(|fact| candidate_fields(fact, ir.register(fact.address, fact.width)))
        .map(|fields| fields.len())
        .sum();
    let ir_only_registers = ir
        .registers
        .keys()
        .filter(|(address, width)| !fact_keys.contains(&(u64::from(*address), u32::from(*width))))
        .count();
    let summary = RegisterReviewSummary {
        observed: facts.registers.len(),
        reviewed,
        ignored: fact_keys.len() - owned_fact_keys.len(),
        non_operational: non_operational_fact_keys.difference(&model_keys).count(),
        unreviewed: owned_fact_keys
            .difference(&model_keys)
            .filter(|key| !non_operational_fact_keys.contains(key))
            .count(),
        model_only,
        field_candidates,
        ir_reports: ir.reports.len(),
        ir_registers: ir.registers.len(),
        ir_only_registers,
        ir_field_candidates: ir.field_count(),
    };

    let mut output = String::new();
    output.push_str("# Register review report\n\n");
    writeln!(
        output,
        "Generated from `{}` and compared with `{}`.",
        markdown_code(&facts_path.display().to_string()),
        markdown_code(&model_path.display().to_string())
    )
    .expect("writing to String cannot fail");
    if !ir.reports.is_empty() {
        writeln!(
            output,
            "Linked-IR evidence: {}.",
            ir.reports
                .iter()
                .map(|path| format!("`{}`", markdown_code(&path.display().to_string())))
                .collect::<Vec<_>>()
                .join(", ")
        )
        .expect("writing to String cannot fail");
    }
    output.push_str(
        "This file is derived evidence, not the register database. Edit the model fragments, then regenerate this report. Candidate names, access modes and bit ranges are mechanical starting points; linked semantic operations are navigation links only. None of them assert hardware semantics, reset values, W1C behavior or completeness.\n\n",
    );
    output.push_str("## Summary\n\n");
    writeln!(output, "- Observed register widths: {}", summary.observed)
        .expect("writing to String cannot fail");
    writeln!(
        output,
        "- Matched by the reviewed model: {}",
        summary.reviewed
    )
    .expect("writing to String cannot fail");
    writeln!(
        output,
        "- Outside the publication scope: {}",
        summary.ignored
    )
    .expect("writing to String cannot fail");
    writeln!(
        output,
        "- Non-operational-only observations: {}",
        summary.non_operational
    )
    .expect("writing to String cannot fail");
    writeln!(output, "- Awaiting review: {}", summary.unreviewed)
        .expect("writing to String cannot fail");
    writeln!(
        output,
        "- Model-only register widths: {}",
        summary.model_only
    )
    .expect("writing to String cannot fail");
    writeln!(
        output,
        "- Draft field partitions: {}",
        summary.field_candidates
    )
    .expect("writing to String cannot fail");
    writeln!(output, "- Linked-IR reports: {}", summary.ir_reports)
        .expect("writing to String cannot fail");
    writeln!(
        output,
        "- Linked-IR register widths: {}",
        summary.ir_registers
    )
    .expect("writing to String cannot fail");
    writeln!(
        output,
        "- Linked-IR-only register widths: {}",
        summary.ir_only_registers
    )
    .expect("writing to String cannot fail");
    writeln!(
        output,
        "- Linked-IR field candidates: {}",
        summary.ir_field_candidates
    )
    .expect("writing to String cannot fail");

    let annotation_map = annotations
        .iter()
        .map(|annotation| (annotation.entity.as_str(), annotation))
        .collect::<BTreeMap<_, _>>();
    let mut ranges = facts.ranges.iter().collect::<Vec<_>>();
    ranges.sort_by_key(|range| (range.start, range.end, range.name.as_str()));
    for range in ranges {
        let owned = owned_ranges.contains(range.name.as_str());
        writeln!(
            output,
            "\n## Range `{}` (`{:#010x}..{:#010x}`) — {}\n",
            markdown_code(&range.name),
            range.start,
            range.end,
            if owned {
                "owned"
            } else {
                "outside publication scope"
            }
        )
        .expect("writing to String cannot fail");
        output.push_str("| Address | Offset | Width | State | Model identity | Catalog name | Reads | Writes | Modified masks |\n");
        output.push_str("| --- | --- | ---: | --- | --- | --- | ---: | ---: | --- |\n");
        let mut registers = facts
            .registers
            .iter()
            .filter(|fact| range.contains(fact.address))
            .collect::<Vec<_>>();
        registers.sort_by_key(|fact| (fact.address, fact.width));
        for fact in &registers {
            let identity = identities.get(&(u64::from(fact.address), u32::from(fact.width)));
            let non_operational = non_operational_fact_keys
                .contains(&(u64::from(fact.address), u32::from(fact.width)));
            let masks = fact
                .candidate_masks
                .iter()
                .map(|mask| format!("`{mask:#010x}`"))
                .collect::<Vec<_>>()
                .join(", ");
            writeln!(
                output,
                "| `{:#010x}` | `{:#x}` | {} | {} | {} | `{}` | {} | {} | {} |",
                fact.address,
                fact.address - range.start,
                fact.width,
                if !owned {
                    "ignored"
                } else if identity.is_some() {
                    "reviewed"
                } else if non_operational {
                    "non-operational-only"
                } else {
                    "unreviewed"
                },
                identity.map_or_else(
                    || "-".to_owned(),
                    |value| format!("`{}`", markdown_code(value))
                ),
                markdown_code(&fact.catalog_name),
                fact.reads,
                fact.writes,
                if masks.is_empty() { "-" } else { &masks },
            )
            .expect("writing to String cannot fail");
        }

        for fact in registers {
            let identity = identities.get(&(u64::from(fact.address), u32::from(fact.width)));
            let non_operational = non_operational_fact_keys
                .contains(&(u64::from(fact.address), u32::from(fact.width)));
            let ir_register = ir.register(fact.address, fact.width);
            writeln!(
                output,
                "\n### `{:#010x}/{}` — {}\n",
                fact.address,
                fact.width,
                if !owned {
                    "outside publication scope"
                } else if non_operational && identity.is_none() {
                    "non-operational-only"
                } else {
                    identity.map_or("unreviewed", String::as_str)
                }
            )
            .expect("writing to String cannot fail");
            if non_operational && identity.is_none() {
                output.push_str(
                    "This observation is produced exclusively by a function explicitly classified as non-operational in the project review policy. Raw evidence is retained, but it does not create publication review debt.\n\n",
                );
            }
            writeln!(
                output,
                "Mechanical access: `{}`. Read users: {}. Write users: {}.",
                inferred_access(fact),
                function_list(&fact.read_functions),
                function_list(&fact.write_functions)
            )
            .expect("writing to String cannot fail");
            if !fact.read_sites.is_empty() || !fact.write_sites.is_empty() {
                let read_sites = fact
                    .read_sites
                    .iter()
                    .map(|site| {
                        format!("`{:#010x}` in `{}`", site.pc, markdown_code(&site.function))
                    })
                    .collect::<Vec<_>>();
                let write_sites = fact
                    .write_sites
                    .iter()
                    .map(|site| {
                        format!("`{:#010x}` in `{}`", site.pc, markdown_code(&site.function))
                    })
                    .collect::<Vec<_>>();
                writeln!(
                    output,
                    "Instruction sites: reads {}; writes {}.",
                    if read_sites.is_empty() {
                        "-".to_owned()
                    } else {
                        read_sites.join(", ")
                    },
                    if write_sites.is_empty() {
                        "-".to_owned()
                    } else {
                        write_sites.join(", ")
                    },
                )
                .expect("writing to String cannot fail");
            }
            if let Some(identity) = identity
                && let Some(annotation) = annotation_map.get(identity.as_str())
            {
                if let (Some(provenance), Some(accuracy), Some(completeness)) = (
                    annotation.provenance,
                    annotation.accuracy,
                    annotation.completeness,
                ) {
                    writeln!(
                        output,
                        "Fact classification: provenance `{:?}`, accuracy `{:?}`, completeness `{:?}`.",
                        provenance, accuracy, completeness,
                    )
                    .expect("writing to String cannot fail");
                }
                if !annotation.sources.is_empty() {
                    writeln!(
                        output,
                        "Review sources: {}.",
                        annotation
                            .sources
                            .iter()
                            .map(|source| format!("`{}`", markdown_code(source)))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                    .expect("writing to String cannot fail");
                }
            }
            for (index, pattern) in fact.write_patterns.iter().enumerate() {
                writeln!(
                    output,
                    "- Write pattern {}: occurrences={}, modified=`{:#010x}`, preserved=`{:#010x}`, inverted=`{:#010x}`, forced-zero=`{:#010x}`, forced-one=`{:#010x}`, read-derived=`{:#010x}`, dynamic=`{:#010x}`; functions: {}.",
                    index + 1,
                    pattern.occurrences,
                    pattern.modified_mask,
                    pattern.preserved_mask,
                    pattern.inverted_mask,
                    pattern.forced_zero_mask,
                    pattern.forced_one_mask,
                    pattern.read_derived_mask,
                    pattern.dynamic_mask,
                    function_list(&pattern.functions)
                )
                .expect("writing to String cannot fail");
            }
            if let Some(ir_register) = ir_register {
                output.push('\n');
                write_ir_evidence(&mut output, ir_register);
            }
            if owned && identity.is_none() && !non_operational {
                write_draft(&mut output, fact, range.start, ir_register);
            }
        }
    }

    if ir_only_registers != 0 {
        output.push_str("\n## Linked-IR-only registers\n\n");
        output.push_str("These addresses appear in the optional linked-IR inputs but not in the current MMIO discovery facts. They remain navigation evidence, not model entries; refresh `mmio discover` before promoting them.\n\n");
        output.push_str("| Address | Width | Names | Functions | Field candidates |\n| --- | ---: | --- | --- | ---: |\n");
        for ((address, width), register) in &ir.registers {
            if fact_keys.contains(&(u64::from(*address), u32::from(*width))) {
                continue;
            }
            writeln!(
                output,
                "| `{address:#010x}` | {width} | {} | {} | {} |",
                function_list(&register.names),
                function_list(&register.functions),
                register.fields.len()
            )
            .expect("writing to String cannot fail");
        }
    }

    if model_only != 0 {
        output.push_str("\n## Model-only registers\n\n");
        output.push_str("These entries are valid model data but were not observed in the current best-effort discovery facts. Absence is not proof that vendor code never accesses them.\n\n");
        output.push_str("| Address | Width | Model identity |\n| --- | ---: | --- |\n");
        for ((address, width), identity) in identities {
            if !fact_keys.contains(&(*address, *width)) {
                writeln!(
                    output,
                    "| `{address:#010x}` | {width} | `{}` |",
                    markdown_code(identity)
                )
                .expect("writing to String cannot fail");
            }
        }
    }

    Ok((output, summary))
}

fn function_list(functions: &BTreeSet<String>) -> String {
    if functions.is_empty() {
        return "-".to_owned();
    }
    functions
        .iter()
        .map(|function| format!("`{}`", markdown_code(function)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn markdown_code(value: &str) -> String {
    value.replace('`', "'").replace('|', "\\|")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registers::{FactRange, RegisterFact, RegisterWritePatternFact};

    #[test]
    fn report_links_functions_and_emits_safe_unreviewed_drafts() {
        let facts = RegisterFacts {
            ranges: vec![FactRange {
                name: "radio".to_owned(),
                start: 0x1000,
                end: 0x2000,
            }],
            registers: vec![RegisterFact {
                address: 0x1010,
                width: 32,
                catalog_name: "UNMAPPED".to_owned(),
                reads: 1,
                writes: 2,
                read_functions: ["rom:read_status".to_owned()].into(),
                write_functions: ["lib:member.o:enable".to_owned()].into(),
                read_sites: BTreeSet::new(),
                write_sites: BTreeSet::new(),
                write_patterns: vec![RegisterWritePatternFact {
                    occurrences: 2,
                    modified_mask: 0x33,
                    preserved_mask: !0x33,
                    inverted_mask: 0,
                    forced_zero_mask: 0,
                    forced_one_mask: 0x1,
                    read_derived_mask: 0,
                    dynamic_mask: 0x32,
                    functions: ["lib:member.o:enable".to_owned()].into(),
                }],
                candidate_masks: vec![0x33],
            }],
        };
        let (report, summary) = render_report(
            &facts,
            RegisterReviewContext {
                identities: &BTreeMap::new(),
                annotations: &[],
                ir: &RegisterReviewIr::default(),
                owned_ranges: &["radio".to_owned()],
                non_operational_functions: &[],
                facts_path: Path::new("mmio.json"),
                model_path: Path::new("device.toml"),
            },
        )
        .unwrap();
        assert_eq!(summary.unreviewed, 1);
        assert_eq!(summary.field_candidates, 2);
        assert!(report.contains("`rom:read_status`"));
        assert!(report.contains("name = \"REG_00000010_W32\""));
        assert!(report.contains("name = \"FIELD_1_0\""));
        assert!(report.contains("name = \"FIELD_5_4\""));
        assert!(report.contains("Candidate names, access modes and bit ranges are mechanical"));
    }
}
