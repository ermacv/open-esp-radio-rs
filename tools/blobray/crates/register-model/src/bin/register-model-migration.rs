//! Read-only register-model migration planner.

use std::{env, path::PathBuf, process::ExitCode};

use open_esp_radio_register_model::RegisterModel;
use open_radio_vendor_review::ReviewKnowledge;
use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ConcisePlan<'a> {
    schema: u32,
    address_space: &'a str,
    review_fingerprint: &'a str,
    overlay_changes_effective_output: bool,
    summary: &'a open_esp_radio_register_model::RegisterMigrationSummary,
    assertions: &'a [open_esp_radio_register_model::RegisterMigrationAssertion],
    diagnostics: &'a [open_esp_radio_register_model::RegisterMigrationDiagnostic],
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("register-model-migration: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = env::args_os().skip(1);
    let first = arguments.next();
    let (details, model) = match first {
        Some(argument) if argument == "--details" => (true, arguments.next()),
        model => (false, model),
    };
    let Some(model) = model else {
        return Err(
            "usage: register-model-migration [--details] MODEL.toml REVIEWED.toml [REVIEWED.toml ...]"
                .into(),
        );
    };
    let packs = arguments.map(PathBuf::from).collect::<Vec<_>>();
    if packs.is_empty() {
        return Err("at least one reviewed-knowledge pack is required".into());
    }

    let model = RegisterModel::load(&PathBuf::from(model))?;
    let knowledge = ReviewKnowledge::load_all(&packs)?;
    let plan = model.plan_review_migration(&knowledge)?;
    let mut output = if details {
        toml_edit::ser::to_string_pretty(&plan)?
    } else {
        toml_edit::ser::to_string_pretty(&ConcisePlan {
            schema: plan.schema,
            address_space: &plan.address_space,
            review_fingerprint: &plan.review_fingerprint,
            overlay_changes_effective_output: plan.overlay_changes_effective_output,
            summary: &plan.summary,
            assertions: &plan.assertions,
            diagnostics: &plan.diagnostics,
        })?
    };
    if !output.ends_with('\n') {
        output.push('\n');
    }
    print!("{output}");
    Ok(())
}
