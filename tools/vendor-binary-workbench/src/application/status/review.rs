//! Reviewed register, interface, function, and context workspace readiness.

use super::model::{Component, Phase, Readiness};
use crate::application::ProjectContext;
use crate::{
    artifacts::symbol_inventory::load_code_boundary_facts,
    code_workspace::CodeWorkspace,
    function_workspace::FunctionWorkspace,
    interfaces::InterfaceWorkspace,
    registers::{
        ProjectRegisterWorkspace, validate_pac_api, validate_register_evidence,
        validate_register_lints, validate_register_memory_map,
    },
};

pub(super) fn collect(context: &ProjectContext<'_>) -> Phase {
    Phase::collect(
        "review",
        vec![
            code(context),
            registers(context),
            interfaces(context),
            functions(context),
        ],
    )
}

fn code(context: &ProjectContext<'_>) -> Component {
    let Some(paths) = &context.project.code else {
        return Component::new("code_boundaries", Readiness::NotConfigured);
    };
    let Some(inventory) = &context.project.symbol_inventory else {
        return Component::new("code_boundaries", Readiness::Invalid)
            .diagnostic("[code] requires [analysis.symbols]");
    };
    if !inventory.output.is_file() {
        return Component::new("code_boundaries", Readiness::Incomplete)
            .detail("facts", inventory.output.display().to_string())
            .diagnostic("symbol inventory has not been generated")
            .next_action(project_command(context, "project analyze"));
    }
    if !paths.pack.is_file() {
        return Component::new("code_boundaries", Readiness::Incomplete)
            .detail("pack", paths.pack.display().to_string())
            .diagnostic("reviewed code-boundary pack has not been initialized")
            .next_action(project_command(context, "code init-pack"));
    }
    let workspace = load_code_boundary_facts(&inventory.output)
        .and_then(|facts| CodeWorkspace::load(&facts, &paths.pack, &context.project.id));
    match workspace {
        Ok(workspace) => {
            let summary = workspace.summary();
            let mut component = Component::new(
                "code_boundaries",
                if summary.unreviewed == 0 {
                    Readiness::Ready
                } else {
                    Readiness::Incomplete
                },
            )
            .detail("facts", inventory.output.display().to_string())
            .detail("pack", paths.pack.display().to_string())
            .detail("candidates", summary.observed_candidates)
            .detail("accepted", summary.accepted)
            .detail("rejected", summary.rejected)
            .detail("unreviewed", summary.unreviewed);
            if summary.unreviewed != 0 {
                component = component.next_action(format!(
                    "review {} candidate(s) in {}; regenerate the reading view with `{}`",
                    summary.unreviewed,
                    paths.pack.display(),
                    project_command(context, "code review")
                ));
            }
            component
        }
        Err(error) => Component::new("code_boundaries", Readiness::Invalid)
            .detail("pack", paths.pack.display().to_string())
            .diagnostic(error),
    }
}

fn registers(context: &ProjectContext<'_>) -> Component {
    let Some(paths) = &context.project.registers else {
        return Component::new("registers", Readiness::NotConfigured);
    };
    if !paths.model.is_file() {
        return Component::new("registers", Readiness::Incomplete)
            .detail("model", paths.model.display().to_string())
            .diagnostic("register model has not been initialized")
            .next_action(project_command(context, "registers init-model"));
    }
    let workspace = match ProjectRegisterWorkspace::load(paths) {
        Ok(workspace) => workspace,
        Err(error) => {
            return Component::new("registers", Readiness::Invalid)
                .detail("model", paths.model.display().to_string())
                .diagnostic(error);
        }
    };
    let summary = match workspace.summary() {
        Ok(summary) => summary,
        Err(error) => {
            return Component::new("registers", Readiness::Invalid)
                .detail("model", paths.model.display().to_string())
                .diagnostic(error);
        }
    };
    let validation = validate_register_memory_map(paths, context.memory_map)
        .and_then(|_| validate_pac_api(paths).map(|_| ()))
        .and_then(|_| validate_register_lints(paths).map(|_| ()))
        .and_then(|_| validate_register_evidence(paths, context.memory_map).map(|_| ()));
    if let Err(error) = validation {
        return Component::new("registers", Readiness::Invalid)
            .detail("model", paths.model.display().to_string())
            .diagnostic(error);
    }
    let mut component = Component::new(
        "registers",
        if summary.unreviewed == 0 {
            Readiness::Ready
        } else {
            Readiness::Incomplete
        },
    )
    .detail("owned_ranges", paths.owned_ranges.join(","))
    .detail("format", workspace.format_label())
    .detail("facts", paths.facts.display().to_string())
    .detail("model", paths.model.display().to_string())
    .detail("observed", summary.observed)
    .detail("reviewed", summary.reviewed)
    .detail("ignored", summary.ignored)
    .detail("manual", summary.manual)
    .detail("unreviewed", summary.unreviewed)
    .detail("fields", summary.fields)
    .detail("review_ir_reports", paths.review_ir_reports.len())
    .detail("api_pack_configured", paths.api_pack.is_some())
    .detail("lint_pack_configured", paths.lint_pack.is_some())
    .detail("evidence_catalogs", paths.evidence_catalogs.len());
    if summary.unreviewed != 0 {
        let report = paths.review_output.as_deref().map_or_else(
            || paths.facts.display().to_string(),
            |path| path.display().to_string(),
        );
        component = component.next_action(format!(
            "inspect {} unreviewed observation(s) in {report}; edit {}; regenerate with `{}`",
            summary.unreviewed,
            paths.model.display(),
            project_command(context, "registers review")
        ));
    }
    component
}

fn interfaces(context: &ProjectContext<'_>) -> Component {
    let Some(paths) = &context.project.interfaces else {
        return Component::new("interfaces", Readiness::NotConfigured);
    };
    if !paths.facts.is_file() {
        return Component::new("interfaces", Readiness::Incomplete)
            .detail("facts", paths.facts.display().to_string())
            .diagnostic("interface facts have not been generated")
            .next_action(project_command(context, "project analyze"));
    }
    let Some(pack) = paths.pack.as_deref() else {
        return Component::new("interfaces", Readiness::Incomplete)
            .diagnostic("interface pack is not configured")
            .next_action(format!(
                "configure [interfaces].pack in {}",
                context.project_path.display()
            ));
    };
    if !pack.is_file() {
        return Component::new("interfaces", Readiness::Incomplete)
            .detail("pack", pack.display().to_string())
            .diagnostic("interface pack has not been initialized")
            .next_action(project_command(context, "interfaces init-pack"));
    }
    match InterfaceWorkspace::load(
        &paths.facts,
        pack,
        &paths.semantic_catalogs,
        context.target.calling_convention.label(),
        context
            .target
            .harness
            .as_deref()
            .and_then(|harness| crate::harnesses::contracts(harness).ok()),
    ) {
        Ok(workspace) => {
            let summary = workspace.summary();
            let mut component = Component::new(
                "interfaces",
                if summary.unreviewed_anchors == 0 && summary.unreviewed_slots == 0 {
                    Readiness::Ready
                } else {
                    Readiness::Incomplete
                },
            )
            .detail("facts", paths.facts.display().to_string())
            .detail("pack", pack.display().to_string())
            .detail("reviewed_anchors", summary.reviewed_anchors)
            .detail("ignored_anchors", summary.ignored_anchors)
            .detail("unreviewed_anchors", summary.unreviewed_anchors)
            .detail("reviewed_slots", summary.reviewed_slots)
            .detail("ignored_slots", summary.ignored_slots)
            .detail("unreviewed_slots", summary.unreviewed_slots)
            .detail("semantic_links", summary.semantic_links)
            .detail("execution_contracts", summary.execution_contracts)
            .detail("execution_models", summary.execution_models)
            .detail("resolved_calls", summary.resolved_calls);
            if summary.unreviewed_anchors != 0 || summary.unreviewed_slots != 0 {
                component = component.next_action(format!(
                    "review {} anchor(s) and {} slot(s) in {}; validate with `{}`",
                    summary.unreviewed_anchors,
                    summary.unreviewed_slots,
                    pack.display(),
                    project_command(context, "interfaces validate")
                ));
            }
            component
        }
        Err(error) => Component::new("interfaces", Readiness::Invalid)
            .detail("facts", paths.facts.display().to_string())
            .detail("pack", pack.display().to_string())
            .diagnostic(error),
    }
}

fn functions(context: &ProjectContext<'_>) -> Component {
    let Some(paths) = &context.project.functions else {
        return Component::new("functions", Readiness::NotConfigured);
    };
    let reports = match context.project.function_ir_reports() {
        Ok(reports) => reports,
        Err(error) => {
            return Component::new("functions", Readiness::Invalid).diagnostic(error);
        }
    };
    let missing = reports
        .iter()
        .filter(|(_, report)| !report.is_file())
        .count();
    if missing != 0 {
        return Component::new("functions", Readiness::Incomplete)
            .detail("profiles", reports.len())
            .detail("missing_reports", missing)
            .diagnostic("linked-IR function facts have not been generated")
            .next_action(project_command(context, "ir build"));
    }
    if !paths.pack.is_file() {
        return Component::new("functions", Readiness::Incomplete)
            .detail("pack", paths.pack.display().to_string())
            .diagnostic("function pack has not been initialized")
            .next_action(project_command(context, "functions init-pack"));
    }
    match FunctionWorkspace::load(&reports, &paths.pack) {
        Ok(workspace) => {
            let summary = workspace.summary();
            let mut component = Component::new(
                "functions",
                if summary.unreviewed_functions == 0
                    && summary.unreviewed_contexts == 0
                    && summary.unreviewed_fields == 0
                    && summary.unreviewed_type_fields == 0
                {
                    Readiness::Ready
                } else {
                    Readiness::Incomplete
                },
            )
            .detail("pack", paths.pack.display().to_string())
            .detail("profiles", reports.len())
            .detail("root_functions", summary.observed_functions)
            .detail("reviewed_functions", summary.reviewed_functions)
            .detail("ignored_functions", summary.ignored_functions)
            .detail("unreviewed_functions", summary.unreviewed_functions)
            .detail("reviewed_contexts", summary.reviewed_contexts)
            .detail("unreviewed_contexts", summary.unreviewed_contexts)
            .detail("reviewed_fields", summary.reviewed_fields)
            .detail("unreviewed_fields", summary.unreviewed_fields)
            .detail("logical_types", summary.logical_types)
            .detail("type_bindings", summary.type_bindings)
            .detail("unreviewed_type_fields", summary.unreviewed_type_fields)
            .detail("accepted_incomplete", summary.accepted_incomplete);
            let outstanding = summary.unreviewed_functions
                + summary.unreviewed_contexts
                + summary.unreviewed_fields
                + summary.unreviewed_type_fields;
            if outstanding != 0 {
                let report = paths.review_output.as_deref().map_or_else(
                    || paths.pack.display().to_string(),
                    |path| path.display().to_string(),
                );
                component = component.next_action(format!(
                    "inspect {outstanding} unreviewed function/context item(s) in {report}; edit {}; regenerate with `{}`",
                    paths.pack.display(),
                    project_command(context, "functions review")
                ));
            }
            component
        }
        Err(error) => Component::new("functions", Readiness::Invalid)
            .detail("pack", paths.pack.display().to_string())
            .diagnostic(error),
    }
}

fn project_command(context: &ProjectContext<'_>, command: &str) -> String {
    format!(
        "vendor-binary-workbench {command} --project {}",
        context.project_path.display()
    )
}
