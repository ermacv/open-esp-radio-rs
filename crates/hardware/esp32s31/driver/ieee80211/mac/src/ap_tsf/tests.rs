use super::*;

#[derive(Default)]
struct RecordingHardware {
    starts: usize,
    stops: usize,
}

impl ApTsfHardware for RecordingHardware {
    fn reset_and_start_access_point_tsf(&mut self) {
        self.starts += 1;
    }

    fn stop_access_point_tsf(&mut self) {
        self.stops += 1;
    }
}

#[test]
fn low_mac_publishes_exactly_one_start_and_stop_edge() {
    let mut hardware = RecordingHardware::default();

    reset_and_start_access_point_tsf(&mut hardware);
    stop_access_point_tsf(&mut hardware);

    assert_eq!(hardware.starts, 1);
    assert_eq!(hardware.stops, 1);
}
