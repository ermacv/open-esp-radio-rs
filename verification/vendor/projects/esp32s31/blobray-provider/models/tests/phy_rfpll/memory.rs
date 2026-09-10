use super::*;

fn retained_word(address: u32, initial_value: u32) -> Arc<DeviceModelSpec> {
    Arc::new(DeviceModelSpec::SelfClearing {
        id: format!("frequency-{address:x}"),
        address,
        width: 32,
        initial_value,
        store_mask: u32::MAX,
        command_mask: 0,
    })
}

#[test]
#[ignore = "requires authenticated PHY archive, ROM, and compiled production probe; run through blobray-run"]
fn compiled_nonzero_correction_matches_all_memory_transactions() {
    let archive = input(
        "OER_PHY_ARCHIVE",
        Some("d4218e359b9716c616cbf116172f44d9195d4f2e020fad73279067e92d08e580"),
    );
    let rom = input(
        "OER_PHY_ROM",
        Some("d01bde81d9b3806e37ef1d9ac3b58af4f5b3d91eeef4f44d20e79d6a9f227542"),
    );
    let probe = input("OER_PHY_PROBE", None);
    let vendor_entry = "phy_rfpll_cap_track_new";
    let rust_entry = "open_phy_rfpll_trace_maintain";
    let vendor = image(&archive, Some(&rom), vendor_entry);
    let rust = image(&probe, None, rust_entry);
    let param = vendor.symbol_address("phy_param").unwrap();
    for (name, statuses, delta, boundary_word) in [
        ("positive", [vec![1; 2], vec![0; 10]].concat(), 5, None),
        ("negative", [vec![0; 10], vec![2; 2]].concat(), -5, None),
        (
            "underflow",
            [vec![0; 10], vec![2; 2]].concat(),
            -5,
            Some(0x00aa_bf00),
        ),
        (
            "overflow",
            [vec![1; 2], vec![0; 10]].concat(),
            5,
            Some(0x00aa_ffff),
        ),
    ] {
        for channel in [1_u16, 13, 14, 2412, 2484] {
            for fill in [0x5a, 0xa5] {
                let context = format!("{name} channel={channel} stack={fill:#x}");
                // Distinct caller-supplied table contents exercise every read
                // and preserve unrelated bits. No adjustment algorithm lives
                // in this peripheral scenario.
                let words = (0..85_u32)
                    .map(|index| {
                        boundary_word.unwrap_or(0x0055_0000 | (index << 8) | (100 + index))
                    })
                    .collect();
                let scenario = Scenario {
                    arguments: vec![0, u32::from(channel)],
                    private_stack_fill: Some(fill),
                    device_models: vec![
                        Arc::new(Rfpll::new(100, statuses.clone(), 0).unwrap()),
                        retained_word(0x2010_001c, 0x4128_0055),
                        retained_word(0x2010_0020, 0xa5a4_5678),
                        // Exact table layout installed by production cold init.
                        retained_word(0x2010_0028, 0x2582_4e58),
                        retained_word(0x2010_002c, 0),
                        retained_word(0x2010_0030, 0x1234_5678),
                        Arc::new(DeviceModelSpec::SequenceRead {
                            id: "frequency-memory-data".into(),
                            address: 0x2010_0040,
                            width: 32,
                            values: words,
                        }),
                        Arc::new(DeviceModelSpec::SequenceRead {
                            id: "sdm-observation".into(),
                            address: 0x2010_d800,
                            width: 32,
                            values: vec![0x9876_5432],
                        }),
                    ],
                    ..Default::default()
                };
                let mut vendor_scenario = scenario.clone();
                for (address, bytes) in [
                    (param, 100_i16.to_le_bytes().to_vec()),
                    (param + 304, 0_i16.to_le_bytes().to_vec()),
                    (param + 9, vec![0]),
                    (param + 404, vec![0]),
                    (param + 432, vec![0]),
                    (0x2f07_fc40, 0x3fff_0000_u32.to_le_bytes().to_vec()),
                    (0x3fff_011c, channel.to_le_bytes().to_vec()),
                ] {
                    vendor_scenario.memory_initial.extend(
                        bytes
                            .into_iter()
                            .enumerate()
                            .map(|(i, byte)| (address + i as u32, byte)),
                    );
                }
                let run = |image, entry, scenario| {
                    let result = execution::execute(image, &frequency_map(), entry, scenario)
                        .unwrap_or_else(|error| panic!("INCOMPLETE {entry} {context}: {error}"));
                    for coverage in &result.device_model_coverage {
                        assert!(
                            coverage.coverage.complete,
                            "INCOMPLETE {context}: {coverage:?}"
                        );
                    }
                    result
                };
                let vendor = run(&vendor, vendor_entry, vendor_scenario);
                let rust = run(&rust, rust_entry, scenario);
                assert_eq!(rust.return_value as i32, delta);
                let mut vendor_events = envelope_events(vendor.events);
                let rust_events = envelope_events(rust.events);
                // Vendor queries the installed table layout once; production
                // owns that fixed layout. This projection permits that one
                // additional read, never a missing/reordered memory transaction.
                let layout_read = vendor_events
                    .iter()
                    .enumerate()
                    .filter_map(|(i, event)| {
                        matches!(
                            event,
                            ExecutionEvent::Read {
                                address: 0x2010_0028,
                                ..
                            }
                        )
                        .then_some(i)
                    })
                    .nth(1)
                    .expect("vendor layout query");
                let removed = vendor_events.remove(layout_read);
                assert!(matches!(
                    removed,
                    ExecutionEvent::Read {
                        value: 0x2582_4e5a,
                        ..
                    }
                ));
                assert_eq!(
                    vendor_events.len(),
                    rust_events.len(),
                    "DIFF effect count {context}"
                );
                for (index, (vendor, rust)) in vendor_events.iter().zip(&rust_events).enumerate() {
                    assert_eq!(
                        vendor, rust,
                        "DIFF memory transaction {context} effect {index}"
                    );
                }
                println!("MATCH all memory transactions and channel restoration {context}");
            }
        }
    }
}
