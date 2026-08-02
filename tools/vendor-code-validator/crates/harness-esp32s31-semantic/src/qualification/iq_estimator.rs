//! `phy_iq_est_enable` ROM-to-async-driver qualification.

use std::path::Path;

use sha2::{Digest, Sha256};

use super::*;
use crate::{MmioRegisterMap, artifact_sha256};
use open_radio_vendor_validator_semantic::{
    ContractEffect, ContractValue, EffectComparisonVerdict, EffectPolicy, ReadyCondition,
    RegisterId, StateField, Timeout, compare_effects,
};

const PHY_PARAM_POINTER: u32 = 0x2f07_fc40;
const VENDOR_PHY_PARAM: u32 = 0x3fcd_0000;
const VENDOR_ACTIVITY_COUNTER: u32 = VENDOR_PHY_PARAM + 0x1ac;
const RUST_STATE: u32 = 0x3fce_0000;
const ESTIMATOR_CONFIG: u32 = 0x2010_044c;
const ESTIMATOR_CONTROL: u32 = 0x2010_0450;
const ESTIMATOR_READY: u32 = 0x2010_047c;
const ESTIMATOR_ACTIVITY: u32 = 0x2010_08d0;
const READY_MASK: u32 = 1 << 16;

#[derive(Clone, Debug)]
struct IqCase {
    name: &'static str,
    control: u32,
    config_initial: u32,
    control_initial: u32,
    ready: &'static [u32],
    activity: &'static [u32],
    expected_activity_edges: u16,
}

const CASES: &[IqCase] = &[
    IqCase {
        name: "immediate-ready",
        control: 0,
        config_initial: 0,
        control_initial: 0,
        ready: &[READY_MASK],
        activity: &[],
        expected_activity_edges: 0,
    },
    IqCase {
        name: "inactive-then-ready",
        control: 0x2345,
        config_initial: 0x8102_0304,
        control_initial: 0xa500_0004,
        ready: &[0, READY_MASK],
        activity: &[0],
        expected_activity_edges: 0,
    },
    IqCase {
        name: "active-inactive-ready",
        control: 0x1_ffff,
        config_initial: 0x9020_3040,
        control_initial: 0x5a00_000c,
        ready: &[0, 0, READY_MASK],
        activity: &[1 << 20, 0],
        expected_activity_edges: 1,
    },
];

fn state_field() -> StateField {
    StateField {
        projection: "phy_dc_iq".to_owned(),
        field: "readiness_activity_edges".to_owned(),
        width: 16,
    }
}

fn push_mmio_effect(effects: &mut Vec<ContractEffect>, event: &execution::ExecutionEvent) {
    match event {
        execution::ExecutionEvent::Read {
            width,
            address,
            register: name,
            value,
        } => effects.push(ContractEffect::MmioRead {
            register: RegisterId {
                address: *address,
                width: *width,
                name: name.clone(),
            },
            value: ContractValue::Concrete(*value),
        }),
        execution::ExecutionEvent::Write {
            width,
            address,
            register: name,
            value,
        } => effects.push(ContractEffect::MmioWrite {
            register: RegisterId {
                address: *address,
                width: *width,
                name: name.clone(),
            },
            value: ContractValue::Concrete(*value),
        }),
        execution::ExecutionEvent::DelayMicros(micros) => {
            effects.push(ContractEffect::Delay {
                micros: ContractValue::Concrete(*micros),
            });
        }
        execution::ExecutionEvent::Fence { .. } => unreachable!("IQ enable has no fence"),
    }
}

fn normalized_raw_events(
    events: &[execution::ExecutionEvent],
    rust: bool,
) -> Result<(Vec<execution::ExecutionEvent>, usize)> {
    let mut normalized = Vec::with_capacity(events.len());
    let mut async_samples = 0;
    for (index, event) in events.iter().enumerate() {
        let readiness_schedule = rust
            && matches!(event, execution::ExecutionEvent::DelayMicros(1))
            && matches!(
                events.get(index + 1),
                Some(execution::ExecutionEvent::Read {
                    address: ESTIMATOR_READY,
                    ..
                })
            );
        if readiness_schedule {
            async_samples += 1;
        } else {
            normalized.push(event.clone());
        }
    }
    let ready_reads = normalized
        .iter()
        .filter(|event| {
            matches!(
                event,
                execution::ExecutionEvent::Read {
                    address: ESTIMATOR_READY,
                    ..
                }
            )
        })
        .count();
    if rust && async_samples != ready_reads {
        return Err(format!(
            "compiled Rust IQ adapter scheduled {async_samples} async samples for {ready_reads} ready reads"
        )
        .into());
    }
    Ok((normalized, async_samples))
}

fn contract_effects(
    events: &[execution::ExecutionEvent],
    rust: bool,
) -> Result<Vec<ContractEffect>> {
    let mut effects = vec![ContractEffect::StateWrite {
        field: state_field(),
        value: ContractValue::Concrete(0),
    }];
    let mut activity_edges = 0_u16;
    let mut measurement_enabled = false;
    let mut readiness_delay_pending = false;
    let mut control_writes = 0_u8;

    for event in events {
        match event {
            execution::ExecutionEvent::DelayMicros(1) if rust && measurement_enabled => {
                if readiness_delay_pending {
                    return Err("two Rust readiness delays occurred without a ready sample".into());
                }
                readiness_delay_pending = true;
            }
            execution::ExecutionEvent::DelayMicros(1) if rust => {
                effects.push(ContractEffect::AwaitReady {
                    condition: ReadyCondition::Named("timer-1us".to_owned()),
                    timeout: Timeout::DeadlineMicros(1),
                });
            }
            execution::ExecutionEvent::Read {
                address: ESTIMATOR_READY,
                ..
            } if rust => {
                if !readiness_delay_pending {
                    return Err("compiled Rust ready read has no preceding async schedule".into());
                }
                readiness_delay_pending = false;
                effects.push(ContractEffect::AwaitReady {
                    condition: ReadyCondition::Named("iq-estimator-ready".to_owned()),
                    timeout: Timeout::Attempts(u32::from(
                        open_esp_radio_esp32s31_phy::HARDWARE_EDGE_LIMIT,
                    )),
                });
            }
            execution::ExecutionEvent::Read {
                address: ESTIMATOR_ACTIVITY,
                value,
                ..
            } => {
                push_mmio_effect(&mut effects, event);
                if *value != 0 {
                    effects.push(ContractEffect::StateRead {
                        field: state_field(),
                        value: ContractValue::Concrete(u32::from(activity_edges)),
                    });
                    activity_edges = activity_edges.wrapping_add(1);
                    effects.push(ContractEffect::StateWrite {
                        field: state_field(),
                        value: ContractValue::Concrete(u32::from(activity_edges)),
                    });
                }
            }
            execution::ExecutionEvent::Write {
                address: ESTIMATOR_CONTROL,
                ..
            } => {
                control_writes = control_writes.saturating_add(1);
                push_mmio_effect(&mut effects, event);
                if control_writes == 4 {
                    measurement_enabled = true;
                }
            }
            _ => push_mmio_effect(&mut effects, event),
        }
    }
    if readiness_delay_pending {
        return Err("compiled Rust IQ adapter ended with an unconsumed readiness delay".into());
    }
    Ok(effects)
}

fn final_u16(result: &execution::ExecutionResult, address: u32) -> Result<u16> {
    let byte = |offset: u32| {
        result
            .persistent_memory
            .get(&(address + offset))
            .copied()
            .or_else(|| result.initial_memory.get(&(address + offset)).copied())
            .ok_or_else(|| format!("missing final byte at {:#010x}", address + offset))
    };
    Ok(u16::from_le_bytes([byte(0)?, byte(1)?]))
}

fn validate_vendor_counter_timeline(
    result: &execution::ExecutionResult,
    expected_activity_edges: u16,
) -> Result<()> {
    let writes = result
        .timeline
        .iter()
        .filter_map(|event| match event {
            execution::ExecutionTimelineEvent::RamWrite {
                width: 16,
                address: VENDOR_ACTIVITY_COUNTER,
                value,
            } => Some(*value as u16),
            _ => None,
        })
        .collect::<Vec<_>>();
    let expected = (0..=expected_activity_edges).collect::<Vec<_>>();
    if writes != expected {
        return Err(
            format!("vendor IQ activity counter writes {writes:?}, expected {expected:?}").into(),
        );
    }
    Ok(())
}

fn vendor_scenario(case: &IqCase) -> execution::Scenario {
    let mut scenario = execution::Scenario {
        arguments: vec![0, case.control],
        max_steps: 20_000,
        ..execution::Scenario::default()
    };
    seed_ram_word(&mut scenario, PHY_PARAM_POINTER, VENDOR_PHY_PARAM);
    crate::write_ram_word(&mut scenario, VENDOR_ACTIVITY_COUNTER, u32::from(u16::MAX));
    scenario.persistent_memory.push(execution::MemoryRange {
        start: VENDOR_ACTIVITY_COUNTER,
        length: 2,
    });
    scenario
        .mmio_initial
        .insert(ESTIMATOR_CONFIG, case.config_initial);
    scenario
        .mmio_initial
        .insert(ESTIMATOR_CONTROL, case.control_initial);
    scenario
        .mmio_reads
        .insert(ESTIMATOR_READY, case.ready.iter().copied().collect());
    scenario
        .mmio_reads
        .insert(ESTIMATOR_ACTIVITY, case.activity.iter().copied().collect());
    scenario
}

fn rust_scenario(case: &IqCase) -> execution::Scenario {
    let mut scenario = execution::Scenario {
        arguments: vec![case.control, RUST_STATE, 0],
        max_steps: 20_000,
        ..execution::Scenario::default()
    };
    crate::write_ram_word(&mut scenario, RUST_STATE, u32::MAX);
    scenario.persistent_memory.push(execution::MemoryRange {
        start: RUST_STATE,
        length: 4,
    });
    scenario
        .mmio_initial
        .insert(ESTIMATOR_CONFIG, case.config_initial);
    scenario
        .mmio_initial
        .insert(ESTIMATOR_CONTROL, case.control_initial);
    scenario
        .mmio_reads
        .insert(ESTIMATOR_READY, case.ready.iter().copied().collect());
    scenario
        .mmio_reads
        .insert(ESTIMATOR_ACTIVITY, case.activity.iter().copied().collect());
    scenario
}

fn validate_typed_transition(case: &IqCase) -> Result<()> {
    use open_esp_radio_esp32s31_phy::phy_dc_iq::{
        PhyDcIqAction, PhyDcIqCompletion, PhyDcIqDelayPhase, PhyDcIqEnablePhase,
        PhyDcIqEstimateRequest, PhyDcIqEstimateTransition, PhyDcIqExternalBinding,
        PhyDcIqReadinessSnapshot,
    };

    let request = PhyDcIqEstimateRequest {
        iteration: 0,
        chain: 0,
        control: case.control as u16,
        mode: 0,
    };
    let mut transition = PhyDcIqEstimateTransition::new(request);
    for completion in [
        PhyDcIqCompletion::Configured(request),
        PhyDcIqCompletion::EnableSet {
            request,
            phase: PhyDcIqEnablePhase::Start,
            enabled: true,
        },
        PhyDcIqCompletion::DelayElapsed {
            request,
            phase: PhyDcIqDelayPhase::Start,
            micros: 1,
        },
        PhyDcIqCompletion::EnableSet {
            request,
            phase: PhyDcIqEnablePhase::Measurement,
            enabled: true,
        },
    ] {
        let action = transition.action();
        match action {
            PhyDcIqAction::Configure(_) | PhyDcIqAction::SetEnable { .. } => {
                if !matches!(
                    PhyDcIqExternalBinding::lower(action),
                    Ok(PhyDcIqExternalBinding::Mmio(_))
                ) {
                    return Err(format!("typed IQ action did not lower to MMIO: {action:?}").into());
                }
            }
            PhyDcIqAction::DelayMicros { .. } => {
                if !matches!(
                    PhyDcIqExternalBinding::lower(action),
                    Ok(PhyDcIqExternalBinding::Timer(_))
                ) {
                    return Err(format!("typed IQ delay did not lower to Timer: {action:?}").into());
                }
            }
            _ => return Err(format!("unexpected typed IQ prefix action {action:?}").into()),
        }
        transition
            .advance(completion)
            .map_err(|error| format!("typed IQ prefix rejected completion: {error:?}"))?;
    }

    let mut activity_edges = 0_u16;
    for (sample, ready_value) in case.ready.iter().copied().enumerate() {
        let activity = if ready_value & READY_MASK == 0 {
            case.activity[sample] != 0
        } else {
            false
        };
        let action = transition.action();
        let PhyDcIqAction::AwaitReadinessEdge {
            request: actual_request,
            readiness_activity_edges,
            readiness_samples,
        } = action
        else {
            return Err(
                format!("typed IQ transition did not await sample {sample}: {action:?}").into(),
            );
        };
        if actual_request != request
            || readiness_activity_edges != activity_edges
            || usize::from(readiness_samples) != sample
        {
            return Err(format!(
                "typed IQ readiness identity mismatch at sample {sample}: {action:?}"
            )
            .into());
        }
        if !matches!(
            PhyDcIqExternalBinding::lower(action),
            Ok(PhyDcIqExternalBinding::Readiness(binding))
                if usize::from(binding.samples()) == sample
        ) {
            return Err(format!("typed IQ sample did not lower to Readiness: {action:?}").into());
        }
        transition
            .advance(PhyDcIqCompletion::ReadinessObserved {
                request,
                snapshot: PhyDcIqReadinessSnapshot {
                    ready: ready_value & READY_MASK != 0,
                    activity,
                },
            })
            .map_err(|error| format!("typed IQ readiness rejected completion: {error:?}"))?;
        if activity {
            activity_edges = activity_edges.wrapping_add(1);
        }
    }
    if !matches!(transition.action(), PhyDcIqAction::ReadAccumulators(actual) if actual == request)
    {
        return Err(format!(
            "typed IQ enable boundary did not end at ReadAccumulators: {:?}",
            transition.action()
        )
        .into());
    }
    if activity_edges != case.expected_activity_edges {
        return Err("typed IQ activity count disagrees with the scenario".into());
    }
    Ok(())
}

fn validate_typed_timeout() -> Result<()> {
    use open_esp_radio_esp32s31_phy::{
        HARDWARE_EDGE_LIMIT,
        phy_dc_iq::{
            PhyDcIqAction, PhyDcIqCompletion, PhyDcIqDelayPhase, PhyDcIqEnablePhase,
            PhyDcIqEstimateRequest, PhyDcIqEstimateTransition, PhyDcIqExternalBinding,
            PhyDcIqFailure, PhyDcIqReadinessSnapshot,
        },
    };

    let request = PhyDcIqEstimateRequest {
        iteration: 0,
        chain: 0,
        control: 0x2345,
        mode: 0,
    };
    let mut transition = PhyDcIqEstimateTransition::new(request);
    for completion in [
        PhyDcIqCompletion::Configured(request),
        PhyDcIqCompletion::EnableSet {
            request,
            phase: PhyDcIqEnablePhase::Start,
            enabled: true,
        },
        PhyDcIqCompletion::DelayElapsed {
            request,
            phase: PhyDcIqDelayPhase::Start,
            micros: 1,
        },
        PhyDcIqCompletion::EnableSet {
            request,
            phase: PhyDcIqEnablePhase::Measurement,
            enabled: true,
        },
    ] {
        transition
            .advance(completion)
            .map_err(|error| format!("typed IQ timeout prefix failed: {error:?}"))?;
    }
    for sample in 0..HARDWARE_EDGE_LIMIT {
        let action = transition.action();
        let PhyDcIqAction::AwaitReadinessEdge {
            readiness_samples, ..
        } = action
        else {
            return Err("typed IQ timeout left readiness early".into());
        };
        if readiness_samples != sample {
            return Err("typed IQ timeout sample count is not monotonic".into());
        }
        transition
            .advance(PhyDcIqCompletion::ReadinessObserved {
                request,
                snapshot: PhyDcIqReadinessSnapshot {
                    ready: false,
                    activity: false,
                },
            })
            .map_err(|error| format!("typed IQ timeout sample failed: {error:?}"))?;
    }
    let action = transition.action();
    let Ok(PhyDcIqExternalBinding::Readiness(binding)) = PhyDcIqExternalBinding::lower(action)
    else {
        return Err("typed IQ deadline did not remain a readiness binding".into());
    };
    if binding.samples() != HARDWARE_EDGE_LIMIT {
        return Err("typed IQ readiness binding exposes the wrong deadline count".into());
    }
    transition
        .advance(binding.into_timeout_completion())
        .map_err(|error| format!("typed IQ timeout completion failed: {error:?}"))?;
    for completion in [
        PhyDcIqCompletion::EnableSet {
            request,
            phase: PhyDcIqEnablePhase::Measurement,
            enabled: false,
        },
        PhyDcIqCompletion::DelayElapsed {
            request,
            phase: PhyDcIqDelayPhase::Stop,
            micros: 1,
        },
        PhyDcIqCompletion::EnableSet {
            request,
            phase: PhyDcIqEnablePhase::Start,
            enabled: false,
        },
    ] {
        transition
            .advance(completion)
            .map_err(|error| format!("typed IQ timeout cleanup failed: {error:?}"))?;
    }
    if !matches!(
        transition.action(),
        PhyDcIqAction::Failed(PhyDcIqFailure::ReadinessTimedOut { request: actual, .. })
            if actual == request
    ) {
        return Err("typed IQ timeout did not finish through the complete disable tail".into());
    }
    Ok(())
}

fn generated_reference_identity(
    svd: &MmioRegisterMap,
    vendor_artifact: &Path,
    vendor_companion: Option<&Path>,
) -> Result<String> {
    let companions = vendor_companion
        .into_iter()
        .map(Path::to_path_buf)
        .collect::<Vec<_>>();
    let resolver = crate::ReferenceResolver::load_with_entry_contract(
        vendor_artifact,
        &companions,
        &crate::RISCV_HARNESS,
        entry_contract::PHY_REGISTERED,
    )?;
    let trace = resolver.trace(None, "phy_iq_est_enable", svd)?;
    if !trace.is_reference_eligible() {
        return Err("phy_iq_est_enable is no longer eligible for fail-closed generation".into());
    }
    let program = crate::ResolvedReferenceProgram::try_from(&trace)?;
    let artifact_digest = artifact_sha256(vendor_artifact)?;
    let companion_identities = companions
        .iter()
        .map(|path| {
            Ok((
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("companion")
                    .to_owned(),
                artifact_sha256(path)?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    let generated = crate::codegen::generate(
        &program,
        vendor_artifact
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("vendor-rom.elf"),
        &artifact_digest,
        None,
        &companion_identities,
    )
    .map_err(|error| format!("IQ reference generation failed: {error}"))?;
    Ok(format!(
        "generated-reference phy_iq_est_enable sha256:{:x} exit-a0-modeled={}\n",
        Sha256::digest(generated.source.as_bytes()),
        generated.exit_a0_modeled,
    ))
}

#[allow(
    clippy::too_many_arguments,
    reason = "qualification binds both immutable artifacts, one exact symbol and its policy"
)]
pub fn qualify_esp32s31_iq_est_enable(
    svd: &MmioRegisterMap,
    vendor_artifact: &Path,
    vendor_companion: Option<&Path>,
    rust_artifact: &Path,
    rust_companion: Option<&Path>,
    rust_symbol: &str,
    policy: &EffectPolicy,
    print_oracles: bool,
) -> Result<DriverAdapterQualification> {
    let vendor_digest = artifact_sha256(vendor_artifact)?;
    let rust_digest = crate::artifact_sha256(rust_artifact)?;
    if print_oracles {
        println!(
            "ORACLE\trom\t{}\tsha256={vendor_digest}",
            vendor_artifact.display()
        );
    }
    let mut vendor_image = execution::ExecutableImage::load(vendor_artifact)?;
    if let Some(companion) = vendor_companion {
        vendor_image.add_companion(companion)?;
    }
    let mut rust_image = execution::ExecutableImage::load(rust_artifact)?;
    if let Some(companion) = rust_companion {
        rust_image.add_companion(companion)?;
    }

    let generated_identity = generated_reference_identity(svd, vendor_artifact, vendor_companion)?;
    let vendor_inventory = vendor_image.coverage_inventory("phy_iq_est_enable")?;
    let mut vendor_covered = std::collections::BTreeSet::new();
    let mut matched = true;
    let mut canonical = String::from("driver-adapter esp32s31-iq-est-enable-v1\n");
    canonical.push_str(&format!("rust-artifact-sha256 {rust_digest}\n"));
    canonical.push_str(&generated_identity);

    for case in CASES {
        validate_typed_transition(case)?;
        let vendor_result = execution::execute(
            &vendor_image,
            svd,
            "phy_iq_est_enable",
            vendor_scenario(case),
        )?;
        let rust_result = execution::execute(&rust_image, svd, rust_symbol, rust_scenario(case))?;
        vendor_covered.extend(vendor_result.branches.iter().copied());

        validate_vendor_counter_timeline(&vendor_result, case.expected_activity_edges)?;
        let vendor_counter = final_u16(&vendor_result, VENDOR_ACTIVITY_COUNTER)?;
        let rust_counter = final_u16(&rust_result, RUST_STATE)?;
        let rust_samples = final_u16(&rust_result, RUST_STATE + 2)?;
        let (vendor_raw, _) = normalized_raw_events(&vendor_result.events, false)?;
        let (rust_raw, async_samples) = normalized_raw_events(&rust_result.events, true)?;
        let case_matched = vendor_raw == rust_raw
            && vendor_counter == case.expected_activity_edges
            && rust_counter == case.expected_activity_edges
            && usize::from(rust_samples) == case.ready.len()
            && async_samples == case.ready.len();
        if !case_matched {
            matched = false;
            let divergence = vendor_raw
                .iter()
                .zip(&rust_raw)
                .position(|(vendor, rust)| vendor != rust)
                .unwrap_or_else(|| vendor_raw.len().min(rust_raw.len()));
            println!(
                "IQ-ADAPTER-DIFF\t{}\tindex={divergence}\tvendor={:?}\trust={:?}\tvendor-state={vendor_counter}\trust-state={rust_counter}\trust-samples={rust_samples}",
                case.name,
                vendor_raw.get(divergence),
                rust_raw.get(divergence),
            );
        }

        if case.name == "active-inactive-ready" {
            let vendor_effects = contract_effects(&vendor_result.events, false)?;
            let rust_effects = contract_effects(&rust_result.events, true)?;
            match compare_effects(&vendor_effects, &rust_effects, policy)? {
                EffectComparisonVerdict::Match => {}
                EffectComparisonVerdict::Mismatch(reason) => {
                    matched = false;
                    println!("IQ-EFFECT-DIFF\t{}\t{reason}", case.name);
                }
            }
            canonical.push_str(&format!(
                "effect-scenario {} vendor-effects={} rust-effects={}\n",
                case.name,
                vendor_effects.len(),
                rust_effects.len(),
            ));
        }
        canonical.push_str(&format!(
            "scenario {} control={:#010x} config={:#010x} control-initial={:#010x} ready={:?} activity={:?} activity-edges={} async-samples={}\n",
            case.name,
            case.control,
            case.config_initial,
            case.control_initial,
            case.ready,
            case.activity,
            case.expected_activity_edges,
            async_samples,
        ));
        println!(
            "IQ-ADAPTER-CASE\t{}\t{}\teffects={}\tasync-samples={}\tactivity-edges={}",
            case.name,
            if case_matched { "MATCH" } else { "MISMATCH" },
            vendor_raw.len(),
            async_samples,
            vendor_counter,
        );
    }

    validate_typed_timeout()?;
    let uncovered = vendor_inventory
        .branch_outcomes
        .difference(&vendor_covered)
        .copied()
        .collect::<Vec<_>>();
    if !uncovered.is_empty() {
        matched = false;
        for (site, taken) in &uncovered {
            println!(
                "IQ-ADAPTER-UNCOVERED-BRANCH\t{}\ttaken={taken}",
                vendor_image.location(*site)
            );
        }
    }
    canonical.push_str(&format!(
        "vendor-branch-outcomes={} covered={} timeout-attempts={}\n",
        vendor_inventory.branch_outcomes.len(),
        vendor_covered.len(),
        open_esp_radio_esp32s31_phy::HARDWARE_EDGE_LIMIT,
    ));
    println!(
        "IQ-ADAPTER-SUMMARY\tphy_iq_est_enable\t{}\tscenarios={}\tvendor-branches={}/{}\ttimeout-attempts={}",
        if matched { "MATCH" } else { "MISMATCH" },
        CASES.len(),
        vendor_covered.len(),
        vendor_inventory.branch_outcomes.len(),
        open_esp_radio_esp32s31_phy::HARDWARE_EDGE_LIMIT,
    );
    Ok(DriverAdapterQualification { matched, canonical })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_transition_covers_all_reviewed_readiness_scenarios_and_timeout() {
        for case in CASES {
            validate_typed_transition(case).unwrap();
        }
        validate_typed_timeout().unwrap();
    }

    #[test]
    fn async_normalizer_requires_one_schedule_per_ready_read() {
        let events = [
            execution::ExecutionEvent::DelayMicros(1),
            execution::ExecutionEvent::Read {
                width: 32,
                address: ESTIMATOR_READY,
                register: "READY".to_owned(),
                value: READY_MASK,
            },
        ];
        let (normalized, samples) = normalized_raw_events(&events, true).unwrap();
        assert_eq!(samples, 1);
        assert!(matches!(
            normalized.as_slice(),
            [execution::ExecutionEvent::Read { .. }]
        ));
        assert!(normalized_raw_events(&events[1..], true).is_err());
    }
}
