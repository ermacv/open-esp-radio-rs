//! Concrete vendor/Rust execution comparison.

use std::collections::BTreeSet;

use crate::*;

mod diff;
mod scenario;

use diff::{coverage_gap, ordered_transactions, ordered_transactions_equal, trace_difference};
pub(crate) use scenario::*;

/// Project a concrete replay trace into the same fail-closed effect vocabulary
/// used by symbolic binding contracts.
///
/// The execution backend intentionally remains independent of reviewed
/// dispositions. This adapter lives in the generic verifier and rejects
/// unnamed MMIO instead of inventing a register identity.
fn concrete_effects(
    events: &[execution::ExecutionEvent],
) -> core::result::Result<Vec<effect_contract::ContractEffect>, String> {
    events
        .iter()
        .map(|event| match event {
            execution::ExecutionEvent::Read {
                width,
                address,
                register,
                value,
                ..
            } => Ok(effect_contract::ContractEffect::MmioRead {
                register: effect_contract::RegisterId {
                    address: *address,
                    width: *width,
                    name: register.clone().ok_or_else(|| {
                        format!("cannot apply an effect contract to unnamed MMIO {address:#010x}")
                    })?,
                },
                value: effect_contract::ContractValue::Concrete(*value),
            }),
            execution::ExecutionEvent::Write {
                width,
                address,
                register,
                value,
                ..
            } => Ok(effect_contract::ContractEffect::MmioWrite {
                register: effect_contract::RegisterId {
                    address: *address,
                    width: *width,
                    name: register.clone().ok_or_else(|| {
                        format!("cannot apply an effect contract to unnamed MMIO {address:#010x}")
                    })?,
                },
                value: effect_contract::ContractValue::Concrete(*value),
            }),
            execution::ExecutionEvent::DelayMicros(micros) => {
                Ok(effect_contract::ContractEffect::Delay {
                    micros: effect_contract::ContractValue::Concrete(*micros),
                })
            }
            execution::ExecutionEvent::Fence {
                fm,
                predecessor,
                successor,
            } => Ok(effect_contract::ContractEffect::Fence {
                fm: *fm,
                predecessor: *predecessor,
                successor: *successor,
            }),
        })
        .collect()
}

fn artifact_report(input: ExecutionInput<'_>) -> Result<ArtifactReport> {
    Ok(ArtifactReport {
        path: input.artifact.display().to_string(),
        sha256: artifact_sha256(input.artifact)?,
        companion: input
            .companion
            .map(|path| -> Result<ArtifactIdentity> {
                Ok(ArtifactIdentity {
                    path: path.display().to_string(),
                    sha256: artifact_sha256(path)?,
                })
            })
            .transpose()?,
        symbol: input.symbol.to_owned(),
    })
}

fn coverage_report(
    image: &execution::ExecutableImage,
    inventory: &execution::CoverageInventory,
    covered: &BTreeSet<(u32, bool)>,
    calls: BTreeSet<String>,
    indirect_calls: &BTreeSet<execution::IndirectCall>,
    unnamed_mmio: BTreeSet<u32>,
) -> CoverageReport {
    CoverageReport {
        covered_calls: calls.into_iter().collect(),
        branch_outcomes: inventory
            .branch_outcomes
            .iter()
            .map(|(site, taken)| BranchOutcomeReport {
                site: *site,
                location: image.location(*site),
                taken: *taken,
                covered: covered.contains(&(*site, *taken)),
            })
            .collect(),
        unresolved_control_flow: inventory
            .unresolved_edges
            .iter()
            .map(|(site, edge)| {
                let targets = indirect_calls
                    .iter()
                    .filter(|call| call.site == *site)
                    .map(|call| call.symbol.clone())
                    .collect::<Vec<_>>();
                ControlFlowReport {
                    site: *site,
                    location: image.location(*site),
                    edge: edge.clone(),
                    covered: !targets.is_empty(),
                    targets,
                }
            })
            .collect(),
        unnamed_mmio: unnamed_mmio.into_iter().collect(),
    }
}

fn coverage_is_complete(coverage: &CoverageReport) -> bool {
    coverage.uncovered_branch_outcomes() == 0 && coverage.uncovered_control_flow() == 0
}

fn table_instance_report(instance: &crate::execution_model::TableInstance) -> TableInstanceReport {
    TableInstanceReport {
        layout_id: instance.layout_id.clone(),
        base_address: instance.base_address,
        layout_size: instance.layout_size,
        pointer_cells: instance.pointer_cells.clone(),
        pointer_cell_symbols: instance.pointer_cell_symbols.clone(),
        slots: instance
            .slots
            .iter()
            .map(|slot| TableInstanceSlotReport {
                offset: slot.offset,
                target: match &slot.target {
                    crate::execution_model::TableSlotTarget::Null => TableSlotTargetReport::Null,
                    crate::execution_model::TableSlotTarget::Address(address) => {
                        TableSlotTargetReport::Address { address: *address }
                    }
                    crate::execution_model::TableSlotTarget::Symbol(symbol) => {
                        TableSlotTargetReport::Symbol {
                            symbol: symbol.clone(),
                        }
                    }
                    crate::execution_model::TableSlotTarget::ModeledSymbol(symbol) => {
                        TableSlotTargetReport::ModeledSymbol {
                            symbol: symbol.clone(),
                        }
                    }
                },
            })
            .collect(),
    }
}

fn fifo_service_report(service: &crate::execution_model::FifoServiceInstance) -> FifoServiceReport {
    FifoServiceReport {
        id: service.id.clone(),
        handle: service.handle,
        item_width: service.item_width,
        capacity: service.capacity,
        items: service.items.clone(),
    }
}

fn fifo_lifecycle_report(
    event: &crate::execution_model::FifoLifecycleEvent,
) -> FifoLifecycleReport {
    use crate::execution_model::FifoLifecycleEvent;
    match event {
        FifoLifecycleEvent::Enqueued {
            service_id,
            site,
            value,
            depth_before,
            depth_after,
            woke_receiver,
        } => FifoLifecycleReport::Enqueued {
            service_id: service_id.clone(),
            site: *site,
            value: *value,
            depth_before: *depth_before,
            depth_after: *depth_after,
            woke_receiver: *woke_receiver,
        },
        FifoLifecycleEvent::Dequeued {
            service_id,
            site,
            value,
            depth_before,
            depth_after,
        } => FifoLifecycleReport::Dequeued {
            service_id: service_id.clone(),
            site: *site,
            value: *value,
            depth_before: *depth_before,
            depth_after: *depth_after,
        },
        FifoLifecycleEvent::Full {
            service_id,
            site,
            value,
            depth,
        } => FifoLifecycleReport::Full {
            service_id: service_id.clone(),
            site: *site,
            value: *value,
            depth: *depth,
        },
        FifoLifecycleEvent::Empty { service_id, site } => FifoLifecycleReport::Empty {
            service_id: service_id.clone(),
            site: *site,
        },
        FifoLifecycleEvent::Length {
            service_id,
            site,
            depth,
        } => FifoLifecycleReport::Length {
            service_id: service_id.clone(),
            site: *site,
            depth: *depth,
        },
    }
}

fn memory_instance_report(instance: &RuntimeMemoryInstance) -> RuntimeMemoryInstanceReport {
    RuntimeMemoryInstanceReport {
        id: instance.id.clone(),
        base_address: instance.base_address,
        length: instance.length,
        bindings: instance
            .bindings
            .iter()
            .map(|binding| match binding {
                RuntimeMemoryObjectBinding::Argument { index } => {
                    RuntimeMemoryBindingReport::Argument { index: *index }
                }
                RuntimeMemoryObjectBinding::Global { symbol } => {
                    RuntimeMemoryBindingReport::Global {
                        symbol: symbol.clone(),
                    }
                }
                RuntimeMemoryObjectBinding::DereferencedGlobal {
                    symbol,
                    pointer_offset,
                } => RuntimeMemoryBindingReport::DereferencedGlobal {
                    symbol: symbol.clone(),
                    pointer_offset: *pointer_offset,
                },
                RuntimeMemoryObjectBinding::Absolute {
                    address_space,
                    address,
                } => RuntimeMemoryBindingReport::Absolute {
                    address_space: address_space.clone(),
                    address: *address,
                },
            })
            .collect(),
    }
}

fn memory_input_report(memory: &std::collections::BTreeMap<u32, u8>) -> Vec<MemoryInputReport> {
    memory
        .iter()
        .map(|(address, value)| MemoryInputReport {
            address: *address,
            value: *value,
        })
        .collect()
}

fn table_lifecycle_report(
    event: &crate::execution_model::TableLifecycleEvent,
) -> TableLifecycleReport {
    match event {
        crate::execution_model::TableLifecycleEvent::SlotInitialized {
            layout_id,
            offset,
            target,
        } => TableLifecycleReport::SlotInitialized {
            layout_id: layout_id.clone(),
            offset: *offset,
            target: *target,
        },
        crate::execution_model::TableLifecycleEvent::SlotWritten {
            layout_id,
            offset,
            width,
            value,
            site,
        } => TableLifecycleReport::SlotWritten {
            layout_id: layout_id.clone(),
            offset: *offset,
            width: *width,
            value: *value,
            site: *site,
        },
        crate::execution_model::TableLifecycleEvent::PointerInstalled {
            layout_id,
            address,
            base_address,
        } => TableLifecycleReport::PointerInstalled {
            layout_id: layout_id.clone(),
            address: *address,
            base_address: *base_address,
        },
        crate::execution_model::TableLifecycleEvent::IndirectCall {
            layout_id,
            slot_offset,
            site,
            target,
            symbol,
        } => TableLifecycleReport::IndirectCall {
            layout_id: layout_id.clone(),
            slot_offset: *slot_offset,
            site: *site,
            target: *target,
            symbol: symbol.clone(),
        },
    }
}

fn device_coverage_report(
    outcome: &crate::execution_model::DeviceModelOutcome,
) -> DeviceModelCoverageReport {
    DeviceModelCoverageReport {
        id: outcome.descriptor.id.clone(),
        kind: outcome.descriptor.kind.clone(),
        complete: outcome.coverage.complete,
        reason: outcome.coverage.reason.clone(),
    }
}

fn allocation_lifecycle_report(
    event: &crate::execution::AllocationLifecycleEvent,
) -> AllocationLifecycleReport {
    AllocationLifecycleReport {
        site: event.site,
        symbol: event.symbol.clone(),
        address: event.address,
        requested: event.requested,
        capacity: event.capacity,
        zeroed: event.zeroed,
    }
}

fn scenario_environment(named: &NamedScenario) -> ScenarioEnvironmentReport {
    let common = &named.scenario.table_instances;
    ScenarioEnvironmentReport {
        vendor_stack_fill: named.vendor_stack_fill,
        rust_stack_fill: named.rust_stack_fill,
        vendor_tables: common
            .iter()
            .chain(&named.vendor_table_instances)
            .map(table_instance_report)
            .collect(),
        rust_tables: common
            .iter()
            .chain(&named.rust_table_instances)
            .map(table_instance_report)
            .collect(),
        vendor_memory_instances: named
            .vendor_memory_instances
            .iter()
            .map(memory_instance_report)
            .collect(),
        rust_memory_instances: named
            .rust_memory_instances
            .iter()
            .map(memory_instance_report)
            .collect(),
        vendor_carried_memory: Vec::new(),
        rust_carried_memory: Vec::new(),
        vendor_explicit_memory: Vec::new(),
        rust_explicit_memory: Vec::new(),
        device_models: named
            .scenario
            .device_models
            .iter()
            .map(|model| {
                let descriptor = model.descriptor();
                DeviceModelReport {
                    id: descriptor.id,
                    kind: descriptor.kind,
                    start: descriptor.range.start,
                    length: descriptor.range.length,
                    configuration: descriptor.configuration,
                }
            })
            .collect(),
        vendor_device_coverage: Vec::new(),
        rust_device_coverage: Vec::new(),
        vendor_allocations: Vec::new(),
        rust_allocations: Vec::new(),
        vendor_table_lifecycle: Vec::new(),
        rust_table_lifecycle: Vec::new(),
        vendor_table_lifecycle_complete: None,
        rust_table_lifecycle_complete: None,
        vendor_fifo_services: named
            .vendor_fifo_services
            .iter()
            .map(fifo_service_report)
            .collect(),
        rust_fifo_services: named
            .rust_fifo_services
            .iter()
            .map(fifo_service_report)
            .collect(),
        vendor_fifo_lifecycle: Vec::new(),
        rust_fifo_lifecycle: Vec::new(),
        vendor_completion: None,
        rust_completion: None,
    }
}

#[tracing::instrument(
    name = "compare_execution_scenarios",
    skip_all,
    fields(
        vendor = %vendor.artifact.display(),
        vendor_symbol = vendor.symbol,
        rust = %rust.artifact.display(),
        rust_symbol = rust.symbol,
        scenarios = scenarios.len()
    )
)]
pub(crate) fn compare_execution_scenarios(
    svd: &MmioMap,
    vendor: ExecutionInput<'_>,
    rust: ExecutionInput<'_>,
    policy: ExecutionComparisonPolicy<'_>,
    scenarios: &[NamedScenario],
) -> Result<ExecutionComparisonReport> {
    let ExecutionComparisonPolicy {
        compare_return,
        case_execution,
        transaction_comparison,
        effect_policy,
        call_equivalences,
        coverage_domain,
        vendor_setup,
    } = policy;
    let compare_under_effect_contract = matches!(
        transaction_comparison,
        profiles::TransactionComparison::ObservablesUnderEffectContract
    );
    if compare_under_effect_contract != effect_policy.is_some() {
        return Err(crate::Error::invalid(if compare_under_effect_contract {
            "observables-under-effect-contract requires one reviewed disposition effect contract"
        } else {
            "a disposition effect contract may affect concrete execution only when the profile selects observables-under-effect-contract"
        }));
    }
    if compare_return
        && scenarios.iter().any(|scenario| {
            !matches!(
                scenario.vendor_goal,
                crate::execution_model::ExecutionGoal::Return
            ) || !matches!(
                scenario.rust_goal,
                crate::execution_model::ExecutionGoal::Return
            )
        })
    {
        return Err(crate::Error::invalid(
            "return comparison requires return-complete vendor and Rust execution goals",
        ));
    }
    let vendor_report = artifact_report(vendor)?;
    let rust_report = artifact_report(rust)?;
    let mut vendor_image = execution::ExecutableImage::load(vendor.artifact)?;
    if let Some(companion) = vendor.companion {
        vendor_image.add_companion(companion)?;
    }
    let mut rust_image = execution::ExecutableImage::load(rust.artifact)?;
    if let Some(companion) = rust.companion {
        rust_image.add_companion(companion)?;
    }
    let concrete_state_cases = transaction_comparison.state_domain();
    let mut vendor_inventory = if concrete_state_cases {
        execution::CoverageInventory::default()
    } else {
        static_inventory_for_argument_domain(
            &vendor_image,
            vendor.symbol,
            coverage_domain,
            scenarios
                .iter()
                .map(|scenario| scenario.vendor_goal.clone()),
        )?
    };
    let mut rust_inventory = if concrete_state_cases {
        execution::CoverageInventory::default()
    } else {
        static_inventory_for_argument_domain(
            &rust_image,
            rust.symbol,
            coverage_domain,
            scenarios.iter().map(|scenario| scenario.rust_goal.clone()),
        )?
    };
    let mut vendor_covered = BTreeSet::new();
    let mut rust_covered = BTreeSet::new();
    let mut vendor_calls = BTreeSet::new();
    let mut rust_calls = BTreeSet::new();
    let mut vendor_indirect_calls = BTreeSet::new();
    let mut rust_indirect_calls = BTreeSet::new();
    let mut vendor_unmapped = BTreeSet::new();
    let mut rust_unmapped = BTreeSet::new();
    let mut rust_executed_pcs = BTreeSet::new();
    let mut matched_cases = 0_usize;
    let mut different_cases = 0_usize;
    let mut incomplete_cases = 0_usize;
    let mut case_reports = Vec::with_capacity(scenarios.len());
    let mut vendor_session = execution::ExecutionSession::default();
    let mut rust_session = execution::ExecutionSession::default();
    let mut stateful_blocker: Option<String> = None;
    let mut vendor_setup_reports = Vec::with_capacity(vendor_setup.len());
    for setup in vendor_setup {
        let result = vendor_session
            .execute(&vendor_image, svd, &setup.symbol, setup.scenario.clone())
            .map_err(|error| {
                crate::Error::invalid(format!(
                    "vendor setup phase {:?} (symbol {}) failed: {error}",
                    setup.name, setup.symbol
                ))
            })?;
        vendor_setup_reports.push(SetupPhaseReport {
            name: setup.name.clone(),
            symbol: setup.symbol.clone(),
            completion: ExecutionCompletionReport::from(&result.completion),
            steps: result.steps,
            calls: result.calls.iter().cloned().collect(),
            memory_changes: result
                .memory_changes
                .iter()
                .map(MemoryChangeReport::from)
                .collect(),
        });
    }

    for named in scenarios {
        let mut environment = scenario_environment(named);
        if case_execution == profiles::CaseExecution::Independent {
            vendor_session = execution::ExecutionSession::default();
            rust_session = execution::ExecutionSession::default();
        } else if let Some(blocker) = &stateful_blocker {
            incomplete_cases += 1;
            case_reports.push(CaseReport::Incomplete {
                name: named.name.clone(),
                environment,
                vendor_error: Some(blocker.clone()),
                rust_error: Some(blocker.clone()),
            });
            continue;
        }
        let vendor_lengths: Vec<_> = named
            .vendor_observations
            .iter()
            .map(MemoryObservation::length)
            .collect();
        let rust_lengths: Vec<_> = named
            .rust_observations
            .iter()
            .map(MemoryObservation::length)
            .collect();
        if vendor_lengths != rust_lengths {
            return Err(crate::Error::invalid(format!(
                "scenario {} has different vendor/Rust observation layouts",
                named.name
            )));
        }
        let vendor_result = vendor_session.execute(
            &vendor_image,
            svd,
            vendor.symbol,
            resolved_scenario(named, &vendor_image, true)?,
        );
        let rust_result = rust_session.execute(
            &rust_image,
            svd,
            rust.symbol,
            resolved_scenario(named, &rust_image, false)?,
        );
        let (vendor_result, rust_result) = match (vendor_result, rust_result) {
            (Ok(vendor_result), Ok(rust_result)) => (vendor_result, rust_result),
            (vendor_result, rust_result) => {
                if case_execution == profiles::CaseExecution::Stateful {
                    stateful_blocker = Some(format!(
                        "stateful comparison stopped after incomplete phase {:?}",
                        named.name
                    ));
                }
                incomplete_cases += 1;
                case_reports.push(CaseReport::Incomplete {
                    name: named.name.clone(),
                    environment,
                    vendor_error: vendor_result.err().map(|error| error.to_string()),
                    rust_error: rust_result.err().map(|error| error.to_string()),
                });
                continue;
            }
        };
        environment.vendor_carried_memory = memory_input_report(&vendor_result.carried_memory);
        environment.rust_carried_memory = memory_input_report(&rust_result.carried_memory);
        environment.vendor_explicit_memory = memory_input_report(&vendor_result.explicit_memory);
        environment.rust_explicit_memory = memory_input_report(&rust_result.explicit_memory);
        environment.vendor_table_lifecycle = vendor_result
            .table_lifecycle
            .iter()
            .map(table_lifecycle_report)
            .collect();
        environment.vendor_fifo_services = vendor_result
            .fifo_services
            .iter()
            .map(fifo_service_report)
            .collect();
        environment.rust_fifo_services = rust_result
            .fifo_services
            .iter()
            .map(fifo_service_report)
            .collect();
        environment.vendor_fifo_lifecycle = vendor_result
            .fifo_lifecycle
            .iter()
            .map(fifo_lifecycle_report)
            .collect();
        environment.rust_fifo_lifecycle = rust_result
            .fifo_lifecycle
            .iter()
            .map(fifo_lifecycle_report)
            .collect();
        environment.vendor_completion = Some((&vendor_result.completion).into());
        environment.rust_completion = Some((&rust_result.completion).into());
        environment.vendor_allocations = vendor_result
            .allocations
            .iter()
            .map(allocation_lifecycle_report)
            .collect();
        environment.rust_allocations = rust_result
            .allocations
            .iter()
            .map(allocation_lifecycle_report)
            .collect();
        environment.rust_table_lifecycle = rust_result
            .table_lifecycle
            .iter()
            .map(table_lifecycle_report)
            .collect();
        environment.vendor_table_lifecycle_complete = Some(vendor_result.table_lifecycle_complete);
        environment.rust_table_lifecycle_complete = Some(rust_result.table_lifecycle_complete);
        environment.vendor_device_coverage = vendor_result
            .device_model_coverage
            .iter()
            .map(device_coverage_report)
            .collect();
        environment.rust_device_coverage = rust_result
            .device_model_coverage
            .iter()
            .map(device_coverage_report)
            .collect();
        let vendor_device_incomplete = environment
            .vendor_device_coverage
            .iter()
            .find(|coverage| !coverage.complete)
            .map(|coverage| {
                format!(
                    "device model {} is incomplete: {}",
                    coverage.id,
                    coverage.reason.as_deref().unwrap_or("unspecified reason")
                )
            });
        let rust_device_incomplete = environment
            .rust_device_coverage
            .iter()
            .find(|coverage| !coverage.complete)
            .map(|coverage| {
                format!(
                    "device model {} is incomplete: {}",
                    coverage.id,
                    coverage.reason.as_deref().unwrap_or("unspecified reason")
                )
            });
        if !vendor_result.table_lifecycle_complete
            || !rust_result.table_lifecycle_complete
            || vendor_device_incomplete.is_some()
            || rust_device_incomplete.is_some()
        {
            if case_execution == profiles::CaseExecution::Stateful {
                stateful_blocker = Some(format!(
                    "stateful comparison stopped after incomplete phase {:?}",
                    named.name
                ));
            }
            incomplete_cases += 1;
            case_reports.push(CaseReport::Incomplete {
                name: named.name.clone(),
                environment,
                vendor_error: if !vendor_result.table_lifecycle_complete {
                    Some("runtime table call could not be linked to one slot".to_owned())
                } else {
                    vendor_device_incomplete
                },
                rust_error: if !rust_result.table_lifecycle_complete {
                    Some("runtime table call could not be linked to one slot".to_owned())
                } else {
                    rust_device_incomplete
                },
            });
            continue;
        }
        vendor_covered.extend(vendor_result.branches.iter().copied());
        rust_covered.extend(rust_result.branches.iter().copied());
        vendor_calls.extend(vendor_result.calls.iter().cloned());
        rust_calls.extend(rust_result.calls.iter().cloned());
        vendor_indirect_calls.extend(vendor_result.indirect_calls.iter().cloned());
        rust_indirect_calls.extend(rust_result.indirect_calls.iter().cloned());
        vendor_unmapped.extend(
            vendor_result
                .events
                .iter()
                .filter_map(unnamed_execution_address),
        );
        rust_unmapped.extend(
            rust_result
                .events
                .iter()
                .filter_map(unnamed_execution_address),
        );
        rust_executed_pcs.extend(rust_result.executed_pcs.iter().copied());

        let events_equal = vendor_result.events == rust_result.events;
        let effect_contract_outcome = effect_policy.map(|policy| {
            let vendor_effects = concrete_effects(&vendor_result.events)
                .map_err(|error| format!("vendor observable: {error}"))?;
            let rust_effects = concrete_effects(&rust_result.events)
                .map_err(|error| format!("Rust observable: {error}"))?;
            effect_contract::compare_effects(&vendor_effects, &rust_effects, policy)
                .map_err(|error| error.to_string())
        });
        let effect_contract_outcome = match effect_contract_outcome.transpose() {
            Ok(outcome) => outcome,
            Err(reason) => {
                incomplete_cases += 1;
                case_reports.push(CaseReport::Incomplete {
                    name: named.name.clone(),
                    environment,
                    vendor_error: Some(format!(
                        "concrete effect-contract comparison is incomplete: {reason}"
                    )),
                    rust_error: None,
                });
                continue;
            }
        };
        if let Some(outcome) = &effect_contract_outcome
            && outcome.verdict == effect_contract::EquivalenceVerdict::Incomplete
        {
            incomplete_cases += 1;
            case_reports.push(CaseReport::Incomplete {
                name: named.name.clone(),
                environment,
                vendor_error: Some(format!(
                    "concrete effect-contract comparison is incomplete: {}",
                    outcome.reason.as_deref().unwrap_or("unclassified effect")
                )),
                rust_error: None,
            });
            continue;
        }
        let contract_equal = effect_contract_outcome
            .as_ref()
            .is_some_and(|outcome| outcome.verdict == effect_contract::EquivalenceVerdict::Match);
        let transactions_equal = if compare_under_effect_contract {
            contract_equal
        } else {
            ordered_transactions_equal(
                &vendor_result,
                &rust_result,
                transaction_comparison,
                compare_return,
                call_equivalences,
            )
        };
        let memory_equal = vendor_result.memory_changes == rust_result.memory_changes;
        let returns_equal =
            !compare_return || vendor_result.return_value == rust_result.return_value;
        let case_equal = if transaction_comparison.state_domain() {
            transactions_equal
        } else {
            (events_equal || compare_under_effect_contract && contract_equal)
                && transactions_equal
                && memory_equal
                && returns_equal
        };
        if case_equal {
            matched_cases += 1;
            case_reports.push(CaseReport::Match {
                name: named.name.clone(),
                environment,
                events: vendor_result.events.len(),
                memory_changes: vendor_result.memory_changes.len(),
                return_compared: compare_return,
                trace: MatchedTraceReport {
                    events: vendor_result
                        .events
                        .iter()
                        .enumerate()
                        .map(|(index, event)| MatchedEventReport {
                            index,
                            event: event.into(),
                            vendor_producer: vendor_result
                                .event_producers
                                .get(index)
                                .map(Into::into),
                            rust_producer: rust_result.event_producers.get(index).map(Into::into),
                        })
                        .collect(),
                    vendor_transactions: ordered_transactions(
                        &vendor_result,
                        transaction_comparison,
                        compare_return,
                    ),
                    rust_transactions: ordered_transactions(
                        &rust_result,
                        transaction_comparison,
                        compare_return,
                    ),
                    memory_changes: vendor_result
                        .memory_changes
                        .iter()
                        .map(Into::into)
                        .collect(),
                    return_value: compare_return.then_some(vendor_result.return_value),
                },
            });
        } else {
            different_cases += 1;
            let difference = trace_difference(
                &vendor_result,
                &rust_result,
                compare_return,
                transaction_comparison,
                call_equivalences,
            )
            .ok_or_else(|| {
                crate::Error::invalid(format!(
                    "scenario {} differs without a renderable first difference (events_equal={events_equal}, transactions_equal={transactions_equal}, memory_equal={memory_equal}, returns_equal={returns_equal})",
                    named.name
                ))
            })?;
            case_reports.push(CaseReport::Diff {
                name: named.name.clone(),
                environment,
                difference: Box::new(difference),
            });
        }
    }

    if !concrete_state_cases {
        extend_dynamic_inventory(&vendor_image, &mut vendor_inventory, &vendor_indirect_calls)?;
        extend_dynamic_inventory(&rust_image, &mut rust_inventory, &rust_indirect_calls)?;
    }
    let vendor_coverage = coverage_report(
        &vendor_image,
        &vendor_inventory,
        &vendor_covered,
        vendor_calls,
        &vendor_indirect_calls,
        vendor_unmapped,
    );
    let rust_coverage = coverage_report(
        &rust_image,
        &rust_inventory,
        &rust_covered,
        rust_calls,
        &rust_indirect_calls,
        rust_unmapped,
    );
    let vendor_uncovered = vendor_coverage.uncovered_branch_outcomes();
    let rust_uncovered = rust_coverage.uncovered_branch_outcomes();
    let vendor_unresolved = vendor_coverage.uncovered_control_flow();
    let rust_unresolved = rust_coverage.uncovered_control_flow();
    let cases_match = matched_cases == scenarios.len();
    let coverage_complete =
        coverage_is_complete(&vendor_coverage) && coverage_is_complete(&rust_coverage);
    let verdict = if different_cases != 0 {
        EquivalenceVerdict::Diff
    } else if incomplete_cases != 0 || !coverage_complete || !cases_match {
        EquivalenceVerdict::Incomplete
    } else {
        EquivalenceVerdict::Match
    };
    Ok(ExecutionComparisonReport {
        schema_version: super::execution_report::EXECUTION_COMPARISON_REPORT_SCHEMA,
        command: "execute compare",
        mode: EquivalenceMode::Physical,
        vendor: vendor_report,
        rust: rust_report,
        compare_return,
        case_execution,
        coverage_scope: if concrete_state_cases {
            CoverageScopeReport::ConcreteStateCases
        } else {
            CoverageScopeReport::StaticDomain
        },
        vendor_setup: vendor_setup_reports,
        rust_executed_pcs,
        cases: case_reports,
        coverage_gap: coverage_gap(&vendor_coverage, &rust_coverage),
        summary: ComparisonSummary {
            cases: scenarios.len(),
            matched: matched_cases,
            different: different_cases,
            incomplete: incomplete_cases,
            vendor_uncovered_branch_outcomes: vendor_uncovered,
            rust_uncovered_branch_outcomes: rust_uncovered,
            vendor_unresolved_control_flow: vendor_unresolved,
            rust_unresolved_control_flow: rust_unresolved,
            vendor_unnamed_mmio: vendor_coverage.unnamed_mmio.len(),
            rust_unnamed_mmio: rust_coverage.unnamed_mmio.len(),
        },
        vendor_coverage,
        rust_coverage,
        verdict,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unnamed_registers_are_enrichment_not_a_coverage_gap() {
        let coverage = CoverageReport {
            covered_calls: Vec::new(),
            branch_outcomes: Vec::new(),
            unresolved_control_flow: Vec::new(),
            unnamed_mmio: vec![0x4000_0010],
        };

        assert!(coverage_is_complete(&coverage));
    }

    #[test]
    fn concrete_effect_contract_accepts_only_the_reviewed_ordering_fence() {
        let read = execution::ExecutionEvent::Read {
            width: 32,
            address: 0x2010_1340,
            region: "radio".to_owned(),
            register: Some("IRQ_STATUS".to_owned()),
            value: 0xa55a_00f0,
        };
        let write = execution::ExecutionEvent::Write {
            width: 32,
            address: 0x2010_1058,
            region: "radio".to_owned(),
            register: Some("IRQ_CLEAR".to_owned()),
            value: 0xa55a_00f0,
        };
        let fence = execution::ExecutionEvent::Fence {
            fm: 0,
            predecessor: 15,
            successor: 15,
        };
        let policy = effect_contract::EffectPolicy::new(
            effect_contract::EffectComparison::ExactEffectsV2,
            [
                (
                    effect_contract::EffectSelector::MmioRead {
                        width: 32,
                        address: 0x2010_1340,
                    },
                    effect_contract::EffectDisposition::Required,
                ),
                (
                    effect_contract::EffectSelector::MmioWrite {
                        width: 32,
                        address: 0x2010_1058,
                    },
                    effect_contract::EffectDisposition::Required,
                ),
                (
                    effect_contract::EffectSelector::Fence {
                        fm: 0,
                        predecessor: 15,
                        successor: 15,
                    },
                    effect_contract::EffectDisposition::RustAddition(
                        effect_contract::RustAdditionReason::DeviceOrdering,
                    ),
                ),
            ],
        )
        .unwrap();

        let vendor = concrete_effects(&[read.clone(), write.clone()]).unwrap();
        let rust = concrete_effects(&[read.clone(), write.clone(), fence]).unwrap();
        let outcome = effect_contract::compare_effects(&vendor, &rust, &policy).unwrap();
        assert_eq!(outcome.verdict, effect_contract::EquivalenceVerdict::Match);

        let reordered = concrete_effects(&[write, read]).unwrap();
        let outcome = effect_contract::compare_effects(&vendor, &reordered, &policy).unwrap();
        assert_eq!(outcome.verdict, effect_contract::EquivalenceVerdict::Diff);
    }

    #[test]
    fn concrete_effect_contract_rejects_unnamed_mmio() {
        let event = execution::ExecutionEvent::Read {
            width: 32,
            address: 0x2010_1340,
            region: "radio".to_owned(),
            register: None,
            value: 0,
        };

        assert!(
            concrete_effects(&[event])
                .unwrap_err()
                .contains("unnamed MMIO")
        );
    }
}
