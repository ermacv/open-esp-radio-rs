extern crate std;

use std::vec::Vec;

use super::{
    BluetoothControllerHalInitConfig, BluetoothHalInitPeriod, BluetoothHalInitScale,
    HalInitOperation, HalInitRegister, HalInitTransaction, execute_hal_init,
};

#[derive(Default)]
struct Recorder {
    operations: Vec<HalInitOperation>,
}

impl HalInitTransaction for Recorder {
    fn apply(&mut self, operation: HalInitOperation) {
        self.operations.push(operation);
    }
}

#[test]
fn standalone_time_scale_matches_complete_shift_helpers() {
    let scale = BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale();

    assert_eq!(scale.shift_image(), 3);
    assert_eq!(scale.micros_from_raw_ticks(0), 0);
    assert_eq!(scale.micros_from_raw_ticks(625), 2_500);
    assert_eq!(scale.micros_from_raw_ticks(0x4000_0000), 0);
    assert_eq!(
        scale.raw_ticks_from_micros(2_503),
        super::BluetoothRawTickDeltaProjection {
            whole_ticks: 625,
            remainder_micros: 3,
        }
    );
}

#[test]
fn every_accepted_time_scale_retains_inverse_remainder() {
    let cases = [
        (
            BluetoothHalInitScale::Eight,
            BluetoothHalInitPeriod::Image2000,
            2,
        ),
        (
            BluetoothHalInitScale::Eight,
            BluetoothHalInitPeriod::Image1000,
            3,
        ),
        (
            BluetoothHalInitScale::Eight,
            BluetoothHalInitPeriod::Image500,
            4,
        ),
        (
            BluetoothHalInitScale::Sixteen,
            BluetoothHalInitPeriod::Image500,
            5,
        ),
    ];

    for (scale, period, shift_image) in cases {
        let time_scale =
            BluetoothControllerHalInitConfig::new(scale, 11, 33, period).controller_time_scale();
        let micros = 0x1234_567b;
        let projection = time_scale.raw_ticks_from_micros(micros);
        let shift = u32::from(shift_image - 1);

        assert_eq!(time_scale.shift_image(), shift_image);
        assert_eq!(projection.whole_ticks, micros >> shift);
        assert_eq!(
            u32::from(projection.remainder_micros),
            micros & ((1_u32 << shift) - 1)
        );
    }
}

#[test]
fn complete_transaction_has_semantic_prefix_and_thirty_two_lane_edges() {
    let mut recorder = Recorder::default();
    execute_hal_init(
        &mut recorder,
        BluetoothControllerHalInitConfig::reviewed_standalone(),
    );

    assert_eq!(recorder.operations.len(), 50);
    assert_eq!(
        recorder.operations[..18],
        [
            HalInitOperation::PublishSchedulerSramPrefix,
            HalInitOperation::PublishSleepTimerShift(3),
            HalInitOperation::PublishValue0(22),
            HalInitOperation::PublishValue1(66),
            HalInitOperation::InitializeLatch,
            HalInitOperation::InitializeLow20,
            HalInitOperation::EnableLatch,
            HalInitOperation::ConfigureControl1High,
            HalInitOperation::ConfigureControl1Low,
            HalInitOperation::EnableControl0,
            HalInitOperation::ResetSleepTimerHigh { config_24: false },
            HalInitOperation::ClearSchedulerConfig16To20,
            HalInitOperation::PublishSchedulerConfig16To20,
            HalInitOperation::EnableSchedulerControl,
            HalInitOperation::ClearLowHalf,
            HalInitOperation::FillLowHalf,
            HalInitOperation::ClearSchedulerByte1,
            HalInitOperation::PublishSchedulerByte1,
        ]
    );

    for (global_index, pair) in recorder.operations[18..].chunks_exact(2).enumerate() {
        let register = if global_index < 8 {
            HalInitRegister::SlotMap0
        } else {
            HalInitRegister::SlotMap1
        };
        let lane = (global_index % 8) as u8;
        let index_in_group = global_index % 4;
        assert_eq!(
            pair,
            [
                HalInitOperation::ClearSlotLaneUpper { register, lane },
                HalInitOperation::PublishSlotLane {
                    register,
                    lane,
                    set_retained_index_low: index_in_group % 2 == 1,
                    index_high: u8::from(index_in_group >= 2),
                },
            ]
        );
    }
}
