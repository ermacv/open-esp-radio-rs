//! Real RX-gain root execution against the current archive and ROM.
use super::*;
use open_radio_vendor_models_esp32s31::execution_model::MemoryRange;

const INPUT: u32 = 0x3ffe_1000;
const OUTPUT: u32 = 0x3ffe_2000;
const MAX_PRODUCTION_STEP_MULTIPLIER: u64 = 2;

pub(super) fn map() -> MmioMap {
    MmioMap {
        registers: vec![],
        regions: [
            ("frequency", 0x2010_0000, 0x2010_0044),
            ("phy-i2c", PORT_BASE, PORT_BASE + 0x24),
            ("nrx", 0x2010_7848, 0x2010_784c),
            ("baseband", 0x2010_0400, 0x2010_0480),
            ("gain-memory", 0x2010_0844, 0x2010_0854),
            ("pbus", 0x2010_0880, 0x2010_08e0),
            ("cal-clock", 0x2010_0800, 0x2010_0804),
            ("tx-iq", 0x2010_0c0c, 0x2010_0c10),
            ("agc", 0x2010_7000, 0x2010_7140),
            ("work-mode", 0x2010_9c18, 0x2010_9c1c),
        ]
        .into_iter()
        .map(|(name, start, end)| MmioRegion {
            name: name.into(),
            start,
            end,
            readable: true,
            writable: true,
        })
        .collect(),
    }
}

fn retained(
    address: u32,
    value: u32,
) -> Arc<dyn open_radio_vendor_models_esp32s31::execution_model::DeviceModel> {
    Arc::new(DeviceModelSpec::SelfClearing {
        id: format!("rx-register-{address:x}"),
        address,
        width: 32,
        initial_value: value,
        store_mask: u32::MAX,
        command_mask: 0,
    })
}

fn scenario(fill: u8, settle: bool) -> Scenario {
    Scenario {
        private_stack_fill: Some(fill),
        max_steps: 30_000_000,
        device_models: [
            0x2010_0434,
            0x2010_0438,
            0x2010_0844,
            0x2010_0884,
            0x2010_088c,
            0x2010_0890,
            0x2010_0c0c,
            0x2010_702c,
            0x2010_713c,
            0x2010_9c18,
        ]
        .into_iter()
        .map(|address| {
            retained(
                address,
                match address {
                    0x2010_9c18 => u32::from(settle) * 2,
                    0x2010_0890 => 0x1234, // Idle PBus, retained clock bits are writable.
                    _ => u32::from(fill) * 0x0101_0101,
                },
            )
        })
        .collect(),
        ..Default::default()
    }
}

fn put(scenario: &mut Scenario, address: u32, bytes: impl IntoIterator<Item = u8>) {
    scenario.memory_initial.extend(
        bytes
            .into_iter()
            .enumerate()
            .map(|(i, b)| (address + i as u32, b)),
    );
}

fn parameters(seed: u16) -> [u16; 53] {
    core::array::from_fn(|i| match i {
        0..40 => (seed.wrapping_add(i as u16 * 7)) & 0x1ff,
        40..52 => (i as i16 - 46).wrapping_mul(seed as i16) as u16,
        _ => 0x125,
    })
}

fn inputs(
    vendor: &ExecutableImage,
    values: [u16; 53],
    flags: u8,
    path: u8,
    scenario: &mut Scenario,
) {
    let param = vendor.symbol_address("phy_param").unwrap();
    put(
        scenario,
        vendor.symbol_address("phy_param_rom").unwrap(),
        param.to_le_bytes(),
    );
    put(
        scenario,
        param + 164,
        (u32::from(flags & 1) * 128 + u32::from((flags >> 1) & 1) * 512).to_le_bytes(),
    );
    put(scenario, param + 16, 0u16.to_le_bytes()); // Qualified normal gain table, alternate vendor table mode excluded.
    put(
        scenario,
        param + 334,
        values[..16].iter().flat_map(|v| v.to_le_bytes()),
    );
    put(
        scenario,
        param + 366,
        values[16..18].iter().flat_map(|v| v.to_le_bytes()),
    );
    put(
        scenario,
        param + 436,
        values[18..40].iter().flat_map(|v| v.to_le_bytes()),
    );
    put(
        scenario,
        param + 480,
        values[40..52].iter().flat_map(|v| v.to_le_bytes()),
    );
    put(scenario, param + 212, values[52].to_le_bytes());
    scenario.memory_initial.insert(param + 2, path);
    scenario.memory_initial.insert(param + 288, 75);
    scenario.memory_initial.insert(param + 289, 71);
    scenario.memory_initial.insert(param + 79, 0);
    scenario.memory_initial.insert(param + 430, 0);
}

pub(super) fn effects(events: Vec<ExecutionEvent>, flags: u8) -> Vec<ExecutionEvent> {
    let mut result = Vec::new();
    for event in dcode::effects(events) {
        // The vendor snapshots outer DC control even when DC is skipped.
        if flags != 0
            && matches!(
                event,
                ExecutionEvent::Read {
                    address: 0x2010_0434,
                    ..
                }
            )
        {
            continue;
        }
        result.push(event);
    }
    result
}

#[test]
#[ignore = "requires authenticated current PHY archive, ROM and fresh production probe; run through blobray-run"]
fn compiled_rx_gain_publication_matches_current_vendor() {
    compare_roots(&[1, 3], &[0]);
}

#[test]
#[ignore = "requires authenticated current PHY archive, ROM and fresh production probe; run through blobray-run"]
fn compiled_rx_gain_calibration_matches_current_vendor() {
    compare_roots(&[0], &[0, 64, -64, 16_777_216, -16_777_216]);
}

fn compare_roots(guard_flags: &[u8], samples: &[i32]) {
    let archive = input(
        "OER_PHY_ARCHIVE",
        Some("d4218e359b9716c616cbf116172f44d9195d4f2e020fad73279067e92d08e580"),
    );
    let rom = input(
        "OER_PHY_ROM",
        Some("d01bde81d9b3806e37ef1d9ac3b58af4f5b3d91eeef4f44d20e79d6a9f227542"),
    );
    let probe = input("OER_PHY_PROBE", None);
    let vendor =
        ExecutableImage::load_entry_with_companion(&archive, "phy_set_rx_gain_table", &rom)
            .expect("link vendor with ROM data");
    let rust_entry = "open_phy_calibration_trace_rx_gain";
    let rust = image(&probe, None, rust_entry);
    for &sample in samples {
        for &flags in guard_flags {
            for seed in [1, 17] {
                for fill in [0x5a, 0xa5] {
                    for settle in [false, true] {
                        let context = format!(
                            "sample={sample} flags={flags} seed={seed} fill={fill:x} settle={settle}"
                        );
                        let values = parameters(seed);
                        let mut v = scenario(fill, settle);
                        if flags == 0 {
                            calibration_inputs(&mut v, fill, sample, u16::from(settle) * 2);
                        }
                        inputs(&vendor, values, flags, 0xbf, &mut v);
                        v.arguments = vec![2437, 0];
                        let param = vendor.symbol_address("phy_param").unwrap();
                        v.observed_memory = vec![
                            MemoryRange {
                                start: param + 334,
                                length: 36,
                            },
                            MemoryRange {
                                start: param + 436,
                                length: 68,
                            },
                            MemoryRange {
                                start: param + 288,
                                length: 2,
                            },
                        ];
                        let vendor_initial = v.memory_initial.clone();
                        let mut r = scenario(fill, settle);
                        if flags == 0 {
                            calibration_inputs(&mut r, fill, sample, u16::from(settle) * 2);
                        }
                        put(&mut r, INPUT, values.iter().flat_map(|v| v.to_le_bytes()));
                        put(&mut r, OUTPUT, [0xa5; 108]);
                        r.observed_memory = vec![MemoryRange {
                            start: OUTPUT,
                            length: 108,
                        }];
                        r.arguments = vec![INPUT, u32::from(flags), 0, 0xbf, OUTPUT];
                        let run = |image, entry, scenario| {
                            execution::execute(image, &map(), entry, scenario).unwrap_or_else(
                                |error| panic!("INCOMPLETE {entry} {context}: {error}"),
                            )
                        };
                        let v = run(&vendor, "phy_set_rx_gain_table", v);
                        let r = run(&rust, rust_entry, r);
                        assert_eq!(r.return_value, 0, "production root failed {context}");
                        assert!(
                            r.steps <= v.steps.saturating_mul(MAX_PRODUCTION_STEP_MULTIPLIER),
                            "DIFF RX gain execution cost {context}: vendor_steps={}, production_steps={}, maximum_multiplier={MAX_PRODUCTION_STEP_MULTIPLIER}",
                            v.steps,
                            r.steps,
                        );
                        if let Some(directory) = std::env::var_os("OER_PHY_TRACE_DIR") {
                            for (name, result) in [("vendor", &v), ("production", &r)] {
                                let trace = result
                                    .events
                                    .iter()
                                    .zip(&result.event_producers)
                                    .enumerate()
                                    .map(|(i, (event, producer))| {
                                        format!("{i}: {event:?} {producer:?}\n")
                                    })
                                    .collect::<String>();
                                std::fs::write(
                                    Path::new(&directory).join(format!("rx-gain-{name}.txt")),
                                    trace,
                                )
                                .unwrap();
                            }
                        }

                        for (name, result) in [("vendor", &v), ("production", &r)] {
                            for coverage in &result.device_model_coverage {
                                assert!(
                                    coverage.coverage.complete,
                                    "INCOMPLETE {name} {context}: {coverage:?}"
                                );
                            }
                        }
                        let mut final_vendor = vendor_initial;
                        for change in &v.memory_changes {
                            final_vendor.insert(change.address, change.after);
                        }
                        let mut final_rust = [0xa5; 108];
                        for change in &r.memory_changes {
                            final_rust[(change.address - OUTPUT) as usize] = change.after;
                        }
                        let expected_state: Vec<u8> = (334..370)
                            .chain(436..504)
                            .map(|offset| final_vendor[&(param + offset)])
                            .chain([
                                final_vendor[&(param + 289)],
                                0,
                                final_vendor[&(param + 288)],
                                0,
                            ])
                            .collect();
                        assert_eq!(
                            expected_state, final_rust,
                            "DIFF semantic RX state {context}"
                        );
                        let expected = effects(v.events, flags);
                        let actual = effects(r.events, flags);
                        for (i, (e, a)) in expected.iter().zip(&actual).enumerate() {
                            assert_eq!(
                                e,
                                a,
                                "DIFF {context} effect={i}\nvendor: {:?}\nproduction: {:?}",
                                &expected[i.saturating_sub(2)..(i + 5).min(expected.len())],
                                &actual[i.saturating_sub(2)..(i + 5).min(actual.len())]
                            );
                        }
                        assert_eq!(expected.len(), actual.len(), "DIFF effects {context}");
                        println!(
                            "MATCH RX gain state and effects {context}: {} effects, vendor_steps={}, production_steps={}",
                            actual.len(),
                            v.steps,
                            r.steps,
                        );
                    }
                }
            }
        }
    }
}

fn calibration_inputs(s: &mut Scenario, fill: u8, sample: i32, busy: u16) {
    use open_radio_vendor_models_esp32s31::phy_i2c::RegisterBank;
    s.device_models.extend(
        [
            0x2010_001c,
            0x2010_7848,
            0x2010_044c,
            0x2010_0450,
            0x2010_0800,
            0x2010_0424,
        ]
        .into_iter()
        .map(|a| retained(a, u32::from(fill) * 0x01010101)),
    );
    s.device_models.push(Arc::new(RegisterBank {
        bbpll_control: None,
        registers: BTreeMap::from([((0x67, 3), fill)]),
        reads: BTreeMap::new(),
        busy_reads: busy,
    }));
    for (address, value) in [
        (0x2010_0894, 0x01000100),
        (0x2010_0028, 0x100),
        (0x2010_047c, 0x10000),
        (0x2010_0464, sample as u32),
        (0x2010_0468, (-sample) as u32),
        (0x2010_046c, sample.unsigned_abs()),
    ] {
        s.device_models
            .push(Arc::new(DeviceModelSpec::ConstantRead {
                id: format!("rx-input-{address:x}"),
                address,
                width: 32,
                value,
            }));
    }
}

#[test]
#[ignore = "requires fresh production probe; run through blobray-run"]
fn compiled_rx_gain_failed_channel_does_not_publish_coefficients() {
    let probe = input("OER_PHY_PROBE", None);
    let entry = "open_phy_calibration_trace_rx_gain";
    let rust = image(&probe, None, entry);
    for fill in [0x5a, 0xa5] {
        let mut s = scenario(fill, false);
        calibration_inputs(&mut s, fill, 0, 0);
        s.device_models
            .retain(|model| model.descriptor().range.start != 0x2010_0028);
        s.device_models
            .push(Arc::new(DeviceModelSpec::ConstantRead {
                id: "channel-never-ready".into(),
                address: 0x2010_0028,
                width: 32,
                value: 0,
            }));
        let values = parameters(17);
        put(&mut s, INPUT, values.iter().flat_map(|v| v.to_le_bytes()));
        put(&mut s, OUTPUT, [0xa5; 108]);
        s.observed_memory = vec![MemoryRange {
            start: OUTPUT,
            length: 108,
        }];
        s.arguments = vec![INPUT, 0, 0, 0xbf, OUTPUT];
        let result =
            execution::execute(&rust, &map(), entry, s).expect("execute bounded failed channel");
        assert_eq!(
            result.return_value, 4,
            "must report typed RX calibration failure"
        );
        assert!(
            result.memory_changes.is_empty(),
            "failed RX root published coefficients"
        );
        assert!(
            !result.events.iter().any(|event| matches!(
                event,
                ExecutionEvent::Write {
                    address: 0x2010_0844,
                    ..
                }
            )),
            "gain tables published after failed channel"
        );
        println!("MATCH bounded RX channel failure retains unpublished output fill={fill:x}");
    }
}

#[test]
#[ignore = "requires fresh production probe; run through blobray-run"]
fn compiled_rx_gain_minimum_failure_and_shared_budget_do_not_publish_coefficients() {
    let probe = input("OER_PHY_PROBE", None);
    let entry = "open_phy_calibration_trace_rx_gain";
    let rust = image(&probe, None, entry);
    for slow_successful_minima in [false, true] {
        for fill in [0x5a, 0xa5] {
            let mut s = scenario(fill, false);
            s.max_steps = 200_000_000;
            calibration_inputs(&mut s, fill, 0, 0);
            s.device_models
                .retain(|model| model.descriptor().range.start != 0x2010_047c);
            // Each slow estimator becomes ready below its own timeout. The
            // enclosing operation budget must still span successive minima.
            let values = if slow_successful_minima {
                (0..20)
                    .flat_map(|_| core::iter::repeat_n(0, 9_000).chain([0x10000]))
                    .collect()
            } else {
                vec![0; 10_000]
            };
            s.device_models
                .push(Arc::new(DeviceModelSpec::SequenceRead {
                    id: "rx-estimator-readiness".into(),
                    address: 0x2010_047c,
                    width: 32,
                    values,
                }));
            // The not-ready branch also samples PBus estimator activity.
            // This profile models an idle input independently of READY.
            s.device_models
                .push(Arc::new(DeviceModelSpec::ConstantRead {
                    id: "rx-estimator-activity".into(),
                    address: 0x2010_08d0,
                    width: 32,
                    value: 0,
                }));
            let values = parameters(17);
            put(&mut s, INPUT, values.iter().flat_map(|v| v.to_le_bytes()));
            put(&mut s, OUTPUT, [0xa5; 108]);
            s.observed_memory = vec![MemoryRange {
                start: OUTPUT,
                length: 108,
            }];
            s.arguments = vec![INPUT, 0, 0, 0xbf, OUTPUT];
            let result = execution::execute(&rust, &map(), entry, s)
                .expect("execute production minimum timeout / shared budget exhaustion");
            assert_eq!(
                result.return_value,
                if slow_successful_minima { 5 } else { 4 }
            );
            assert!(
                result.memory_changes.is_empty(),
                "failed root published coefficients"
            );
            assert!(
                !result.events.iter().any(|event| matches!(
                    event,
                    ExecutionEvent::Write {
                        address: 0x2010_0844,
                        ..
                    }
                )),
                "failed root published gain memory"
            );
            let ready = result
                .events
                .iter()
                .filter(|event| {
                    matches!(
                        event,
                        ExecutionEvent::Read {
                            address: 0x2010_047c,
                            value: 0x10000,
                            ..
                        }
                    )
                })
                .count();
            if slow_successful_minima {
                assert!(
                    ready > 1,
                    "must cross multiple successful minimum boundaries"
                );
                assert!(
                    ready < 20,
                    "shared budget must stop before the input sequence ends"
                );
            } else {
                assert_eq!(ready, 0);
            }
            println!(
                "MATCH bounded RX minimum failure slow={slow_successful_minima} fill={fill:x} completed_estimators={ready}; output unpublished"
            );
        }
    }
}

#[test]
fn readiness_wait_is_visible_to_rx_effect_comparison() {
    let read = ExecutionEvent::Read {
        width: 4,
        address: 0x2010_047c,
        region: "baseband".into(),
        register: None,
        value: 0,
    };
    let direct = effects(vec![read.clone()], 0);
    let delayed = effects(vec![ExecutionEvent::DelayMicros(1), read], 0);
    assert_ne!(
        direct, delayed,
        "an added readiness wait must not disappear from comparison"
    );
    assert!(delayed.contains(&ExecutionEvent::DelayMicros(1)));
}
