//! Focused project-aware investigation of one MMIO register.

use std::collections::BTreeMap;

use serde::Serialize;

use super::super::*;

#[derive(Serialize)]
struct RegisterInvestigationReport {
    schema_version: u32,
    command: &'static str,
    register: crate::RegisterDetailSummary,
    neighbors: Vec<RegisterNeighbor>,
    recording: Option<RegisterRecordingGuide>,
    conclusion: String,
}

#[derive(Serialize)]
struct RegisterNeighbor {
    address: u32,
    width: u8,
    name: String,
    reviewed: bool,
}

#[derive(Serialize)]
struct RegisterRecordingGuide {
    subject: String,
    reviewed_knowledge_packs: Vec<String>,
    supported_register_facts: Vec<&'static str>,
    supported_field_facts: Vec<&'static str>,
    field_subject_suffix: &'static str,
    evidence_rule: &'static str,
    reuse_rule: &'static str,
}

pub(super) fn run(
    arguments: InspectRegisterArgs,
    project: &ProjectSpec,
    catalog: &MmioMap,
) -> Result<bool> {
    let address = parse_address(&arguments.address)?;
    let detail = crate::application::register_detail_for_project(project, catalog, address)?
        .ok_or_else(|| {
            crate::Error::invalid(format!(
                "MMIO address {address:#010x} is absent from discovery facts, the reviewed model and loaded SVD catalogs"
            ))
        })?;
    let report = RegisterInvestigationReport {
        neighbors: neighbors(project, &detail)?,
        recording: recording_guide(project, &detail)?,
        conclusion: conclusion(&detail),
        schema_version: 2,
        command: "inspect register",
        register: detail,
    };
    crate::cli::output::render_report(&report, || render_human(&report));
    Ok(true)
}

fn recording_guide(
    project: &ProjectSpec,
    detail: &crate::RegisterDetailSummary,
) -> Result<Option<RegisterRecordingGuide>> {
    let Some(paths) = project.registers.as_ref() else {
        return Ok(None);
    };
    let Some(width) = detail.width else {
        return Ok(None);
    };
    let model = crate::registers::load_effective_register_model(paths)?;
    Ok(Some(RegisterRecordingGuide {
        subject: format!(
            "mmio:{}:{:#010x}/{width}",
            model.address_space(),
            detail.address
        ),
        reviewed_knowledge_packs: project
            .reviewed_knowledge
            .iter()
            .map(|path| path.display().to_string())
            .collect(),
        supported_register_facts: vec![
            "register-declaration",
            "register-name",
            "register-description",
            "register-access",
            "hardware-write-semantics",
        ],
        supported_field_facts: vec![
            "field-name",
            "field-description",
            "field-access",
            "field-write-semantics",
        ],
        field_subject_suffix: "#bits:<offset>/<width>",
        evidence_rule: "Add an assertion only after manual review and link it to durable evidence; generated reads, writes, masks, names and neighboring addresses are candidates, not hardware truth.",
        reuse_rule: "Keep a blob-specific conclusion in a project reviewed-knowledge pack; promote it to the chip baseline only when independently reviewed and reusable across investigations.",
    }))
}

fn parse_address(value: &str) -> Result<u32> {
    value
        .strip_prefix("0x")
        .map_or_else(|| value.parse(), |digits| u32::from_str_radix(digits, 16))
        .map_err(|_| crate::Error::invalid(format!("invalid MMIO address {value:?}")))
}

fn neighbors(
    project: &ProjectSpec,
    selected: &crate::RegisterDetailSummary,
) -> Result<Vec<RegisterNeighbor>> {
    let Some(paths) = project.registers.as_ref() else {
        return Ok(Vec::new());
    };
    let identities = paths
        .model
        .is_file()
        .then(|| crate::registers::load_effective_register_model(paths))
        .transpose()?
        .map(|model| model.register_identities())
        .transpose()?
        .unwrap_or_default();
    let facts = paths
        .facts
        .is_file()
        .then(|| crate::registers::RegisterFacts::load(&paths.facts))
        .transpose()?;
    let mut rows = BTreeMap::<(u32, u8), String>::new();
    if let Some(facts) = facts {
        for fact in facts.registers {
            if fact.address.abs_diff(selected.address) <= 0x10 {
                rows.insert((fact.address, fact.width), fact.catalog_name);
            }
        }
    }
    for ((address, width), identity) in &identities {
        let Ok(address) = u32::try_from(*address) else {
            continue;
        };
        let Ok(width) = u8::try_from(*width) else {
            continue;
        };
        if address.abs_diff(selected.address) <= 0x10 {
            rows.insert((address, width), identity.clone());
        }
    }
    Ok(rows
        .into_iter()
        .map(|((address, width), name)| RegisterNeighbor {
            address,
            width,
            name,
            reviewed: crate::registers::physical_register_identity(
                &identities,
                u64::from(address),
                u32::from(width),
            )
            .is_some(),
        })
        .collect())
}

fn conclusion(detail: &crate::RegisterDetailSummary) -> String {
    match detail.review_status {
        crate::RegisterReviewState::Reviewed | crate::RegisterReviewState::Manual => {
            "The register has an explicit reviewed project identity; consult its provenance, accuracy, completeness, and evidence before relying on field semantics.".to_owned()
        }
        crate::RegisterReviewState::NonOperational => {
            "The address is observed exclusively in reviewed non-operational code. Evidence is retained, but it does not control the driver and does not block publication.".to_owned()
        }
        crate::RegisterReviewState::Ignored => {
            "The address lies outside the project-owned publication ranges. It remains visible as external MMIO evidence.".to_owned()
        }
        crate::RegisterReviewState::Unreviewed if detail.writes == 0 => {
            "Only read evidence is known. The hardware meaning and fields are not proven; do not assign a semantic SVD name from address adjacency alone.".to_owned()
        }
        crate::RegisterReviewState::Unreviewed => {
            "The address has operational evidence but no reviewed identity. Review its write patterns and call paths before publishing it.".to_owned()
        }
    }
}

fn render_human(report: &RegisterInvestigationReport) {
    let detail = &report.register;
    outputln!("{}", crate::cli::output::heading("Register"));
    outputln!("Address:      {:#010x}", detail.address);
    outputln!(
        "Name:         {} ({})",
        detail.name,
        detail.name_source.label()
    );
    outputln!(
        "Location:     {} / {}",
        detail.range.as_deref().unwrap_or("unknown range"),
        detail.width.map_or_else(
            || "unknown width".to_owned(),
            |width| format!("{width}-bit")
        )
    );
    outputln!("Review:       {}", detail.review_status.label());
    outputln!(
        "Publication:  {}{}",
        if detail.publication_debt {
            "BLOCKED"
        } else {
            "not blocking"
        },
        if detail.publication_scopes.is_empty() {
            String::new()
        } else {
            format!(" ({})", detail.publication_scopes.join(", "))
        }
    );
    outputln!(
        "Accesses:     reads={} writes={} RMW={}",
        detail.reads,
        detail.writes,
        detail.read_modify_writes
    );
    outputln!("\n{}", crate::cli::output::heading("Conclusion"));
    outputln!("{}", report.conclusion);

    if let Some(recording) = &report.recording {
        outputln!(
            "\n{}",
            crate::cli::output::heading("Record accepted progress")
        );
        outputln!("Subject:      {}", recording.subject);
        if recording.reviewed_knowledge_packs.is_empty() {
            outputln!("Pack:         configure [reviewed-knowledge].packs before recording facts");
        } else {
            outputln!(
                "Pack:         {}",
                recording.reviewed_knowledge_packs.join(", ")
            );
        }
        outputln!(
            "Register facts: {}",
            recording.supported_register_facts.join(", ")
        );
        outputln!(
            "Field facts:    {} (append {})",
            recording.supported_field_facts.join(", "),
            recording.field_subject_suffix
        );
        outputln!("Evidence:     {}", recording.evidence_rule);
        outputln!("Reuse:        {}", recording.reuse_rule);
    }

    if !detail.operational_functions.is_empty()
        || !detail.non_operational_functions.is_empty()
        || !detail.related_functions.is_empty()
    {
        outputln!("\n{}", crate::cli::output::heading("Users"));
        outputln!(
            "{}",
            crate::cli::table::render(
                ["Class", "Function"],
                detail
                    .operational_functions
                    .iter()
                    .map(|function| ["operational".to_owned(), function.clone()])
                    .chain(
                        detail
                            .non_operational_functions
                            .iter()
                            .map(|function| ["non-operational".to_owned(), function.clone(),])
                    )
                    .chain(
                        detail
                            .related_functions
                            .iter()
                            .map(|function| ["related IR alias".to_owned(), function.clone()]),
                    ),
            )
        );
    }
    if !detail.read_sites.is_empty() || !detail.write_sites.is_empty() {
        outputln!("\n{}", crate::cli::output::heading("Instruction sites"));
        outputln!(
            "{}",
            crate::cli::table::render(
                ["Access", "PC", "Function"],
                detail
                    .read_sites
                    .iter()
                    .map(|site| [
                        "read".to_owned(),
                        format!("{:#010x}", site.pc),
                        site.function.clone()
                    ])
                    .chain(detail.write_sites.iter().map(|site| [
                        "write".to_owned(),
                        format!("{:#010x}", site.pc),
                        site.function.clone(),
                    ])),
            )
        );
    }
    if !report.neighbors.is_empty() {
        outputln!("\n{}", crate::cli::output::heading("Register neighborhood"));
        outputln!(
            "{}",
            crate::cli::table::render(
                ["Address", "Width", "Identity", "Review"],
                report.neighbors.iter().map(|neighbor| [
                    format!("{:#010x}", neighbor.address),
                    neighbor.width.to_string(),
                    neighbor.name.clone(),
                    if neighbor.reviewed {
                        "reviewed"
                    } else {
                        "observed"
                    }
                    .to_owned(),
                ]),
            )
        );
    }
    if crate::cli::output::details() && !detail.review_sources.is_empty() {
        outputln!("\nEvidence: {}", detail.review_sources.join(", "));
    }
}
