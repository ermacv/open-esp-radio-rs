//! Complete current TX-DC/PWDET root and compiled production executor.
//! Synthetic peripheral measurements establish software effects, not RF accuracy.
use super::*;
use open_radio_vendor_models_esp32s31::execution_model::MemoryRange;

#[path = "tx_dc_pwdet/samples.rs"]
mod samples;
pub(super) use samples::Samples;

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

pub(super) fn map() -> MmioMap {
    MmioMap {
        registers: vec![],
        regions: [
            ("baseband", 0x2010_0400, 0x2010_0480),
            ("power-detector", 0x2010_0800, 0x2010_0844),
            ("pbus", 0x2010_0880, 0x2010_08e0),
            ("agc", 0x2010_7000, 0x2010_7040),
            ("work-mode", 0x2010_9c18, 0x2010_9c1c),
            ("lp-sar", 0x2070_1068, 0x2070_106c),
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

pub(super) fn scenario(fill: u8, sample: Samples, settle: bool) -> Scenario {
    let mut s = Scenario {
        private_stack_fill: Some(fill),
        max_steps: 30_000_000,
        ..Default::default()
    };
    for address in [
        0x2010_0400,
        0x2010_0404,
        0x2010_0408,
        0x2010_040c,
        0x2010_0410,
        0x2010_0414,
        0x2010_0418,
        0x2010_041c,
        0x2010_0420,
        0x2010_0428,
        0x2010_0434,
        0x2010_0438,
        0x2010_0800,
        0x2010_0808,
        0x2010_080c,
        0x2010_0814,
        0x2010_0884,
        0x2010_088c,
        0x2010_0890,
        0x2010_702c,
        0x2010_9c18,
        0x2070_1068,
    ] {
        s.device_models
            .push(Arc::new(DeviceModelSpec::SelfClearing {
                id: format!("txdc-register-{address:x}"),
                address,
                width: 32,
                initial_value: match address {
                    0x2010_080c => 7 << 14,
                    0x2010_0890 => 0x1234,
                    0x2010_9c18 => u32::from(settle) * 2,
                    _ => u32::from(fill) * 0x0101_0101,
                },
                store_mask: u32::MAX,
                command_mask: 0,
            }));
    }
    s.device_models.push(Arc::new(sample));
    s.device_models
        .push(Arc::new(DeviceModelSpec::ConstantRead {
            id: "bluetooth-pbus-path".into(),
            address: 0x2010_0894,
            width: 32,
            value: u32::from(fill) * 0x0101_0101,
        }));
    for address in [0x2010_0820, 0x2010_0824, 0x2010_0828] {
        s.device_models
            .push(Arc::new(DeviceModelSpec::ConstantRead {
                id: format!("sar-unused-{address:x}"),
                address,
                width: 32,
                value: u32::from(fill) * 0x0101_0101,
            }));
    }
    s
}

// ROM phy_read_sar_dout snapshots four result words, while its tone-average
// caller consumes only the upper sample of the first word. Production reads
// that sample directly. Exclude only those three unused, read-only result
// observations; their independent values remain explicit scenario inputs.
// Every control/status access, write and non-PBus-settle delay remains compared.
pub(super) fn effects(events: Vec<ExecutionEvent>) -> Vec<ExecutionEvent> {
    calibration::pbus_effects(events)
        .into_iter()
        .filter(|e| {
            !matches!(
                e,
                ExecutionEvent::Read {
                    address: 0x2010_0820 | 0x2010_0824 | 0x2010_0828,
                    ..
                }
            )
        })
        .collect()
}

#[test]
#[ignore = "requires authenticated current archive/ROM and fresh production probe; run through blobray-run"]
fn compiled_tx_dc_pwdet_matches_current_root() {
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
        &["phy_txdc_cal_pwdet_init"],
        Some(&rom),
    )
    .unwrap();
    let rust_entry = "open_phy_calibration_trace_tx_dc_pwdet";
    let rust = image(&probe, None, rust_entry);
    let param = vendor.symbol_address("phy_param").unwrap();
    for bluetooth in [false, true] {
        for sample in [
            Samples::Constant(0),
            Samples::Constant(123),
            Samples::Constant(8191),
            Samples::Alternating,
        ] {
            for clear in [false, true] {
                for (fill, settle) in [(0x5a, false), (0xa5, false), (0x5a, true), (0xa5, true)] {
                    let context = format!(
                        "bt={bluetooth} sample={sample:?} clear={clear} fill={fill:x} settle={settle}"
                    );
                    let values: [u16; 12] =
                        core::array::from_fn(|i| if fill == 0x5a { 256 } else { 240 + i as u16 });
                    let offset = if bluetooth { 260 } else { 168 };
                    let mut session = execution::ExecutionSession::default();
                    session
                        .execute(&vendor, &map(), "phy_get_romfunc_addr", Scenario::default())
                        .unwrap();
                    let mut v = scenario(fill, sample, settle);
                    v.arguments = vec![0, 0, bluetooth.into()];
                    put(&mut v, param, [0; 516]);
                    put(
                        &mut v,
                        vendor.symbol_address("phy_param_rom").unwrap(),
                        param.to_le_bytes(),
                    );
                    put(&mut v, param + 20, [7]);
                    put(&mut v, param + 426, [clear.into()]);
                    put(&mut v, param + 434, [fill]);
                    put(
                        &mut v,
                        param + offset,
                        values.iter().flat_map(|v| v.to_le_bytes()),
                    );
                    v.observed_memory = vec![MemoryRange {
                        start: param + offset,
                        length: 24,
                    }];
                    let mut r = scenario(fill, sample, settle);
                    r.arguments = vec![INPUT, bluetooth.into(), 7, clear.into(), OUTPUT];
                    put(&mut r, INPUT, values.iter().flat_map(|v| v.to_le_bytes()));
                    put(&mut r, OUTPUT, [fill; 24]);
                    r.observed_memory = vec![MemoryRange {
                        start: OUTPUT,
                        length: 24,
                    }];
                    let v = session
                        .execute(&vendor, &map(), "phy_txdc_cal_pwdet_init", v)
                        .unwrap_or_else(|e| panic!("INCOMPLETE vendor {context}: {e}"));
                    assert_eq!(
                        session.byte(&vendor, param + 434),
                        Some(fill),
                        "TX-DC root must not overwrite the independent Wi-Fi gain adjustment {context}"
                    );
                    let r = execution::execute(&rust, &map(), rust_entry, r)
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
                                Path::new(&directory).join(format!("txdc-{name}.txt")),
                                trace,
                            )
                            .unwrap();
                        }
                    }
                    assert_eq!(r.return_value, 0, "production failed {context}");
                    let mut expected: Vec<u8> =
                        values.iter().flat_map(|v| v.to_le_bytes()).collect();
                    for c in &v.memory_changes {
                        expected[(c.address - param - offset) as usize] = c.after;
                    }
                    let mut actual = vec![fill; 24];
                    for c in &r.memory_changes {
                        actual[(c.address - OUTPUT) as usize] = c.after;
                    }
                    assert_eq!(expected, actual, "DIFF DC rows {context}");
                    let expected = effects(v.events);
                    let actual = effects(r.events);
                    for (i, (e, a)) in expected.iter().zip(&actual).enumerate() {
                        assert_eq!(e, a, "DIFF effect {i} {context}");
                    }
                    assert_eq!(expected.len(), actual.len(), "DIFF effect count {context}");
                    println!(
                        "MATCH current TX-DC/PWDET {context}: {} effects",
                        actual.len()
                    );
                }
            }
        }
    }
}

#[test]
#[ignore = "requires fresh compiled production probe; run through blobray-run"]
fn compiled_tx_dc_pwdet_fault_does_not_publish_calibration() {
    let probe = input("OER_PHY_PROBE", None);
    let entry = "open_phy_calibration_trace_tx_dc_pwdet";
    let rust = image(&probe, None, entry);
    // Both failures have a typed terminal cause. SAR's observation budget is
    // distinct from an elapsed-time deadline and the global executor budget.
    for (address, stuck, expected) in [(0x2010_0890, 0x8000_0000, 4), (0x2010_080c, 0, 5)] {
        let mut s = scenario(0x5a, Samples::Constant(123), false);
        s.device_models
            .retain(|m| m.descriptor().range.start != address);
        s.device_models
            .push(Arc::new(DeviceModelSpec::SelfClearing {
                id: "stuck-peripheral".into(),
                address,
                width: 32,
                initial_value: stuck,
                store_mask: u32::MAX,
                command_mask: 0,
            }));
        s.arguments = vec![INPUT, 0, 7, 0, OUTPUT];
        put(
            &mut s,
            INPUT,
            [256_u16; 12].iter().flat_map(|v| v.to_le_bytes()),
        );
        put(&mut s, OUTPUT, [0xa5; 24]);
        s.observed_memory = vec![MemoryRange {
            start: OUTPUT,
            length: 24,
        }];
        let result =
            execution::execute(&rust, &map(), entry, s).expect("bounded production failure");
        assert_eq!(
            result.return_value, expected,
            "wrong terminal cause for {address:x}"
        );
        assert!(
            result.memory_changes.is_empty(),
            "failed calibration must not publish DC rows"
        );
        assert!(
            !result.events.iter().any(|e| matches!(
                e,
                ExecutionEvent::Read {
                    address: 0x2010_081c,
                    ..
                }
            )),
            "failed readiness must not consume a SAR result"
        );
        for coverage in result.device_model_coverage {
            assert!(
                coverage.coverage.complete,
                "INCOMPLETE fault model: {coverage:?}"
            );
        }
    }
}
