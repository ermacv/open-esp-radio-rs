//! Current-archive gain calculation and its ROM kernel boundary.
//! The production comparison consumes the real current callback and tables.
use super::*;
use open_radio_vendor_models_esp32s31::execution_model::MemoryRange;

const PARAM: u32 = 0x3ffe_1000;
const OUTPUT: u32 = 0x3ffe_2000;

fn output(result: &execution::ExecutionResult) -> Vec<u8> {
    (0..160)
        .map(|i| {
            *result
                .persistent_memory
                .get(&(OUTPUT + i))
                .expect("complete output")
        })
        .collect()
}

#[test]
#[ignore = "requires authenticated current archive and ROM; characterizes actual callback/kernel boundary, not production profile equivalence; run through blobray-run"]
fn current_gain_callback_uses_rom_kernel_with_its_own_coefficients() {
    let archive = input(
        "OER_PHY_ARCHIVE",
        Some("d4218e359b9716c616cbf116172f44d9195d4f2e020fad73279067e92d08e580"),
    );
    let rom = input(
        "OER_PHY_ROM",
        Some("d01bde81d9b3806e37ef1d9ac3b58af4f5b3d91eeef4f44d20e79d6a9f227542"),
    );
    let entry = "phy_wifi_get_tx_tab_new";
    let vendor = image(&archive, Some(&rom), entry);
    let kernel = "phy_wifi_get_tx_gain";
    let rom_image = image(&rom, None, kernel);
    let extent = vendor.symbol_extent(kernel).expect("gain kernel body");
    assert_eq!(Some(extent.clone()), rom_image.symbol_extent(kernel));
    assert!(
        extent
            .into_iter()
            .all(|address| vendor.loaded_byte(address) == rom_image.loaded_byte(address))
    );
    let param = vendor.symbol_address("phy_param").unwrap();
    let map = MmioMap {
        registers: vec![],
        regions: vec![],
    };
    let mut cases = 0;
    for curve in [[0; 6], [127, 128, 255, 1, 2, 3], [255, 0, 127, 128, 7, 9]] {
        for channel in [1, 6, 11, 12, 13] {
            for (base, adjustment, correction) in [
                (0_u8, 0_u8, 0_i8),
                (127, 1, 127),
                (128, 255, -128),
                (255, 1, -17),
                (0, 255, 17),
            ] {
                for fill in [0x5a, 0xa5] {
                    let mut scenario = Scenario {
                        arguments: vec![channel, OUTPUT, OUTPUT + 32, OUTPUT + 96, 0],
                        private_stack_fill: Some(fill),
                        persistent_memory: vec![MemoryRange {
                            start: OUTPUT,
                            length: 160,
                        }],
                        ..Default::default()
                    };
                    scenario
                        .memory_initial
                        .extend((0..516).map(|i| (param + i, 0)));
                    scenario
                        .memory_initial
                        .extend((0..160).map(|i| (OUTPUT + i, fill)));
                    scenario.memory_initial.extend(
                        curve
                            .into_iter()
                            .enumerate()
                            .map(|(i, b)| (param + 241 + i as u32, b)),
                    );
                    scenario.memory_initial.extend([
                        (param + 291, base),
                        (param + 434, adjustment),
                        (param + 247, correction as u8),
                    ]);
                    let run = |entry, scenario| {
                        let result = execution::execute(&vendor, &map, entry, scenario)
                            .unwrap_or_else(|error| panic!("INCOMPLETE {entry}: {error}"));
                        assert!(
                            result.events.is_empty(),
                            "gain calculation touched hardware"
                        );
                        result
                    };
                    let callback = run(entry, scenario.clone());
                    let calls: Vec<_> = callback
                        .ordered_calls
                        .iter()
                        .filter(|call| call.symbol == kernel)
                        .collect();
                    assert_eq!(calls.len(), 1, "callback must execute the real kernel once");
                    let effective = base.wrapping_add(adjustment) as i8 as i32 as u32;
                    assert_eq!(
                        &calls[0].arguments[..4],
                        &[channel, param + 241, correction as i32 as u32, effective]
                    );
                    // Consume the actual coefficient sources observed at the
                    // callback's three copies. No vendor table is embedded in
                    // this fixture and the callback body is never substituted.
                    let tables: Vec<_> = callback
                        .ordered_calls
                        .iter()
                        .filter(|call| call.symbol == "memcpy")
                        .map(|call| {
                            assert_eq!(call.arguments[2], 36);
                            let source = call.arguments[1];
                            assert!((0..36).all(|i| vendor.loaded_byte(source + i).is_some()));
                            source
                        })
                        .collect();
                    assert_eq!(tables.len(), 3);
                    scenario.arguments = vec![
                        channel,
                        param + 241,
                        correction as i32 as u32,
                        effective,
                        tables[0],
                        tables[1],
                        tables[2],
                        OUTPUT,
                        OUTPUT + 32,
                        OUTPUT + 96,
                        0,
                    ];
                    let direct = run(kernel, scenario);
                    assert_eq!(
                        output(&callback),
                        output(&direct),
                        "DIFF current callback/kernel boundary"
                    );
                    cases += 1;
                }
            }
        }
    }
    println!(
        "CHARACTERIZED current gain callback: {cases} cases use unchanged ROM kernel, archive coefficient sources and additive signed-byte input; no production profile equivalence claim"
    );
}

#[test]
#[ignore = "requires authenticated current archive/ROM and fresh compiled production probe; run through blobray-run"]
fn compiled_gain_calculation_matches_current_profile() {
    let archive = input(
        "OER_PHY_ARCHIVE",
        Some("d4218e359b9716c616cbf116172f44d9195d4f2e020fad73279067e92d08e580"),
    );
    let rom = input(
        "OER_PHY_ROM",
        Some("d01bde81d9b3806e37ef1d9ac3b58af4f5b3d91eeef4f44d20e79d6a9f227542"),
    );
    let probe = input("OER_PHY_PROBE", None);
    let vendor_entry = "phy_wifi_get_tx_tab_new";
    let rust_entry = "open_phy_channel_trace_calculate_tx_gain";
    let vendor = image(&archive, Some(&rom), vendor_entry);
    let rust = image(&probe, None, rust_entry);
    let param = vendor.symbol_address("phy_param").unwrap();
    let map = MmioMap {
        registers: vec![],
        regions: vec![],
    };
    let mut cases = 0;
    for curve in [
        [0; 6],
        [0, 5, 10, 15, 20, 25],
        [127, 128, 255, 1, 2, 3],
        [255, 0, 127, 128, 7, 9],
    ] {
        for channel in [1, 2, 5, 6, 7, 10, 11, 12, 13] {
            for (base, correction) in [(0_i8, 0_i8), (-128, 127), (127, -128), (31, -17), (-31, 17)]
            {
                for fill in [0x5a, 0xa5] {
                    let context = format!(
                        "channel={channel} curve={curve:?} base={base} correction={correction} fill={fill:x}"
                    );
                    let mut scenario = Scenario {
                        private_stack_fill: Some(fill),
                        persistent_memory: vec![MemoryRange {
                            start: OUTPUT,
                            length: 160,
                        }],
                        ..Default::default()
                    };
                    scenario
                        .memory_initial
                        .extend((0..512).map(|i| (PARAM + i, 0)));
                    scenario
                        .memory_initial
                        .extend((0..160).map(|i| (OUTPUT + i, fill)));
                    scenario.memory_initial.extend(
                        curve
                            .into_iter()
                            .enumerate()
                            .map(|(i, b)| (PARAM + 241 + i as u32, b)),
                    );
                    scenario
                        .memory_initial
                        .extend([(PARAM + 291, base as u8), (PARAM + 247, correction as u8)]);
                    let mut v = scenario.clone();
                    v.arguments = vec![channel, OUTPUT, OUTPUT + 32, OUTPUT + 96, 0];
                    v.memory_initial.extend((0..516).map(|i| (param + i, 0)));
                    v.memory_initial.extend(
                        curve
                            .into_iter()
                            .enumerate()
                            .map(|(i, byte)| (param + 241 + i as u32, byte)),
                    );
                    v.memory_initial
                        .extend([(param + 291, base as u8), (param + 247, correction as u8)]);
                    scenario.arguments = vec![
                        channel,
                        PARAM + 241,
                        correction as i32 as u32,
                        base as i32 as u32,
                        OUTPUT,
                    ];
                    let run = |image, entry, scenario| {
                        let result = execution::execute(image, &map, entry, scenario)
                            .unwrap_or_else(|error| {
                                panic!("INCOMPLETE {entry} {context}: {error}")
                            });
                        assert!(result.events.is_empty(), "{entry} touched hardware");
                        output(&result)
                    };
                    assert_eq!(
                        run(&vendor, vendor_entry, v),
                        run(&rust, rust_entry, scenario),
                        "DIFF {context}"
                    );
                    cases += 1;
                }
            }
        }
    }
    println!("MATCH current gain profile arithmetic: {cases} cases, all 32 entries per case");
}
