//! Concrete vendor/Rust execution comparison.

use std::collections::BTreeSet;

use crate::*;

mod diff;
mod scenario;

use diff::{coverage_gap, trace_difference};
pub(crate) use scenario::*;

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
    compare_return: bool,
    coverage_domain: &[profiles::ProfileCoverageConstraint],
    scenarios: &[NamedScenario],
) -> Result<ExecutionComparisonReport> {
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
    let mut vendor_inventory =
        static_inventory_for_argument_domain(&vendor_image, vendor.symbol, coverage_domain)?;
    let mut rust_inventory =
        static_inventory_for_argument_domain(&rust_image, rust.symbol, coverage_domain)?;
    let mut vendor_covered = BTreeSet::new();
    let mut rust_covered = BTreeSet::new();
    let mut vendor_calls = BTreeSet::new();
    let mut rust_calls = BTreeSet::new();
    let mut vendor_indirect_calls = BTreeSet::new();
    let mut rust_indirect_calls = BTreeSet::new();
    let mut vendor_unmapped = BTreeSet::new();
    let mut rust_unmapped = BTreeSet::new();
    let mut matched_cases = 0_usize;
    let mut different_cases = 0_usize;
    let mut incomplete_cases = 0_usize;
    let mut case_reports = Vec::with_capacity(scenarios.len());

    for named in scenarios {
        let mut environment = scenario_environment(named);
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
        let vendor_result = execution::execute(
            &vendor_image,
            svd,
            vendor.symbol,
            resolved_scenario(named, &vendor_image, true)?,
        );
        let rust_result = execution::execute(
            &rust_image,
            svd,
            rust.symbol,
            resolved_scenario(named, &rust_image, false)?,
        );
        let (vendor_result, rust_result) = match (vendor_result, rust_result) {
            (Ok(vendor_result), Ok(rust_result)) => (vendor_result, rust_result),
            (vendor_result, rust_result) => {
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

        let events_equal = vendor_result.events == rust_result.events;
        let memory_equal = vendor_result.memory_changes == rust_result.memory_changes;
        let returns_equal =
            !compare_return || vendor_result.return_value == rust_result.return_value;
        if events_equal && memory_equal && returns_equal {
            matched_cases += 1;
            case_reports.push(CaseReport::Match {
                name: named.name.clone(),
                environment,
                events: vendor_result.events.len(),
                memory_changes: vendor_result.memory_changes.len(),
                return_compared: compare_return,
            });
        } else {
            different_cases += 1;
            case_reports.push(CaseReport::Diff {
                name: named.name.clone(),
                environment,
                difference: Box::new(
                    trace_difference(&vendor_result, &rust_result, compare_return)
                        .expect("a different execution outcome has a first difference"),
                ),
            });
        }
    }

    extend_dynamic_inventory(&vendor_image, &mut vendor_inventory, &vendor_indirect_calls)?;
    extend_dynamic_inventory(&rust_image, &mut rust_inventory, &rust_indirect_calls)?;
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
        schema_version: 9,
        command: "execute compare",
        mode: EquivalenceMode::Physical,
        vendor: vendor_report,
        rust: rust_report,
        compare_return,
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
}
