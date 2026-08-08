use std::sync::Arc;

use super::*;

#[test]
fn w1c_and_read_clear_semantics_are_owned_by_the_neutral_model() {
    let model = DeviceModelSpec::W1c {
        id: "status".to_owned(),
        address: 0x4000,
        width: 32,
        initial_value: 0b1111,
        clear_mask: 0b0110,
        read_clear_mask: 0b1000,
    };
    let mut instance = model.instantiate().unwrap();

    assert_eq!(instance.read(0x4000, 32).unwrap(), 0b1111);
    assert_eq!(instance.read(0x4000, 32).unwrap(), 0b0111);
    instance.write(0x4000, 32, 0b0010).unwrap();
    assert_eq!(instance.read(0x4000, 32).unwrap(), 0b0101);
}

#[test]
fn scripted_model_reports_unconsumed_state_as_incomplete() {
    let model = DeviceModelSpec::SequenceRead {
        id: "poll".to_owned(),
        address: 0x5000,
        width: 32,
        values: vec![0, 1],
    };
    let mut instance = model.instantiate().unwrap();
    assert_eq!(instance.read(0x5000, 32).unwrap(), 0);

    let coverage = instance.finish().unwrap();
    assert!(!coverage.complete);
    assert_eq!(
        coverage.reason.as_deref(),
        Some("1 sequence read values were not consumed")
    );
}

#[test]
fn compiled_registry_rejects_ambiguous_ids() {
    let mut registry = DeviceModelRegistry::default();
    let model: Arc<dyn DeviceModel> = Arc::new(DeviceModelSpec::ConstantRead {
        id: "clock".to_owned(),
        address: 0x6000,
        width: 32,
        value: 1,
    });

    registry.register("clock", model.clone()).unwrap();
    assert!(registry.register("clock", model).is_err());
    assert_eq!(registry.ids().collect::<Vec<_>>(), vec!["clock"]);
}
