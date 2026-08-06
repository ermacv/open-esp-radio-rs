//! ESP32-S31 semantic verification command implementations.

use std::{collections::BTreeSet, path::Path};

use crate::*;

pub fn verify_esp32s31_channel(
    svd: &MmioRegisterMap,
    vendor_artifact: &Path,
    vendor_companion: &Path,
    print_oracles: bool,
) -> Result<bool> {
    let artifact_digest = artifact_sha256(vendor_artifact)?;
    let companion_digest = artifact_sha256(vendor_companion)?;
    if print_oracles {
        println!(
            "ORACLE\tarchive\t{}\tsha256={artifact_digest}",
            vendor_artifact.display()
        );
        println!(
            "ORACLE\trom-companion\t{}\tsha256={companion_digest}",
            vendor_companion.display()
        );
    }

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
    let mut vendor_session = execution::ExecutionSession::default();
    let mut rust_state = open_esp_radio_esp32s31_phy::phy_cold::PhyColdState::new();
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
        let unmapped: BTreeSet<_> = result
            .events
            .iter()
            .filter_map(unmapped_execution_address)
            .collect();
        if !unmapped.is_empty() {
            for address in &unmapped {
                println!("SEMANTIC-UNCOVERED\t{name}\tunmapped-mmio\t{address:#010x}");
            }
            println!(
                "VERIFICATION-CASE\t{name}\tINCOMPLETE\tunmapped-mmio={}",
                unmapped.len()
            );
            continue;
        }
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
            println!(
                "SEMANTIC-DIFF\t{name}\tindex={divergence}\tvendor={:?}\trust={:?}",
                vendor_events.get(divergence),
                rust_events.get(divergence),
            );
            if !reported_full_diff {
                for (index, event) in vendor_events.iter().enumerate() {
                    println!("SEMANTIC-EVENT\t{name}\tvendor\t{index}\t{event:?}");
                }
                for (index, event) in rust_events.iter().enumerate() {
                    println!("SEMANTIC-EVENT\t{name}\trust\t{index}\t{event:?}");
                }
                reported_full_diff = true;
            }
            println!(
                "VERIFICATION-CASE\t{name}\tMISMATCH\tvendor-events={}\trust-events={}",
                vendor_events.len(),
                rust_events.len(),
            );
            continue;
        }

        passed += 1;
        total_steps = total_steps.saturating_add(result.steps);
        all_branches.extend(result.branches.iter().copied());
        all_calls.extend(result.calls.iter().cloned());
        println!(
            "VERIFICATION-CASE\t{name}\tSTATE-SCENARIO-MATCH\tevents={}\tsteps={}\tbranch-outcomes={}\tbranch-events={}\tcalls={}\tcall-events={}\tstate-read-bytes={}\tstate-written-bytes={}\tstate-ranges={}",
            vendor_events.len(),
            result.steps,
            result.branches.len(),
            result.ordered_branches.len(),
            result.calls.len(),
            result.ordered_calls.len(),
            footprint.read_bytes,
            footprint.written_bytes,
            footprint.classified_ranges,
        );
    }
    let verdict = if passed == total {
        "STATE-SCENARIO-MATCH"
    } else {
        "FAIL"
    };
    println!(
        "VERIFICATION-SUMMARY\tphy_chip_set_chan\t{verdict}\tscenarios={total}\tmatched={passed}\tfailed={}\tsteps={total_steps}\tbranch-outcomes={}\tcalls={}",
        total - passed,
        all_branches.len(),
        all_calls.len(),
    );
    Ok(passed == total)
}

pub fn verify_esp32s31_rf_init(
    svd: &MmioRegisterMap,
    vendor_artifact: &Path,
    vendor_companion: &Path,
    print_oracles: bool,
) -> Result<bool> {
    let artifact_digest = artifact_sha256(vendor_artifact)?;
    let companion_digest = artifact_sha256(vendor_companion)?;
    if print_oracles {
        println!(
            "ORACLE\tarchive\t{}\tsha256={artifact_digest}",
            vendor_artifact.display()
        );
        println!(
            "ORACLE\trom-companion\t{}\tsha256={companion_digest}",
            vendor_companion.display()
        );
    }

    let mut image = execution::ExecutableImage::load(vendor_artifact)?;
    image.add_companion(vendor_companion)?;
    let phy_param = image
        .symbol_address("phy_param")
        .ok_or("vendor artifact has no phy_param symbol")?;
    let phy_functions_pointer = image
        .symbol_address("g_phyFuns")
        .ok_or("vendor artifact has no g_phyFuns symbol")?;

    let mut vendor_session = execution::ExecutionSession::default();
    let mut rust_state = open_esp_radio_esp32s31_phy::phy_cold::PhyColdState::new();
    let mut passed = 0_usize;
    let mut total_steps = 0_u64;
    let mut all_branches = BTreeSet::new();
    let mut all_calls = BTreeSet::new();
    let cases = ["cold-image", "retained-state"];
    for (case_index, name) in cases.into_iter().enumerate() {
        let mut scenario = verification::vendor_rf_init_scenario(phy_param, phy_functions_pointer);
        scenario.reset_policy = if case_index == 0 {
            execution::ResetPolicy::ColdBoot
        } else {
            execution::ResetPolicy::Continue
        };
        let result = vendor_session.execute(&image, svd, "phy_rf_init", scenario)?;
        let unmapped: BTreeSet<_> = result
            .events
            .iter()
            .filter_map(unmapped_execution_address)
            .collect();
        if !unmapped.is_empty() {
            for address in &unmapped {
                println!("SEMANTIC-UNCOVERED\t{name}\tunmapped-mmio\t{address:#010x}");
            }
            println!(
                "VERIFICATION-CASE\t{name}\tINCOMPLETE\tunmapped-mmio={}",
                unmapped.len()
            );
            continue;
        }

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
            println!(
                "SEMANTIC-DIFF\t{name}\tindex={divergence}\tvendor={:?}\trust={:?}",
                vendor_events.get(divergence),
                rust_events.get(divergence),
            );
            for (index, event) in vendor_events.iter().enumerate() {
                println!("SEMANTIC-EVENT\t{name}\tvendor\t{index}\t{event:?}");
            }
            for (index, event) in rust_events.iter().enumerate() {
                println!("SEMANTIC-EVENT\t{name}\trust\t{index}\t{event:?}");
            }
            println!(
                "VERIFICATION-CASE\t{name}\tMISMATCH\tvendor-events={}\trust-events={}",
                vendor_events.len(),
                rust_events.len(),
            );
            continue;
        }

        let retained_rc = vendor_session
            .byte(&image, phy_param + 0xa6)
            .ok_or("persistent vendor session lost phy_param RC state")?
            & 0x80
            != 0;
        if retained_rc != rust_state.rc_calibration_complete() {
            println!(
                "VERIFICATION-CASE\t{name}\tMISMATCH\tpersistent-rc-vendor={retained_rc}\tpersistent-rc-rust={}",
                rust_state.rc_calibration_complete()
            );
            continue;
        }

        passed += 1;
        total_steps = total_steps.saturating_add(result.steps);
        all_branches.extend(result.branches.iter().copied());
        all_calls.extend(result.calls.iter().cloned());
        println!(
            "VERIFICATION-CASE\t{name}\tSTATE-SEQUENCE-MATCH\tevents={}\tsteps={}\tbranch-outcomes={}\tbranch-events={}\tcalls={}\tcall-events={}\tstate-read-bytes={}\tstate-written-bytes={}\tstate-ranges={}",
            vendor_events.len(),
            result.steps,
            result.branches.len(),
            result.ordered_branches.len(),
            result.calls.len(),
            result.ordered_calls.len(),
            footprint.read_bytes,
            footprint.written_bytes,
            footprint.classified_ranges,
        );
    }

    let verdict = if passed == cases.len() {
        "STATE-SEQUENCE-MATCH"
    } else {
        "FAIL"
    };
    println!(
        "VERIFICATION-SUMMARY\tphy_rf_init\t{verdict}\tscenarios={}\tmatched={passed}\tfailed={}\tsteps={total_steps}\tbranch-outcomes={}\tcalls={}",
        cases.len(),
        cases.len() - passed,
        all_branches.len(),
        all_calls.len(),
    );
    Ok(passed == cases.len())
}

pub fn verify_esp32s31_bluetooth_txdc(
    svd: &MmioRegisterMap,
    vendor_artifact: &Path,
    vendor_companion: &Path,
    print_oracles: bool,
) -> Result<bool> {
    let artifact_digest = artifact_sha256(vendor_artifact)?;
    let companion_digest = artifact_sha256(vendor_companion)?;
    if print_oracles {
        println!(
            "ORACLE\tarchive\t{}\tsha256={artifact_digest}",
            vendor_artifact.display()
        );
        println!(
            "ORACLE\trom-companion\t{}\tsha256={companion_digest}",
            vendor_companion.display()
        );
    }

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
    let unmapped: BTreeSet<_> = result
        .events
        .iter()
        .filter_map(unmapped_execution_address)
        .collect();
    if !unmapped.is_empty() {
        for address in &unmapped {
            println!("SEMANTIC-UNCOVERED\tbluetooth-txdc\tunmapped-mmio\t{address:#010x}");
        }
        println!(
            "VERIFICATION-SUMMARY\tphy_bt_txdc_cal_new\tINCOMPLETE\tunmapped-mmio={}",
            unmapped.len()
        );
        return Ok(false);
    }

    let vendor_events = verification::normalize_vendor_bluetooth_txdc(&image, &result, phy_param)?;
    let (rust_events, _) = verification::rust_bluetooth_txdc_events(
        open_esp_radio_esp32s31_phy::phy_cold::PhyColdState::new(),
    )?;
    let matched = vendor_events == rust_events;
    if !matched {
        let divergence = vendor_events
            .iter()
            .zip(&rust_events)
            .position(|(vendor, rust)| vendor != rust)
            .unwrap_or_else(|| vendor_events.len().min(rust_events.len()));
        println!(
            "SEMANTIC-DIFF\tbluetooth-txdc\tindex={divergence}\tvendor={:?}\trust={:?}",
            vendor_events.get(divergence),
            rust_events.get(divergence),
        );
        for (index, event) in vendor_events.iter().enumerate() {
            println!("SEMANTIC-EVENT\tbluetooth-txdc\tvendor\t{index}\t{event:?}");
        }
        for (index, event) in rust_events.iter().enumerate() {
            println!("SEMANTIC-EVENT\tbluetooth-txdc\trust\t{index}\t{event:?}");
        }
    }
    let verdict = if matched {
        "STATE-SCENARIO-MATCH"
    } else {
        "MISMATCH"
    };
    println!(
        "VERIFICATION-SUMMARY\tphy_bt_txdc_cal_new\t{verdict}\tscenarios=1\tmatched={}\tfailed={}\tevents={}\tsteps={}\tbranch-outcomes={}\tcalls={}",
        usize::from(matched),
        usize::from(!matched),
        vendor_events.len(),
        result.steps,
        result.branches.len(),
        result.calls.len(),
    );
    Ok(matched)
}

pub fn verify_esp32s31_bluetooth_tx_power(
    svd: &MmioRegisterMap,
    vendor_artifact: &Path,
    vendor_companion: &Path,
    print_oracles: bool,
) -> Result<bool> {
    let artifact_digest = artifact_sha256(vendor_artifact)?;
    let companion_digest = artifact_sha256(vendor_companion)?;
    if print_oracles {
        println!(
            "ORACLE\tarchive\t{}\tsha256={artifact_digest}",
            vendor_artifact.display()
        );
        println!(
            "ORACLE\trom-companion\t{}\tsha256={companion_digest}",
            vendor_companion.display()
        );
    }

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
    let unmapped: BTreeSet<_> = result
        .events
        .iter()
        .filter_map(unmapped_execution_address)
        .collect();
    if !unmapped.is_empty() {
        for address in &unmapped {
            println!("SEMANTIC-UNCOVERED\tbluetooth-tx-power\tunmapped-mmio\t{address:#010x}");
        }
        println!(
            "VERIFICATION-SUMMARY\tphy_bt_tx_pwctrl_init\tINCOMPLETE\tunmapped-mmio={}",
            unmapped.len()
        );
        return Ok(false);
    }

    let footprint = verification::vendor_bluetooth_tx_power_state_footprint(&result, phy_param)?;
    let vendor_events =
        verification::normalize_vendor_bluetooth_tx_power(&image, &result, phy_param)?;
    let (rust_events, _) = verification::rust_bluetooth_tx_power_events(
        open_esp_radio_esp32s31_phy::phy_cold::PhyColdState::new(),
    )?;
    let matched = vendor_events == rust_events;
    if !matched {
        let divergence = vendor_events
            .iter()
            .zip(&rust_events)
            .position(|(vendor, rust)| vendor != rust)
            .unwrap_or_else(|| vendor_events.len().min(rust_events.len()));
        println!(
            "SEMANTIC-DIFF\tbluetooth-tx-power\tindex={divergence}\tvendor={:?}\trust={:?}",
            vendor_events.get(divergence),
            rust_events.get(divergence),
        );
        for (index, event) in vendor_events.iter().enumerate() {
            println!("SEMANTIC-EVENT\tbluetooth-tx-power\tvendor\t{index}\t{event:?}");
        }
        for (index, event) in rust_events.iter().enumerate() {
            println!("SEMANTIC-EVENT\tbluetooth-tx-power\trust\t{index}\t{event:?}");
        }
    }
    let verdict = if matched {
        "STATE-SCENARIO-MATCH"
    } else {
        "MISMATCH"
    };
    println!(
        "VERIFICATION-SUMMARY\tphy_bt_tx_pwctrl_init\t{verdict}\tscenarios=1\tmatched={}\tfailed={}\tevents={}\tsteps={}\tbranch-outcomes={}\tcalls={}\tstate-read-bytes={}\tstate-write-bytes={}\tstate-ranges={}",
        usize::from(matched),
        usize::from(!matched),
        vendor_events.len(),
        result.steps,
        result.branches.len(),
        result.calls.len(),
        footprint.read_bytes,
        footprint.written_bytes,
        footprint.classified_ranges,
    );
    Ok(matched)
}

pub fn verify_esp32s31_bluetooth_txdc_pwdet(
    svd: &MmioRegisterMap,
    vendor_artifact: &Path,
    vendor_companion: &Path,
    print_oracles: bool,
) -> Result<bool> {
    let artifact_digest = artifact_sha256(vendor_artifact)?;
    let companion_digest = artifact_sha256(vendor_companion)?;
    if print_oracles {
        println!(
            "ORACLE\tarchive\t{}\tsha256={artifact_digest}",
            vendor_artifact.display()
        );
        println!(
            "ORACLE\trom-companion\t{}\tsha256={companion_digest}",
            vendor_companion.display()
        );
    }

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
    let unmapped: BTreeSet<_> = result
        .events
        .iter()
        .filter_map(unmapped_execution_address)
        .collect();
    if !unmapped.is_empty() {
        for address in &unmapped {
            println!("SEMANTIC-UNCOVERED\tbluetooth-txdc-pwdet\tunmapped-mmio\t{address:#010x}");
        }
        println!(
            "VERIFICATION-SUMMARY\tphy_txdc_cal_pwdet_init\tINCOMPLETE\tunmapped-mmio={}",
            unmapped.len()
        );
        return Ok(false);
    }

    let footprint = verification::vendor_bluetooth_txdc_pwdet_state_footprint(&result, phy_param)?;
    let vendor_events =
        verification::normalize_vendor_bluetooth_txdc_pwdet(&image, &result, phy_param)?;
    let (rust_events, _) = verification::rust_bluetooth_txdc_pwdet_events(
        open_esp_radio_esp32s31_phy::phy_cold::PhyColdState::new(),
    )?;
    let matched = vendor_events == rust_events;
    if !matched {
        let divergence = vendor_events
            .iter()
            .zip(&rust_events)
            .position(|(vendor, rust)| vendor != rust)
            .unwrap_or_else(|| vendor_events.len().min(rust_events.len()));
        println!(
            "SEMANTIC-DIFF\tbluetooth-txdc-pwdet\tindex={divergence}\tvendor={:?}\trust={:?}",
            vendor_events.get(divergence),
            rust_events.get(divergence),
        );
        let window_start = divergence.saturating_sub(8);
        let window_end = divergence
            .saturating_add(9)
            .max(window_start)
            .min(vendor_events.len().max(rust_events.len()));
        for (index, event) in vendor_events
            .iter()
            .enumerate()
            .skip(window_start)
            .take(window_end - window_start)
        {
            println!("SEMANTIC-EVENT\tbluetooth-txdc-pwdet\tvendor\t{index}\t{event:?}");
        }
        for (index, event) in rust_events
            .iter()
            .enumerate()
            .skip(window_start)
            .take(window_end - window_start)
        {
            println!("SEMANTIC-EVENT\tbluetooth-txdc-pwdet\trust\t{index}\t{event:?}");
        }
    }
    let verdict = if matched {
        "STATE-SCENARIO-MATCH"
    } else {
        "MISMATCH"
    };
    println!(
        "VERIFICATION-SUMMARY\tphy_txdc_cal_pwdet_init\t{verdict}\tscenarios=1\tmatched={}\tfailed={}\tevents={}\tsteps={}\tbranch-outcomes={}\tcalls={}\tstate-read-bytes={}\tstate-write-bytes={}\tstate-ranges={}",
        usize::from(matched),
        usize::from(!matched),
        vendor_events.len(),
        result.steps,
        result.branches.len(),
        result.calls.len(),
        footprint.read_bytes,
        footprint.written_bytes,
        footprint.classified_ranges,
    );
    Ok(matched)
}
