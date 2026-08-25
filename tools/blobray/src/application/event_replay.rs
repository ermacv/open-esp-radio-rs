//! Frontend-neutral execution and publication of one reviewed event replay.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::{
    MmioMap, ProjectSpec, Result, TargetSpec, execution, execution_model,
    interfaces::InterfaceWorkspace,
};

#[derive(Clone, Debug)]
pub(crate) struct EventReplayRequest {
    pub(crate) manifest: PathBuf,
    pub(crate) artifact: PathBuf,
    pub(crate) companion: Option<PathBuf>,
}

pub(crate) struct PreparedEventReplay {
    pub(crate) request: EventReplayRequest,
    manifest: ReplayManifest,
}

impl PreparedEventReplay {
    pub(crate) fn requires_reviewed_interfaces(&self) -> bool {
        self.manifest
            .phases
            .iter()
            .any(|phase| !phase.tables.is_empty())
    }
}

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
    arguments: Vec<ReplayValue>,
    #[serde(default)]
    memory: Vec<ReplayMemorySeed>,
    #[serde(default)]
    mmio: Vec<ReplayMmioValue>,
    #[serde(default)]
    mmio_reads: Vec<ReplayMmioValue>,
    #[serde(default)]
    observe_memory: Vec<ReplayMemoryObservation>,
    #[serde(default)]
    expectations: Vec<ReplayExpectation>,
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
    value: ReplayValue,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ReplayValue {
    Literal(u32),
    Symbol {
        symbol: String,
        #[serde(default)]
        offset: i32,
    },
}

impl ReplayValue {
    fn resolve(&self, image: &execution::ExecutableImage) -> Result<u32> {
        match self {
            Self::Literal(value) => Ok(*value),
            Self::Symbol { symbol, offset } => image
                .symbol_address(symbol)
                .map(|address| address.wrapping_add(*offset as u32))
                .ok_or_else(|| {
                    crate::Error::invalid(format!(
                        "replay value refers to missing linked symbol {symbol}"
                    ))
                }),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct ReplayMemoryObservation {
    id: String,
    symbol: String,
    #[serde(default)]
    offset: i32,
    width: u8,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayMmioValue {
    address: u32,
    value: u32,
}

#[derive(Debug, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "kebab-case"
)]
enum ReplayExpectation {
    Memory {
        observation: String,
        before: ReplayValue,
        after: ReplayValue,
    },
    FifoEnqueue {
        service_id: String,
        value: u32,
        depth_before: usize,
        depth_after: usize,
        woke_receiver: bool,
    },
    FifoDequeue {
        service_id: String,
        value: u32,
        depth_before: usize,
        depth_after: usize,
    },
    Call {
        symbol: String,
        count: usize,
        #[serde(default)]
        argument0: Option<u32>,
    },
    NoDelay,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct ReplayCall {
    symbol: String,
    returns: Vec<ReplayValue>,
}

pub(crate) fn execute(
    request: &EventReplayRequest,
    svd: &MmioMap,
    target: &TargetSpec,
    project: Option<&ProjectSpec>,
) -> Result<crate::artifacts::ReplayEvidenceDocument> {
    execute_prepared(prepare(request.clone())?, svd, target, project)
}

pub(crate) fn prepare(request: EventReplayRequest) -> Result<PreparedEventReplay> {
    let manifest = load_manifest(&request.manifest)?;
    Ok(PreparedEventReplay { request, manifest })
}

pub(crate) fn manifest_requires_reviewed_interfaces(path: &Path) -> Result<bool> {
    Ok(load_manifest(path)?
        .phases
        .iter()
        .any(|phase| !phase.tables.is_empty()))
}

fn load_manifest(path: &Path) -> Result<ReplayManifest> {
    let input = std::fs::read_to_string(path)
        .map_err(|error| crate::Error::read("replay manifest", path, error))?;
    let manifest: ReplayManifest = toml_edit::de::from_str(&input).map_err(|error| {
        crate::Error::invalid(format!(
            "invalid replay manifest {}: {error}",
            path.display()
        ))
    })?;
    if manifest.schema != 2 {
        return Err(crate::Error::invalid(format!(
            "replay manifest {} requires schema = 2",
            path.display()
        )));
    }
    Ok(manifest)
}

pub(crate) fn execute_prepared(
    prepared: PreparedEventReplay,
    svd: &MmioMap,
    target: &TargetSpec,
    project: Option<&ProjectSpec>,
) -> Result<crate::artifacts::ReplayEvidenceDocument> {
    let PreparedEventReplay { request, manifest } = prepared;
    let mut image = execution::ExecutableImage::load(&request.artifact)?;
    if let Some(companion) = request.companion.as_deref() {
        image.add_companion(companion)?;
    }
    let diagnostic_contracts =
        crate::harnesses::diagnostic_contracts_or_empty(target.knowledge_provider.as_deref())?;
    image.configure_diagnostic_calls(diagnostic_contracts.configured_calls())?;
    validate_tables(project, target, &manifest)?;

    let mut services = Some(manifest.fifo_services);
    let mut phase_evidence = Vec::with_capacity(manifest.phases.len());
    let mut names = std::collections::BTreeSet::new();
    let mut session = execution::ExecutionSession::default();
    for phase in manifest.phases {
        if phase.name.trim().is_empty() || !names.insert(phase.name.clone()) {
            return Err(crate::Error::invalid(format!(
                "execution replay phase names must be non-empty and unique: {:?}",
                phase.name
            )));
        }
        let arguments = phase
            .arguments
            .iter()
            .map(|value| value.resolve(&image))
            .collect::<Result<Vec<_>>>()?;
        let mut scenario = execution::Scenario {
            arguments,
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
        for seed in phase.mmio {
            if scenario
                .mmio_initial
                .insert(seed.address, seed.value)
                .is_some()
            {
                return Err(crate::Error::invalid(format!(
                    "execution replay phase {} repeats MMIO initial address {:#010x}",
                    phase.name, seed.address
                )));
            }
        }
        for read in phase.mmio_reads {
            scenario
                .mmio_reads
                .entry(read.address)
                .or_default()
                .push_back(read.value);
        }
        let observations = resolve_observations(&image, &phase.observe_memory)?;
        scenario
            .observed_memory
            .extend(
                observations
                    .iter()
                    .map(|observation| execution_model::MemoryRange {
                        start: observation.address,
                        length: u32::from(observation.width / 8),
                    }),
            );
        let before = observations
            .iter()
            .map(|observation| read_word_before(&image, &session, &scenario, observation))
            .collect::<Result<Vec<_>>>()?;
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
                    .iter()
                    .map(|value| value.resolve(&image))
                    .collect::<Result<Vec<_>>>()?
                    .into_iter()
                    .map(execution::ModeledCallResponse::scalar)
                    .collect(),
            );
        }
        let result = session
            .execute(&image, svd, &phase.symbol, scenario)
            .map_err(|error| {
                crate::Error::invalid(format!(
                    "execution replay phase {} ({}) failed: {error}",
                    phase.name, phase.symbol
                ))
            })?;
        let memory_observations = observations
            .iter()
            .zip(before)
            .map(|(observation, before)| {
                let after = read_word_after(&image, &session, observation)?;
                let writes = result
                    .timeline
                    .iter()
                    .filter_map(|event| match event {
                        execution::ExecutionTimelineEvent::RamWrite {
                            site,
                            width,
                            address,
                            value,
                        } if *width == observation.width && *address == observation.address => {
                            Some(crate::artifacts::ReplayMemoryWriteDocument {
                                site: *site,
                                value: *value,
                            })
                        }
                        _ => None,
                    })
                    .collect();
                Ok(crate::artifacts::ReplayMemoryObservationDocument {
                    id: observation.id.clone(),
                    symbol: observation.symbol.clone(),
                    address: observation.address,
                    width: observation.width,
                    before,
                    after,
                    writes,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        validate_expectations(
            &image,
            &phase.name,
            &phase.expectations,
            &result,
            &memory_observations,
        )?;
        phase_evidence.push(crate::artifacts::ReplayPhaseEvidence {
            execution: execution::ExecutionPhaseResult {
                name: phase.name,
                symbol: phase.symbol,
                result,
            },
            memory_observations,
        });
    }
    crate::artifacts::build_replay_evidence(
        &request.manifest,
        &request.artifact,
        diagnostic_contracts,
        phase_evidence,
    )
}

struct ResolvedMemoryObservation {
    id: String,
    symbol: String,
    address: u32,
    width: u8,
}

fn resolve_observations(
    image: &execution::ExecutableImage,
    observations: &[ReplayMemoryObservation],
) -> Result<Vec<ResolvedMemoryObservation>> {
    let mut ids = std::collections::BTreeSet::new();
    observations
        .iter()
        .map(|observation| {
            if observation.id.trim().is_empty() || !ids.insert(observation.id.as_str()) {
                return Err(crate::Error::invalid(format!(
                    "replay memory observation IDs must be non-empty and unique: {:?}",
                    observation.id
                )));
            }
            if !matches!(observation.width, 8 | 16 | 32) {
                return Err(crate::Error::invalid(format!(
                    "memory observation {:?} width must be 8, 16, or 32",
                    observation.id
                )));
            }
            let base = image.symbol_address(&observation.symbol).ok_or_else(|| {
                crate::Error::invalid(format!(
                    "memory observation {:?} refers to missing linked symbol {}",
                    observation.id, observation.symbol
                ))
            })?;
            Ok(ResolvedMemoryObservation {
                id: observation.id.clone(),
                symbol: observation.symbol.clone(),
                address: base.wrapping_add(observation.offset as u32),
                width: observation.width,
            })
        })
        .collect()
}

fn read_word_before(
    image: &execution::ExecutableImage,
    session: &execution::ExecutionSession,
    scenario: &execution::Scenario,
    observation: &ResolvedMemoryObservation,
) -> Result<u32> {
    read_word(observation, |address| {
        scenario
            .memory_initial
            .get(&address)
            .copied()
            .or_else(|| session.byte(image, address))
    })
}

fn read_word_after(
    image: &execution::ExecutableImage,
    session: &execution::ExecutionSession,
    observation: &ResolvedMemoryObservation,
) -> Result<u32> {
    read_word(observation, |address| session.byte(image, address))
}

fn read_word(
    observation: &ResolvedMemoryObservation,
    mut byte: impl FnMut(u32) -> Option<u8>,
) -> Result<u32> {
    let mut value = 0_u32;
    for offset in 0..u32::from(observation.width / 8) {
        let address = observation.address.wrapping_add(offset);
        let current = byte(address).ok_or_else(|| {
            crate::Error::invalid(format!(
                "memory observation {:?} cannot read {address:#010x}",
                observation.id
            ))
        })?;
        value |= u32::from(current) << (offset * 8);
    }
    Ok(value)
}

fn validate_expectations(
    image: &execution::ExecutableImage,
    phase: &str,
    expectations: &[ReplayExpectation],
    result: &execution::ExecutionResult,
    observations: &[crate::artifacts::ReplayMemoryObservationDocument],
) -> Result<()> {
    for expectation in expectations {
        match expectation {
            ReplayExpectation::Memory {
                observation,
                before,
                after,
            } => {
                let before = before.resolve(image)?;
                let after = after.resolve(image)?;
                let matches = observations
                    .iter()
                    .filter(|candidate| candidate.id == *observation)
                    .collect::<Vec<_>>();
                let [actual] = matches.as_slice() else {
                    return Err(expectation_error(
                        phase,
                        expectation,
                        format!(
                            "expected exactly one memory observation {observation:?}, found {}",
                            matches.len()
                        ),
                    ));
                };
                if actual.before != before || actual.after != after {
                    return Err(expectation_error(
                        phase,
                        expectation,
                        format!(
                            "observed {} transition {:#x} -> {:#x}",
                            actual.id, actual.before, actual.after
                        ),
                    ));
                }
            }
            ReplayExpectation::FifoEnqueue {
                service_id,
                value,
                depth_before,
                depth_after,
                woke_receiver,
            } => {
                let matches = result
                    .fifo_lifecycle
                    .iter()
                    .filter(|event| {
                        matches!(
                            event,
                            execution_model::FifoLifecycleEvent::Enqueued {
                                service_id: actual_service,
                                value: actual_value,
                                depth_before: actual_before,
                                depth_after: actual_after,
                                woke_receiver: actual_woke,
                                ..
                            } if actual_service == service_id
                                && actual_value == value
                                && actual_before == depth_before
                                && actual_after == depth_after
                                && actual_woke == woke_receiver
                        )
                    })
                    .count();
                if matches != 1 {
                    return Err(expectation_error(
                        phase,
                        expectation,
                        format!("found {matches} matching enqueue events"),
                    ));
                }
            }
            ReplayExpectation::FifoDequeue {
                service_id,
                value,
                depth_before,
                depth_after,
            } => {
                let matches = result
                    .fifo_lifecycle
                    .iter()
                    .filter(|event| {
                        matches!(
                            event,
                            execution_model::FifoLifecycleEvent::Dequeued {
                                service_id: actual_service,
                                value: actual_value,
                                depth_before: actual_before,
                                depth_after: actual_after,
                                ..
                            } if actual_service == service_id
                                && actual_value == value
                                && actual_before == depth_before
                                && actual_after == depth_after
                        )
                    })
                    .count();
                if matches != 1 {
                    return Err(expectation_error(
                        phase,
                        expectation,
                        format!("found {matches} matching dequeue events"),
                    ));
                }
            }
            ReplayExpectation::Call {
                symbol,
                count,
                argument0,
            } => {
                let matches = result
                    .ordered_calls
                    .iter()
                    .filter(|call| {
                        call.symbol == *symbol
                            && argument0.is_none_or(|expected| call.arguments[0] == expected)
                    })
                    .count();
                if matches != *count {
                    return Err(expectation_error(
                        phase,
                        expectation,
                        format!("found {matches} matching calls"),
                    ));
                }
            }
            ReplayExpectation::NoDelay => {
                let delays = result
                    .events
                    .iter()
                    .filter(|event| matches!(event, execution::ExecutionEvent::DelayMicros(_)))
                    .count();
                if delays != 0 {
                    return Err(expectation_error(
                        phase,
                        expectation,
                        format!("observed {delays} delay effects"),
                    ));
                }
            }
        }
    }
    Ok(())
}

fn expectation_error(
    phase: &str,
    expectation: &ReplayExpectation,
    actual: impl std::fmt::Display,
) -> crate::Error {
    crate::Error::invalid(format!(
        "execution replay phase {phase:?} did not satisfy {expectation:?}: {actual}"
    ))
}

pub(crate) fn publish(
    document: &crate::artifacts::ReplayEvidenceDocument,
    output: &Path,
    check: bool,
) -> Result<()> {
    crate::application::generated_file::write_or_check(
        output,
        &crate::artifacts::render_replay_evidence(document)?,
        check,
        "execution replay evidence",
    )
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
    let value = seed.value.resolve(image)?;
    for (offset, byte) in value.to_le_bytes().into_iter().take(bytes).enumerate() {
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
        .knowledge_provider
        .as_deref()
        .and_then(|harness| crate::harnesses::contracts(harness).ok());
    let workspace = InterfaceWorkspace::load_with_templates(
        &paths.facts,
        pack,
        &paths.semantic_catalogs,
        &paths.interface_template_packs,
        target.calling_convention.label(),
        harness,
    )?;
    for table in tables {
        workspace.validate_table_instance(table)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{EventReplayRequest, ReplayManifest, ReplayValue, prepare};

    #[test]
    fn preparation_exposes_only_the_interface_inputs_the_replay_consumes() {
        let directory = std::env::temp_dir().join(format!(
            "blobray-event-replay-inputs-{}",
            std::process::id()
        ));
        if directory.is_dir() {
            std::fs::remove_dir_all(&directory).unwrap();
        }
        std::fs::create_dir_all(&directory).unwrap();
        let manifest = directory.join("replay.toml");
        let request = || EventReplayRequest {
            manifest: manifest.clone(),
            artifact: directory.join("vendor.elf"),
            companion: None,
        };
        std::fs::write(
            &manifest,
            "schema = 2\n\n[[phases]]\nname = \"plain\"\nsymbol = \"entry\"\n",
        )
        .unwrap();
        assert!(!prepare(request()).unwrap().requires_reviewed_interfaces());

        std::fs::write(
            &manifest,
            r#"schema = 2

[[phases]]
name = "with-table"
symbol = "entry"

[[phases.tables]]
layout-id = "fixture::table"
base-address = 4096
layout-size = 4
pointer-cells = []
slots = []
"#,
        )
        .unwrap();
        assert!(prepare(request()).unwrap().requires_reviewed_interfaces());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn modeled_call_return_may_reference_a_linked_symbol() {
        let manifest: ReplayManifest = toml_edit::de::from_str(
            r#"
schema = 2

[[phases]]
name = "symbolic-return"
symbol = "entry"

[[phases.calls]]
symbol = "allocate"
returns = [{ symbol = "fixture", offset = 4 }]
"#,
        )
        .expect("symbolic modeled-call return must deserialize");

        assert!(matches!(
            manifest.phases[0].calls[0].returns.as_slice(),
            [ReplayValue::Symbol { symbol, offset: 4 }] if symbol == "fixture"
        ));
    }
}
