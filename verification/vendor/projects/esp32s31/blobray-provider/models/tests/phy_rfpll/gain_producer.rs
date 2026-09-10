//! Characterization of the optional certification/test library's power policy.
//! This executes vendor bodies; it does not implement a production power API.
use super::*;
use std::{
    io::Write,
    process::{Command, Stdio},
};

fn linked_image(phy: &Path, rftest: &Path, rom: &Path) -> ExecutableImage {
    // Combine unchanged members from authenticated inputs. Fixed local names
    // keep MRI commands independent of caller paths. No substitute callees.
    let directory = tempfile::tempdir().expect("temporary vendor composition");
    std::fs::copy(phy, directory.path().join("phy.a")).unwrap();
    std::fs::copy(rftest, directory.path().join("rftest.a")).unwrap();
    let mut child = Command::new("ar")
        .arg("-M")
        .current_dir(directory.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("archive composition requires ar with MRI support");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"create combined.a\naddlib phy.a\naddlib rftest.a\nsave\nend\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "archive composition: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    ExecutableImage::load_entry_with_roots(
        &directory.path().join("combined.a"),
        "phy_get_romfunc_addr",
        &["set_rate_power_index"],
        Some(rom),
    )
    .expect("load actual RF-test, PHY and ROM composition")
}

#[test]
#[ignore = "requires authenticated PHY, RF-test and ROM artifacts; vendor characterization, not production equivalence; run through blobray-run"]
fn current_rf_test_power_selection_produces_wifi_gain_adjustment() {
    let phy = input(
        "OER_PHY_ARCHIVE",
        Some("d4218e359b9716c616cbf116172f44d9195d4f2e020fad73279067e92d08e580"),
    );
    let rftest = input(
        "OER_PHY_RFTEST",
        Some("547786cd684eb9cd8902955176e9a9a7f113d8faa3f415e12108ed261f55a11e"),
    );
    let rom = input(
        "OER_PHY_ROM",
        Some("d01bde81d9b3806e37ef1d9ac3b58af4f5b3d91eeef4f44d20e79d6a9f227542"),
    );
    let image = linked_image(&phy, &rftest, &rom);
    let param = image.symbol_address("phy_param").unwrap();
    let map = MmioMap {
        registers: vec![],
        regions: [
            ("base", 0x2010_0408, 0x2010_040c),
            ("gain", 0x2010_0844, 0x2010_0854),
            ("test-mac-power", 0x2010_5500, 0x2010_5504),
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
    // Expected policy outputs are explicit, including signed remainder and
    // signed-byte wrap boundaries. They are not RF power measurements.
    let cases = [
        (80_i8, 0_i8, 0_u8, 20_i32, false),
        (80, 1, 3, 19, false),
        (80, 2, 2, 19, false),
        (80, 3, 1, 19, false),
        (80, 4, 0, 19, false),
        (80, -1, 1, 20, false),
        (80, -2, 2, 20, false),
        (80, -3, 3, 20, false),
        (80, -4, 0, 21, false),
        (80, -5, 1, 21, true),
        (80, -8, 4, 21, true),
        (84, -1, 1, 21, true),
        (80, -48, 0, -32, false),
        (80, 127, 1, -12, false),
        (80, -128, 0, -12, false),
    ];
    for (target, attenuation, adjustment, index, publish) in cases {
        for fill in [0x5a_u8, 0xa5] {
            let context = format!("target={target} attenuation={attenuation} fill={fill:x}");
            let mut session = execution::ExecutionSession::default();
            let init = session
                .execute(&image, &map, "phy_get_romfunc_addr", Scenario::default())
                .unwrap_or_else(|error| panic!("INCOMPLETE ROM callback registration: {error}"));
            assert!(init.events.is_empty());
            let mut scenario = Scenario {
                arguments: vec![0],
                private_stack_fill: Some(fill),
                mmio_initial: BTreeMap::from([(0x2010_0408, 0)]),
                device_models: [
                    ("gain-index", 0x2010_0844, 0),
                    ("test-mac-power", 0x2010_5500, 0xa5a5_a5a5),
                ]
                .into_iter()
                .map(|(id, address, initial_value)| {
                    Arc::new(DeviceModelSpec::SelfClearing {
                        id: id.into(),
                        address,
                        width: 32,
                        initial_value,
                        store_mask: u32::MAX,
                        command_mask: 0,
                    })
                        as Arc<dyn open_radio_vendor_models_esp32s31::execution_model::DeviceModel>
                })
                .collect(),
                ..Default::default()
            };
            scenario
                .memory_initial
                .extend((0..516).map(|i| (param + i, 0)));
            scenario
                .memory_initial
                .extend((80..98).map(|i| (param + i, target as u8)));
            scenario.memory_initial.extend([
                (param + 6, 84),
                (param + 8, attenuation as u8),
                (param + 284, 13),
                (param + 434, 0x5a),
            ]);
            let result = session
                .execute(&image, &map, "set_rate_power_index", scenario)
                .unwrap_or_else(|error| panic!("INCOMPLETE {context}: {error}"));
            assert_eq!(
                session.byte(&image, param + 434),
                Some(adjustment),
                "DIFF adjustment {context}"
            );
            let gains: Vec<_> = result
                .ordered_calls
                .iter()
                .filter(|call| call.symbol == "phy_wifi_set_tx_gain_new")
                .collect();
            assert_eq!(
                gains.len(),
                usize::from(publish),
                "DIFF gain regeneration {context}"
            );
            if let Some(gain) = gains.first() {
                assert_eq!(&gain.arguments[..2], &[13, 0]);
            }
            let powers: Vec<_> = result
                .ordered_calls
                .iter()
                .filter(|call| call.symbol == "mac_power_set")
                .collect();
            assert_eq!(powers.len(), 1, "power publication {context}");
            assert_eq!(powers[0].arguments[0], index as u32, "DIFF index {context}");
            let writes: Vec<_> = result
                .events
                .iter()
                .filter_map(|event| match event {
                    ExecutionEvent::Write {
                        address: 0x2010_5500,
                        value,
                        ..
                    } => Some(*value),
                    _ => None,
                })
                .collect();
            let index = index as u32 & 63;
            let first = (0xa5a5_a5a5 & !63) | index;
            assert_eq!(
                writes,
                vec![first, (first & !(63 << 8)) | (index << 8)],
                "DIFF MAC effects {context}"
            );
            println!(
                "CHARACTERIZED RF-test power policy {context}: adjustment={adjustment} index={index} gain-publication={publish}"
            );
        }
    }
}
