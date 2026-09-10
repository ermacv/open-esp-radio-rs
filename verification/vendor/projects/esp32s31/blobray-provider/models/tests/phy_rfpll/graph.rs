//! Execute the authenticated current parent with explicit child boundaries.
//! This verifies call order/ABI/guards only, not child effects or RF safety.
use super::*;
use open_radio_vendor_models_esp32s31::execution::ModeledCallResponse;
use std::collections::VecDeque;

#[test]
#[ignore = "requires authenticated PHY archive; run through blobray-run"]
fn current_parent_calls_combined_calibration_after_power_and_before_temperature() {
    let archive = input(
        "OER_PHY_ARCHIVE",
        Some("d4218e359b9716c616cbf116172f44d9195d4f2e020fad73279067e92d08e580"),
    );
    let entry = "phy_param_track_tot";
    let vendor = image(&archive, None, entry);
    let param = vendor.symbol_address("phy_param").unwrap();
    for (wifi, shared) in [(false, false), (true, false), (false, true), (true, true)] {
        for rfpll in [false, true] {
            for calibration in [false, true] {
                for guard in [0_u32, 23, 405] {
                    let mut expected: Vec<(&str, Vec<u32>)> =
                        vec![("phy_i2c_enter_critical", vec![])];
                    if guard == 0 {
                        if rfpll {
                            expected.push(("phy_rfpll_cap_track_new", vec![1]));
                        }
                        if shared {
                            expected.push(("phy_bt_track_tx_power_new", vec![1, 1]));
                        }
                        if wifi {
                            expected.push(("phy_tx_i2c_track", vec![]));
                            expected.push(("phy_wifi_track_tx_power_new", vec![1, 1]));
                        }
                        if calibration {
                            expected
                                .push(("phy_cal_param_track", vec![1, wifi.into(), shared.into()]));
                        }
                        expected.push(("phy_tsens_temp_read", vec![]));
                    }
                    expected.push(("phy_i2c_exit_critical", vec![]));
                    let mut scenario = Scenario {
                        arguments: vec![wifi.into(), shared.into()],
                        private_stack_fill: Some(0xa5),
                        ..Default::default()
                    };
                    // Explicit no-effect children isolate the actual vendor parent.
                    // No modeled child may be skipped, duplicated or unconsumed.
                    for (symbol, _) in &expected {
                        scenario.call_responses.insert(
                            (*symbol).into(),
                            VecDeque::from([ModeledCallResponse::scalar(0)]),
                        );
                    }
                    for (offset, value) in [
                        (9, 1),
                        (10, rfpll.into()),
                        (11, 1),
                        (23, 0),
                        (405, 0),
                        (402, u8::from(!calibration)),
                    ] {
                        scenario.memory_initial.insert(param + offset, value);
                    }
                    for offset in [0, 4, 72, 304, 400] {
                        scenario.memory_initial.insert(param + offset, 20);
                        scenario.memory_initial.insert(param + offset + 1, 0);
                    }
                    if guard != 0 {
                        scenario.memory_initial.insert(param + guard, 1);
                    }
                    let context = format!(
                        "wifi={wifi} shared={shared} rfpll={rfpll} calibration={calibration} guard={guard}"
                    );
                    let result = execution::execute(
                        &vendor,
                        &MmioMap {
                            registers: vec![],
                            regions: vec![],
                        },
                        entry,
                        scenario,
                    )
                    .unwrap_or_else(|error| panic!("INCOMPLETE parent {context}: {error}"));
                    assert_eq!(
                        result.ordered_calls.len(),
                        expected.len(),
                        "call count {context}"
                    );
                    for (actual, (name, arguments)) in result.ordered_calls.iter().zip(&expected) {
                        assert_eq!(actual.symbol, *name, "call order {context}");
                        assert_eq!(
                            &actual.arguments[..arguments.len()],
                            arguments,
                            "child ABI {name}: {context}"
                        );
                    }
                    assert!(result.events.is_empty(), "unexpected parent MMIO {context}");
                    println!(
                        "PASS current parent boundary {context}; modeled child effects, no whole-graph equivalence claim"
                    );
                }
            }
        }
    }
}

#[test]
#[ignore = "requires authenticated PHY archive and ROM; run through blobray-run"]
fn current_calibration_brackets_rx_and_shared_tx_with_separate_grants() {
    let archive = input(
        "OER_PHY_ARCHIVE",
        Some("d4218e359b9716c616cbf116172f44d9195d4f2e020fad73279067e92d08e580"),
    );
    let rom = input(
        "OER_PHY_ROM",
        Some("d01bde81d9b3806e37ef1d9ac3b58af4f5b3d91eeef4f44d20e79d6a9f227542"),
    );
    let entry = "phy_cal_param_track";
    let vendor = image(&archive, Some(&rom), entry);
    let param = vendor.symbol_address("phy_param").unwrap();
    let callbacks = vendor.symbol_address("g_phyFuns").unwrap();
    // This is an explicit callback fixture, not proof of the vendor startup
    // callback installation. Only the parent's slot/argument/order is tested.
    let restore = "phy_txgain_comp_pacfg_";
    let restore_address = vendor.symbol_address(restore).unwrap();
    for (wifi, shared) in [(false, false), (true, false), (false, true), (true, true)] {
        for (rx, tx) in [(false, false), (true, false), (false, true), (true, true)] {
            let mut expected: Vec<(&str, Vec<u32>)> = vec![("phy_abs_temp", vec![])];
            if rx {
                expected.extend([
                    ("phy_acquire_grant_protect", vec![]),
                    ("phy_pbus_clear_reg", vec![]),
                    ("phy_dcode_cal_init", vec![]),
                    ("phy_set_rx_gain_table", vec![2437, 0]),
                    ("phy_chip_set_chan", vec![13, 1]),
                    ("phy_mac_enable_bb", vec![]),
                    (restore, vec![1]),
                    ("phy_release_grant_protect", vec![]),
                ]);
            }
            expected.push(("phy_abs_temp", vec![]));
            if tx {
                expected.extend([
                    ("phy_acquire_grant_protect", vec![]),
                    ("phy_dis_hw_set_freq_new", vec![]),
                    ("phy_force_txrx_off", vec![1]),
                    (
                        "phy_force_dig_gain",
                        vec![1, (-120_i32) as u32, (-120_i32) as u32],
                    ),
                    ("phy_pbus_clear_reg", vec![]),
                    ("phy_bb_cbw_chan_cfg", vec![0]),
                ]);
                if wifi {
                    expected.extend([
                        ("phy_txdc_cal_pwdet_init", vec![0, 0, 0]),
                        ("phy_wifi_set_tx_gain_new", vec![13, 0]),
                    ]);
                }
                if shared {
                    expected.extend([
                        ("phy_txdc_cal_pwdet_init", vec![0, 0, 1]),
                        ("phy_bt_set_tx_gain_new", vec![0]),
                    ]);
                }
                expected.extend([
                    ("phy_bb_cbw_chan_cfg", vec![1]),
                    ("phy_mac_enable_bb", vec![]),
                    (
                        "phy_force_dig_gain",
                        vec![0, (-120_i32) as u32, (-120_i32) as u32],
                    ),
                    ("phy_force_txrx_off", vec![0]),
                    ("phy_en_hw_set_freq_new", vec![]),
                    (restore, vec![1]),
                    ("phy_release_grant_protect", vec![]),
                ]);
            }
            let mut scenario = Scenario {
                arguments: vec![0, wifi.into(), shared.into()],
                private_stack_fill: Some(0xa5),
                mmio_initial: BTreeMap::from([(0x2010_9c18, 0xa5a5_ffff)]),
                observed_memory: vec![
                    open_radio_vendor_models_esp32s31::execution_model::MemoryRange {
                        start: param,
                        length: 512,
                    },
                ],
                ..Default::default()
            };
            for (symbol, _) in &expected {
                if matches!(
                    *symbol,
                    "phy_abs_temp" | "phy_acquire_grant_protect" | "phy_release_grant_protect"
                ) {
                    continue;
                }
                scenario
                    .call_responses
                    .entry((*symbol).into())
                    .or_default()
                    .push_back(ModeledCallResponse::scalar(0));
            }
            for (address, bytes) in [
                (param, 50_i16.to_le_bytes().to_vec()),
                (
                    param + 72,
                    (if tx { 20_i16 } else { 21 }).to_le_bytes().to_vec(),
                ),
                (
                    param + 400,
                    (if rx { 20_i16 } else { 21 }).to_le_bytes().to_vec(),
                ),
                (param + 164, 0xffff_ffff_u32.to_le_bytes().to_vec()),
                (param + 284, 13_u16.to_le_bytes().to_vec()),
                (param + 287, vec![1]),
                (param + 432, vec![0]),
                (param + 510, 0xa000_u16.to_le_bytes().to_vec()),
                (callbacks, 0x3ffe_0000_u32.to_le_bytes().to_vec()),
                (0x3ffe_0030, restore_address.to_le_bytes().to_vec()),
            ] {
                scenario.memory_initial.extend(
                    bytes
                        .into_iter()
                        .enumerate()
                        .map(|(i, v)| (address + i as u32, v)),
                );
            }
            let mut final_bytes = scenario.memory_initial.clone();
            let context = format!("wifi={wifi} shared={shared} rx={rx} tx={tx}");
            let result = execution::execute(
                &vendor,
                &MmioMap {
                    registers: vec![],
                    regions: vec![MmioRegion {
                        name: "calibration-control".into(),
                        start: 0x2010_9c18,
                        end: 0x2010_9c1c,
                        readable: true,
                        writable: true,
                    }],
                },
                entry,
                scenario,
            )
            .unwrap_or_else(|error| panic!("INCOMPLETE calibration {context}: {error}"));
            assert_eq!(
                result.ordered_calls.len(),
                expected.len(),
                "call count {context}: {:?}",
                result.ordered_calls
            );
            for (actual, (name, args)) in result.ordered_calls.iter().zip(&expected) {
                assert_eq!(actual.symbol, *name, "order {context}");
                assert_eq!(
                    &actual.arguments[..args.len()],
                    args,
                    "ABI {name} {context}"
                );
            }
            for change in result.memory_changes {
                final_bytes.insert(change.address, change.after);
            }
            let word = |offset| {
                u16::from_le_bytes([
                    final_bytes[&(param + offset)],
                    final_bytes[&(param + offset + 1)],
                ])
            };
            assert_eq!(
                word(400),
                if rx { 50 } else { 21 },
                "RX reference {context}"
            );
            assert_eq!(
                word(72),
                if tx { 50 } else { 21 },
                "shared TX reference {context}"
            );
            assert_eq!(
                word(510),
                0xa000 | if rx { 8 } else { 0 } | if tx { 16 } else { 0 },
                "flags {context}"
            );
            println!(
                "PASS current calibration boundary {context}; child effects and restore callback explicitly modeled"
            );
        }
    }
}
