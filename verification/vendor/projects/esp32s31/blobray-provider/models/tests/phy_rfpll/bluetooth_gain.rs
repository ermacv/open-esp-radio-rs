//! Actual current Bluetooth gain callback and complete bank publication.
//! This is a PHY child comparison, not BT/154 protocol or whole-TXCAL readiness.
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

fn scenario(base: u8, fill: u8) -> Scenario {
    Scenario {
        private_stack_fill: Some(fill),
        persistent_memory: vec![MemoryRange {
            start: OUTPUT,
            length: 80,
        }],
        device_models: vec![
            Arc::new(DeviceModelSpec::ConstantRead {
                id: "gain-base".into(),
                address: 0x2010_0408,
                width: 32,
                value: (u32::from(base) << 24) | 0x005a_a55a,
            }),
            Arc::new(DeviceModelSpec::SelfClearing {
                id: "gain-index".into(),
                address: 0x2010_0844,
                width: 32,
                initial_value: u32::from(fill) * 0x0101_0101,
                store_mask: u32::MAX,
                command_mask: 0,
            }),
        ],
        ..Default::default()
    }
}

#[test]
#[ignore = "requires authenticated current archive/ROM and fresh compiled probe; run through blobray-run"]
fn compiled_bluetooth_gain_matches_current_calculation_and_publication() {
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
        &["phy_bt_set_tx_gain_new"],
        Some(&rom),
    )
    .unwrap();
    let rust_entry = "open_phy_bluetooth_trace_tx_gain";
    let rust = image(&probe, None, rust_entry);
    let param = vendor.symbol_address("phy_param").unwrap();
    let map = MmioMap {
        registers: vec![],
        regions: [
            ("gain-base", 0x2010_0408, 0x2010_040c),
            ("gain-memory", 0x2010_0844, 0x2010_0854),
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
    };
    let mut cases = 0;
    for curve in [[0_u8; 3], [127, 128, 255], [255, 31, 128]] {
        for (base, attenuation, correction) in [
            (0_u8, 0_u8, 0_i8),
            (127, 255, 127),
            (128, 1, -128),
            (255, 31, -17),
            (0, 127, 17),
        ] {
            for memory_base in [0, 32, 224, 255] {
                for fill in [0x5a, 0xa5] {
                    let context = format!(
                        "curve={curve:?} base={base} attenuation={attenuation} correction={correction} memory_base={memory_base} fill={fill:x}"
                    );
                    let mut session = execution::ExecutionSession::default();
                    let installed = session
                        .execute(
                            &vendor,
                            &map,
                            "phy_get_romfunc_addr",
                            Scenario {
                                private_stack_fill: Some(fill),
                                ..Default::default()
                            },
                        )
                        .unwrap();
                    assert!(installed.events.is_empty());
                    let mut words = [0_u32; 9];
                    for (i, word) in words[..6].iter_mut().enumerate() {
                        *word =
                            (u32::from(fill) * 0x0101_0101).wrapping_add(i as u32 * 0x0102_0305);
                    }
                    words[6] = u32::from(fill) * 0x0101;
                    words[7] = u32::from_le_bytes([curve[0], curve[1], curve[2], correction as u8]);
                    words[8] = u32::from_le_bytes([base, attenuation, 0, 0]);
                    let mut v = scenario(memory_base, fill);
                    put(&mut v, param, [0; 516]);
                    put(
                        &mut v,
                        param + 260,
                        words[..6].iter().flat_map(|w| w.to_le_bytes()),
                    );
                    put(&mut v, param + 208, (words[6] as u16).to_le_bytes());
                    put(&mut v, param + 251, words[7].to_le_bytes());
                    put(&mut v, param + 292, [base]);
                    put(&mut v, param + 8, [attenuation]);
                    // Wi-Fi-only inputs deliberately vary independently. They
                    // must not alter the shared BT/154 gain calculation.
                    put(&mut v, param + 291, [fill]);
                    put(&mut v, param + 434, [fill]);
                    put(&mut v, OUTPUT, [fill; 80]);
                    v.arguments = vec![OUTPUT + 48, OUTPUT + 16, OUTPUT, 0];
                    let calculated = session
                        .execute(&vendor, &map, "phy_bt_get_tx_tab_new", v)
                        .unwrap_or_else(|e| panic!("INCOMPLETE calculation {context}: {e}"));
                    assert!(calculated.events.is_empty());
                    let publication = session
                        .execute(
                            &vendor,
                            &map,
                            "phy_bt_set_tx_gain_new",
                            Scenario {
                                arguments: vec![0],
                                ..scenario(memory_base, fill)
                            },
                        )
                        .unwrap_or_else(|e| panic!("INCOMPLETE publication {context}: {e}"));
                    let mut r = scenario(memory_base, fill);
                    r.arguments = vec![INPUT, OUTPUT];
                    put(&mut r, INPUT, words.into_iter().flat_map(u32::to_le_bytes));
                    put(&mut r, OUTPUT, [fill; 80]);
                    let result = execution::execute(&rust, &map, rust_entry, r)
                        .unwrap_or_else(|e| panic!("INCOMPLETE production {context}: {e}"));
                    for result in [&calculated, &publication, &result] {
                        for coverage in &result.device_model_coverage {
                            assert!(
                                coverage.coverage.complete,
                                "INCOMPLETE {context}: {coverage:?}"
                            );
                        }
                    }
                    let output = |result: &execution::ExecutionResult| {
                        (0..80)
                            .map(|i| result.persistent_memory[&(OUTPUT + i)])
                            .collect::<Vec<_>>()
                    };
                    assert_eq!(
                        output(&calculated),
                        output(&result),
                        "DIFF gain image {context}"
                    );
                    assert_eq!(
                        publication.events, result.events,
                        "DIFF gain publication {context}"
                    );
                    assert_eq!(result.events.len(), 81, "complete 16-entry bank {context}");
                    cases += 1;
                }
            }
        }
    }
    println!(
        "MATCH current Bluetooth gain calculation and complete 16-entry publication: {cases} cases; no whole-TXCAL/RF qualification claim"
    );
}
