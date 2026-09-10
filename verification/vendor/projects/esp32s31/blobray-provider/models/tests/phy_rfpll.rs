//! Private-input comparison of compiled PHY tracking and calibration. Each
//! profile states its parent/child scope; grant, timing and RF equivalence are
//! not implied by software-effect matches.

use open_radio_vendor_models_esp32s31::{
    MmioMap, MmioRegion,
    execution::{self, ExecutableImage, ExecutionEvent, Scenario},
    execution_model::DeviceModelSpec,
    phy_i2c::{PORT_BASE, Rfpll},
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

#[path = "phy_rfpll/memory.rs"]
mod memory;

#[path = "phy_rfpll/graph.rs"]
mod graph;

#[path = "phy_rfpll/calibration.rs"]
mod calibration;

#[path = "phy_rfpll/dcode.rs"]
mod dcode;

#[path = "phy_rfpll/rx_gain.rs"]
mod rx_gain;

#[path = "phy_rfpll/channel.rs"]
mod channel;

#[path = "phy_rfpll/tx_gain.rs"]
mod tx_gain;

#[path = "phy_rfpll/gain_state.rs"]
mod gain_state;

#[path = "phy_rfpll/gain_producer.rs"]
mod gain_producer;

#[path = "phy_rfpll/gain_calculation.rs"]
mod gain_calculation;

#[path = "phy_rfpll/bluetooth_gain.rs"]
mod bluetooth_gain;

#[path = "phy_rfpll/tx_dc_pwdet.rs"]
mod tx_dc_pwdet;

#[path = "phy_rfpll/parent.rs"]
mod parent;

// These are oracle scenario inputs, not assertions about generated PAC layout.
// Compare actual vendor and production effects with identical initial values.
fn frequency_map() -> MmioMap {
    MmioMap {
        registers: vec![],
        regions: vec![
            MmioRegion {
                name: "phy-i2c".into(),
                start: PORT_BASE,
                end: PORT_BASE + 0x24,
                readable: true,
                writable: true,
            },
            MmioRegion {
                name: "phy-frequency".into(),
                start: 0x2010_0000,
                end: 0x2010_0044,
                readable: true,
                writable: true,
            },
            MmioRegion {
                name: "sdm-counter".into(),
                start: 0x2010_d800,
                end: 0x2010_d804,
                readable: true,
                writable: false,
            },
        ],
    }
}

fn envelope_events(events: Vec<ExecutionEvent>) -> Vec<ExecutionEvent> {
    events
        .into_iter()
        .filter(|event| match event {
            ExecutionEvent::Read { address, .. } => {
                !(PORT_BASE..PORT_BASE + 0x24).contains(address)
            }
            ExecutionEvent::Write { address, .. } => {
                !(PORT_BASE..PORT_BASE + 0x24).contains(address)
                    || *address == PORT_BASE
                    || *address == PORT_BASE + 4
            }
            // Production's bounded I2C completion wait requests one-microsecond
            // waits; the ROM polls. Their timing is outside this projection.
            ExecutionEvent::DelayMicros(1) => false,
            _ => true,
        })
        .collect()
}

#[test]
#[ignore = "requires authenticated PHY archive, ROM, and a fresh compiled production probe; run through blobray-run"]
fn compiled_zero_delta_maintenance_matches_frequency_envelope() {
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
    let param = vendor
        .symbol_address("phy_param")
        .expect("vendor parameter object");
    // Exercise the admitted vendor branch, with diagnostics disabled and the
    // archive's real weak grant functions. Do not substitute grant behavior.
    let mut parameters = BTreeMap::new();
    for (offset, bytes) in [(0, 100_i16.to_le_bytes()), (304, 0_i16.to_le_bytes())] {
        for (index, byte) in bytes.into_iter().enumerate() {
            parameters.insert(param + offset + index as u32, byte);
        }
    }
    for (offset, value) in [(9, 0), (404, 0), (432, 0)] {
        parameters.insert(param + offset, value);
    }
    for status in [0, 3] {
        for initial in [0x2582_4e58, 0xa5a5_5a5b] {
            for fill in [0x5a, 0xa5] {
                let context = format!("status={status} mode={initial:#x} stack={fill:#x}");
                let scenario = Scenario {
                    arguments: vec![0, 13],
                    private_stack_fill: Some(fill),
                    device_models: vec![
                        Arc::new(Rfpll::new(100, vec![status; 20], 0).unwrap()),
                        Arc::new(DeviceModelSpec::SelfClearing {
                            id: "frequency-control".into(),
                            address: 0x2010_0028,
                            width: 32,
                            initial_value: initial,
                            store_mask: u32::MAX,
                            command_mask: 0,
                        }),
                        Arc::new(DeviceModelSpec::SequenceRead {
                            id: "i2c-number-observation".into(),
                            address: 0x2010_0030,
                            width: 32,
                            values: vec![0x1234_5678],
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
                vendor_scenario.memory_initial = parameters.clone();
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
                assert_eq!(
                    rust.return_value, 0,
                    "production correction failed {context}"
                );
                let poll_waits = rust
                    .events
                    .iter()
                    .filter(|event| matches!(event, ExecutionEvent::DelayMicros(1)))
                    .count();
                let vendor_events = envelope_events(vendor.events);
                let rust_events = envelope_events(rust.events);
                assert_eq!(
                    vendor_events.len(),
                    rust_events.len(),
                    "DIFF effect count {context}"
                );
                for (index, (vendor, rust)) in vendor_events.iter().zip(&rust_events).enumerate() {
                    assert_eq!(
                        vendor, rust,
                        "DIFF frequency envelope {context} effect {index}"
                    );
                }
                println!(
                    "MATCH frequency envelope {context}; additional Rust I2C poll waits={poll_waits}"
                );
            }
        }
    }
}

#[test]
#[ignore = "requires a fresh compiled production probe; run through blobray-run"]
fn compiled_maintenance_timeout_does_not_restore_hardware_control() {
    let probe = input("OER_PHY_PROBE", None);
    let entry = "open_phy_rfpll_trace_maintain";
    let rust = image(&probe, None, entry);
    let result = execution::execute(
        &rust,
        &frequency_map(),
        entry,
        Scenario {
            arguments: vec![0, 13],
            private_stack_fill: Some(0x5a),
            max_steps: 2_000_000,
            mmio_initial: BTreeMap::from([
                (0x2010_0028, 0x2582_4e58),
                (0x2010_0030, 0x1234_5678),
                (0x2010_d800, 0x9876_5432),
            ]),
            // Keep the first I2C command pending beyond the production limit.
            device_models: vec![Arc::new(Rfpll::new(100, vec![], u16::MAX).unwrap())],
            ..Default::default()
        },
    )
    .expect("production must return its bounded timeout, not an executor error");
    assert_eq!(result.return_value, i32::MIN as u32);
    let writes = result
        .events
        .iter()
        .filter(|event| {
            matches!(event,
                ExecutionEvent::Write { region, .. } if region == "phy-frequency"
            )
        })
        .count();
    assert_eq!(
        writes, 1,
        "failure must retain software control without a restoration write"
    );
    assert!(
        result
            .device_model_coverage
            .iter()
            .any(|outcome| !outcome.coverage.complete),
        "the pending hardware operation must remain explicit"
    );
    println!(
        "PASS bounded production timeout retains software frequency control; peripheral remains pending"
    );
}

fn input(name: &str, expected_hash: Option<&str>) -> PathBuf {
    let path = PathBuf::from(std::env::var_os(name).unwrap_or_else(|| panic!("missing {name}")));
    let hash = format!(
        "{:x}",
        Sha256::digest(std::fs::read(&path).expect("read input artifact"))
    );
    if let Some(expected) = expected_hash {
        assert_eq!(hash, expected, "wrong {name} artifact");
    }
    println!("input {name}: {hash}");
    path
}

fn image(artifact: &Path, companion: Option<&Path>, entry: &str) -> ExecutableImage {
    let mut image = ExecutableImage::load_entry(artifact, entry).expect("load executable");
    if let Some(companion) = companion {
        image.add_companion(companion).expect("load ROM");
    }
    image
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SearchEffects {
    delta: i32,
    commands: Vec<(u32, u32)>,
    settles: usize,
}

fn run(
    image: &ExecutableImage,
    entry: &str,
    cap: u16,
    statuses: &[u8],
    fill: u8,
    busy: u16,
) -> Result<SearchEffects, String> {
    let scenario = Scenario {
        private_stack_fill: Some(fill),
        device_models: vec![Arc::new(Rfpll::new(cap, statuses.to_vec(), busy).unwrap())],
        ..Default::default()
    };
    let mmio = MmioMap {
        registers: vec![],
        regions: vec![MmioRegion {
            name: "phy-i2c".into(),
            start: PORT_BASE,
            end: PORT_BASE + 0x24,
            readable: true,
            writable: true,
        }],
    };
    let result = execution::execute(image, &mmio, entry, scenario).map_err(|e| e.to_string())?;
    for outcome in &result.device_model_coverage {
        if !outcome.coverage.complete {
            return Err(format!("incomplete device: {outcome:?}"));
        }
    }
    let commands = result
        .events
        .iter()
        .filter_map(|event| match event {
            ExecutionEvent::Write { address, value, .. }
                if *address == PORT_BASE || *address == PORT_BASE + 4 =>
            {
                Some((*address, *value))
            }
            _ => None,
        })
        .collect();
    let settles = result
        .events
        .iter()
        .filter(|event| matches!(event, ExecutionEvent::DelayMicros(5)))
        .count();
    Ok(SearchEffects {
        delta: result.return_value as i32,
        commands,
        settles,
    })
}

#[test]
#[ignore = "requires authenticated PHY archive, ROM, and a fresh compiled production probe; run through blobray-run"]
fn compiled_search_matches_i2c_commands_and_requested_settles() {
    let archive = input(
        "OER_PHY_ARCHIVE",
        Some("d4218e359b9716c616cbf116172f44d9195d4f2e020fad73279067e92d08e580"),
    );
    let rom = input(
        "OER_PHY_ROM",
        Some("d01bde81d9b3806e37ef1d9ac3b58af4f5b3d91eeef4f44d20e79d6a9f227542"),
    );
    let probe = input("OER_PHY_PROBE", None);
    let vendor = image(&archive, Some(&rom), "phy_rfpll_cap_init_cal_new");
    let rust = image(&probe, None, "open_phy_rfpll_trace_search");
    let cases = [
        ("all-accepted", 100, vec![0; 20]),
        ("lowest-initial-cap", 0, vec![0; 20]),
        ("highest-initial-cap", 511, vec![0; 20]),
        ("no-accepted", 100, vec![3; 20]),
        ("positive", 100, [vec![1; 2], vec![0; 10]].concat()),
        ("negative", 300, [vec![0; 10], vec![2; 2]].concat()),
        ("nonconsecutive", 100, vec![1, 0, 1, 2, 3, 2]),
        ("signed-wrap", 0, vec![1, 0, 1, 2, 2]),
        (
            "opposite-boundaries",
            100,
            [vec![2; 10], vec![1; 10]].concat(),
        ),
    ];
    for (name, cap, statuses) in cases {
        let mut baseline = None;
        for fill in [0x5a, 0xa5] {
            for busy in [0, 1] {
                let context = format!("{name} stack={fill:#x} busy={busy}");
                let vendor = run(
                    &vendor,
                    "phy_rfpll_cap_init_cal_new",
                    cap,
                    &statuses,
                    fill,
                    busy,
                )
                .unwrap_or_else(|error| panic!("INCOMPLETE vendor {context}: {error}"));
                let rust = run(
                    &rust,
                    "open_phy_rfpll_trace_search",
                    cap,
                    &statuses,
                    fill,
                    busy,
                )
                .unwrap_or_else(|error| panic!("INCOMPLETE Rust {context}: {error}"));
                assert_eq!(vendor, rust, "DIFF {context}");
                if let Some(baseline) = &baseline {
                    assert_eq!(&vendor, baseline, "padding/poll dependence {context}");
                } else {
                    baseline = Some(vendor.clone());
                }
                println!(
                    "MATCH I2C-command boundary {context}: delta={} commands={} settles={}",
                    vendor.delta,
                    vendor.commands.len(),
                    vendor.settles
                );
            }
        }
    }
}

#[path = "phy_rfpll/combined.rs"]
mod combined;
