//! Actual calibration-child execution, without modeled function completions.
//! Peripheral inputs describe readiness only; neither sequence is reimplemented.
use super::*;

const PBUS_STATUS: u32 = 0x2010_0890;

pub(super) fn pbus_map() -> MmioMap {
    MmioMap {
        registers: vec![],
        regions: [
            ("pbus", 0x2010_0880, 0x2010_0894),
            ("work-mode-enable", 0x2010_9c18, 0x2010_9c1c),
            ("work-mode-pulse", 0x2010_702c, 0x2010_7030),
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

pub(super) fn pbus_scenario(initial: u32, settle: bool, busy: usize, fill: u8) -> Scenario {
    let mut models: Vec<Arc<dyn open_radio_vendor_models_esp32s31::execution_model::DeviceModel>> =
        [
            (0x2010_0884, initial),
            (0x2010_088c, initial),
            (0x2010_9c18, u32::from(settle) * 2),
            (0x2010_702c, initial),
        ]
        .into_iter()
        .map(|(address, initial_value)| {
            Arc::new(DeviceModelSpec::SelfClearing {
                id: format!("pbus-register-{address:x}"),
                address,
                width: 32,
                initial_value,
                store_mask: u32::MAX,
                command_mask: 0,
            })
                as Arc<dyn open_radio_vendor_models_esp32s31::execution_model::DeviceModel>
        })
        .collect();
    models.push(Arc::new(DeviceModelSpec::SequenceRead {
        id: "pbus-ready".into(),
        address: PBUS_STATUS,
        width: 32,
        values: (0..12)
            .flat_map(|_| std::iter::repeat_n(0x8000_0000, busy).chain([0]))
            .collect(),
    }));
    Scenario {
        private_stack_fill: Some(fill),
        device_models: models,
        ..Default::default()
    }
}

// OER waits before each readiness observation; ROM spins. Keep every
// hardware access and both work-mode settle delays. Exclude only a one-us
// delay immediately followed by a PBus status read.
pub(super) fn pbus_effects(events: Vec<ExecutionEvent>) -> Vec<ExecutionEvent> {
    let mut events = events.into_iter().peekable();
    let mut effects = Vec::new();
    while let Some(event) = events.next() {
        if matches!(event, ExecutionEvent::DelayMicros(1))
            && matches!(events.peek(), Some(ExecutionEvent::Read { address, .. }) if *address == PBUS_STATUS)
        {
            continue;
        }
        effects.push(event);
    }
    effects
}

#[test]
#[ignore = "requires authenticated current PHY archive, ROM, and fresh production probe; run through blobray-run"]
fn compiled_pbus_clear_matches_complete_rom_child() {
    let archive = input(
        "OER_PHY_ARCHIVE",
        Some("d4218e359b9716c616cbf116172f44d9195d4f2e020fad73279067e92d08e580"),
    );
    let rom = input(
        "OER_PHY_ROM",
        Some("d01bde81d9b3806e37ef1d9ac3b58af4f5b3d91eeef4f44d20e79d6a9f227542"),
    );
    let probe = input("OER_PHY_PROBE", None);
    let vendor_entry = "phy_pbus_clear_reg";
    let rust_entry = "open_phy_calibration_trace_pbus_clear";
    // Load current archive first so an archive override cannot silently be
    // bypassed by comparing only the historical ROM.
    let vendor = image(&archive, Some(&rom), "phy_cal_param_track");
    let rust = image(&probe, None, rust_entry);
    for initial in [0, 0xa5a5_5a58, 0x5a5a_a5a4] {
        for settle in [false, true] {
            for busy in [0, 2] {
                for fill in [0x5a, 0xa5] {
                    let context =
                        format!("initial={initial:#x} settle={settle} busy={busy} stack={fill:#x}");
                    let run = |image, entry| {
                        let result = execution::execute(
                            image,
                            &pbus_map(),
                            entry,
                            pbus_scenario(initial, settle, busy, fill),
                        )
                        .unwrap_or_else(|error| panic!("INCOMPLETE {entry} {context}: {error}"));
                        for coverage in &result.device_model_coverage {
                            assert!(
                                coverage.coverage.complete,
                                "INCOMPLETE {context}: {coverage:?}"
                            );
                        }
                        result
                    };
                    let vendor = run(&vendor, vendor_entry);
                    let rust = run(&rust, rust_entry);
                    assert_eq!(rust.return_value, 0, "production child failed {context}");
                    let expected = pbus_effects(vendor.events);
                    let actual = pbus_effects(rust.events);
                    assert_eq!(expected.len(), actual.len(), "DIFF effect count {context}");
                    for (index, (expected, actual)) in expected.iter().zip(&actual).enumerate() {
                        assert_eq!(expected, actual, "DIFF PBus child {context} effect={index}");
                    }
                    println!(
                        "MATCH complete PBus child {context}; {} effects, readiness-wait timing excluded",
                        actual.len()
                    );
                }
            }
        }
    }
}

#[test]
#[ignore = "requires a fresh compiled production probe; run through blobray-run"]
fn compiled_pbus_timeout_preserves_unfinished_transaction() {
    let probe = input("OER_PHY_PROBE", None);
    let entry = "open_phy_calibration_trace_pbus_clear";
    let rust = image(&probe, None, entry);
    let mut scenario = pbus_scenario(0, true, 0, 0xa5);
    scenario.max_steps = 2_000_000;
    scenario.device_models.pop();
    scenario
        .device_models
        .push(Arc::new(DeviceModelSpec::ConstantRead {
            id: "pbus-stuck".into(),
            address: PBUS_STATUS,
            width: 32,
            value: 0x8000_0000,
        }));
    let result = execution::execute(&rust, &pbus_map(), entry, scenario)
        .expect("production must return its own bounded timeout");
    assert_eq!(
        result.return_value, 2,
        "timeout must not produce an accepted child completion"
    );
    // Debug-mode entry plus one command publication, with no transaction
    // clearing, next command, or work-mode restoration after timeout.
    assert_eq!(
        result
            .events
            .iter()
            .filter(|event| matches!(event, ExecutionEvent::Write { .. }))
            .count(),
        3
    );
    assert!(result.events.iter().all(|event| match event {
        ExecutionEvent::Read { region, .. } | ExecutionEvent::Write { region, .. } =>
            region == "pbus",
        _ => true,
    }));
    println!("PASS bounded PBus timeout; no completion or work-mode restoration");
}
