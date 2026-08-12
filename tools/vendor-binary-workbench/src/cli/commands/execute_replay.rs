//! Ordered concrete replay with persistent RAM and stateful external services.

use super::super::*;
use serde::Serialize;

#[derive(Serialize)]
struct CommandDocument<'a> {
    artifact: &'a crate::artifacts::ReplayEvidenceDocument,
    publication: Option<crate::cli::output::Publication>,
}

pub(super) fn run(
    arguments: ExecuteReplayArgs,
    svd: &MmioMap,
    target: &TargetSpec,
    project: Option<&ProjectSpec>,
) -> Result<bool> {
    let manifest_path = arguments
        .manifest
        .ok_or("execute replay requires --manifest")
        .map_err(crate::Error::invalid)?;
    let artifact = arguments
        .artifact
        .ok_or("execute replay requires --artifact or a matching --source run-spec input")
        .map_err(crate::Error::invalid)?;
    let evidence = crate::application::event_replay::execute(
        &crate::application::event_replay::EventReplayRequest {
            manifest: manifest_path,
            artifact,
            companion: arguments.companion,
        },
        svd,
        target,
        project,
    )?;
    let publication = arguments
        .output
        .as_deref()
        .map(|path| crate::cli::output::Publication::new(path, "written"));
    if let Some(path) = arguments.output.as_deref() {
        crate::application::event_replay::publish(&evidence, path, false)?;
    }
    let document = CommandDocument {
        artifact: &evidence,
        publication: publication.clone(),
    };
    crate::cli::output::render_report(&document, || render_human(&evidence, publication.as_ref()));
    Ok(true)
}

fn render_human(
    document: &crate::artifacts::ReplayEvidenceDocument,
    publication: Option<&crate::cli::output::Publication>,
) {
    outputln!(
        "{}  {}",
        crate::cli::output::heading("Execution replay"),
        crate::cli::output::success("COMPLETE")
    );
    outputln!("Artifact: {}", document.artifact.path);
    outputln!("Manifest: {}", document.manifest.path);
    outputln!(
        "\n{}",
        crate::cli::table::render(
            [
                "Phase",
                "Symbol",
                "Completion",
                "Steps",
                "Calls",
                "FIFO",
                "State"
            ],
            document.phases.iter().map(|phase| [
                phase.name.clone(),
                phase.symbol.clone(),
                completion_label(&phase.completion),
                phase.steps.to_string(),
                phase.calls.len().to_string(),
                phase.fifo_lifecycle.len().to_string(),
                phase.memory_observations.len().to_string(),
            ])
        )
    );
    for phase in &document.phases {
        if !phase.fifo_lifecycle.is_empty() {
            outputln!(
                "\n{}",
                crate::cli::output::heading(format!("{} · FIFO", phase.name))
            );
            for event in &phase.fifo_lifecycle {
                outputln!("  {}", fifo_event_label(event));
            }
        }
        if !phase.memory_observations.is_empty() {
            outputln!(
                "\n{}",
                crate::cli::output::heading(format!("{} · state", phase.name))
            );
            for observation in &phase.memory_observations {
                let sites = observation
                    .writes
                    .iter()
                    .map(|write| format!("{:#010x}", write.site))
                    .collect::<Vec<_>>()
                    .join(", ");
                outputln!(
                    "  {} {} @ {:#010x}: {:#x} → {:#x}  writes=[{}]",
                    observation.id,
                    observation.symbol,
                    observation.address,
                    observation.before,
                    observation.after,
                    sites
                );
            }
        }
        if crate::cli::output::details() && !phase.calls.is_empty() {
            outputln!(
                "\n{}",
                crate::cli::output::heading(format!("{} · calls", phase.name))
            );
            for call in &phase.calls {
                outputln!(
                    "  {:#010x} {}({:#x}, {:#x}, {:#x}, {:#x})",
                    call.site,
                    call.symbol,
                    call.arguments[0],
                    call.arguments[1],
                    call.arguments[2],
                    call.arguments[3]
                );
            }
        }
    }
    if let Some(publication) = publication {
        outputln!("\nEvidence {}: {}", publication.status, publication.path);
    }
}

fn completion_label(completion: &crate::artifacts::ReplayCompletionDocument) -> String {
    use crate::artifacts::ReplayCompletionDocument;
    match completion {
        ReplayCompletionDocument::Returned => "returned".to_owned(),
        ReplayCompletionDocument::GoalReached {
            goal: execution_model::ExecutionGoal::Return,
        } => "goal: return".to_owned(),
        ReplayCompletionDocument::GoalReached {
            goal: execution_model::ExecutionGoal::ReachSymbol { symbol },
        } => format!("reached {symbol}"),
        ReplayCompletionDocument::GoalReached {
            goal: execution_model::ExecutionGoal::ObserveCall { symbol },
        } => format!("called {symbol}"),
        ReplayCompletionDocument::GoalReached {
            goal: execution_model::ExecutionGoal::ObserveFifoDequeue { service_id, value },
        } => value.map_or_else(
            || format!("dequeued from {service_id}"),
            |value| format!("dequeued {value:#x} from {service_id}"),
        ),
    }
}

fn fifo_event_label(event: &execution_model::FifoLifecycleEvent) -> String {
    match event {
        execution_model::FifoLifecycleEvent::Enqueued {
            service_id,
            site,
            value,
            depth_before,
            depth_after,
            woke_receiver,
        } => format!(
            "{site:#010x} enqueue {value:#x} into {service_id}: depth {depth_before} → {depth_after}, wake={woke_receiver}"
        ),
        execution_model::FifoLifecycleEvent::Dequeued {
            service_id,
            site,
            value,
            depth_before,
            depth_after,
        } => format!(
            "{site:#010x} dequeue {value:#x} from {service_id}: depth {depth_before} → {depth_after}"
        ),
        execution_model::FifoLifecycleEvent::Full {
            service_id,
            site,
            value,
            depth,
        } => format!("{site:#010x} reject {value:#x}: {service_id} full at depth {depth}"),
        execution_model::FifoLifecycleEvent::Empty { service_id, site } => {
            format!("{site:#010x} {service_id} empty")
        }
        execution_model::FifoLifecycleEvent::Length {
            service_id,
            site,
            depth,
        } => format!("{site:#010x} {service_id} depth={depth}"),
    }
}
