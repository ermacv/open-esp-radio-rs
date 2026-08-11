//! Link-unit to archive-origin association for focused investigation.

use super::{
    FunctionInvestigationRequest, InvestigationLedgerEntry, OriginFunctionEvidence,
    correspondence::{origin_instruction_correspondence, origin_relocation_dependencies},
};
use crate::{ProjectSpec, Result, artifact, artifacts};

pub(super) fn origin_evidence(
    request: &FunctionInvestigationRequest<'_>,
    runtime: &artifact::FunctionBody,
    project: &ProjectSpec,
) -> Result<(Option<OriginFunctionEvidence>, InvestigationLedgerEntry)> {
    let Some(inventory) = request.inventory else {
        return Ok((
            None,
            InvestigationLedgerEntry {
                layer: "link-origin",
                status: "unavailable",
                detail: "selected source has no source-inventory input".to_owned(),
            },
        ));
    };
    let inventory_report = project
        .symbol_inventory
        .as_ref()
        .map(|spec| spec.output.as_path())
        .filter(|path| path.is_file());
    let association = inventory_report
        .map(artifacts::load_link_unit_origins)
        .transpose()?
        .and_then(|origins| {
            origins.into_iter().find(|origin| {
                origin.symbol == request.symbol
                    && origin
                        .linked_sources
                        .iter()
                        .any(|source| source == request.source)
            })
        });
    let member = request.origin_member.or_else(|| {
        association
            .as_ref()
            .and_then(|origin| origin.origin_member.as_deref())
    });
    if member.is_none() && association.is_none() {
        let candidates = artifact::load_code_symbols(
            inventory,
            request.symbol,
            artifact::CodeSymbolSelection::All,
        )?
        .into_iter()
        .filter(|candidate| candidate.name == request.symbol)
        .collect::<Vec<_>>();
        if candidates.len() != 1 {
            return Ok((
                None,
                InvestigationLedgerEntry {
                    layer: "link-origin",
                    status: if candidates.is_empty() {
                        "unavailable"
                    } else {
                        "ambiguous"
                    },
                    detail: if candidates.is_empty() {
                        format!(
                            "raw inventory contains no exact symbol {:?}",
                            request.symbol
                        )
                    } else {
                        format!(
                            "raw inventory contains {} candidates; pass --origin-member or generate a unique link-origin association",
                            candidates.len()
                        )
                    },
                },
            ));
        }
    }
    let body = artifact::inspect_function_body(inventory, member, request.symbol)?;
    let relocation_dependencies = origin_relocation_dependencies(&body);
    let instruction_correspondence = origin_instruction_correspondence(&body, runtime);
    let linked_address = association.as_ref().map(|origin| origin.linked_address);
    let linked_member = association
        .as_ref()
        .and_then(|origin| origin.linked_member.clone());
    let status = if association.is_some() {
        "unique-association"
    } else {
        "unreviewed-selection"
    };
    Ok((
        Some(OriginFunctionEvidence {
            association: status,
            inventory_report: inventory_report.map(|path| path.display().to_string()),
            linked_address,
            linked_member,
            relocation_dependencies,
            instruction_correspondence,
            body,
        }),
        InvestigationLedgerEntry {
            layer: "link-origin",
            status,
            detail: if let Some(association) = association {
                format!(
                    "linked symbol associated by unique name/kind with archive member {}",
                    association
                        .origin_member
                        .as_deref()
                        .unwrap_or("<linked-image>")
                )
            } else {
                "archive body selected without a generated unique-origin association".to_owned()
            },
        },
    ))
}
