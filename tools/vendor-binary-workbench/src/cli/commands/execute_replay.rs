//! Ordered concrete replay with persistent RAM and stateful external services.

use super::super::*;
use crate::interfaces::InterfaceWorkspace;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct ReplayManifest {
    schema: u32,
    #[serde(default)]
    fifo_services: Vec<execution_model::FifoServiceInstance>,
    phases: Vec<ReplayPhase>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct ReplayPhase {
    name: String,
    symbol: String,
    #[serde(default)]
    arguments: Vec<u32>,
    #[serde(default)]
    memory: Vec<ReplayMemorySeed>,
    #[serde(default)]
    tables: Vec<execution_model::TableInstance>,
    #[serde(default)]
    fifo_bindings: Vec<execution_model::FifoServiceBinding>,
    #[serde(default)]
    calls: Vec<ReplayCall>,
    #[serde(default)]
    goal: execution_model::ExecutionGoal,
    #[serde(default)]
    max_steps: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct ReplayMemorySeed {
    symbol: String,
    #[serde(default)]
    offset: i32,
    width: u8,
    value: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct ReplayCall {
    symbol: String,
    returns: Vec<u32>,
}

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
    let input = std::fs::read_to_string(&manifest_path)
        .map_err(|error| crate::Error::read("replay manifest", &manifest_path, error))?;
    let manifest: ReplayManifest = toml_edit::de::from_str(&input).map_err(|error| {
        crate::Error::invalid(format!(
            "invalid replay manifest {}: {error}",
            manifest_path.display()
        ))
    })?;
    if manifest.schema != 1 {
        return Err(crate::Error::invalid(format!(
            "replay manifest {} requires schema = 1",
            manifest_path.display()
        )));
    }
    let artifact = arguments
        .artifact
        .ok_or("execute replay requires --artifact or a matching --source run-spec input")
        .map_err(crate::Error::invalid)?;
    let mut image = execution::ExecutableImage::load(&artifact)?;
    if let Some(companion) = arguments.companion.as_deref() {
        image.add_companion(companion)?;
    }
    validate_tables(project, target, &manifest)?;

    let mut services = Some(manifest.fifo_services);
    let mut phases = Vec::with_capacity(manifest.phases.len());
    for phase in manifest.phases {
        let mut scenario = execution::Scenario {
            arguments: phase.arguments,
            table_instances: phase.tables,
            fifo_services: services.take().unwrap_or_default(),
            fifo_bindings: phase.fifo_bindings,
            goal: phase.goal,
            max_steps: phase.max_steps,
            ..execution::Scenario::default()
        };
        for seed in phase.memory {
            seed_memory(&image, &mut scenario, &seed)?;
        }
        for call in phase.calls {
            if call.symbol.trim().is_empty() || call.returns.is_empty() {
                return Err(crate::Error::invalid(format!(
                    "replay phase {:?} calls require a symbol and at least one return value",
                    phase.name
                )));
            }
            scenario.call_responses.insert(
                call.symbol,
                call.returns
                    .into_iter()
                    .map(execution::ModeledCallResponse::scalar)
                    .collect(),
            );
        }
        phases.push(execution::ExecutionPhase {
            name: phase.name,
            symbol: phase.symbol,
            scenario,
        });
    }

    let mut session = execution::ExecutionSession::default();
    let results = session.execute_phases(&image, svd, phases)?;
    let evidence = crate::artifacts::build_replay_evidence(&manifest_path, &artifact, results)?;
    let publication = arguments
        .output
        .as_deref()
        .map(|path| crate::cli::output::Publication::new(path, "written"));
    if let Some(path) = arguments.output.as_deref() {
        crate::application::generated_file::write_or_check(
            path,
            &crate::artifacts::render_replay_evidence(&evidence)?,
            false,
            "execution replay evidence",
        )?;
    }
    let document = CommandDocument {
        artifact: &evidence,
        publication: publication.clone(),
    };
    crate::cli::output::render_report(&document, || render_human(&evidence, publication.as_ref()));
    Ok(true)
}

fn seed_memory(
    image: &execution::ExecutableImage,
    scenario: &mut execution::Scenario,
    seed: &ReplayMemorySeed,
) -> Result<()> {
    if !matches!(seed.width, 8 | 16 | 32) {
        return Err(crate::Error::invalid(format!(
            "memory seed {} width must be 8, 16, or 32",
            seed.symbol
        )));
    }
    let base = image.symbol_address(&seed.symbol).ok_or_else(|| {
        crate::Error::invalid(format!(
            "memory seed refers to missing linked symbol {}",
            seed.symbol
        ))
    })?;
    let address = base.wrapping_add(seed.offset as u32);
    let bytes = usize::from(seed.width / 8);
    for (offset, byte) in seed.value.to_le_bytes().into_iter().take(bytes).enumerate() {
        scenario
            .memory_initial
            .insert(address + offset as u32, byte);
    }
    Ok(())
}

fn validate_tables(
    project: Option<&ProjectSpec>,
    target: &TargetSpec,
    manifest: &ReplayManifest,
) -> Result<()> {
    let tables = manifest
        .phases
        .iter()
        .flat_map(|phase| phase.tables.iter())
        .collect::<Vec<_>>();
    if tables.is_empty() {
        return Ok(());
    }
    let project = project.ok_or_else(|| {
        crate::Error::invalid(
            "runtime table replay requires --project and a reviewed interface pack",
        )
    })?;
    let paths = project.interfaces.as_ref().ok_or_else(|| {
        crate::Error::invalid("runtime table replay requires configured [interfaces]")
    })?;
    let pack = paths.pack.as_ref().ok_or_else(|| {
        crate::Error::invalid("runtime table replay requires a reviewed interface pack")
    })?;
    let harness = target
        .harness
        .as_deref()
        .and_then(|harness| crate::harnesses::contracts(harness).ok());
    let workspace = InterfaceWorkspace::load(
        &paths.facts,
        pack,
        &paths.semantic_catalogs,
        target.calling_convention.label(),
        harness,
    )?;
    for table in tables {
        workspace.validate_table_instance(table)?;
    }
    Ok(())
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
            ["Phase", "Symbol", "Completion", "Steps", "Calls", "FIFO"],
            document.phases.iter().map(|phase| [
                phase.name.clone(),
                phase.symbol.clone(),
                completion_label(&phase.completion),
                phase.steps.to_string(),
                phase.calls.len().to_string(),
                phase.fifo_lifecycle.len().to_string(),
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
