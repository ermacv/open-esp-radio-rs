//! ESP32-S31 semantic verification command implementations.

use std::{collections::BTreeSet, path::Path};

use super::{
    QualificationArtifact, QualificationCase, QualificationDifference, QualificationReport,
    QualificationSummary, StateFootprintStats,
};
use crate::*;
use open_radio_vendor_semantics::{EquivalenceMode, EquivalenceVerdict};

fn debug_events(events: &[impl std::fmt::Debug]) -> Vec<String> {
    events.iter().map(|event| format!("{event:?}")).collect()
}

fn matched_case(
    name: impl Into<String>,
    result: &execution::ExecutionResult,
    events: usize,
    footprint: StateFootprintStats,
) -> QualificationCase {
    QualificationCase {
        name: name.into(),
        verdict: EquivalenceVerdict::Match,
        events: Some(events),
        steps: Some(result.steps),
        branch_outcomes: Some(result.branches.len()),
        branch_events: Some(result.ordered_branches.len()),
        calls: Some(result.calls.len()),
        call_events: Some(result.ordered_calls.len()),
        state: Some(footprint.into()),
        difference: None,
    }
}

fn artifact_report(role: &'static str, path: &Path, sha256: String) -> QualificationArtifact {
    QualificationArtifact {
        role,
        path: path.to_owned(),
        sha256,
    }
}

fn single_case_report(
    contract: &'static str,
    vendor_symbol: &'static str,
    artifacts: Vec<QualificationArtifact>,
    result: &execution::ExecutionResult,
    case: QualificationCase,
) -> QualificationReport {
    let matched = case.verdict == EquivalenceVerdict::Match;
    let mismatched = usize::from(case.verdict == EquivalenceVerdict::Diff);
    let incomplete = usize::from(case.verdict == EquivalenceVerdict::Incomplete);
    QualificationReport {
        schema: 2,
        mode: EquivalenceMode::Semantic,
        contract,
        vendor_symbol,
        verdict: case.verdict,
        matched,
        artifacts,
        summary: QualificationSummary {
            scenarios: 1,
            matched: usize::from(matched),
            mismatched,
            incomplete,
            failed: usize::from(!matched),
            steps: result.steps,
            branch_outcomes: result.branches.len(),
            calls: result.calls.len(),
        },
        cases: vec![case],
    }
}

pub fn verify_esp32s31_channel(
    svd: &MmioMap,
    vendor_artifact: &Path,
    vendor_companion: &Path,
) -> Result<QualificationReport> {
    let artifact_digest = artifact_sha256(vendor_artifact)?;
    let companion_digest = artifact_sha256(vendor_companion)?;
    let artifacts = vec![
        QualificationArtifact {
            role: "archive",
            path: vendor_artifact.to_owned(),
            sha256: artifact_digest,
        },
        QualificationArtifact {
            role: "rom-companion",
            path: vendor_companion.to_owned(),
            sha256: companion_digest,
        },
    ];

    let mut image = execution::ExecutableImage::load(vendor_artifact)?;
    image.add_companion(vendor_companion)?;
    let phy_param = image
        .symbol_address("phy_param")
        .ok_or("vendor artifact has no phy_param symbol")?;
    let phy_functions_pointer = image
        .symbol_address("g_phyFuns")
        .ok_or("vendor artifact has no g_phyFuns symbol")?;

    let mut cases = Vec::new();
    for channel in 1_u16..=13 {
        cases.push((format!("channel-{channel}-cbw-0"), channel, 0_u8));
        cases.push((format!("channel-{channel}-cbw-1"), channel, 1_u8));
        cases.push((
            format!("frequency-{}-cbw-0", 2_407 + channel * 5),
            2_407 + channel * 5,
            0_u8,
        ));
    }
    for frequency in [2_413_u16, 2_439, 2_476] {
        for cbw in [0_u8, 1] {
            cases.push((
                format!("off-grid-frequency-{frequency}-cbw-{cbw}"),
                frequency,
                cbw,
            ));
        }
    }
    // A reproducible generated tail exercises state carry-over under a less
    // regular sequence than the reviewed edge matrix. Keep the seed and LCG
    // fixed so a failure is directly replayable from its printed case name.
    let mut generated = 0x6d2b_79f5_u32;
    for index in 0..32 {
        generated = generated
            .wrapping_mul(1_664_525)
            .wrapping_add(1_013_904_223);
        let frequency = 2_412 + ((generated >> 8) % 65) as u16;
        let cbw = (generated >> 31) as u8;
        cases.push((
            format!("generated-{index:02}-seed-{generated:08x}-frequency-{frequency}-cbw-{cbw}"),
            frequency,
            cbw,
        ));
    }

    let mut all_branches = BTreeSet::new();
    let mut all_calls = BTreeSet::new();
    let mut total_steps = 0_u64;
    let mut passed = 0_usize;
    let mut reported_full_diff = false;
    let total = cases.len();
    let mut case_reports = Vec::with_capacity(total);
    let mut vendor_session = execution::ExecutionSession::default();
    let mut rust_state = open_esp_radio_esp32s31_phy::PhyState::default();
    for (case_index, (name, channel_or_frequency, cbw)) in cases.into_iter().enumerate() {
        let mut scenario = verification::vendor_channel_scenario(
            channel_or_frequency,
            cbw,
            phy_param,
            phy_functions_pointer,
        )?;
        scenario.reset_policy = if case_index == 0 {
            execution::ResetPolicy::ColdBoot
        } else {
            execution::ResetPolicy::Continue
        };
        let result = vendor_session.execute(&image, svd, "phy_chip_set_chan", scenario)?;
        let footprint = verification::vendor_channel_state_footprint(&result, phy_param)?;
        let vendor_events = verification::normalize_vendor_channel(
            &image,
            &result,
            phy_param,
            channel_or_frequency,
        )?;
        let (rust_events, next_state) =
            verification::rust_channel_events_with_state(rust_state, channel_or_frequency, cbw)?;
        rust_state = next_state;
        if vendor_events != rust_events {
            let divergence = vendor_events
                .iter()
                .zip(&rust_events)
                .position(|(vendor, rust)| vendor != rust)
                .unwrap_or_else(|| vendor_events.len().min(rust_events.len()));
            let retain_full_diff = !reported_full_diff;
            reported_full_diff = true;
            case_reports.push(QualificationCase {
                name,
                verdict: EquivalenceVerdict::Diff,
                events: Some(vendor_events.len()),
                steps: Some(result.steps),
                branch_outcomes: Some(result.branches.len()),
                branch_events: Some(result.ordered_branches.len()),
                calls: Some(result.calls.len()),
                call_events: Some(result.ordered_calls.len()),
                state: Some(footprint.into()),
                difference: Some(QualificationDifference {
                    index: Some(divergence),
                    vendor: vendor_events
                        .get(divergence)
                        .map(|event| format!("{event:?}")),
                    rust: rust_events
                        .get(divergence)
                        .map(|event| format!("{event:?}")),
                    reason: None,
                    vendor_events: if retain_full_diff {
                        debug_events(&vendor_events)
                    } else {
                        Vec::new()
                    },
                    rust_events: if retain_full_diff {
                        debug_events(&rust_events)
                    } else {
                        Vec::new()
                    },
                }),
            });
            continue;
        }

        passed += 1;
        total_steps = total_steps.saturating_add(result.steps);
        all_branches.extend(result.branches.iter().copied());
        all_calls.extend(result.calls.iter().cloned());
        case_reports.push(matched_case(name, &result, vendor_events.len(), footprint));
    }
    let matched = passed == total;
    let incomplete = case_reports
        .iter()
        .filter(|case| case.verdict == EquivalenceVerdict::Incomplete)
        .count();
    let mismatched = total - passed - incomplete;
    Ok(QualificationReport {
        schema: 2,
        mode: EquivalenceMode::Semantic,
        contract: "esp32s31-channel",
        vendor_symbol: "phy_chip_set_chan",
        verdict: if matched {
            EquivalenceVerdict::Match
        } else if incomplete != 0 {
            EquivalenceVerdict::Incomplete
        } else {
            EquivalenceVerdict::Diff
        },
        matched,
        artifacts,
        cases: case_reports,
        summary: QualificationSummary {
            scenarios: total,
            matched: passed,
            mismatched,
            incomplete,
            failed: total - passed,
            steps: total_steps,
            branch_outcomes: all_branches.len(),
            calls: all_calls.len(),
        },
    })
}

pub fn verify_esp32s31_rf_init(
    svd: &MmioMap,
    vendor_artifact: &Path,
    vendor_companion: &Path,
) -> Result<QualificationReport> {
    let artifact_digest = artifact_sha256(vendor_artifact)?;
    let companion_digest = artifact_sha256(vendor_companion)?;
    let artifacts = vec![
        QualificationArtifact {
            role: "archive",
            path: vendor_artifact.to_owned(),
            sha256: artifact_digest,
        },
        QualificationArtifact {
            role: "rom-companion",
            path: vendor_companion.to_owned(),
            sha256: companion_digest,
        },
    ];

    let mut image = execution::ExecutableImage::load(vendor_artifact)?;
    image.add_companion(vendor_companion)?;
    let phy_param = image
        .symbol_address("phy_param")
        .ok_or("vendor artifact has no phy_param symbol")?;
    let phy_functions_pointer = image
        .symbol_address("g_phyFuns")
        .ok_or("vendor artifact has no g_phyFuns symbol")?;

    let mut vendor_session = execution::ExecutionSession::default();
    let mut rust_state = open_esp_radio_esp32s31_phy::PhyState::default();
    let mut passed = 0_usize;
    let mut total_steps = 0_u64;
    let mut all_branches = BTreeSet::new();
    let mut all_calls = BTreeSet::new();
    let cases = ["cold-image", "retained-state"];
    let mut case_reports = Vec::with_capacity(cases.len());
    for (case_index, name) in cases.into_iter().enumerate() {
        let mut scenario = verification::vendor_rf_init_scenario(phy_param, phy_functions_pointer);
        scenario.reset_policy = if case_index == 0 {
            execution::ResetPolicy::ColdBoot
        } else {
            execution::ResetPolicy::Continue
        };
        let result = vendor_session.execute(&image, svd, "phy_rf_init", scenario)?;
        let footprint = verification::vendor_rf_init_state_footprint(&result, phy_param)?;
        let vendor_events = verification::normalize_vendor_rf_init(&image, &result, phy_param)?;
        let (rust_events, next_state) = verification::rust_rf_init_events(rust_state)?;
        rust_state = next_state;
        if vendor_events != rust_events {
            let divergence = vendor_events
                .iter()
                .zip(&rust_events)
                .position(|(vendor, rust)| vendor != rust)
                .unwrap_or_else(|| vendor_events.len().min(rust_events.len()));
            case_reports.push(QualificationCase {
                name: name.to_owned(),
                verdict: EquivalenceVerdict::Diff,
                events: Some(vendor_events.len()),
                steps: Some(result.steps),
                branch_outcomes: Some(result.branches.len()),
                branch_events: Some(result.ordered_branches.len()),
                calls: Some(result.calls.len()),
                call_events: Some(result.ordered_calls.len()),
                state: Some(footprint.into()),
                difference: Some(QualificationDifference {
                    index: Some(divergence),
                    vendor: vendor_events
                        .get(divergence)
                        .map(|event| format!("{event:?}")),
                    rust: rust_events
                        .get(divergence)
                        .map(|event| format!("{event:?}")),
                    reason: None,
                    vendor_events: debug_events(&vendor_events),
                    rust_events: debug_events(&rust_events),
                }),
            });
            continue;
        }

        let retained_rc = vendor_session
            .byte(&image, phy_param + 0xa6)
            .ok_or("persistent vendor session lost phy_param RC state")?
            & 0x80
            != 0;
        if retained_rc != rust_state.rc_calibration_complete() {
            case_reports.push(QualificationCase {
                name: name.to_owned(),
                verdict: EquivalenceVerdict::Diff,
                events: Some(vendor_events.len()),
                steps: Some(result.steps),
                branch_outcomes: Some(result.branches.len()),
                branch_events: Some(result.ordered_branches.len()),
                calls: Some(result.calls.len()),
                call_events: Some(result.ordered_calls.len()),
                state: Some(footprint.into()),
                difference: Some(QualificationDifference {
                    reason: Some(format!(
                        "persistent RC state differs: vendor={retained_rc}, rust={}",
                        rust_state.rc_calibration_complete()
                    )),
                    ..Default::default()
                }),
            });
            continue;
        }

        passed += 1;
        total_steps = total_steps.saturating_add(result.steps);
        all_branches.extend(result.branches.iter().copied());
        all_calls.extend(result.calls.iter().cloned());
        case_reports.push(matched_case(name, &result, vendor_events.len(), footprint));
    }
    let matched = passed == cases.len();
    let incomplete = case_reports
        .iter()
        .filter(|case| case.verdict == EquivalenceVerdict::Incomplete)
        .count();
    let mismatched = cases.len() - passed - incomplete;
    Ok(QualificationReport {
        schema: 2,
        mode: EquivalenceMode::Semantic,
        contract: "esp32s31-rf-init",
        vendor_symbol: "phy_rf_init",
        verdict: if matched {
            EquivalenceVerdict::Match
        } else if incomplete != 0 {
            EquivalenceVerdict::Incomplete
        } else {
            EquivalenceVerdict::Diff
        },
        matched,
        artifacts,
        cases: case_reports,
        summary: QualificationSummary {
            scenarios: cases.len(),
            matched: passed,
            mismatched,
            incomplete,
            failed: cases.len() - passed,
            steps: total_steps,
            branch_outcomes: all_branches.len(),
            calls: all_calls.len(),
        },
    })
}

pub fn verify_esp32s31_bluetooth_txdc(
    svd: &MmioMap,
    vendor_artifact: &Path,
    vendor_companion: &Path,
) -> Result<QualificationReport> {
    let artifact_digest = artifact_sha256(vendor_artifact)?;
    let companion_digest = artifact_sha256(vendor_companion)?;
    let artifacts = vec![
        artifact_report("archive", vendor_artifact, artifact_digest),
        artifact_report("rom-companion", vendor_companion, companion_digest),
    ];

    let mut image = execution::ExecutableImage::load(vendor_artifact)?;
    image.add_companion(vendor_companion)?;
    let phy_param = image
        .symbol_address("phy_param")
        .ok_or("vendor artifact has no phy_param symbol")?;
    let phy_functions_pointer = image
        .symbol_address("g_phyFuns")
        .ok_or("vendor artifact has no g_phyFuns symbol")?;
    let scenario = verification::vendor_bluetooth_txdc_scenario(phy_param, phy_functions_pointer);
    let result = execution::execute(&image, svd, "phy_bt_txdc_cal_new", scenario)?;
    let vendor_events = verification::normalize_vendor_bluetooth_txdc(&image, &result, phy_param)?;
    let (rust_events, _) =
        verification::rust_bluetooth_txdc_events(open_esp_radio_esp32s31_phy::PhyState::default())?;
    let matched = vendor_events == rust_events;
    let difference = if matched {
        None
    } else {
        let divergence = vendor_events
            .iter()
            .zip(&rust_events)
            .position(|(vendor, rust)| vendor != rust)
            .unwrap_or_else(|| vendor_events.len().min(rust_events.len()));
        Some(QualificationDifference {
            index: Some(divergence),
            vendor: vendor_events
                .get(divergence)
                .map(|event| format!("{event:?}")),
            rust: rust_events
                .get(divergence)
                .map(|event| format!("{event:?}")),
            reason: None,
            vendor_events: debug_events(&vendor_events),
            rust_events: debug_events(&rust_events),
        })
    };
    let case = QualificationCase {
        name: "bluetooth-txdc".to_owned(),
        verdict: if matched {
            EquivalenceVerdict::Match
        } else {
            EquivalenceVerdict::Diff
        },
        events: Some(vendor_events.len()),
        steps: Some(result.steps),
        branch_outcomes: Some(result.branches.len()),
        branch_events: Some(result.ordered_branches.len()),
        calls: Some(result.calls.len()),
        call_events: Some(result.ordered_calls.len()),
        state: None,
        difference,
    };
    Ok(single_case_report(
        "esp32s31-bluetooth-txdc",
        "phy_bt_txdc_cal_new",
        artifacts,
        &result,
        case,
    ))
}

pub fn verify_esp32s31_bluetooth_tx_power(
    svd: &MmioMap,
    vendor_artifact: &Path,
    vendor_companion: &Path,
) -> Result<QualificationReport> {
    let artifact_digest = artifact_sha256(vendor_artifact)?;
    let companion_digest = artifact_sha256(vendor_companion)?;
    let artifacts = vec![
        artifact_report("archive", vendor_artifact, artifact_digest),
        artifact_report("rom-companion", vendor_companion, companion_digest),
    ];

    let mut image = execution::ExecutableImage::load(vendor_artifact)?;
    image.add_companion(vendor_companion)?;
    let phy_param = image
        .symbol_address("phy_param")
        .ok_or("vendor artifact has no phy_param symbol")?;
    let phy_functions_pointer = image
        .symbol_address("g_phyFuns")
        .ok_or("vendor artifact has no g_phyFuns symbol")?;
    let scenario =
        verification::vendor_bluetooth_tx_power_scenario(phy_param, phy_functions_pointer);
    let result = execution::execute(&image, svd, "phy_bt_tx_pwctrl_init", scenario)?;
    let footprint = verification::vendor_bluetooth_tx_power_state_footprint(&result, phy_param)?;
    let vendor_events =
        verification::normalize_vendor_bluetooth_tx_power(&image, &result, phy_param)?;
    let (rust_events, _) = verification::rust_bluetooth_tx_power_events(
        open_esp_radio_esp32s31_phy::PhyState::default(),
    )?;
    let matched = vendor_events == rust_events;
    let difference = if matched {
        None
    } else {
        let divergence = vendor_events
            .iter()
            .zip(&rust_events)
            .position(|(vendor, rust)| vendor != rust)
            .unwrap_or_else(|| vendor_events.len().min(rust_events.len()));
        Some(QualificationDifference {
            index: Some(divergence),
            vendor: vendor_events
                .get(divergence)
                .map(|event| format!("{event:?}")),
            rust: rust_events
                .get(divergence)
                .map(|event| format!("{event:?}")),
            reason: None,
            vendor_events: debug_events(&vendor_events),
            rust_events: debug_events(&rust_events),
        })
    };
    let case = QualificationCase {
        name: "bluetooth-tx-power".to_owned(),
        verdict: if matched {
            EquivalenceVerdict::Match
        } else {
            EquivalenceVerdict::Diff
        },
        events: Some(vendor_events.len()),
        steps: Some(result.steps),
        branch_outcomes: Some(result.branches.len()),
        branch_events: Some(result.ordered_branches.len()),
        calls: Some(result.calls.len()),
        call_events: Some(result.ordered_calls.len()),
        state: Some(footprint.into()),
        difference,
    };
    Ok(single_case_report(
        "esp32s31-bluetooth-tx-power",
        "phy_bt_tx_pwctrl_init",
        artifacts,
        &result,
        case,
    ))
}

pub fn verify_esp32s31_bluetooth_txdc_pwdet(
    svd: &MmioMap,
    vendor_artifact: &Path,
    vendor_companion: &Path,
) -> Result<QualificationReport> {
    let artifact_digest = artifact_sha256(vendor_artifact)?;
    let companion_digest = artifact_sha256(vendor_companion)?;
    let artifacts = vec![
        artifact_report("archive", vendor_artifact, artifact_digest),
        artifact_report("rom-companion", vendor_companion, companion_digest),
    ];

    let mut image = execution::ExecutableImage::load(vendor_artifact)?;
    image.add_companion(vendor_companion)?;
    let phy_param = image
        .symbol_address("phy_param")
        .ok_or("vendor artifact has no phy_param symbol")?;
    let phy_functions_pointer = image
        .symbol_address("g_phyFuns")
        .ok_or("vendor artifact has no g_phyFuns symbol")?;
    let scenario =
        verification::vendor_bluetooth_txdc_pwdet_scenario(phy_param, phy_functions_pointer);
    let result = execution::execute(&image, svd, "phy_txdc_cal_pwdet_init", scenario)?;
    let footprint = verification::vendor_bluetooth_txdc_pwdet_state_footprint(&result, phy_param)?;
    let vendor_events =
        verification::normalize_vendor_bluetooth_txdc_pwdet(&image, &result, phy_param)?;
    let (rust_events, _) = verification::rust_bluetooth_txdc_pwdet_events(
        open_esp_radio_esp32s31_phy::PhyState::default(),
    )?;
    let matched = vendor_events == rust_events;
    let difference = if matched {
        None
    } else {
        let divergence = vendor_events
            .iter()
            .zip(&rust_events)
            .position(|(vendor, rust)| vendor != rust)
            .unwrap_or_else(|| vendor_events.len().min(rust_events.len()));
        let window_start = divergence.saturating_sub(8);
        let window_end = divergence
            .saturating_add(9)
            .max(window_start)
            .min(vendor_events.len().max(rust_events.len()));
        Some(QualificationDifference {
            index: Some(divergence),
            vendor: vendor_events
                .get(divergence)
                .map(|event| format!("{event:?}")),
            rust: rust_events
                .get(divergence)
                .map(|event| format!("{event:?}")),
            reason: None,
            vendor_events: debug_events(
                &vendor_events[window_start..vendor_events.len().min(window_end)],
            ),
            rust_events: debug_events(
                &rust_events[window_start..rust_events.len().min(window_end)],
            ),
        })
    };
    let case = QualificationCase {
        name: "bluetooth-txdc-pwdet".to_owned(),
        verdict: if matched {
            EquivalenceVerdict::Match
        } else {
            EquivalenceVerdict::Diff
        },
        events: Some(vendor_events.len()),
        steps: Some(result.steps),
        branch_outcomes: Some(result.branches.len()),
        branch_events: Some(result.ordered_branches.len()),
        calls: Some(result.calls.len()),
        call_events: Some(result.ordered_calls.len()),
        state: Some(footprint.into()),
        difference,
    };
    Ok(single_case_report(
        "esp32s31-bluetooth-txdc-pwdet",
        "phy_txdc_cal_pwdet_init",
        artifacts,
        &result,
        case,
    ))
}
