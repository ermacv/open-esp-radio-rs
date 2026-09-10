//! Gain publication with identical synthetic inputs, independent of vendor tables.
use super::*;
use open_radio_vendor_models_esp32s31::execution_model::MemoryRange;

const INPUT: u32 = 0x3ffe_1000;
const OUTPUT: u32 = 0x3ffe_2000;

fn callback_output(
    image: &ExecutableImage,
    entry: &str,
    base: u8,
    attenuation: u8,
    adjustment: u8,
) -> Vec<u8> {
    let param = if entry.ends_with("_new") {
        image.symbol_address("phy_param").unwrap()
    } else {
        INPUT
    };
    let mut scenario = Scenario {
        arguments: vec![13, OUTPUT, OUTPUT + 32, OUTPUT + 96, 0],
        private_stack_fill: Some(0x5a),
        observed_memory: vec![MemoryRange {
            start: OUTPUT,
            length: 160,
        }],
        ..Default::default()
    };
    scenario
        .memory_initial
        .extend((0..512).map(|i| (param + i, 0)));
    scenario
        .memory_initial
        .extend((0..160).map(|i| (OUTPUT + i, 0xa5)));
    scenario.memory_initial.extend([
        (param + 291, base),
        (param + 8, attenuation),
        (param + 434, adjustment),
    ]);
    if let Some(pointer) = image.symbol_address("phy_param_rom") {
        scenario.memory_initial.extend(
            param
                .to_le_bytes()
                .into_iter()
                .enumerate()
                .map(|(i, byte)| (pointer + i as u32, byte)),
        );
    }
    let result = execution::execute(
        image,
        &MmioMap {
            registers: vec![],
            regions: vec![],
        },
        entry,
        scenario,
    )
    .unwrap_or_else(|error| panic!("INCOMPLETE {entry} gain input: {error}"));
    assert!(
        result.events.is_empty(),
        "gain calculation touched hardware"
    );
    let mut output = vec![0xa5; 160];
    for change in result.memory_changes {
        if let Some(byte) = change
            .address
            .checked_sub(OUTPUT)
            .and_then(|i| output.get_mut(i as usize))
        {
            *byte = change.after;
        }
    }
    output
}

#[test]
#[ignore = "requires authenticated current archive and ROM; vendor characterization, not production equivalence; run through blobray-run"]
fn current_gain_callback_uses_additive_adjustment_not_rom_attenuation() {
    let archive = input(
        "OER_PHY_ARCHIVE",
        Some("d4218e359b9716c616cbf116172f44d9195d4f2e020fad73279067e92d08e580"),
    );
    let rom = input(
        "OER_PHY_ROM",
        Some("d01bde81d9b3806e37ef1d9ac3b58af4f5b3d91eeef4f44d20e79d6a9f227542"),
    );
    let entry = "phy_wifi_get_tx_tab_new";
    let current = image(&archive, Some(&rom), entry);
    let legacy = image(&rom, None, "phy_wifi_get_tx_tab_");
    let baseline = callback_output(&current, entry, 0, 0, 0);
    for attenuation in [1, 31, 127, 255] {
        assert_eq!(
            callback_output(&current, entry, 0, attenuation, 0),
            baseline,
            "current callback must not consume the ROM attenuation field"
        );
    }
    for (base, adjustment) in [(0_u8, 1_u8), (0, 31), (127, 1), (255, 1), (10, 255)] {
        assert_eq!(
            callback_output(&current, entry, base, 0, adjustment),
            callback_output(&current, entry, base.wrapping_add(adjustment), 0, 0),
            "current gain adjustment must add before signed-byte narrowing"
        );
    }
    assert_ne!(callback_output(&current, entry, 0, 0, 31), baseline);
    let old_baseline = callback_output(&legacy, "phy_wifi_get_tx_tab_", 0, 0, 0);
    assert_eq!(
        callback_output(&legacy, "phy_wifi_get_tx_tab_", 31, 31, 0),
        old_baseline
    );
    assert_eq!(
        callback_output(&legacy, "phy_wifi_get_tx_tab_", 0, 0, 31),
        old_baseline
    );
    assert_ne!(
        old_baseline, baseline,
        "coefficient sets also differ with zero adjustment"
    );
    println!(
        "CHARACTERIZED current additive gain adjustment and distinct ROM subtraction; no production equivalence claim"
    );
}

fn map() -> MmioMap {
    MmioMap {
        registers: vec![],
        regions: [
            ("base", 0x2010_0408, 0x2010_040c),
            ("gain", 0x2010_0844, 0x2010_0854),
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

fn scenario(input: &[u32; 47], base: u8, fill: u8) -> Scenario {
    Scenario {
        private_stack_fill: Some(fill),
        memory_initial: input
            .iter()
            .flat_map(|word| word.to_le_bytes())
            .enumerate()
            .map(|(i, byte)| (INPUT + i as u32, byte))
            .collect(),
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
#[ignore = "requires authenticated current archive, ROM and fresh production probe; run through blobray-run"]
fn compiled_tx_gain_publication_matches_current_vendor() {
    let archive = input(
        "OER_PHY_ARCHIVE",
        Some("d4218e359b9716c616cbf116172f44d9195d4f2e020fad73279067e92d08e580"),
    );
    let rom = input(
        "OER_PHY_ROM",
        Some("d01bde81d9b3806e37ef1d9ac3b58af4f5b3d91eeef4f44d20e79d6a9f227542"),
    );
    let probe = input("OER_PHY_PROBE", None);
    let vendor_entry = "phy_set_tx_gain_mem_new";
    let rust_entry = "open_phy_channel_trace_publish_tx_gain";
    let vendor = image(&archive, Some(&rom), vendor_entry);
    let rust = image(&probe, None, rust_entry);
    for seed in [0_u32, 0x1357_2468, 0xffff_ffff] {
        let mut values = [0_u32; 47];
        for (i, value) in values.iter_mut().enumerate() {
            *value = seed.wrapping_add(i as u32 * 0x0102_0305);
        }
        // These three baseband settings address the three supplied DC rows.
        // No vendor calibration table is copied into the scenario.
        for i in 0..32 {
            let gain = [0_u32, 128, 256][i % 3];
            let shift = (i % 2) * 16;
            values[14 + i / 2] = (values[14 + i / 2] & !(0xffff << shift)) | (gain << shift);
        }
        for base in [0, 32, 224, 255] {
            for fill in [0x5a, 0xa5] {
                let context = format!("seed={seed:x} base={base} fill={fill:x}");
                let mut v = scenario(&values, base, fill);
                v.arguments = vec![
                    0,
                    32,
                    INPUT + 120,
                    INPUT + 56,
                    INPUT + 24,
                    INPUT,
                    INPUT + 184,
                ];
                let mut r = scenario(&values, base, fill);
                r.arguments = vec![INPUT];
                let run = |image, entry, scenario| {
                    execution::execute(image, &map(), entry, scenario)
                        .unwrap_or_else(|error| panic!("INCOMPLETE {entry} {context}: {error}"))
                };
                let v = run(&vendor, vendor_entry, v);
                let r = run(&rust, rust_entry, r);
                assert_eq!(r.return_value, 0, "production publication failed {context}");
                for (name, result) in [("vendor", &v), ("production", &r)] {
                    for coverage in &result.device_model_coverage {
                        assert!(
                            coverage.coverage.complete,
                            "INCOMPLETE {name} {context}: {coverage:?}"
                        );
                    }
                }
                for (i, (expected, actual)) in v.events.iter().zip(&r.events).enumerate() {
                    assert_eq!(expected, actual, "DIFF {context} effect={i}");
                }
                assert_eq!(
                    v.events.len(),
                    r.events.len(),
                    "DIFF effect count {context}"
                );
                println!(
                    "MATCH TX gain publication {context}: {} effects",
                    r.events.len()
                );
            }
        }
    }
}
