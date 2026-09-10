//! Current thermal child comparison. Vendor busy/grant policy remains separate
//! from OER's physical owner; no simultaneous-client admission claim is made.
use super::*;
use open_radio_vendor_models_esp32s31::{
    execution::ExecutionTimelineEvent, execution_model::MemoryRange,
};

#[test]
#[ignore = "requires authenticated PHY archive, ROM and a fresh compiled probe; run through blobray-run"]
fn vendor_parent_guards_and_reference_commit_are_explicit() {
    let archive = input(
        "OER_PHY_ARCHIVE",
        Some("d4218e359b9716c616cbf116172f44d9195d4f2e020fad73279067e92d08e580"),
    );
    let rom = input(
        "OER_PHY_ROM",
        Some("d01bde81d9b3806e37ef1d9ac3b58af4f5b3d91eeef4f44d20e79d6a9f227542"),
    );
    let entry = "phy_rfpll_cap_track_new";
    let vendor = image(&archive, Some(&rom), entry);
    let probe = input("OER_PHY_PROBE", None);
    let rust_entry = "open_phy_rfpll_trace_track";
    let rust = image(&probe, None, rust_entry);
    let param = vendor.symbol_address("phy_param").unwrap();
    // Expected observations are explicit cases, not a shadow threshold policy.
    for (name, current, previous, flags, threshold, busy, executes) in [
        ("below-default", 114_i16, 100_i16, 0, 0, 0, false),
        ("exact-default", 115, 100, 0, 0, 0, true),
        ("cooling-below", 86, 100, 0, 0, 0, false),
        ("cooling-exact", 85, 100, 0, 0, 0, true),
        ("busy", 200, 100, 0, 0, 1, false),
        ("override-below", 119, 100, 1, 20, 0, false),
        ("override-exact", 120, 100, 1, 20, 0, true),
        ("calibration-flag-only", 115, 100, 2, 255, 0, true),
        ("zero-override", 100, 100, 1, 0, 0, true),
        ("zero-override-busy", 100, 100, 1, 0, 1, false),
        ("negative-temperatures", -85, -100, 0, 0, 0, true),
        ("full-signed-span", i16::MAX, i16::MIN, 0, 0, 0, true),
    ] {
        let mut scenario = Scenario {
            arguments: vec![0],
            private_stack_fill: Some(0xa5),
            device_models: if executes {
                vec![Arc::new(Rfpll::new(100, vec![3; 20], 0).unwrap())]
            } else {
                vec![]
            },
            mmio_initial: BTreeMap::from([
                (0x2010_0028, 0x2582_4e58),
                (0x2010_0030, 0x1234_5678),
                (0x2010_d800, 0x9876_5432),
            ]),
            observed_memory: vec![
                MemoryRange {
                    start: param + 304,
                    length: 2,
                },
                MemoryRange {
                    start: param + 404,
                    length: 1,
                },
                MemoryRange {
                    start: param + 510,
                    length: 2,
                },
            ],
            ..Default::default()
        };
        for (offset, bytes) in [
            (0, current.to_le_bytes().to_vec()),
            (9, vec![0]),
            (304, previous.to_le_bytes().to_vec()),
            (404, vec![busy]),
            (432, vec![flags, threshold]),
            (510, 0xa004_u16.to_le_bytes().to_vec()),
        ] {
            scenario.memory_initial.extend(
                bytes
                    .into_iter()
                    .enumerate()
                    .map(|(i, byte)| (param + offset + i as u32, byte)),
            );
        }
        let mut rust_scenario = scenario.clone();
        rust_scenario.memory_initial.clear();
        rust_scenario.observed_memory.clear();
        rust_scenario.arguments = vec![
            0,
            current as i32 as u32,
            previous as i32 as u32,
            if flags & 1 != 0 {
                u32::from(threshold)
            } else {
                u32::MAX
            },
            13,
        ];
        let mut final_bytes = scenario.memory_initial.clone();
        let result = execution::execute(&vendor, &frequency_map(), entry, scenario)
            .unwrap_or_else(|error| panic!("INCOMPLETE {name}: {error}"));
        for coverage in &result.device_model_coverage {
            assert!(
                coverage.coverage.complete,
                "INCOMPLETE {name}: {coverage:?}"
            );
        }
        assert_eq!(
            !result.events.is_empty(),
            executes,
            "hardware admission {name}"
        );
        if executes {
            let reference = result
                .timeline
                .iter()
                .position(|event| {
                    matches!(event,
                        ExecutionTimelineEvent::RamWrite { address, .. } if *address == param + 304
                    )
                })
                .expect("reference publication");
            let flags = result
                .timeline
                .iter()
                .position(|event| {
                    matches!(event,
                        ExecutionTimelineEvent::RamWrite { address, .. } if *address == param + 510
                    )
                })
                .expect("result publication");
            let restore = result
                .timeline
                .iter()
                .rposition(|event| {
                    matches!(
                        event,
                        ExecutionTimelineEvent::Observable(ExecutionEvent::Write {
                            address: 0x2010_0028,
                            ..
                        })
                    )
                })
                .expect("hardware control restoration");
            let release = result.timeline.iter().position(|event| matches!(event,
                ExecutionTimelineEvent::RamWrite { address, value: 0, .. } if *address == param + 404
            )).expect("busy release");
            assert!(
                reference < flags && flags < restore && restore < release,
                "vendor publication/restoration order {name}"
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
            word(304) as i16,
            if executes { current } else { previous },
            "reference {name}"
        );
        assert_eq!(
            word(510),
            if executes { 0xa005 } else { 0xa004 },
            "flags {name}"
        );
        assert_eq!(final_bytes[&(param + 404)], busy, "busy state {name}");
        if busy == 0 {
            // OER is invoked only after its physical admission protocol, not
            // given a duplicate software busy flag inside the PHY algorithm.
            let rust = execution::execute(&rust, &frequency_map(), rust_entry, rust_scenario)
                .unwrap_or_else(|error| panic!("INCOMPLETE Rust {name}: {error}"));
            for coverage in &rust.device_model_coverage {
                assert!(
                    coverage.coverage.complete,
                    "INCOMPLETE Rust {name}: {coverage:?}"
                );
            }
            assert_eq!(
                rust.return_value,
                u32::from(word(304)) | (u32::from(executes) << 16),
                "DIFF thermal outcome {name}"
            );
            assert_eq!(
                envelope_events(result.events),
                envelope_events(rust.events),
                "DIFF thermal effects {name}"
            );
            println!("MATCH compiled thermal child {name}");
        }
        println!("PASS vendor policy {name}: executes={executes}; reference and flags checked");
    }
}
