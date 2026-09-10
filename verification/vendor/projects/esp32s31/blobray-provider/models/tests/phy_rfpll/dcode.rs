//! Execute RXCAL's PBus/D-code prefix with real ROM and compiled OER children.
use super::*;
use open_radio_vendor_models_esp32s31::{execution_model::MemoryRange, phy_i2c::RegisterBank};

fn map() -> MmioMap {
    let mut map = calibration::pbus_map();
    map.regions.extend([
        MmioRegion {
            name: "frequency".into(),
            start: 0x2010_0000,
            end: 0x2010_0044,
            readable: true,
            writable: true,
        },
        MmioRegion {
            name: "phy-i2c".into(),
            start: PORT_BASE,
            end: PORT_BASE + 0x24,
            readable: true,
            writable: true,
        },
        MmioRegion {
            name: "nrx".into(),
            start: 0x2010_7848,
            end: 0x2010_784c,
            readable: true,
            writable: true,
        },
    ]);
    map
}

fn bank(fill: u8, busy: u16) -> RegisterBank {
    let mut registers = BTreeMap::new();
    for register in [4, 19, 20] {
        registers.insert((0x62, register), fill);
    }
    RegisterBank {
        bbpll_control: None,
        registers,
        reads: BTreeMap::from([
            ((0x62, 17), vec![0xc0, 0xdf, 0xe0, 0xff]),
            ((0x62, 18), vec![0xff, 0xe0, 0xdf, 0xc0]),
        ]),
        busy_reads: busy,
    }
}

pub(super) fn effects(events: Vec<ExecutionEvent>) -> Vec<ExecutionEvent> {
    // BBPLL control shares the aperture but is an observable PHY operation.
    // Exclude only command polling/configuration, never the entire aperture.
    let transport_register = |address: u32| {
        [PORT_BASE, PORT_BASE + 4, PORT_BASE + 0x1c, PORT_BASE + 0x20].contains(&address)
    };
    let events = calibration::pbus_effects(events);
    let mut events = events.into_iter().peekable();
    let mut effects = Vec::new();
    while let Some(event) = events.next() {
        match &event {
            ExecutionEvent::DelayMicros(1) if matches!(events.peek(), Some(ExecutionEvent::Read { address, .. }) if transport_register(*address)) =>
            {
                continue;
            }
            ExecutionEvent::Read { address, .. } if transport_register(*address) => {
                continue;
            }
            ExecutionEvent::Write { address, .. }
                if *address == PORT_BASE + 0x1c || *address == PORT_BASE + 0x20 =>
            {
                continue;
            }
            _ => effects.push(event),
        }
    }
    effects
}

#[test]
#[ignore = "requires authenticated current PHY archive, ROM and fresh production probe; run through blobray-run"]
fn compiled_dcode_matches_measurements_and_nested_rfpll() {
    let archive = input(
        "OER_PHY_ARCHIVE",
        Some("d4218e359b9716c616cbf116172f44d9195d4f2e020fad73279067e92d08e580"),
    );
    let rom = input(
        "OER_PHY_ROM",
        Some("d01bde81d9b3806e37ef1d9ac3b58af4f5b3d91eeef4f44d20e79d6a9f227542"),
    );
    let probe = input("OER_PHY_PROBE", None);
    let vendor = image(&archive, Some(&rom), "phy_cal_param_track");
    let rust_entry = "open_phy_calibration_trace_dcode";
    let rust = image(&probe, None, rust_entry);
    let param = vendor.symbol_address("phy_param").unwrap();
    let param_pointer = vendor.symbol_address("phy_param_rom").unwrap();
    for crystal in [0, 1, 2, 3] {
        for fill in [0x5a, 0xa5] {
            for busy in [0, 2] {
                let context = format!("crystal={crystal} fill={fill:#x} busy={busy}");
                let mut scenario = Scenario {
                    private_stack_fill: Some(fill),
                    max_steps: 2_000_000,
                    mmio_initial: BTreeMap::from([(0x2010_001c, 0x4128_0055)]),
                    device_models: vec![
                        Arc::new(bank(fill, busy)),
                        Arc::new(DeviceModelSpec::SequenceRead {
                            id: "channel-ready".into(),
                            address: 0x2010_0028,
                            width: 32,
                            values: (0..4)
                                .flat_map(|_| {
                                    std::iter::repeat_n(0x2582_4e58, usize::from(busy))
                                        .chain([0x2582_4f58])
                                })
                                .collect(),
                        }),
                        Arc::new(DeviceModelSpec::SelfClearing {
                            id: "nrx".into(),
                            address: 0x2010_7848,
                            width: 32,
                            initial_value: 0x1655_a55a,
                            store_mask: u32::MAX,
                            command_mask: 0,
                        }),
                    ],
                    ..Default::default()
                };
                let output = 0x3ffe_1000;
                let mut rust_scenario = calibration::pbus_scenario(0, false, 0, fill);
                rust_scenario
                    .device_models
                    .extend(scenario.device_models.clone());
                rust_scenario.max_steps = scenario.max_steps;
                rust_scenario.mmio_initial = scenario.mmio_initial.clone();
                rust_scenario.arguments = vec![crystal, output];
                rust_scenario.observed_memory = vec![MemoryRange {
                    start: output,
                    length: 8,
                }];
                for index in 0..8 {
                    rust_scenario.memory_initial.insert(output + index, 0xa5);
                }
                scenario.memory_initial.extend(
                    param
                        .to_le_bytes()
                        .into_iter()
                        .enumerate()
                        .map(|(index, byte)| (param_pointer + index as u32, byte)),
                );
                scenario.memory_initial.insert(param + 79, crystal as u8);
                for index in 0..8 {
                    scenario.memory_initial.insert(param + 417 + index, 0xa5);
                }
                scenario.observed_memory = vec![MemoryRange {
                    start: param + 417,
                    length: 8,
                }];
                let run = |image, entry, scenario| {
                    let result = execution::execute(image, &map(), entry, scenario)
                        .unwrap_or_else(|error| panic!("INCOMPLETE {entry} {context}: {error}"));
                    for coverage in &result.device_model_coverage {
                        assert!(
                            coverage.coverage.complete,
                            "INCOMPLETE {context}: {coverage:?}"
                        );
                    }
                    result
                };
                // These two children use disjoint modeled peripherals. Their
                // parent call order is checked separately by the graph test.
                let pbus = run(
                    &vendor,
                    "phy_pbus_clear_reg",
                    calibration::pbus_scenario(0, false, 0, fill),
                );
                let vendor = run(&vendor, "phy_dcode_cal_init", scenario);
                let rust = run(&rust, rust_entry, rust_scenario);
                assert_eq!(rust.return_value, 0, "production child failed {context}");
                let expected = effects([pbus.events, vendor.events].concat());
                let actual = effects(rust.events);
                for (index, (expected, actual)) in expected.iter().zip(&actual).enumerate() {
                    assert_eq!(expected, actual, "DIFF {context} effect={index}");
                }
                assert_eq!(expected.len(), actual.len(), "DIFF effect count {context}");
                let bytes = |changes: Vec<execution::MemoryChange>, start| {
                    let mut bytes = [0xa5; 8];
                    for change in changes {
                        bytes[(change.address - start) as usize] = change.after;
                    }
                    bytes
                };
                let expected = bytes(vendor.memory_changes, param + 417);
                let actual = bytes(rust.memory_changes, output);
                assert_eq!(expected, actual, "DIFF measured codes {context}");
                assert_eq!(
                    actual,
                    [0, 63, 31, 32, 32, 31, 63, 0],
                    "output sample consumption {context}"
                );
                println!("MATCH D-code measurements and nested RFPLL commands {context}");
            }
        }
    }
}

#[test]
#[ignore = "requires a fresh compiled production probe; run through blobray-run"]
fn compiled_dcode_failures_do_not_publish_partial_codes() {
    let probe = input("OER_PHY_PROBE", None);
    let entry = "open_phy_calibration_trace_dcode";
    let rust = image(&probe, None, entry);
    for channel_ready in [false, true] {
        let mut scenario = calibration::pbus_scenario(0, false, 0, 0xa5);
        scenario.max_steps = 20_000_000;
        scenario.mmio_initial = BTreeMap::from([
            (0x2010_001c, 0x4128_0055),
            (0x2010_0028, if channel_ready { 0x100 } else { 0 }),
            (0x2010_7848, 0x1655_a55a),
        ]);
        let output = 0x3ffe_1000;
        scenario.arguments = vec![0, output];
        scenario.observed_memory = vec![MemoryRange {
            start: output,
            length: 8,
        }];
        for index in 0..8 {
            scenario.memory_initial.insert(output + index, 0xa5);
        }
        if channel_ready {
            // The first CKGEN read never completes. No D-code measurement is
            // supplied; advancing beyond this failure must be rejected.
            let mut stuck = bank(0x5a, u16::MAX);
            stuck.reads.clear();
            scenario.device_models.push(Arc::new(stuck));
        }
        let result = execution::execute(&rust, &map(), entry, scenario)
            .expect("production must return a bounded failure");
        assert_eq!(result.return_value, if channel_ready { 5 } else { 7 });
        assert!(
            result.memory_changes.is_empty(),
            "failed calibration published result bytes"
        );
        let commands = result.events.iter().filter(|event| matches!(event,
            ExecutionEvent::Write { address, .. } if *address == PORT_BASE || *address == PORT_BASE + 4)).count();
        assert_eq!(
            commands,
            usize::from(channel_ready),
            "hardware progressed past the failed boundary"
        );
        println!("PASS D-code failure containment channel_ready={channel_ready}");
    }
}
