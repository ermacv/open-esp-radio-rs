use super::*;

#[derive(Default)]
struct RecordingHardware {
    starts: usize,
    stops: usize,
}

impl ApTsfHardware for RecordingHardware {
    fn reset_and_start_access_point_tsf(&mut self, _: AccessPointTsfWrite) {
        self.starts += 1;
    }

    fn stop_access_point_tsf(&mut self, _: AccessPointTsfWrite) {
        self.stops += 1;
    }
}

#[test]
fn the_owner_publishes_exactly_one_start_and_stop_edge() {
    let mut hardware = RecordingHardware::default();
    let mut owner = AccessPointTsf::new(1);

    owner.restart(&mut hardware);
    owner.stop(&mut hardware);

    assert_eq!(hardware.starts, 1);
    assert_eq!(hardware.stops, 1);
}

#[test]
fn every_restart_and_stop_starts_a_new_generation() {
    let mut hardware = RecordingHardware::default();
    let mut owner = AccessPointTsf::new(7);
    let created = owner.generation();
    owner.restart(&mut hardware);
    let started = owner.generation();
    assert_ne!(started, created);
    owner.stop(&mut hardware);
    assert_ne!(owner.generation(), started);
    owner.restart(&mut hardware);
    assert_ne!(
        owner.generation(),
        started,
        "a restart never reuses a generation"
    );
}

#[test]
fn owners_of_different_epochs_never_share_a_generation() {
    let mut hardware = RecordingHardware::default();
    let mut first = AccessPointTsf::new(1);
    let mut second = AccessPointTsf::new(2);
    first.restart(&mut hardware);
    second.restart(&mut hardware);
    assert_ne!(first.generation(), second.generation());
}
