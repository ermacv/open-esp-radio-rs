//! Current combined calibration and parameter parent with actual archive/ROM
//! children and compiled production execution. Peripheral inputs are synthetic; grant overrides and
//! RF exclusion are outside this software-effects comparison.
use super::*;
use open_radio_vendor_models_esp32s31::execution_model::MemoryRange;

const INPUT: u32 = 0x3ffe_1000;
const OUTPUT: u32 = 0x3ffe_2000;

fn put(s: &mut Scenario, address: u32, bytes: impl IntoIterator<Item = u8>) {
    s.memory_initial.extend(
        bytes
            .into_iter()
            .enumerate()
            .map(|(i, b)| (address + i as u32, b)),
    );
}

fn map() -> MmioMap {
    let mut regions = tx_dc_pwdet::map().regions;
    regions.extend(channel::map().regions);
    regions.extend(frequency_map().regions);
    regions.extend(rx_gain::map().regions);
    regions.sort_by_key(|r| r.start);
    let mut merged: Vec<MmioRegion> = Vec::new();
    for r in regions {
        if let Some(last) = merged.last_mut().filter(|last| last.end >= r.start) {
            last.end = last.end.max(r.end);
        } else {
            merged.push(r);
        }
    }
    MmioMap {
        registers: vec![],
        regions: merged,
    }
}

fn tracking_scenario(fill: u8, rx: bool, correction: Option<i8>) -> Scenario {
    let mut s = tx_dc_pwdet::scenario(fill, tx_dc_pwdet::Samples::Alternating, fill == 0x5a);
    for address in [
        0x2010_0030,
        0x2010_d800,
        0x2010_001c,
        0x2010_7030,
        0x2010_0844,
        0x2010_0874,
        0x2010_4400,
        0x2010_70a0,
        0x2010_703c,
        0x2010_0c0c,
        0x2010_713c,
        0x2010_044c,
        0x2010_0450,
        0x2010_0424,
        0x2010_7ce0,
        0x2010_7ce4,
        0x2010_7848,
    ] {
        s.device_models
            .push(Arc::new(DeviceModelSpec::SelfClearing {
                id: format!("combined-register-{address:x}"),
                address,
                width: 32,
                initial_value: u32::from(fill) * 0x01010101,
                store_mask: u32::MAX,
                command_mask: 0,
            }));
    }
    s.device_models
        .push(Arc::new(DeviceModelSpec::SelfClearing {
            id: "frequency-ready".into(),
            address: 0x2010_0028,
            width: 32,
            initial_value: if correction.is_some_and(|delta| delta != 0) {
                0x2582_4f58
            } else {
                0x100
            },
            store_mask: u32::MAX,
            command_mask: 0,
        }));
    s.device_models.push(Arc::new(
        open_radio_vendor_models_esp32s31::phy_i2c::RegisterBank {
            bbpll_control: Some(u32::from(fill) * 0x01010101),
            registers: BTreeMap::from([
                ((0x62, 1), 100),
                ((0x62, 2), 0x95),
                ((0x62, 5), 100),
                ((0x62, 7), 0xc2),
                ((0x62, 11), 0x15),
                ((0x62, 4), fill),
                ((0x62, 19), fill),
                ((0x62, 20), fill),
                ((0x67, 3), fill),
                ((0x69, 6), 0x55),
                ((0x6b, 2), 0),
                ((0x6b, 3), fill),
                ((0x6b, 7), fill),
            ]),
            reads: {
                let mut reads = BTreeMap::new();
                if rx {
                    reads.insert((0x62, 17), vec![0xc0, 0xdf, 0xe0, 0xff]);
                    reads.insert((0x62, 18), vec![0xff, 0xe0, 0xdf, 0xc0]);
                }
                if let Some(delta) = correction {
                    let statuses = match delta {
                        0 => vec![0xaf; 20],
                        5 => [vec![0xa7; 2], vec![0xa3; 10]].concat(),
                        -5 => [vec![0xa3; 10], vec![0xab; 2]].concat(),
                        _ => panic!("unsupported fixture correction"),
                    };
                    reads.insert((0x62, 12), statuses);
                }
                reads
            },
            busy_reads: 0,
        },
    ));
    if correction.is_some_and(|delta| delta != 0) {
        for (address, value) in [(0x2010_0020, 0xa5a4_5678), (0x2010_002c, 0)] {
            s.device_models
                .push(Arc::new(DeviceModelSpec::SelfClearing {
                    id: format!("rfpll-parent-retained-{address:x}"),
                    address,
                    width: 32,
                    initial_value: value,
                    store_mask: u32::MAX,
                    command_mask: 0,
                }));
        }
        s.device_models
            .push(Arc::new(DeviceModelSpec::SequenceRead {
                id: "rfpll-parent-frequency-memory".into(),
                address: 0x2010_0040,
                width: 32,
                values: (0..85_u32)
                    .map(|index| 0x0055_0000 | (index << 8) | (100 + index))
                    .collect(),
            }));
    }
    let rx_sample: i32 = if fill == 0x5a { 64 } else { -64 };
    for (address, value) in [
        (0x2010_047c, 0x10000),
        (0x2010_0464, rx_sample as u32),
        (0x2010_0468, (-rx_sample) as u32),
        (0x2010_046c, rx_sample.unsigned_abs()),
        (0x2081_8000, 128),
    ] {
        s.device_models
            .push(Arc::new(DeviceModelSpec::ConstantRead {
                id: format!("combined-input-{address:x}"),
                address,
                width: 32,
                value,
            }));
    }
    s
}

#[test]
#[ignore = "requires authenticated current archive/ROM and fresh production probe; run through blobray-run"]
fn compiled_combined_calibration_matches_current_vendor() {
    compare(false, None);
}

#[test]
#[ignore = "requires authenticated current archive/ROM and fresh production probe; run through blobray-run"]
fn compiled_tracking_parent_matches_current_vendor() {
    compare(true, None);
}

#[test]
#[ignore = "requires authenticated current archive/ROM and fresh production probe; run through blobray-run"]
fn compiled_tracking_parent_with_rfpll_matches_current_vendor() {
    for delta in [0, 5, -5] {
        compare(true, Some(delta));
    }
}

fn compare(parent: bool, correction: Option<i8>) {
    let rfpll = correction.is_some();
    let archive = input(
        "OER_PHY_ARCHIVE",
        Some("d4218e359b9716c616cbf116172f44d9195d4f2e020fad73279067e92d08e580"),
    );
    let rom = input(
        "OER_PHY_ROM",
        Some("d01bde81d9b3806e37ef1d9ac3b58af4f5b3d91eeef4f44d20e79d6a9f227542"),
    );
    let probe = input("OER_PHY_PROBE", None);
    let root = if parent {
        "phy_param_track_tot"
    } else {
        "phy_cal_param_track"
    };
    let output_len = if parent { 176 } else { 162 };
    let vendor = ExecutableImage::load_entry_with_roots(
        &archive,
        "phy_get_romfunc_addr",
        &[root],
        Some(&rom),
    )
    .unwrap();
    let entry = if parent {
        "open_phy_tracking_trace_parent"
    } else {
        "open_phy_calibration_trace_combined"
    };
    let rust = image(&probe, None, entry);
    let param = vendor.symbol_address("phy_param").unwrap();
    for clients in [(false, false), (true, false), (false, true), (true, true)] {
        // RX expectation controls only availability of peripheral observations;
        // cases are explicit so the fixture does not reproduce the gate algorithm.
        let mut cases = vec![
            ("no-op", [40_u16, 40, 40], false, 256),
            ("tx-only", [40, 40, 0], false, 256),
            ("rx-and-tx", [40, 0, 0], true, 256),
            ("rx-removes-tx-demand", [40, 0, 90], true, 256),
            ("cooling-tx", [0, 0, 40], false, 256),
            ("rx-creates-tx-demand", [40, 0, 40], true, 256),
            ("exact-default", [30, 0, 30], true, 256),
            ("below-default", [29, 0, 29], false, 256),
            ("debug-threshold", [20, 0, 20], true, 20),
            ("zero-threshold", [40, 40, 40], true, 0),
        ];
        if parent {
            for (name, temperature) in [
                ("cold-clamp", -61_i16),
                ("cold-limit", -60),
                ("cold-band", -20),
                ("nominal-low", -19),
                ("nominal-high", 54),
                ("elevated-low", 55),
                ("wifi-clamp", 81),
                ("elevated-high", 94),
                ("hot-low", 95),
                ("bt-clamp", 106),
                ("small-delta", 2),
                ("threshold-boundary", 10),
            ] {
                cases.push((name, [temperature as u16; 3], false, 256));
            }
        }
        if rfpll {
            cases = vec![
                ("rfpll-and-power", [40, 40, 40], false, 256),
                ("rfpll-and-calibration", [40, 0, 0], true, 256),
                ("rfpll-below", [14, 14, 14], false, 256),
                ("rfpll-exact", [15, 15, 15], false, 256),
                ("rfpll-cooling", [(-40_i16) as u16; 3], false, 256),
                ("calibration-without-rfpll", [10, 10, 40], false, 256),
            ];
        }
        for (name, temperatures, rx, threshold) in cases {
            for fill in [0x5a, 0xa5] {
                let context = format!(
                    "{name} clients={clients:?} temperatures={temperatures:?} fill={fill:x} correction={correction:?}"
                );
                let values = [
                    temperatures[0],
                    temperatures[1],
                    temperatures[2],
                    13,
                    1,
                    0,
                    threshold,
                ];
                let mut session = execution::ExecutionSession::default();
                session
                    .execute(&vendor, &map(), "phy_get_romfunc_addr", Scenario::default())
                    .unwrap();
                let search = rfpll && !matches!(name, "rfpll-below" | "calibration-without-rfpll");
                let mut v = tracking_scenario(fill, rx, correction.filter(|_| search));
                v.arguments = if parent {
                    vec![clients.0.into(), clients.1.into()]
                } else {
                    vec![0, clients.0.into(), clients.1.into()]
                };
                put(&mut v, param, [0; 516]);
                put(
                    &mut v,
                    vendor.symbol_address("phy_param_rom").unwrap(),
                    param.to_le_bytes(),
                );
                for (offset, value) in [
                    (0, values[0]),
                    (400, values[1]),
                    (72, values[2]),
                    (284, values[3]),
                ] {
                    put(&mut v, param + offset, value.to_le_bytes());
                }
                for (offset, value) in [(2, 0xbf), (20, 1), (287, 1), (288, 75), (289, 71)] {
                    put(&mut v, param + offset, [value]);
                }
                if parent {
                    put(&mut v, param + 427, [u8::from(fill == 0x5a)]);
                    put(&mut v, param + 434, [fill]);
                    put(&mut v, param + 11, [1]);
                    put(&mut v, param + 10, [u8::from(rfpll)]);
                }
                if threshold < 256 {
                    put(&mut v, param + 432, [2, threshold as u8]);
                }
                let mut r = tracking_scenario(fill, rx, correction.filter(|_| search));
                r.arguments = vec![INPUT, clients.0.into(), clients.1.into(), OUTPUT];
                put(&mut r, INPUT, values.iter().flat_map(|v| v.to_le_bytes()));
                if parent {
                    put(&mut r, INPUT + 14, u16::from(fill).to_le_bytes());
                    put(&mut r, INPUT + 16, u16::from(fill == 0x5a).to_le_bytes());
                    put(&mut r, INPUT + 18, u16::from(rfpll).to_le_bytes());
                }
                put(&mut r, OUTPUT, vec![0xa5; output_len]);
                r.observed_memory = vec![MemoryRange {
                    start: OUTPUT,
                    length: output_len as u32,
                }];
                let v = session
                    .execute(&vendor, &map(), root, v)
                    .unwrap_or_else(|e| panic!("INCOMPLETE vendor {context}: {e}"));
                let r = execution::execute(&rust, &map(), entry, r)
                    .unwrap_or_else(|e| panic!("INCOMPLETE production {context}: {e}"));
                for (name, result) in [("vendor", &v), ("production", &r)] {
                    for coverage in &result.device_model_coverage {
                        assert!(
                            coverage.coverage.complete,
                            "INCOMPLETE {name} {context}: {coverage:?}"
                        );
                    }
                    if let Some(directory) = std::env::var_os("OER_PHY_TRACE_DIR") {
                        let trace = result
                            .events
                            .iter()
                            .zip(&result.event_producers)
                            .enumerate()
                            .map(|(i, (e, p))| format!("{i}: {e:?} {p:?}\n"))
                            .collect::<String>();
                        std::fs::write(
                            Path::new(&directory).join(format!("{root}-{name}.txt")),
                            trace,
                        )
                        .unwrap();
                    }
                }
                assert_eq!(r.return_value, 0, "production failed {context}");
                let mut expected = Vec::new();
                for offset in [0, 400, 72, 284] {
                    expected.extend([
                        session.byte(&vendor, param + offset).unwrap(),
                        session.byte(&vendor, param + offset + 1).unwrap(),
                    ]);
                }
                expected.extend([session.byte(&vendor, param + 287).unwrap(), 0]);
                for offset in [168, 260] {
                    expected.extend(
                        (0..24).map(|i| session.byte(&vendor, param + offset + i).unwrap()),
                    );
                }
                for (offset, count) in [(334, 36), (436, 68)] {
                    expected.extend(
                        (0..count).map(|i| session.byte(&vendor, param + offset + i).unwrap()),
                    );
                }
                if parent {
                    expected.extend([
                        session.byte(&vendor, param + 4).unwrap(),
                        session.byte(&vendor, param + 5).unwrap(),
                    ]);
                    for offset in [290, 291, 292] {
                        expected.extend(
                            (session.byte(&vendor, param + offset).unwrap() as i8 as i16)
                                .to_le_bytes(),
                        );
                    }
                }
                if parent {
                    let adjustment = session.byte(&vendor, param + 434).unwrap();
                    assert_eq!(
                        adjustment, fill,
                        "runtime parent changed gain adjustment {context}"
                    );
                    expected.extend((adjustment as i8 as i16).to_le_bytes());
                    expected.extend(
                        u16::from(session.byte(&vendor, param + 77).unwrap()).to_le_bytes(),
                    );
                }
                if parent {
                    expected.extend([
                        session.byte(&vendor, param + 304).unwrap(),
                        session.byte(&vendor, param + 305).unwrap(),
                    ]);
                }
                let mut actual = vec![0xa5; output_len];
                for c in &r.memory_changes {
                    actual[(c.address - OUTPUT) as usize] = c.after;
                }
                let semantic = (actual, expected);
                let mut vendor_events = v.events;
                if search && correction != Some(0) {
                    // Same single installed-layout query as the isolated
                    // nonzero correction profile; retain every table transaction.
                    let query = vendor_events
                        .iter()
                        .enumerate()
                        .filter_map(|(i, e)| {
                            matches!(
                                e,
                                ExecutionEvent::Read {
                                    address: 0x2010_0028,
                                    ..
                                }
                            )
                            .then_some(i)
                        })
                        .nth(1)
                        .expect("vendor installed-layout query");
                    assert!(
                        matches!(
                            vendor_events.remove(query),
                            ExecutionEvent::Read {
                                address: 0x2010_0028,
                                value: 0x2582_4f5a,
                                ..
                            }
                        ),
                        "unexpected installed-layout observation {context}"
                    );
                }
                let expected = rx_gain::effects(tx_dc_pwdet::effects(vendor_events), 0);
                let actual = rx_gain::effects(tx_dc_pwdet::effects(r.events), 0);
                for (i, (e, a)) in expected.iter().zip(&actual).enumerate() {
                    assert_eq!(a, e, "DIFF effect {i} {context}");
                }
                assert_eq!(actual.len(), expected.len(), "DIFF effect count {context}");
                assert_eq!(semantic.0, semantic.1, "DIFF semantic commit {context}");
                println!("MATCH {root} {context}; {} effects", actual.len());
            }
        }
    }
}

#[test]
#[ignore = "requires fresh production probe; run through blobray-run"]
fn compiled_combined_failed_tx_retains_pre_rx_semantic_state() {
    failed_tx(false, None);
}

#[test]
#[ignore = "requires fresh production probe; run through blobray-run"]
fn compiled_tracking_parent_failed_tx_retains_completed_power_only() {
    failed_tx(true, None);
}

#[test]
#[ignore = "requires fresh production probe; run through blobray-run"]
fn compiled_tracking_parent_failed_tx_retains_completed_rfpll_and_power() {
    for correction in [0, 5, -5] {
        failed_tx(true, Some(correction));
    }
}

fn failed_tx(parent: bool, correction: Option<i8>) {
    let probe = input("OER_PHY_PROBE", None);
    let entry = if parent {
        "open_phy_tracking_trace_parent"
    } else {
        "open_phy_calibration_trace_combined"
    };
    let output_len = if parent { 176 } else { 162 };
    let rust = image(&probe, None, entry);
    for fill in [0x5a, 0xa5] {
        let mut s = tracking_scenario(fill, true, correction);
        s.device_models
            .retain(|m| m.descriptor().range.start != 0x2010_080c);
        s.device_models
            .push(Arc::new(DeviceModelSpec::SelfClearing {
                id: "sar-never-ready".into(),
                address: 0x2010_080c,
                width: 32,
                initial_value: 0,
                store_mask: !(7 << 14),
                command_mask: 0,
            }));
        let values = [40_u16, 0, 0, 13, 1, 0, 256];
        put(&mut s, INPUT, values.iter().flat_map(|v| v.to_le_bytes()));
        if parent {
            put(&mut s, INPUT + 14, u16::from(fill).to_le_bytes());
            put(&mut s, INPUT + 16, 1_u16.to_le_bytes());
            put(
                &mut s,
                INPUT + 18,
                u16::from(correction.is_some()).to_le_bytes(),
            );
        }
        put(&mut s, OUTPUT, vec![0xa5; output_len]);
        s.arguments = vec![INPUT, 1, 1, OUTPUT];
        s.observed_memory = vec![MemoryRange {
            start: OUTPUT,
            length: output_len as u32,
        }];
        let result = execution::execute(&rust, &map(), entry, s)
            .unwrap_or_else(|e| panic!("INCOMPLETE failed combined calibration: {e}"));
        assert_eq!(
            result.return_value, 1,
            "failed TX must not commit the combined transaction"
        );
        for coverage in &result.device_model_coverage {
            assert!(coverage.coverage.complete, "INCOMPLETE: {coverage:?}");
        }
        assert!(
            result.events.iter().any(|e| matches!(
                e,
                ExecutionEvent::Read {
                    address: 0x2081_8000,
                    ..
                }
            )),
            "RX channel restoration never executed"
        );
        assert!(
            result.events.iter().any(|e| matches!(
                e,
                ExecutionEvent::Read {
                    address: 0x2010_080c,
                    ..
                }
            )),
            "TX fault never reached"
        );
        let mut actual = vec![0xa5; output_len];
        for c in result.memory_changes {
            actual[(c.address - OUTPUT) as usize] = c.after;
        }
        let mut expected = vec![0; 162];
        expected[..10].copy_from_slice(
            &values[..5]
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect::<Vec<_>>(),
        );
        if parent {
            // BT computes +8 at 40; Wi-Fi reuses the shared cache. These
            // children completed before RX/TX failed, and are not rolled back.
            expected.extend(
                [
                    40_u16,
                    8,
                    8,
                    8,
                    fill as i8 as i16 as u16,
                    0,
                    if correction.is_some() { 40 } else { 0 },
                ]
                .into_iter()
                .flat_map(u16::to_le_bytes),
            );
        }
        assert_eq!(
            actual, expected,
            "incorrect state at failed parent boundary"
        );
        println!(
            "MATCH {entry} failed TX retains pre-RX coefficients and completed children fill={fill:x} RFPLL={correction:?}"
        );
    }
}
