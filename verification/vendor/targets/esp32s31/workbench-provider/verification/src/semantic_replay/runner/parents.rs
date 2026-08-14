//! Hierarchical semantic-contract runners for composed PHY parents.

use super::*;

pub fn verify_esp32s31_bluetooth_tx_gain_init(
    svd: &MmioMap,
    vendor_artifact: &Path,
    vendor_companion: &Path,
) -> Result<SemanticVerificationReport> {
    let artifacts = vec![
        artifact_report(
            "archive",
            vendor_artifact,
            artifact_sha256(vendor_artifact)?,
        ),
        artifact_report(
            "rom-companion",
            vendor_companion,
            artifact_sha256(vendor_companion)?,
        ),
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
    let mut case_reports = Vec::new();
    let mut all_branches = BTreeSet::new();
    let mut all_calls = BTreeSet::new();
    let mut total_steps = 0_u64;
    let mut passed = 0_usize;
    for (case_index, name) in ["cold-image", "retained-state"].into_iter().enumerate() {
        let mut scenario =
            verification::vendor_bluetooth_tx_gain_init_scenario(phy_param, phy_functions_pointer);
        scenario.reset_policy = if case_index == 0 {
            execution::ResetPolicy::ColdBoot
        } else {
            execution::ResetPolicy::Continue
        };
        let result = vendor_session.execute(&image, svd, "phy_bt_tx_gain_init", scenario)?;
        let footprint =
            verification::vendor_bluetooth_tx_gain_init_state_footprint(&result, phy_param)?;
        let vendor_events = verification::normalize_vendor_bluetooth_tx_gain_init(
            vendor_artifact,
            &image,
            &result,
            phy_param,
        )?;
        let (rust_events, next_state) =
            verification::rust_bluetooth_tx_gain_init_events(rust_state)?;
        rust_state = next_state;
        let matched = vendor_events == rust_events;
        let difference = if matched {
            passed += 1;
            total_steps = total_steps.saturating_add(result.steps);
            all_branches.extend(result.branches.iter().copied());
            all_calls.extend(result.calls.iter().cloned());
            None
        } else {
            let divergence = vendor_events
                .iter()
                .zip(&rust_events)
                .position(|(vendor, rust)| vendor != rust)
                .unwrap_or_else(|| vendor_events.len().min(rust_events.len()));
            Some(SemanticVerificationDifference {
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
        case_reports.push(SemanticVerificationCase {
            name: name.to_owned(),
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
        });
    }
    let scenarios = case_reports.len();
    let matched = passed == scenarios;
    Ok(SemanticVerificationReport {
        schema: 2,
        mode: EquivalenceMode::Semantic,
        contract: "esp32s31-bluetooth-tx-gain-init",
        vendor_symbol: "phy_bt_tx_gain_init",
        verdict: if matched {
            EquivalenceVerdict::Match
        } else {
            EquivalenceVerdict::Diff
        },
        matched,
        artifacts,
        cases: case_reports,
        summary: SemanticVerificationSummary {
            scenarios,
            matched: passed,
            mismatched: scenarios - passed,
            incomplete: 0,
            failed: scenarios - passed,
            steps: total_steps,
            branch_outcomes: all_branches.len(),
            calls: all_calls.len(),
        },
    })
}

pub fn verify_esp32s31_baseband_init(
    svd: &MmioMap,
    vendor_artifact: &Path,
    vendor_companion: &Path,
) -> Result<SemanticVerificationReport> {
    let artifacts = vec![
        artifact_report(
            "archive",
            vendor_artifact,
            artifact_sha256(vendor_artifact)?,
        ),
        artifact_report(
            "rom-companion",
            vendor_companion,
            artifact_sha256(vendor_companion)?,
        ),
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
    let mut case_reports = Vec::new();
    let mut all_branches = BTreeSet::new();
    let mut all_calls = BTreeSet::new();
    let mut total_steps = 0_u64;
    let mut passed = 0_usize;
    for (case_index, name) in ["cold-image", "retained-state"].into_iter().enumerate() {
        let mut scenario =
            verification::vendor_baseband_init_scenario(phy_param, phy_functions_pointer);
        scenario.reset_policy = if case_index == 0 {
            execution::ResetPolicy::ColdBoot
        } else {
            execution::ResetPolicy::Continue
        };
        let result = vendor_session.execute(&image, svd, "phy_bb_init", scenario)?;
        let footprint = verification::vendor_baseband_init_state_footprint(&result, phy_param)?;
        let vendor_events = verification::normalize_vendor_baseband_init(vendor_artifact, &result)?;
        let (rust_events, next_state) = verification::rust_baseband_init_events(rust_state, 11)?;
        rust_state = next_state;
        let matched = vendor_events == rust_events;
        let difference = if matched {
            passed += 1;
            total_steps = total_steps.saturating_add(result.steps);
            all_branches.extend(result.branches.iter().copied());
            all_calls.extend(result.calls.iter().cloned());
            None
        } else {
            let divergence = vendor_events
                .iter()
                .zip(&rust_events)
                .position(|(vendor, rust)| vendor != rust)
                .unwrap_or_else(|| vendor_events.len().min(rust_events.len()));
            Some(SemanticVerificationDifference {
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
        case_reports.push(SemanticVerificationCase {
            name: name.to_owned(),
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
        });
    }
    let scenarios = case_reports.len();
    let matched = passed == scenarios;
    Ok(SemanticVerificationReport {
        schema: 2,
        mode: EquivalenceMode::Semantic,
        contract: "esp32s31-baseband-init",
        vendor_symbol: "phy_bb_init",
        verdict: if matched {
            EquivalenceVerdict::Match
        } else {
            EquivalenceVerdict::Diff
        },
        matched,
        artifacts,
        cases: case_reports,
        summary: SemanticVerificationSummary {
            scenarios,
            matched: passed,
            mismatched: scenarios - passed,
            incomplete: 0,
            failed: scenarios - passed,
            steps: total_steps,
            branch_outcomes: all_branches.len(),
            calls: all_calls.len(),
        },
    })
}

pub fn verify_esp32s31_register_init(
    svd: &MmioMap,
    vendor_artifact: &Path,
    vendor_companion: &Path,
) -> Result<SemanticVerificationReport> {
    let artifacts = vec![
        artifact_report(
            "archive",
            vendor_artifact,
            artifact_sha256(vendor_artifact)?,
        ),
        artifact_report(
            "rom-companion",
            vendor_companion,
            artifact_sha256(vendor_companion)?,
        ),
    ];
    let mut image = execution::ExecutableImage::load(vendor_artifact)?;
    image.add_companion(vendor_companion)?;
    let phy_param = image
        .symbol_address("phy_param")
        .ok_or("vendor artifact has no phy_param symbol")?;
    let phy_functions_pointer = image
        .symbol_address("g_phyFuns")
        .ok_or("vendor artifact has no g_phyFuns symbol")?;
    let scenario = verification::vendor_register_init_scenario(phy_param, phy_functions_pointer);
    let result = execution::execute(&image, svd, "register_chipv7_phy", scenario)?;
    let footprint = verification::vendor_register_init_state_footprint(&result, phy_param)?;
    let vendor_events = verification::normalize_vendor_register_init(vendor_artifact, &result)?;
    let rust_events = verification::rust_register_init_events()?;
    let matched = vendor_events == rust_events;
    let difference = if matched {
        None
    } else {
        let divergence = vendor_events
            .iter()
            .zip(&rust_events)
            .position(|(vendor, rust)| vendor != rust)
            .unwrap_or_else(|| vendor_events.len().min(rust_events.len()));
        Some(SemanticVerificationDifference {
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
    let case = SemanticVerificationCase {
        name: "cold-full-calibration".to_owned(),
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
        "esp32s31-register-init",
        "register_chipv7_phy",
        artifacts,
        &result,
        case,
    ))
}
