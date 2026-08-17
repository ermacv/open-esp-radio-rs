//! Caller-owned run-spec artifact readiness inspection.

use crate::{artifact, cli::commands::ProjectContext};

use super::model::{DoctorReport, InputReport};

pub(super) fn collect(context: &ProjectContext<'_>, report: &mut DoctorReport) {
    let Some(run_spec) = context.run_spec else {
        report.absorb(0, 1);
        return;
    };
    for input in run_spec.inputs() {
        let role = input.role.to_string();
        if !input.path.is_file() {
            report.error();
            report.inputs.push(InputReport {
                role,
                status: "missing",
                path: input.path.clone(),
                container: None,
                objects: None,
                skipped_members: None,
                symbol_facts: None,
                code_definitions: None,
                exported_definitions: None,
                undefined: None,
                error: None,
            });
            continue;
        }
        match artifact::inspect_artifact(&input.path) {
            Ok(inventory) => {
                report.valid_inputs += 1;
                let symbol_facts = inventory.symbols().count();
                let code_definitions = inventory
                    .symbols()
                    .filter(|(_, fact)| {
                        fact.kind == artifact::ArtifactSymbolKind::Text
                            && fact.definition.is_definition()
                    })
                    .count();
                let exported_definitions = inventory
                    .symbols()
                    .filter(|(_, fact)| fact.is_exported_definition())
                    .count();
                let undefined = inventory
                    .symbols()
                    .filter(|(_, fact)| {
                        fact.definition == artifact::ArtifactSymbolDefinitionState::Undefined
                    })
                    .count();
                let status = if symbol_facts == 0 {
                    report.absorb(0, 1);
                    "readable-no-symbols"
                } else {
                    "ready"
                };
                report.inputs.push(InputReport {
                    role,
                    status,
                    path: input.path.clone(),
                    container: Some(inventory.container.label()),
                    objects: Some(inventory.objects.len()),
                    skipped_members: Some(inventory.skipped_members),
                    symbol_facts: Some(symbol_facts),
                    code_definitions: Some(code_definitions),
                    exported_definitions: Some(exported_definitions),
                    undefined: Some(undefined),
                    error: None,
                });
            }
            Err(error) => {
                report.error();
                report.inputs.push(InputReport {
                    role,
                    status: "invalid",
                    path: input.path.clone(),
                    container: None,
                    objects: None,
                    skipped_members: None,
                    symbol_facts: None,
                    code_definitions: None,
                    exported_definitions: None,
                    undefined: None,
                    error: Some(error.to_string()),
                });
            }
        }
    }
}
