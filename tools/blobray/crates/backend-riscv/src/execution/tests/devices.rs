//! Pluggable device-model execution regressions.

use std::sync::Arc;

use super::*;

fn radio_map(address: u32) -> MmioMap {
    MmioMap {
        registers: Vec::new(),
        regions: vec![crate::MmioRegion {
            name: "radio".to_owned(),
            start: address,
            end: address + 4,
            readable: true,
            writable: true,
        }],
    }
}

fn register_model(address: u32) -> DeviceModelSpec {
    DeviceModelSpec::W1c {
        id: "irq-status".to_owned(),
        address,
        width: 32,
        initial_value: 0x0f,
        clear_mask: 0x03,
        read_clear_mask: 0x0c,
    }
}

#[test]
fn register_model_applies_w1c_and_read_to_clear_without_hiding_bus_events() {
    let address = 0x2010_0000;
    let image = tiny_image(
        vec![
            0x93, 0x02, 0x05, 0x00, // addi t0, a0, 0
            0x23, 0xa0, 0xb2, 0x00, // sw a1, 0(t0)
            0x03, 0xa5, 0x02, 0x00, // lw a0, 0(t0)
            0x83, 0xa5, 0x02, 0x00, // lw a1, 0(t0)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        20,
    );
    let scenario = Scenario {
        arguments: vec![address, 1],
        device_models: vec![Arc::new(register_model(address))],
        ..Scenario::default()
    };

    let result = execute(&image, &radio_map(address), "test", scenario).unwrap();

    assert_eq!(result.return_value, 0x0e);
    assert_eq!(
        result
            .events
            .iter()
            .map(|event| match event {
                ExecutionEvent::Write { value, .. } => ("write", *value),
                ExecutionEvent::Read { value, .. } => ("read", *value),
                _ => unreachable!(),
            })
            .collect::<Vec<_>>(),
        [("write", 1), ("read", 0x0e), ("read", 0x02)]
    );
}

#[test]
fn device_models_reject_ambiguous_seed_and_range_ownership() {
    let address = 0x2010_0000;
    let image = tiny_image(vec![0x67, 0x80, 0x00, 0x00], 4);
    let mut seeded = Scenario {
        device_models: vec![Arc::new(register_model(address))],
        ..Scenario::default()
    };
    seeded.mmio_initial.insert(address, 0);
    assert!(
        execute(&image, &radio_map(address), "test", seeded)
            .unwrap_err()
            .to_string()
            .contains("overlaps an explicit MMIO seed")
    );

    let overlapping = Scenario {
        device_models: vec![
            Arc::new(register_model(address)),
            Arc::new(DeviceModelSpec::W1c {
                id: "other".to_owned(),
                address,
                width: 32,
                initial_value: 0,
                clear_mask: 0,
                read_clear_mask: 0,
            }),
        ],
        ..Scenario::default()
    };
    assert!(
        execute(&image, &radio_map(address), "test", overlapping)
            .unwrap_err()
            .to_string()
            .contains("overlaps another device range")
    );
}

#[test]
fn self_clearing_model_rejects_overlapping_write_semantics() {
    let address = 0x2010_0000;
    let image = tiny_image(vec![0x67, 0x80, 0x00, 0x00], 4);
    let scenario = Scenario {
        device_models: vec![Arc::new(DeviceModelSpec::SelfClearing {
            id: "command".to_owned(),
            address,
            width: 32,
            initial_value: 0,
            store_mask: 1,
            command_mask: 1,
        })],
        ..Scenario::default()
    };
    assert!(
        execute(&image, &radio_map(address), "test", scenario)
            .unwrap_err()
            .to_string()
            .contains("overlapping store and self-clearing masks")
    );
}

#[test]
fn sequence_model_reports_unconsumed_coverage_without_discarding_the_trace() {
    let address = 0x2010_0000;
    let image = tiny_image(
        vec![
            0x03, 0x25, 0x05, 0x00, // lw a0, 0(a0)
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        8,
    );
    let scenario = Scenario {
        arguments: vec![address],
        device_models: vec![Arc::new(DeviceModelSpec::SequenceRead {
            id: "ready".to_owned(),
            address,
            width: 32,
            values: vec![0, 1],
        })],
        ..Scenario::default()
    };

    let result = execute(&image, &radio_map(address), "test", scenario).unwrap();
    assert_eq!(result.events.len(), 1);
    assert_eq!(result.return_value, 0);
    assert_eq!(result.device_model_coverage.len(), 1);
    assert!(!result.device_model_coverage[0].coverage.complete);
    assert_eq!(
        result.device_model_coverage[0].coverage.reason.as_deref(),
        Some("1 sequence read values were not consumed")
    );
}

#[test]
fn indexed_bank_and_fifo_specs_round_trip_through_serde() {
    let specs = [
        DeviceModelSpec::Fifo {
            id: "rx".to_owned(),
            address: 0x2010_0000,
            width: 32,
            read_values: vec![1, 2],
            expected_writes: vec![3],
        },
        DeviceModelSpec::IndexedBank {
            id: "rf".to_owned(),
            index_address: 0x2010_0010,
            data_address: 0x2010_0014,
            width: 32,
            initial_values: vec![0x10, 0x20],
        },
    ];

    let json = serde_json::to_string(&specs).unwrap();
    let decoded: Vec<DeviceModelSpec> = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, specs);
}

#[test]
fn compiled_model_registry_requires_an_exact_unique_id() {
    let mut registry = DeviceModelRegistry::default();
    registry
        .register(
            "platform.ready-v1",
            Arc::new(DeviceModelSpec::ConstantRead {
                id: "ready".to_owned(),
                address: 0x2010_0000,
                width: 32,
                value: 1,
            }),
        )
        .unwrap();
    assert_eq!(
        registry
            .resolve("platform.ready-v1")
            .unwrap()
            .descriptor()
            .id,
        "ready"
    );
    assert!(registry.resolve("ready").is_err());
    assert!(
        registry
            .register(
                "platform.ready-v1",
                Arc::new(DeviceModelSpec::ConstantRead {
                    id: "other".to_owned(),
                    address: 0x2010_0004,
                    width: 32,
                    value: 0,
                }),
            )
            .unwrap_err()
            .to_string()
            .contains("duplicate")
    );
}
