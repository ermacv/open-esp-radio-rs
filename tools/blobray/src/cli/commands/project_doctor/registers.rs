//! Register workspace, review, and publication-pack readiness inspection.

use crate::{
    cli::commands::ProjectContext,
    registers::{
        ProjectRegisterWorkspace, RegisterFacts, inspect_register_review_ir, validate_pac_api,
        validate_register_evidence, validate_register_lints,
    },
};

use super::model::{CapabilityReport, DoctorReport};

pub(super) fn collect(context: &ProjectContext<'_>, report: &mut DoctorReport) {
    collect_workspace(context, report);
    collect_pac_api(context, report);
    collect_lints(context, report);
    collect_evidence(context, report);
}

fn collect_workspace(context: &ProjectContext<'_>, report: &mut DoctorReport) {
    let Some(paths) = &context.project.registers else {
        report.capability(CapabilityReport::new(
            "register-workspace",
            "not-configured",
        ));
        return;
    };

    if paths.model.is_file() {
        match ProjectRegisterWorkspace::load(paths)
            .and_then(|workspace| Ok((workspace.summary()?, workspace.format_label())))
        {
            Ok((summary, format)) => {
                report.capability(
                    CapabilityReport::new("register-workspace", "available")
                        .field("format", format)
                        .field("owned-ranges", paths.owned_ranges.clone())
                        .field("ranges", summary.ranges)
                        .field("observed", summary.observed)
                        .field("reviewed", summary.reviewed)
                        .field("ignored", summary.ignored)
                        .field("manual", summary.manual)
                        .field("unreviewed", summary.unreviewed)
                        .field("fields", summary.fields)
                        .field("facts", paths.facts.display().to_string())
                        .field("model", paths.model.display().to_string())
                        .field(
                            "review-output",
                            display_optional(paths.review_output.as_deref()),
                        )
                        .field("review-ir-reports", paths.review_ir_reports.len())
                        .field("svd-output", display_optional(paths.svd_output.as_deref()))
                        .field(
                            "pac-raw-output",
                            paths.pac_raw.as_ref().map_or_else(
                                || "-".to_owned(),
                                |pac| pac.output.display().to_string(),
                            ),
                        )
                        .field(
                            "pac-api-output",
                            display_optional(paths.api_output.as_deref()),
                        )
                        .field(
                            "bindings-output",
                            paths.bindings.as_ref().map_or_else(
                                || "-".to_owned(),
                                |bindings| bindings.output.display().to_string(),
                            ),
                        )
                        .field(
                            "bindings-crate",
                            paths
                                .bindings
                                .as_ref()
                                .map_or("-", |bindings| bindings.crate_name.as_str()),
                        ),
                );
                collect_review_ir(context, report);
            }
            Err(error) => {
                report.error();
                report.capability(
                    CapabilityReport::new("register-workspace", "invalid")
                        .field("facts", paths.facts.display().to_string())
                        .field("model", paths.model.display().to_string())
                        .field("error", error.to_string()),
                );
            }
        }
    } else if !paths.facts.is_file() {
        report.absorb(0, 1);
        report.capability(
            CapabilityReport::new("register-workspace", "not-generated")
                .field("facts", paths.facts.display().to_string())
                .field("model", paths.model.display().to_string()),
        );
    } else {
        match RegisterFacts::load(&paths.facts) {
            Ok(facts) => {
                report.absorb(0, 1);
                report.capability(
                    CapabilityReport::new("register-workspace", "model-not-initialized")
                        .field("ranges", facts.ranges.len())
                        .field("observed", facts.registers.len())
                        .field("facts", paths.facts.display().to_string())
                        .field("model", paths.model.display().to_string()),
                );
            }
            Err(error) => {
                report.error();
                report.capability(
                    CapabilityReport::new("register-workspace", "invalid-facts")
                        .field("facts", paths.facts.display().to_string())
                        .field("error", error.to_string()),
                );
            }
        }
    }
}

fn collect_review_ir(context: &ProjectContext<'_>, report: &mut DoctorReport) {
    let paths = context
        .project
        .registers
        .as_ref()
        .expect("register review inspection follows workspace configuration");
    if paths.review_ir_reports.is_empty() {
        return;
    }
    let missing_project_outputs = paths
        .review_ir_reports
        .iter()
        .filter(|path| !path.exists())
        .filter(|path| {
            context
                .project
                .ir_profiles
                .iter()
                .any(|profile| profile.output.as_path() == path.as_path())
        })
        .count();
    let missing_reports = paths
        .review_ir_reports
        .iter()
        .filter(|path| !path.exists())
        .count();
    let report_paths = paths
        .review_ir_reports
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();
    let capability = if missing_reports != 0 && missing_reports == missing_project_outputs {
        CapabilityReport::new("register-review-ir", "not-generated")
            .field("reports", paths.review_ir_reports.len())
            .field("missing", missing_reports)
            .field("paths", report_paths)
    } else {
        match inspect_register_review_ir(&paths.review_ir_reports) {
            Ok(ir) => CapabilityReport::new("register-review-ir", "available")
                .field("reports", ir.reports)
                .field("registers", ir.registers)
                .field("field-candidates", ir.fields)
                .field("paths", report_paths),
            Err(error) => {
                report.error();
                CapabilityReport::new("register-review-ir", "invalid")
                    .field("reports", paths.review_ir_reports.len())
                    .field("error", error.to_string())
                    .field("paths", report_paths)
            }
        }
    };
    report.capability(capability);
}

fn collect_pac_api(context: &ProjectContext<'_>, report: &mut DoctorReport) {
    let configured = context
        .project
        .registers
        .as_ref()
        .and_then(|paths| paths.api_pack.as_deref().map(|pack| (paths, pack)));
    let capability = match configured {
        None => CapabilityReport::new("pac-api", "not-configured"),
        Some((_, path)) if !path.is_file() => {
            report.absorb(0, 1);
            CapabilityReport::new("pac-api", "not-initialized")
                .field("pack", path.display().to_string())
        }
        Some((paths, path)) => match validate_pac_api(paths) {
            Ok(Some(pack)) => CapabilityReport::new("pac-api", "available")
                .field("schema", pack.schema as usize)
                .field("domains", pack.domain_count())
                .field("operations", pack.operation_count())
                .field("sources", pack.source_ids().len())
                .field("ownership-partitions", pack.ownership_partition_count())
                .field("device-access", pack.options.device_access)
                .field("pack", path.display().to_string())
                .field("output", display_optional(paths.api_output.as_deref())),
            Ok(None) => unreachable!("PAC API path was configured before validation"),
            Err(error) => {
                report.error();
                CapabilityReport::new("pac-api", "invalid")
                    .field("pack", path.display().to_string())
                    .field("error", error.to_string())
            }
        },
    };
    report.capability(capability);
}

fn collect_lints(context: &ProjectContext<'_>, report: &mut DoctorReport) {
    let configured = context
        .project
        .registers
        .as_ref()
        .and_then(|paths| paths.lint_pack.as_deref().map(|pack| (paths, pack)));
    let capability = match configured {
        None => CapabilityReport::new("register-lints", "not-configured"),
        Some((_, path)) if !path.is_file() => {
            report.absorb(0, 1);
            CapabilityReport::new("register-lints", "not-initialized")
                .field("pack", path.display().to_string())
        }
        Some((paths, path)) => match validate_register_lints(paths) {
            Ok(Some(pack)) => CapabilityReport::new("register-lints", "available")
                .field("schema", pack.schema as usize)
                .field(
                    "forbidden-field-name-substrings",
                    pack.forbidden_field_name_substrings.len(),
                )
                .field("pack", path.display().to_string()),
            Ok(None) => unreachable!("register lint path was configured before validation"),
            Err(error) => {
                report.error();
                CapabilityReport::new("register-lints", "invalid")
                    .field("pack", path.display().to_string())
                    .field("error", error.to_string())
            }
        },
    };
    report.capability(capability);
}

fn collect_evidence(context: &ProjectContext<'_>, report: &mut DoctorReport) {
    let capability = match context.project.registers.as_ref() {
        None => CapabilityReport::new("register-evidence", "not-configured"),
        Some(paths) if paths.evidence_catalogs.is_empty() => {
            CapabilityReport::new("register-evidence", "not-configured")
        }
        Some(paths) => match validate_register_evidence(paths, context.memory_map) {
            Ok(Some(evidence)) => CapabilityReport::new("register-evidence", "available")
                .field("catalogs", paths.evidence_catalogs.len())
                .field("sources", evidence.sources.len())
                .field("ranges", evidence.ranges.len()),
            Ok(None) => unreachable!("evidence catalogs were configured before validation"),
            Err(error) => {
                report.error();
                CapabilityReport::new("register-evidence", "invalid")
                    .field("catalogs", paths.evidence_catalogs.len())
                    .field("error", error.to_string())
            }
        },
    };
    report.capability(capability);
}

fn display_optional(path: Option<&std::path::Path>) -> String {
    path.map_or_else(|| "-".to_owned(), |path| path.display().to_string())
}
