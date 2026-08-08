//! Concrete single-symbol execution command.

use super::super::*;
use serde::Serialize;

#[derive(Serialize)]
struct ArtifactDocument {
    path: String,
    sha256: String,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum TimelineEventDocument {
    Observable {
        event: ExecutionEventReport,
    },
    Call {
        site: u32,
        location: String,
        symbol: String,
        arguments: [u32; 8],
    },
    Branch {
        site: u32,
        location: String,
        taken: bool,
    },
    RamRead {
        width: u8,
        address: u32,
        value: u32,
    },
    RamWrite {
        width: u8,
        address: u32,
        value: u32,
    },
}

#[derive(Serialize)]
struct BranchOutcomeDocument {
    site: u32,
    location: String,
    taken: bool,
    covered: bool,
}

#[derive(Serialize)]
struct ControlFlowGapDocument {
    site: u32,
    location: String,
    edge: String,
}

#[derive(Serialize)]
struct ExecutionSummary {
    evidence: &'static str,
    steps: u64,
    return_value: u32,
    branches: usize,
    branch_events: usize,
    calls: usize,
    call_events: usize,
    timeline_events: usize,
    memory_changes: usize,
    uncovered_branch_outcomes: usize,
    unresolved_control_flow: usize,
    unnamed_mmio: usize,
    complete: bool,
}

#[derive(Serialize)]
struct ExecutionDocument {
    schema_version: u32,
    command: &'static str,
    artifact: ArtifactDocument,
    companion: Option<ArtifactDocument>,
    symbol: String,
    events: Vec<ExecutionEventReport>,
    covered_calls: Vec<String>,
    timeline: Option<Vec<TimelineEventDocument>>,
    branch_outcomes: Vec<BranchOutcomeDocument>,
    unresolved_control_flow: Vec<ControlFlowGapDocument>,
    unnamed_mmio: Vec<u32>,
    memory_changes: Vec<MemoryChangeReport>,
    summary: ExecutionSummary,
}

pub(super) fn run(arguments: ExecuteRunArgs, svd: &MmioMap) -> Result<bool> {
    let mut scenario = resolve_scenario(arguments.scenario)?;
    for assignment in arguments.call {
        let (symbol, value) = parse_call_return(&assignment, "--call")?;
        scenario
            .call_returns
            .entry(symbol)
            .or_default()
            .push_back(value);
    }
    if let Some(value) = arguments.stack_fill {
        let value = parse_u32(&value)
            .ok_or("invalid --stack-fill value")
            .map_err(crate::Error::invalid)?;
        scenario.private_stack_fill = Some(
            u8::try_from(value)
                .map_err(|_| "--stack-fill value exceeds one byte")
                .map_err(crate::Error::invalid)?,
        );
    }
    let artifact = arguments
        .artifact
        .ok_or("missing --artifact")
        .map_err(crate::Error::invalid)?;
    let symbol = arguments
        .symbol
        .ok_or("missing --symbol")
        .map_err(crate::Error::invalid)?;
    let concrete_only = arguments.concrete_only;
    let print_timeline = arguments.timeline;
    let companion = arguments.companion;
    let mut image = execution::ExecutableImage::load(&artifact)?;
    if let Some(companion) = companion.as_deref() {
        image.add_companion(companion)?;
    }
    let inventory = if concrete_only {
        execution::CoverageInventory::default()
    } else {
        image.coverage_inventory(&symbol)?
    };
    let result = execution::execute(&image, svd, &symbol, scenario)?;
    render_result(ExecutionRenderInput {
        artifact: &artifact,
        companion: companion.as_deref(),
        symbol,
        concrete_only,
        print_timeline,
        image,
        inventory,
        result,
    })
}

fn artifact_document(path: &std::path::Path) -> Result<ArtifactDocument> {
    Ok(ArtifactDocument {
        path: path.display().to_string(),
        sha256: artifact_sha256(path)?,
    })
}

fn timeline_document(
    image: &execution::ExecutableImage,
    event: &execution::ExecutionTimelineEvent,
) -> TimelineEventDocument {
    match event {
        execution::ExecutionTimelineEvent::Observable(event) => TimelineEventDocument::Observable {
            event: event.into(),
        },
        execution::ExecutionTimelineEvent::Call(call) => TimelineEventDocument::Call {
            site: call.site,
            location: image.location(call.site),
            symbol: call.symbol.clone(),
            arguments: call.arguments,
        },
        execution::ExecutionTimelineEvent::Branch { site, taken } => {
            TimelineEventDocument::Branch {
                site: *site,
                location: image.location(*site),
                taken: *taken,
            }
        }
        execution::ExecutionTimelineEvent::RamRead {
            width,
            address,
            value,
        } => TimelineEventDocument::RamRead {
            width: *width,
            address: *address,
            value: *value,
        },
        execution::ExecutionTimelineEvent::RamWrite {
            width,
            address,
            value,
        } => TimelineEventDocument::RamWrite {
            width: *width,
            address: *address,
            value: *value,
        },
    }
}

struct ExecutionRenderInput<'a> {
    artifact: &'a std::path::Path,
    companion: Option<&'a std::path::Path>,
    symbol: String,
    concrete_only: bool,
    print_timeline: bool,
    image: execution::ExecutableImage,
    inventory: execution::CoverageInventory,
    result: execution::ExecutionResult,
}

fn execution_document(input: &ExecutionRenderInput<'_>) -> Result<ExecutionDocument> {
    let ExecutionRenderInput {
        artifact,
        companion,
        symbol,
        concrete_only,
        print_timeline,
        image,
        inventory,
        result,
    } = input;
    let unnamed_mmio = result
        .events
        .iter()
        .filter_map(unnamed_execution_address)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let branch_outcomes = inventory
        .branch_outcomes
        .iter()
        .map(|(site, taken)| BranchOutcomeDocument {
            site: *site,
            location: image.location(*site),
            taken: *taken,
            covered: result.branches.contains(&(*site, *taken)),
        })
        .collect::<Vec<_>>();
    let uncovered_branch_outcomes = branch_outcomes
        .iter()
        .filter(|outcome| !outcome.covered)
        .count();
    let unresolved_control_flow = inventory
        .unresolved_edges
        .iter()
        .map(|(site, edge)| ControlFlowGapDocument {
            site: *site,
            location: image.location(*site),
            edge: edge.clone(),
        })
        .collect::<Vec<_>>();
    let complete = uncovered_branch_outcomes == 0 && unresolved_control_flow.is_empty();
    Ok(ExecutionDocument {
        schema_version: 2,
        command: "execute run",
        artifact: artifact_document(artifact)?,
        companion: companion.map(artifact_document).transpose()?,
        symbol: symbol.clone(),
        events: result.events.iter().map(Into::into).collect(),
        covered_calls: result.calls.iter().cloned().collect(),
        timeline: print_timeline.then(|| {
            result
                .timeline
                .iter()
                .map(|event| timeline_document(image, event))
                .collect()
        }),
        branch_outcomes,
        unresolved_control_flow,
        unnamed_mmio,
        memory_changes: result.memory_changes.iter().map(Into::into).collect(),
        summary: ExecutionSummary {
            evidence: if *concrete_only {
                "concrete-only"
            } else {
                "branch-complete"
            },
            steps: result.steps,
            return_value: result.return_value,
            branches: result.branches.len(),
            branch_events: result.ordered_branches.len(),
            calls: result.calls.len(),
            call_events: result.ordered_calls.len(),
            timeline_events: result.timeline.len(),
            memory_changes: result.memory_changes.len(),
            uncovered_branch_outcomes,
            unresolved_control_flow: inventory.unresolved_edges.len(),
            unnamed_mmio: result
                .events
                .iter()
                .filter_map(unnamed_execution_address)
                .collect::<BTreeSet<_>>()
                .len(),
            complete,
        },
    })
}

pub(super) fn resolve_scenario(arguments: ScenarioArgs) -> Result<execution::Scenario> {
    let mut scenario = execution::Scenario::default();
    for value in arguments.arg {
        scenario.arguments.push(
            parse_u32(&value)
                .ok_or("invalid --arg value")
                .map_err(crate::Error::invalid)?,
        );
    }
    for assignment in arguments.mmio {
        let (address, value) = parse_assignment(&assignment, "--mmio")?;
        scenario.mmio_initial.insert(address, value);
    }
    for assignment in arguments.read {
        let (address, value) = parse_assignment(&assignment, "--read")?;
        scenario
            .mmio_reads
            .entry(address)
            .or_default()
            .push_back(value);
    }
    for assignment in arguments.ram {
        let (address, value) = parse_assignment(&assignment, "--ram")?;
        seed_ram_word(&mut scenario, address, value);
    }
    for assignment in arguments.observe {
        let (address, length) = parse_assignment(&assignment, "--observe")?;
        observe_memory(&mut scenario, address, length)?;
    }
    if let Some(max_steps) = arguments.max_steps {
        scenario.max_steps = max_steps;
    }
    Ok(scenario)
}

fn render_result(input: ExecutionRenderInput<'_>) -> Result<bool> {
    let document = execution_document(&input)?;
    let complete = document.summary.complete;
    crate::cli::output::render_report(
        &document,
        || render_execution(&input),
        || render_execution(&input),
    );
    Ok(complete)
}

fn render_execution(input: &ExecutionRenderInput<'_>) {
    let ExecutionRenderInput {
        symbol,
        concrete_only,
        print_timeline,
        image,
        inventory,
        result,
        ..
    } = input;
    let unnamed: BTreeSet<_> = result
        .events
        .iter()
        .filter_map(unnamed_execution_address)
        .collect();
    for event in &result.events {
        match event {
            execution::ExecutionEvent::Read {
                width,
                address,
                region,
                register,
                value,
            } => {
                let register = register.as_deref().unwrap_or("-");
                outputln!(
                    "EVENT\tR\t{width}\t{address:#010x}\tregion={region}\tregister={register}\tvalue={value:#010x}"
                );
            }
            execution::ExecutionEvent::Write {
                width,
                address,
                region,
                register,
                value,
            } => {
                let register = register.as_deref().unwrap_or("-");
                outputln!(
                    "EVENT\tW\t{width}\t{address:#010x}\tregion={region}\tregister={register}\tvalue={value:#010x}"
                );
            }
            execution::ExecutionEvent::DelayMicros(micros) => {
                outputln!("EVENT\tDELAY\tmicros={micros}");
            }
            execution::ExecutionEvent::Fence {
                fm,
                predecessor,
                successor,
            } => outputln!("EVENT\tFENCE\tfm={fm:#x}\tpred={predecessor:#x}\tsucc={successor:#x}"),
        }
    }
    for call in &result.calls {
        outputln!("COVERED-CALL\t{call}");
    }
    if *print_timeline {
        for (index, event) in result.timeline.iter().enumerate() {
            match event {
                execution::ExecutionTimelineEvent::Observable(event) => {
                    outputln!("TIMELINE-EVENT\t{index}\tOBSERVABLE\t{event:?}");
                }
                execution::ExecutionTimelineEvent::Call(call) => outputln!(
                    "TIMELINE-EVENT\t{index}\tCALL\t{}\t{}\targs={:08x},{:08x},{:08x},{:08x},{:08x},{:08x},{:08x},{:08x}",
                    image.location(call.site),
                    call.symbol,
                    call.arguments[0],
                    call.arguments[1],
                    call.arguments[2],
                    call.arguments[3],
                    call.arguments[4],
                    call.arguments[5],
                    call.arguments[6],
                    call.arguments[7],
                ),
                execution::ExecutionTimelineEvent::Branch { site, taken } => outputln!(
                    "TIMELINE-EVENT\t{index}\tBRANCH\t{}\ttaken={taken}",
                    image.location(*site)
                ),
                execution::ExecutionTimelineEvent::RamRead {
                    width,
                    address,
                    value,
                } => outputln!(
                    "TIMELINE-EVENT\t{index}\tRAM-READ\t{width}\t{address:#010x}\tvalue={value:#010x}"
                ),
                execution::ExecutionTimelineEvent::RamWrite {
                    width,
                    address,
                    value,
                } => outputln!(
                    "TIMELINE-EVENT\t{index}\tRAM-WRITE\t{width}\t{address:#010x}\tvalue={value:#010x}"
                ),
            }
        }
    }
    let uncovered_branches = crate::cli::render::branch_coverage(
        "image",
        image,
        &inventory.branch_outcomes,
        &result.branches,
    );
    for (address, edge) in &inventory.unresolved_edges {
        outputln!(
            "UNCOVERED-CONTROL-FLOW\timage\t{}\t{edge}",
            image.location(*address)
        );
    }
    for address in &unnamed {
        outputln!("UNNAMED-MMIO\timage\t{address:#010x}");
    }
    for change in &result.memory_changes {
        outputln!(
            "MEMORY-CHANGE\t{:#010x}\tbefore={:#04x}\tafter={:#04x}",
            change.address,
            change.before,
            change.after
        );
    }
    outputln!(
        "RESULT\tsymbol={symbol}\tevidence={}\tsteps={}\treturn={:#010x}\tbranches={}\tbranch-events={}\tcalls={}\tcall-events={}\ttimeline-events={}\tmemory-changes={}\tuncovered-branch-outcomes={uncovered_branches}\tunresolved-control-flow={}\tunnamed-mmio={}",
        if *concrete_only {
            "concrete-only"
        } else {
            "branch-complete"
        },
        result.steps,
        result.return_value,
        result.branches.len(),
        result.ordered_branches.len(),
        result.calls.len(),
        result.ordered_calls.len(),
        result.timeline.len(),
        result.memory_changes.len(),
        inventory.unresolved_edges.len(),
        unnamed.len(),
    );
}
