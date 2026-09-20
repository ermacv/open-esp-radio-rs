//! Compare the cold-init queue protection edge, not the complete HE initializer.
//! Other initializer effects and the six pre-edge child calls are outside this
//! property. The four queue reads/writes execute in the real vendor parent and
//! in the same compiled PAC helper called by production initialization.

use open_radio_vendor_models_esp32s31::{
    MmioMap, MmioRegion,
    execution::{self, ExecutableImage, ExecutionEvent, ModeledCallResponse, Scenario},
    execution_model::ExecutionGoal,
};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::PathBuf};

fn input(name: &str, expected: Option<&str>) -> PathBuf {
    let path = PathBuf::from(std::env::var_os(name).unwrap_or_else(|| panic!("missing {name}")));
    let hash = format!("{:x}", Sha256::digest(std::fs::read(&path).unwrap()));
    if let Some(expected) = expected {
        assert_eq!(hash, expected, "incorrect {name} artifact");
    }
    println!("input {name}: {hash}");
    path
}

// These addresses define the observed vendor interface, not PAC layout tests.
const PROTECTION: [u32; 4] = [0x2010_4d64, 0x2010_4d54, 0x2010_4d44, 0x2010_4d34];

fn protection_events(events: Vec<ExecutionEvent>) -> Vec<ExecutionEvent> {
    events
        .into_iter()
        .filter(|event| match event {
            ExecutionEvent::Read { address, .. } | ExecutionEvent::Write { address, .. } => {
                PROTECTION.contains(address)
            }
            _ => false,
        })
        .collect()
}

#[test]
#[ignore = "requires authenticated libpp and a fresh production probe; run through blobray-run"]
fn compiled_cold_init_cts_reset_preserves_vendor_rts_state() {
    let archive = input(
        "OER_TX_ARCHIVE",
        Some("f863c65c3ed89cf5d2a2cbe0d6bca3b783ca35788a704bb68e13958e4b94958e"),
    );
    let probe = input("OER_TX_PROBE", None);
    let vendor_entry = "hal_he_init";
    let rust_entry = "open_tx_protection_initialize_cts";
    let vendor = ExecutableImage::load_entry(&archive, vendor_entry).unwrap();
    let rust = ExecutableImage::load_entry(&probe, rust_entry).unwrap();
    let mmio = MmioMap {
        registers: vec![],
        regions: vec![MmioRegion {
            name: "wifi-initialization".into(),
            start: 0x2010_4000,
            end: 0x2010_6000,
            readable: true,
            writable: true,
        }],
    };
    for initial in [
        0,
        0x4000_0000,
        0x8000_0000,
        0xc000_0000,
        0xa5a5_5a5a,
        u32::MAX,
    ] {
        let mut retained: BTreeMap<u32, u32> = (0x2010_4000..0x2010_6000)
            .step_by(4)
            .map(|a| (a, 0))
            .collect();
        for (queue, address) in PROTECTION.into_iter().enumerate() {
            // Distinct queue state also catches reversed physical traversal.
            retained.insert(address, initial ^ queue as u32);
        }
        let mut scenario = Scenario {
            mmio_initial: retained.clone(),
            goal: ExecutionGoal::ObserveCall {
                symbol: "hal_he_set_bcast_ru".into(),
            },
            ..Default::default()
        };
        for symbol in [
            "hal_init_bf",
            "hal_init_tb_tx",
            "hal_init_tx_pwr",
            "dbg_read_tx_power",
            "hal_he_set_ersu",
            "hal_set_tx_min_pwr",
        ] {
            scenario.call_responses.insert(
                symbol.into(),
                [ModeledCallResponse {
                    return_words: [Some(0), None],
                    outputs: vec![],
                    allocation: None,
                }]
                .into(),
            );
        }
        let expected = protection_events(
            execution::execute(&vendor, &mmio, vendor_entry, scenario)
                .expect("INCOMPLETE vendor queue reset")
                .events,
        );
        let actual = execution::execute(
            &rust,
            &mmio,
            rust_entry,
            Scenario {
                mmio_initial: retained,
                ..Default::default()
            },
        )
        .expect("INCOMPLETE production queue reset")
        .events;
        assert_eq!(
            expected.len(),
            8,
            "four complete vendor queue RMWs required"
        );
        assert_eq!(actual, expected, "DIFF initial={initial:#x}");
        println!("MATCH cold-init CTS queue reset initial={initial:#x}");
    }
}
