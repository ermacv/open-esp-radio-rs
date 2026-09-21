//! Focused project-aware investigation of one MMIO register.

use serde::Serialize;

use super::super::*;
use crate::registers::{
    RegisterFacts, RegisterPublicationOwnership, classify_register_publication,
    load_effective_register_model, render_sparse_review_draft,
};

#[derive(Serialize)]
struct RegisterInvestigationReport {
    schema_version: u32,
    command: &'static str,
    register: crate::RegisterDetailSummary,
    neighbors: Vec<RegisterNeighbor>,
    recording: Option<RegisterRecordingGuide>,
    reviewed_assertions: Option<RegisterReviewedAssertions>,
    review_draft: Option<RegisterReviewDraft>,
    conclusion: String,
}

#[derive(Serialize)]
struct RegisterReviewedAssertions {
    subject: open_radio_vendor_contracts::SemanticEntityId,
    completion_claim: bool,
    assertions: Vec<open_radio_vendor_review::EffectiveAssertion>,
}

#[derive(Serialize)]
struct RegisterReviewDraft {
    state: &'static str,
    completion_claim: bool,
    finding_id: String,
    destination: String,
    raw_toml: String,
    validation_actions: Vec<crate::application::ExecutableAction>,
}

#[derive(Serialize)]
struct RegisterNeighbor {
    address: u32,
    subject: String,
    width: Option<u32>,
    name: String,
}

#[derive(Serialize)]
struct RegisterRecordingGuide {
    subject: open_radio_vendor_contracts::SemanticEntityId,
    reviewed_knowledge_destination: Option<String>,
    supported_register_facts: Vec<&'static str>,
    supported_field_facts: Vec<&'static str>,
    field_subject_template: String,
    evidence_rule: &'static str,
    reuse_rule: &'static str,
}

pub(super) fn run(
    arguments: InspectRegisterArgs,
    session: &crate::application::ProjectSession,
) -> Result<bool> {
    let mut inventory =
        crate::application::register_inventory::load(&session.project, &session.mmio)?;
    let address = if let Some(subject) = inventory.registers.get(&arguments.address) {
        u32::try_from(subject.subject.address).map_err(|_| {
            crate::Error::invalid(
                "inspect address exceeds the current 32-bit frontend; use registers list --subject",
            )
        })?
    } else {
        parse_address(&arguments.address)?
    };
    let neighbors = neighbors(&inventory, address);
    if inventory.registers.contains_key(&arguments.address) {
        inventory.registers.retain(|id, _| id == &arguments.address);
    }
    let mut detail =
        crate::application::register_detail_from_inventory(&session.project, inventory, address)?
            .ok_or_else(|| {
            crate::Error::invalid(format!(
                "register {address:#010x} is absent from all configured inventory sources"
            ))
        })?;
    fn optional<T>(
        result: Result<Option<T>>,
        scope: &str,
        detail: &mut crate::RegisterDetailSummary,
    ) -> Option<T> {
        match result {
            Ok(value) => value,
            Err(error) => {
                detail.coverage_gaps.push(
                    open_radio_vendor_contracts::register_inventory::CoverageGap {
                        source: scope.to_owned(),
                        scope: scope.to_owned(),
                        reason: error.to_string(),
                    },
                );
                None
            }
        }
    }
    let recording = optional(
        recording_guide(&session.project, &detail),
        "recording-guide",
        &mut detail,
    );
    let reviewed_assertions = optional(
        reviewed_assertions(session, &detail),
        "reviewed-assertions",
        &mut detail,
    );
    let review_draft = optional(review_draft(session, &detail), "review-draft", &mut detail);
    let report = RegisterInvestigationReport {
        neighbors,
        recording,
        reviewed_assertions,
        review_draft,
        conclusion: conclusion(&detail),
        schema_version: 8,
        command: "inspect register",
        register: detail,
    };
    crate::cli::output::render_report(&report, || render_human(&report));
    Ok(true)
}

fn reviewed_assertions(
    session: &crate::application::ProjectSession,
    detail: &crate::RegisterDetailSummary,
) -> Result<Option<RegisterReviewedAssertions>> {
    let Some(paths) = session.project.registers.as_ref() else {
        return Ok(None);
    };
    let Some(width) = detail.width else {
        return Ok(None);
    };
    let model = load_effective_register_model(paths)?;
    let subject = open_radio_vendor_contracts::SemanticEntityId::register(
        model.chip(),
        model.address_space(),
        u64::from(detail.address),
        u32::from(width),
    )
    .map_err(|error| crate::Error::invalid(error.to_string()))?;
    let knowledge =
        open_radio_vendor_review::ReviewKnowledge::load_all(&session.project.reviewed_knowledge)
            .and_then(|knowledge| knowledge.select_for(&session.project.review_context))
            .map_err(|error| {
                crate::Error::invalid(format!(
                    "cannot inspect reviewed knowledge for register: {error}"
                ))
            })?;
    let assertions = knowledge
        .assertions()
        .values()
        .filter(|assertion| assertion.subject == subject)
        .cloned()
        .collect();
    Ok(Some(RegisterReviewedAssertions {
        subject,
        completion_claim: false,
        assertions,
    }))
}

fn review_draft(
    session: &crate::application::ProjectSession,
    detail: &crate::RegisterDetailSummary,
) -> Result<Option<RegisterReviewDraft>> {
    let Some(paths) = session.project.registers.as_ref() else {
        return Ok(None);
    };
    let Some(destination) = session.project.reviewed_knowledge_default.as_ref() else {
        return Ok(None);
    };
    let Some(width) = detail.width else {
        return Ok(None);
    };
    if !paths.facts.is_file() {
        return Ok(None);
    }
    let facts = RegisterFacts::load(&paths.facts)?;
    let Some(fact) = facts
        .registers
        .iter()
        .find(|fact| fact.address == detail.address && fact.width == width)
    else {
        return Ok(None);
    };
    let ownership =
        classify_register_publication(&facts, &paths.owned_ranges, fact.address, fact.width)?;
    if !may_render_review_draft(detail.review_status, Some(ownership)) {
        return Ok(None);
    }
    let model = load_effective_register_model(paths)?;
    let finding_id = format!("register-{:#010x}-{}", fact.address, fact.width);
    let context = session.context();
    Ok(Some(RegisterReviewDraft {
        state: "review-required",
        completion_claim: false,
        finding_id: finding_id.clone(),
        destination: destination.display().to_string(),
        raw_toml: render_sparse_review_draft(fact, model.chip(), model.address_space()),
        validation_actions: vec![
            context.follow_up_action(
                ["registers", "validate"],
                crate::application::ProjectContextRequirement::Target,
            )?,
            context.follow_up_action(
                ["project", "analyze"],
                crate::application::ProjectContextRequirement::Analysis,
            )?,
            context.follow_up_action(
                [
                    "project".to_owned(),
                    "research".to_owned(),
                    "next".to_owned(),
                    "--finding".to_owned(),
                    finding_id,
                ],
                crate::application::ProjectContextRequirement::Analysis,
            )?,
        ],
    }))
}

fn may_render_review_draft(
    status: crate::RegisterReviewState,
    ownership: Option<RegisterPublicationOwnership<'_>>,
) -> bool {
    status == crate::RegisterReviewState::Unreviewed
        && matches!(ownership, Some(RegisterPublicationOwnership::Owned(_)))
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
        subject: open_radio_vendor_contracts::SemanticEntityId::register(
            model.chip(),
            model.address_space(),
            u64::from(detail.address),
            u32::from(width),
        )
        .map_err(|error| crate::Error::invalid(error.to_string()))?,
        reviewed_knowledge_destination: project
            .reviewed_knowledge_default
            .as_ref()
            .map(|path| path.display().to_string()),
        supported_register_facts: vec![
            "register-identity",
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
        field_subject_template: format!(
            "register-field:{}/{}/{:#x}/{width}/<offset>/<width>",
            model.chip(),
            model.address_space(),
            detail.address
        ),
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

fn neighbors(inventory: &crate::RegisterInventory, address: u32) -> Vec<RegisterNeighbor> {
    inventory
        .registers
        .values()
        .filter(|register| register.subject.address.abs_diff(u64::from(address)) <= 0x10)
        .filter_map(|register| {
            Some(RegisterNeighbor {
                address: u32::try_from(register.subject.address).ok()?,
                subject: register.id.clone(),
                width: register.width(),
                name: register.label(),
            })
        })
        .collect()
}

fn conclusion(detail: &crate::RegisterDetailSummary) -> String {
    match detail.review_status {
        crate::RegisterReviewState::Reviewed | crate::RegisterReviewState::Manual => {
            "The register has an explicit reviewed identity; consult its provenance, accuracy, completeness, and evidence before relying on field semantics.".to_owned()
        }
        crate::RegisterReviewState::NonOperational => {
            "The address is observed exclusively in reviewed non-operational code. Evidence is retained, but it does not control the driver and does not block publication.".to_owned()
        }
        crate::RegisterReviewState::Ignored => {
            "The address lies outside the project-owned publication ranges. It remains visible as external MMIO evidence.".to_owned()
        }
        crate::RegisterReviewState::Unreviewed if detail.writes == 0 && detail.reads > 0 => {
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
        match detail.publication_debt {
            Some(true) => "BLOCKED",
            Some(false) => "not blocking",
            None => "unknown",
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
    for subject in &detail.subjects {
        outputln!("Subject: {}", subject.id);
        outputln!(
            "  physical-width={:?} access-widths={:?} semantics={:?}",
            subject.width(),
            subject.access_widths,
            subject.semantics
        );
        outputln!(
            "  coverage: {}",
            serde_json::to_string(&subject.coverage).expect("coverage serializes")
        );
        for field in subject.fields.values() {
            outputln!(
                "  bits {}..{} {} names={:?} semantics={:?} evidence={:?}",
                field.offset,
                field.offset + field.width - 1,
                field.kind,
                field.names,
                field.semantics,
                field.evidence
            );
        }
    }
    for pattern in &detail.write_patterns {
        outputln!(
            "Write pattern: modified={:#010x} preserved={:#010x} inverted={:#010x} dynamic={:#010x}",
            pattern.modified_mask,
            pattern.preserved_mask,
            pattern.inverted_mask,
            pattern.dynamic_mask
        );
    }
    for gap in &detail.coverage_gaps {
        outputln!("INCOMPLETE {}: {}", gap.scope, gap.reason);
    }
    outputln!(
        "Evidence: {} records; use registers evidence <ID> or JSON for full payloads",
        detail.evidence.len()
    );
    outputln!("\n{}", crate::cli::output::heading("Conclusion"));
    outputln!("{}", report.conclusion);

    if let Some(recording) = &report.recording {
        outputln!(
            "\n{}",
            crate::cli::output::heading("Record accepted progress")
        );
        outputln!("Subject:      {}", recording.subject);
        if let Some(destination) = &recording.reviewed_knowledge_destination {
            outputln!("Pack:         {destination}");
        } else {
            outputln!(
                "Pack:         configure [reviewed-knowledge].packs and default-pack before recording facts"
            );
        }
        outputln!(
            "Register facts: {}",
            recording.supported_register_facts.join(", ")
        );
        outputln!(
            "Field facts:    {} (use {})",
            recording.supported_field_facts.join(", "),
            recording.field_subject_template
        );
        outputln!("Evidence:     {}", recording.evidence_rule);
        outputln!("Reuse:        {}", recording.reuse_rule);
    }

    if let Some(reviewed) = &report.reviewed_assertions {
        outputln!(
            "\n{}",
            crate::cli::output::heading("Applicable reviewed assertions — not a completion claim")
        );
        outputln!("Subject:      {}", reviewed.subject);
        if reviewed.assertions.is_empty() {
            outputln!("No selected reviewed assertion targets this exact physical subject.");
        } else {
            outputln!(
                "{}",
                crate::cli::table::render(
                    ["Pack", "ID", "Kind", "Value", "Evidence"],
                    reviewed.assertions.iter().map(|assertion| [
                        assertion.pack.clone(),
                        assertion.id.clone(),
                        assertion.kind.clone(),
                        serde_json::to_string(&assertion.value)
                            .unwrap_or_else(|_| "<unrenderable>".to_owned()),
                        assertion
                            .metadata
                            .evidence
                            .iter()
                            .map(|evidence| format!("{}:{}", evidence.source, evidence.locator))
                            .collect::<Vec<_>>()
                            .join(", "),
                    ]),
                )
            );
        }
        outputln!("Completion:   false");
    }

    if let Some(draft) = &report.review_draft {
        outputln!(
            "\n{}",
            crate::cli::output::heading("Unaccepted review draft — manual evidence required")
        );
        outputln!("State:        {}", draft.state);
        outputln!("Completion:   false — this template does not prove or complete the finding");
        outputln!("Finding:      {}", draft.finding_id);
        outputln!("Destination:  {}", draft.destination);
        outputln!("\n```toml\n{}```", draft.raw_toml);
        outputln!("After editing and manually reviewing every placeholder:");
        for action in &draft.validation_actions {
            outputln!("  {}", action.render_posix());
        }
        outputln!(
            "A not-present finding after reanalysis means only that the ID is absent from current analyzed inputs; it is not proof of correctness or completion."
        );
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
                ["Address", "Width", "Identity", "Subject"],
                report.neighbors.iter().map(|neighbor| [
                    format!("{:#010x}", neighbor.address),
                    neighbor
                        .width
                        .map_or_else(|| "unknown".to_owned(), |width| width.to_string()),
                    neighbor.name.clone(),
                    neighbor.subject.clone(),
                ]),
            )
        );
    }
    if crate::cli::output::details() && !detail.review_sources.is_empty() {
        outputln!("\nEvidence: {}", detail.review_sources.join(", "));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(name: &str) -> crate::registers::FactRange {
        crate::registers::FactRange {
            name: name.to_owned(),
            start: 0x2010_0000,
            end: 0x2011_0000,
        }
    }

    #[test]
    fn draft_eligibility_requires_an_owned_unreviewed_observation() {
        let owned = range("radio");
        let external = range("platform");

        assert!(may_render_review_draft(
            crate::RegisterReviewState::Unreviewed,
            Some(RegisterPublicationOwnership::Owned(&owned))
        ));
        assert!(!may_render_review_draft(
            crate::RegisterReviewState::Unreviewed,
            Some(RegisterPublicationOwnership::External(&external))
        ));
        assert!(!may_render_review_draft(
            crate::RegisterReviewState::Reviewed,
            Some(RegisterPublicationOwnership::Owned(&owned))
        ));
        assert!(!may_render_review_draft(
            crate::RegisterReviewState::NonOperational,
            Some(RegisterPublicationOwnership::Owned(&owned))
        ));
        assert!(!may_render_review_draft(
            crate::RegisterReviewState::Unreviewed,
            None
        ));
    }
}
