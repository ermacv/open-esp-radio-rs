//! Channel execution with actual current archive/ROM callback bodies.
//! The temperature prefix excludes gain publication and is not full-root equivalence.
use super::*;
use open_radio_vendor_models_esp32s31::{execution_model::MemoryRange, phy_i2c::RegisterBank};
const OUTPUT: u32 = 0x3ffe_2000;

pub(super) fn map() -> MmioMap {
    MmioMap {
        registers: vec![],
        regions: [
            ("frequency", 0x2010_0000, 0x2010_0044),
            ("gain-base", 0x2010_0408, 0x2010_040c),
            ("gain-memory", 0x2010_0844, 0x2010_0854),
            ("rx-control", 0x2010_0874, 0x2010_0878),
            ("baseband", 0x2010_4400, 0x2010_4404),
            ("agc", 0x2010_7000, 0x2010_70a4),
            ("nrx", 0x2010_7848, 0x2010_784c),
            ("rx-compensation", 0x2010_7ce0, 0x2010_7ce8),
            ("work-mode", 0x2010_9c18, 0x2010_9c1c),
            ("phy-i2c", PORT_BASE, PORT_BASE + 0x24),
            ("tx-capacitance-memory", 0x2010_fc00, 0x2010_fc08),
            ("temperature", 0x2081_8000, 0x2081_8004),
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
fn put(s: &mut Scenario, address: u32, bytes: impl IntoIterator<Item = u8>) {
    s.memory_initial.extend(
        bytes
            .into_iter()
            .enumerate()
            .map(|(i, b)| (address + i as u32, b)),
    );
}
fn scenario(fill: u8, dac: u8, code: u8, ready: bool) -> Scenario {
    let mut s = Scenario {
        private_stack_fill: Some(fill),
        max_steps: 5_000_000,
        device_models: vec![
            Arc::new(RegisterBank {
                bbpll_control: Some(u32::from(fill) * 0x01010101),
                registers: BTreeMap::from([((0x69, 6), dac), ((0x6b, 2), 0)]),
                reads: BTreeMap::new(),
                busy_reads: 0,
            }),
            Arc::new(DeviceModelSpec::ConstantRead {
                id: "temperature-code".into(),
                address: 0x2081_8000,
                width: 32,
                value: u32::from(code),
            }),
            Arc::new(DeviceModelSpec::ConstantRead {
                id: "channel-ready".into(),
                address: 0x2010_0028,
                width: 32,
                value: if ready { 0x100 } else { 0 },
            }),
        ],
        ..Default::default()
    };
    for address in [
        0x2010_7030,
        0x2010_001c,
        0x2010_7848,
        0x2010_4400,
        0x2010_7ce0,
        0x2010_7ce4,
        0x2010_702c,
        0x2010_70a0,
        0x2010_9c18,
        0x2010_0874,
        0x2010_0408,
        0x2010_0844,
        0x2010_703c,
    ] {
        s.device_models
            .push(Arc::new(DeviceModelSpec::SelfClearing {
                id: format!("channel-register-{address:x}"),
                address,
                width: 32,
                initial_value: if address == 0x2010_9c18 {
                    0
                } else {
                    u32::from(fill) * 0x01010101
                },
                store_mask: u32::MAX,
                command_mask: 0,
            }));
    }
    s
}
#[test]
#[ignore = "requires authenticated current PHY archive, ROM and fresh production probe; run through blobray-run"]
fn compiled_channel_restoration_matches_current_vendor() {
    compare_channel(false);
}

#[test]
#[ignore = "requires authenticated current PHY archive, ROM and fresh production probe; excludes TX gain and later channel effects; run through blobray-run"]
fn compiled_channel_temperature_prefix_matches_current_vendor() {
    compare_channel(true);
}

fn temperature_prefix(events: &[ExecutionEvent]) -> &[ExecutionEvent] {
    let end = events
        .iter()
        .position(|event| {
            matches!(
                event,
                ExecutionEvent::Read {
                    address: 0x2010_0408,
                    ..
                }
            )
        })
        .expect("channel must reach gain publication after temperature sampling");
    let prefix = &events[..end];
    assert_eq!(
        prefix
            .iter()
            .filter(|event| matches!(
                event,
                ExecutionEvent::Read {
                    address: 0x2081_8000,
                    ..
                }
            ))
            .count(),
        1
    );
    prefix
}

fn compare_channel(prefix_only: bool) {
    let archive = input(
        "OER_PHY_ARCHIVE",
        Some("d4218e359b9716c616cbf116172f44d9195d4f2e020fad73279067e92d08e580"),
    );
    let rom = input(
        "OER_PHY_ROM",
        Some("d01bde81d9b3806e37ef1d9ac3b58af4f5b3d91eeef4f44d20e79d6a9f227542"),
    );
    let probe = input("OER_PHY_PROBE", None);
    let vendor = ExecutableImage::load_entry_with_roots(
        &archive,
        "phy_get_romfunc_addr",
        &["phy_chip_set_chan"],
        Some(&rom),
    )
    .unwrap();
    let rust_entry = "open_phy_channel_trace_state";
    let rust = image(&probe, None, rust_entry);
    let cases: Vec<_> = if prefix_only {
        [5, 7, 15, 11, 10]
            .into_iter()
            .flat_map(|dac| {
                [0, 64, 100, 255]
                    .into_iter()
                    .map(move |code| (13, 1, dac, code))
            })
            .collect()
    } else {
        [1, 6, 11, 13]
            .into_iter()
            .flat_map(|channel| [0, 1].into_iter().map(move |cbw| (channel, cbw, 5, 100)))
            .collect()
    };
    let mut different_cases = 0;
    for (channel, cbw, dac, code) in cases {
        for fill in [0x5a, 0xa5] {
            let dac = dac | (fill & 0xf0);
            let context =
                format!("channel={channel} cbw={cbw} fill={fill:x} dac={dac:x} code={code}");
            let mut v = scenario(fill, dac, code, true);
            let param = vendor.symbol_address("phy_param").unwrap();
            put(&mut v, param, [0; 512]);
            put(
                &mut v,
                vendor.symbol_address("phy_param_rom").unwrap(),
                param.to_le_bytes(),
            );
            // Execute the current archive's callback installation, retaining its
            // actual weak temperature wrapper and updated gain/compensation bodies.
            let mut session = execution::ExecutionSession::default();
            let installed = session
                .execute(
                    &vendor,
                    &map(),
                    "phy_get_romfunc_addr",
                    Scenario {
                        private_stack_fill: Some(fill),
                        ..Default::default()
                    },
                )
                .expect("install current vendor callbacks");
            assert!(
                installed.events.is_empty(),
                "callback installation touched RF"
            );
            v.arguments = vec![channel, cbw];
            let mut r = scenario(fill, dac, code, true);
            r.arguments = vec![channel, cbw, OUTPUT];
            put(&mut r, OUTPUT, [0xa5; 6]);
            r.observed_memory = vec![MemoryRange {
                start: OUTPUT,
                length: 6,
            }];
            let run = |image, entry, s| {
                execution::execute(image, &map(), entry, s)
                    .unwrap_or_else(|error| panic!("INCOMPLETE {entry} {context}: {error}"))
            };
            let v = session
                .execute(&vendor, &map(), "phy_chip_set_chan", v)
                .unwrap_or_else(|error| panic!("INCOMPLETE vendor {context}: {error}"));
            let r = run(&rust, rust_entry, r);
            assert_eq!(r.return_value, 0, "production channel failed {context}");
            for (name, result) in [("vendor", &v), ("production", &r)] {
                for coverage in &result.device_model_coverage {
                    assert!(
                        coverage.coverage.complete,
                        "INCOMPLETE {name} {context}: {coverage:?}"
                    );
                }
            }
            let vendor_byte = |offset: u32| {
                *v.persistent_memory
                    .get(&(param + offset))
                    .or_else(|| v.explicit_memory.get(&(param + offset)))
                    .expect("vendor semantic byte must be observed")
            };
            let mut semantic = [0xa5; 6];
            for change in &r.memory_changes {
                if let Some(byte) = change
                    .address
                    .checked_sub(OUTPUT)
                    .and_then(|i| semantic.get_mut(i as usize))
                {
                    *byte = change.after;
                }
            }
            assert_eq!(
                semantic,
                [
                    vendor_byte(284),
                    vendor_byte(285),
                    vendor_byte(0),
                    vendor_byte(1),
                    vendor_byte(287),
                    0
                ],
                "DIFF channel/temperature semantic commit {context}"
            );
            let expected = dcode::effects(v.events);
            let actual = dcode::effects(r.events);
            let (expected, actual) = if prefix_only {
                (temperature_prefix(&expected), temperature_prefix(&actual))
            } else {
                (expected.as_slice(), actual.as_slice())
            };
            let differences: Vec<_> = expected
                .iter()
                .zip(actual)
                .enumerate()
                .filter(|(_, (e, a))| e != a)
                .collect();
            if !differences.is_empty() || expected.len() != actual.len() {
                different_cases += 1;
                let mut locations = BTreeMap::<String, usize>::new();
                for (_, (e, a)) in &differences {
                    let location = match (e, a) {
                        (
                            ExecutionEvent::Write { address: left, .. },
                            ExecutionEvent::Write { address: right, .. },
                        ) if left == right => format!("write {left:#x}"),
                        _ => format!("{e:?} -> {a:?}"),
                    };
                    *locations.entry(location).or_default() += 1;
                }
                eprintln!(
                    "DIFF {context}: vendor={} production={} effects; differing locations={locations:?}; first={:?}",
                    expected.len(),
                    actual.len(),
                    differences.first()
                );
                continue;
            }
            println!(
                "MATCH {} {context}: {} effects",
                if prefix_only {
                    "temperature prefix (full channel equivalence NOT established)"
                } else {
                    "channel restoration"
                },
                actual.len()
            );
        }
    }
    assert_eq!(
        different_cases, 0,
        "DIFF channel comparison cases; see effect locations above"
    );
}

#[test]
#[ignore = "requires fresh compiled production probe; channel timeout containment, not vendor timing equivalence; run through blobray-run"]
fn compiled_channel_timeout_does_not_publish_gain_or_semantic_output() {
    let probe = input("OER_PHY_PROBE", None);
    let entry = "open_phy_channel_trace_state";
    let rust = image(&probe, None, entry);
    for fill in [0x5a, 0xa5] {
        let mut scenario = scenario(fill, 5, 100, false);
        scenario.arguments = vec![13, 1, OUTPUT];
        put(&mut scenario, OUTPUT, [0xa5; 6]);
        scenario.observed_memory = vec![MemoryRange {
            start: OUTPUT,
            length: 6,
        }];
        let result = execution::execute(&rust, &map(), entry, scenario)
            .unwrap_or_else(|error| panic!("INCOMPLETE production timeout: {error}"));
        assert_eq!(result.return_value, 1, "stuck channel must fail");
        assert!(
            result
                .memory_changes
                .iter()
                .all(|change| !(OUTPUT..OUTPUT + 6).contains(&change.address)),
            "failed channel must not publish semantic output"
        );
        assert!(
            !result.events.iter().any(|event| matches!(
                event,
                ExecutionEvent::Write {
                    address: 0x2010_0848 | 0x2010_084c | 0x2010_0850,
                    ..
                }
            )),
            "failed channel must not publish TX gain"
        );
        assert!(
            result.events.iter().any(|event| matches!(
                event,
                ExecutionEvent::Read {
                    address: 0x2010_0028,
                    ..
                }
            )),
            "readiness must actually be sampled"
        );
        println!(
            "CONTAINED stuck channel fill={fill:x}; no gain or semantic publication; RF resume NOT established"
        );
    }
}
