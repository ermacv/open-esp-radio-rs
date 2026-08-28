//! Complete, fact-bounded BTDM controller HAL initialization.
//!
//! This module owns the finite MMIO body reached by the ESP32-S31 controller
//! enable path immediately before controller-side IRQ output preparation.  It
//! deliberately models the vendor input ABI as typed hardware inputs instead
//! of copying its private structure layout or task/runtime machinery.

use super::{BluetoothControllerSramAddress, BluetoothTaskRegisters, device_fence};

/// First positional scaling input accepted by the complete HAL-config setter.
///
/// The instruction evidence accepts exactly the byte values 8 and 16.  No
/// clock-unit meaning is assigned until a public hardware description or HIL
/// establishes it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothHalInitScale {
    /// Positional input image 8.
    Eight,
    /// Positional input image 16; used by the reviewed standalone profile.
    Sixteen,
}

impl BluetoothHalInitScale {
    const fn image(self) -> u8 {
        match self {
            Self::Eight => 8,
            Self::Sixteen => 16,
        }
    }
}

/// Second positional scaling input accepted by the complete HAL-config setter.
///
/// Only these three complete integer images are accepted by the recovered
/// setter.  The names intentionally do not claim a time unit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothHalInitPeriod {
    /// Positional input image 500.
    Image500,
    /// Positional input image 1000.
    Image1000,
    /// Positional input image 2000; used by the reviewed standalone profile.
    Image2000,
}

impl BluetoothHalInitPeriod {
    const fn image(self) -> u32 {
        match self {
            Self::Image500 => 500,
            Self::Image1000 => 1_000,
            Self::Image2000 => 2_000,
        }
    }

    const fn transform_byte(self, value: u8) -> u8 {
        match self {
            // The internal selector is two and the helper shifts right by
            // selector-minus-one.
            Self::Image500 => value >> 1,
            // Selector one produces a zero-bit right shift.
            Self::Image1000 => value,
            // Selector zero takes the helper's alternate one-bit left shift;
            // the caller then retains only the low byte.
            Self::Image2000 => value.wrapping_shl(1),
        }
    }
}

/// Typed inputs to the complete ESP32-S31 BTDM HAL-init MMIO body.
///
/// `value_0` and `value_1` remain positional because their undocumented
/// hardware meanings have not been established.  Unlike an opaque vendor
/// structure, every field here has a bounded, reviewed transformation into a
/// specific register position.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothControllerHalInitConfig {
    scale: BluetoothHalInitScale,
    value_0: u8,
    value_1: u8,
    period: BluetoothHalInitPeriod,
    scheduler_sram: BluetoothControllerSramAddress,
}

/// Exact integer scale between a raw latched controller-time delta and the
/// BLE scheduler's internal delta domain.
///
/// The type intentionally assigns no physical unit to either side. Complete
/// ESP32-S31 bodies prove only the integer transform selected by the same
/// low-three-bit image written during HAL initialization. Counter width, wrap
/// and physical frequency remain separate evidence obligations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothControllerTimeScale {
    shift_image: u8,
}

/// Projection of one scheduler delta into the raw controller-time domain.
///
/// `remainder` retains the scheduler-domain low bits discarded by the exact
/// inverse shift. Callers must choose a rounding policy explicitly rather than
/// silently treating the projection as exact.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothRawTimeDeltaProjection {
    /// Whole raw-time delta produced by the reviewed inverse transform.
    pub whole: u32,
    /// Scheduler-domain remainder discarded by that transform.
    pub remainder: u8,
}

impl BluetoothControllerTimeScale {
    /// Low-three-bit scale image written by the HAL initialization body.
    pub const fn shift_image(self) -> u8 {
        self.shift_image
    }

    /// Convert one raw latched-time delta into the scheduler delta domain.
    ///
    /// This uses wrapping 32-bit arithmetic, matching the complete RISC-V
    /// shift helper. It does not decide whether a particular wrapped delta is
    /// temporally before or after an anchor.
    pub const fn scheduler_delta_from_raw(self, raw_delta: u32) -> u32 {
        raw_delta.wrapping_shl((self.shift_image - 1) as u32)
    }

    /// Apply the exact inverse transform while retaining discarded low bits.
    pub const fn raw_delta_from_scheduler(
        self,
        scheduler_delta: u32,
    ) -> BluetoothRawTimeDeltaProjection {
        let shift = (self.shift_image - 1) as u32;
        BluetoothRawTimeDeltaProjection {
            whole: scheduler_delta >> shift,
            remainder: (scheduler_delta & ((1_u32 << shift) - 1)) as u8,
        }
    }
}

impl BluetoothControllerHalInitConfig {
    /// Construct one configuration from the complete setter's accepted input
    /// domains and a validated controller-SRAM address.
    pub const fn new(
        scale: BluetoothHalInitScale,
        value_0: u8,
        value_1: u8,
        period: BluetoothHalInitPeriod,
        scheduler_sram: BluetoothControllerSramAddress,
    ) -> Self {
        Self {
            scale,
            value_0,
            value_1,
            period,
            scheduler_sram,
        }
    }

    /// Exact input profile constructed by the pinned ESP32-S31 controller
    /// task: bytes 16, 11 and 33, image 2000 and SRAM base `0x2f00_0000`.
    pub const fn reviewed_standalone() -> Self {
        let scheduler_sram = match BluetoothControllerSramAddress::new(0x2f00_0000) {
            Ok(address) => address,
            Err(_) => panic!("reviewed controller SRAM base must be representable"),
        };
        Self::new(
            BluetoothHalInitScale::Sixteen,
            11,
            33,
            BluetoothHalInitPeriod::Image2000,
            scheduler_sram,
        )
    }

    /// Low-three-bit sleep-timer image derived by exact integer division.
    pub const fn sleep_timer_shift(self) -> u8 {
        match (self.scale, self.period) {
            (BluetoothHalInitScale::Eight, BluetoothHalInitPeriod::Image500) => 4,
            (BluetoothHalInitScale::Eight, BluetoothHalInitPeriod::Image1000) => 3,
            (BluetoothHalInitScale::Eight, BluetoothHalInitPeriod::Image2000) => 2,
            (BluetoothHalInitScale::Sixteen, BluetoothHalInitPeriod::Image500) => 5,
            (BluetoothHalInitScale::Sixteen, BluetoothHalInitPeriod::Image1000) => 4,
            (BluetoothHalInitScale::Sixteen, BluetoothHalInitPeriod::Image2000) => 3,
        }
    }

    /// Exact raw-controller-time to BLE-scheduler scale selected by this
    /// initialization profile.
    pub const fn controller_time_scale(self) -> BluetoothControllerTimeScale {
        BluetoothControllerTimeScale {
            shift_image: self.sleep_timer_shift(),
        }
    }

    /// First byte image after the reviewed period-dependent transformation.
    pub const fn transformed_value_0(self) -> u8 {
        self.period.transform_byte(self.value_0)
    }

    /// Second byte image after the reviewed period-dependent transformation.
    pub const fn transformed_value_1(self) -> u8 {
        self.period.transform_byte(self.value_1)
    }

    /// Ten-bit scheduler-prefix field published by the HAL body.
    pub const fn scheduler_sram_prefix(self) -> u16 {
        (self.scheduler_sram.address() >> 22) as u16
    }

    /// Original finite scale image, useful for evidence and diagnostics.
    pub const fn scale_image(self) -> u8 {
        self.scale.image()
    }

    /// Original finite period image, useful for evidence and diagnostics.
    pub const fn period_image(self) -> u32 {
        self.period.image()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HalInitRegister {
    SlotMap0,
    SlotMap1,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HalInitOperation {
    PublishSchedulerSramPrefix(u16),
    PublishSleepTimerShift(u8),
    PublishValue0(u8),
    PublishValue1(u8),
    InitializeLatch,
    InitializeLow20,
    EnableLatch,
    ConfigureControl1High,
    ConfigureControl1Low,
    EnableControl0,
    ResetSleepTimerHigh {
        config_24: bool,
    },
    ClearSchedulerConfig16To20,
    PublishSchedulerConfig16To20(u8),
    EnableSchedulerControl,
    ClearLowHalf,
    FillLowHalf,
    ClearSchedulerByte1,
    PublishSchedulerByte1(u8),
    ClearSlotLaneUpper {
        register: HalInitRegister,
        lane: u8,
    },
    PublishSlotLane {
        register: HalInitRegister,
        lane: u8,
        set_retained_index_low: bool,
        index_high: u8,
    },
}

trait HalInitTransaction {
    fn apply(&mut self, operation: HalInitOperation);
}

fn execute_hal_init(
    transaction: &mut impl HalInitTransaction,
    config: BluetoothControllerHalInitConfig,
) {
    transaction.apply(HalInitOperation::PublishSchedulerSramPrefix(
        config.scheduler_sram_prefix(),
    ));
    transaction.apply(HalInitOperation::PublishSleepTimerShift(
        config.sleep_timer_shift(),
    ));
    transaction.apply(HalInitOperation::PublishValue0(
        config.transformed_value_0(),
    ));
    transaction.apply(HalInitOperation::PublishValue1(
        config.transformed_value_1(),
    ));
    transaction.apply(HalInitOperation::InitializeLatch);
    transaction.apply(HalInitOperation::InitializeLow20);
    transaction.apply(HalInitOperation::EnableLatch);
    transaction.apply(HalInitOperation::ConfigureControl1High);
    transaction.apply(HalInitOperation::ConfigureControl1Low);
    transaction.apply(HalInitOperation::EnableControl0);
    transaction.apply(HalInitOperation::ResetSleepTimerHigh {
        config_24: matches!(config.scale, BluetoothHalInitScale::Eight),
    });
    transaction.apply(HalInitOperation::ClearSchedulerConfig16To20);
    transaction.apply(HalInitOperation::PublishSchedulerConfig16To20(0x10));
    transaction.apply(HalInitOperation::EnableSchedulerControl);
    transaction.apply(HalInitOperation::ClearLowHalf);
    transaction.apply(HalInitOperation::FillLowHalf);
    transaction.apply(HalInitOperation::ClearSchedulerByte1);
    transaction.apply(HalInitOperation::PublishSchedulerByte1(0x20));

    for global_index in 0..16_u8 {
        let register = if global_index < 8 {
            HalInitRegister::SlotMap0
        } else {
            HalInitRegister::SlotMap1
        };
        let lane = global_index % 8;
        let index_in_group = global_index % 4;
        transaction.apply(HalInitOperation::ClearSlotLaneUpper { register, lane });
        transaction.apply(HalInitOperation::PublishSlotLane {
            register,
            lane,
            set_retained_index_low: index_in_group % 2 == 1,
            index_high: u8::from(index_in_group >= 2),
        });
    }
}

struct MmioHalInit<'a> {
    registers: &'a super::svd::BluetoothControllerCore,
}

impl HalInitTransaction for MmioHalInit<'_> {
    fn apply(&mut self, operation: HalInitOperation) {
        match operation {
            HalInitOperation::PublishSchedulerSramPrefix(prefix) => {
                super::svd::zero_based_field_write::publish_bluetooth_hal_scheduler_sram_prefix(
                    self.registers,
                    prefix,
                );
            }
            HalInitOperation::PublishSleepTimerShift(shift) => {
                self.registers
                    .sleep_timer_control()
                    .modify(|_, writer| writer.config_low_3().set(shift));
            }
            HalInitOperation::PublishValue0(value) => {
                self.registers
                    .hal_init_bytes()
                    .modify(|_, writer| writer.value_0().set(value));
            }
            HalInitOperation::PublishValue1(value) => {
                self.registers
                    .hal_init_bytes()
                    .modify(|_, writer| writer.value_1().set(value));
            }
            HalInitOperation::InitializeLatch => {
                super::svd::zero_based_field_write::initialize_bluetooth_hal_latch(
                    self.registers,
                    true,
                );
            }
            HalInitOperation::InitializeLow20 => {
                super::svd::zero_based_field_write::initialize_bluetooth_hal_low_20(
                    self.registers,
                    0x000f_ffff,
                );
            }
            HalInitOperation::EnableLatch => {
                self.registers
                    .hal_init_latch()
                    .modify(|_, writer| writer.enable_31().set_bit());
            }
            HalInitOperation::ConfigureControl1High => {
                self.registers
                    .hal_init_control_1()
                    .modify(|_, writer| writer.config_15().set_bit().config_18_19().set(3));
            }
            HalInitOperation::ConfigureControl1Low => {
                self.registers
                    .hal_init_control_1()
                    .modify(|_, writer| writer.config_3().set_bit().config_6_7().set(3));
            }
            HalInitOperation::EnableControl0 => {
                self.registers
                    .hal_init_control_0()
                    .modify(|_, writer| writer.enable_31().set_bit());
            }
            HalInitOperation::ResetSleepTimerHigh { config_24 } => {
                self.registers.sleep_timer_control().modify(|_, writer| {
                    let writer = writer
                        .init_clear_25_unknown()
                        .clear_bit()
                        .latch_request()
                        .clear_bit()
                        .init_clear_27_30_unknown()
                        .set(0)
                        .timer_arm()
                        .clear_bit();
                    if config_24 {
                        writer.config_24().set_bit()
                    } else {
                        writer.config_24().clear_bit()
                    }
                });
            }
            HalInitOperation::ClearSchedulerConfig16To20 => {
                self.registers
                    .hal_init_scheduler_control()
                    .modify(|_, writer| writer.config_16_20().set(0));
            }
            HalInitOperation::PublishSchedulerConfig16To20(value) => {
                self.registers
                    .hal_init_scheduler_control()
                    .modify(|_, writer| writer.config_16_20().set(value));
            }
            HalInitOperation::EnableSchedulerControl => {
                self.registers
                    .hal_init_scheduler_control()
                    .modify(|_, writer| writer.enable_31().set_bit());
            }
            HalInitOperation::ClearLowHalf => {
                self.registers
                    .hal_init_low_half()
                    .modify(|_, writer| writer.config_low_16().set(0));
            }
            HalInitOperation::FillLowHalf => {
                self.registers
                    .hal_init_low_half()
                    .modify(|_, writer| writer.config_low_16().set(u16::MAX));
            }
            HalInitOperation::ClearSchedulerByte1 => {
                self.registers
                    .hal_init_scheduler_control()
                    .modify(|_, writer| writer.config_byte_1().set(0));
            }
            HalInitOperation::PublishSchedulerByte1(value) => {
                self.registers
                    .hal_init_scheduler_control()
                    .modify(|_, writer| writer.config_byte_1().set(value));
            }
            HalInitOperation::ClearSlotLaneUpper { register, lane } => {
                let slots = self.registers.hal_init_slot_map(match register {
                    HalInitRegister::SlotMap0 => 0,
                    HalInitRegister::SlotMap1 => 1,
                });
                macro_rules! clear_lane_upper {
                    ($index_high:ident, $clear_high:ident) => {{
                        slots.modify(|_, writer| {
                            writer.$index_high().clear_bit().$clear_high().clear_bit()
                        });
                    }};
                }
                match lane {
                    0 => clear_lane_upper!(lane_0_index_high, lane_0_clear_high_unknown),
                    1 => clear_lane_upper!(lane_1_index_high, lane_1_clear_high_unknown),
                    2 => clear_lane_upper!(lane_2_index_high, lane_2_clear_high_unknown),
                    3 => clear_lane_upper!(lane_3_index_high, lane_3_clear_high_unknown),
                    4 => clear_lane_upper!(lane_4_index_high, lane_4_clear_high_unknown),
                    5 => clear_lane_upper!(lane_5_index_high, lane_5_clear_high_unknown),
                    6 => clear_lane_upper!(lane_6_index_high, lane_6_clear_high_unknown),
                    7 => clear_lane_upper!(lane_7_index_high, lane_7_clear_high_unknown),
                    _ => unreachable!("HAL-init slot lane is bounded to eight entries"),
                };
            }
            HalInitOperation::PublishSlotLane {
                register,
                lane,
                set_retained_index_low,
                index_high,
            } => {
                let slots = self.registers.hal_init_slot_map(match register {
                    HalInitRegister::SlotMap0 => 0,
                    HalInitRegister::SlotMap1 => 1,
                });
                macro_rules! publish_lane {
                    ($enable:ident, $index_low:ident, $index_high:ident) => {{
                        slots.modify(|_, writer| {
                            let writer = writer.$enable().set_bit();
                            let writer = if set_retained_index_low {
                                writer.$index_low().set_bit()
                            } else {
                                writer
                            };
                            if index_high == 0 {
                                writer.$index_high().clear_bit()
                            } else {
                                writer.$index_high().set_bit()
                            }
                        });
                    }};
                }
                match lane {
                    0 => publish_lane!(lane_0_enable, lane_0_retained_index_low, lane_0_index_high),
                    1 => publish_lane!(lane_1_enable, lane_1_retained_index_low, lane_1_index_high),
                    2 => publish_lane!(lane_2_enable, lane_2_retained_index_low, lane_2_index_high),
                    3 => publish_lane!(lane_3_enable, lane_3_retained_index_low, lane_3_index_high),
                    4 => publish_lane!(lane_4_enable, lane_4_retained_index_low, lane_4_index_high),
                    5 => publish_lane!(lane_5_enable, lane_5_retained_index_low, lane_5_index_high),
                    6 => publish_lane!(lane_6_enable, lane_6_retained_index_low, lane_6_index_high),
                    7 => publish_lane!(lane_7_enable, lane_7_retained_index_low, lane_7_index_high),
                    _ => unreachable!("HAL-init slot lane is bounded to eight entries"),
                };
            }
        }
    }
}

impl BluetoothTaskRegisters {
    /// Execute the complete recovered BTDM controller HAL-init MMIO body.
    ///
    /// SOURCE: pinned ESP32-S31 `libbtdm_common.a` member `7.o`, complete
    /// symbol `r_sym_bt_aGdrujd2MUAzWYH75baR`, plus its complete config setter,
    /// byte-scaling helper and caller in member `21.o`.  The transaction has 50
    /// ordered writes/RMWs and finishes before controller IRQ output setup.
    ///
    /// This method does not initialize software events/lists, route a CPU
    /// interrupt, enable the Link Layer or claim HCI readiness.
    ///
    /// Enabled clocks, the selected SRAM-prefix policy and inactive interrupt
    /// lifecycle are upper-layer ownership facts. PHY, BTBB, scheduler lists,
    /// Link Layer and HCI are later stages and are not established here. None
    /// of those facts is represented by a forgeable PAC proof token.
    #[doc(hidden)]
    pub fn initialize_controller_hal(&mut self, config: BluetoothControllerHalInitConfig) {
        let mut transaction = MmioHalInit {
            registers: &self.bluetooth.bluetooth_controller_core,
        };
        execute_hal_init(&mut transaction, config);
        device_fence();
    }
}

#[cfg(test)]
mod tests {
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
        assert_eq!(scale.scheduler_delta_from_raw(0), 0);
        assert_eq!(scale.scheduler_delta_from_raw(625), 2_500);
        assert_eq!(scale.scheduler_delta_from_raw(0x4000_0000), 0);
        assert_eq!(
            scale.raw_delta_from_scheduler(2_503),
            super::BluetoothRawTimeDeltaProjection {
                whole: 625,
                remainder: 3,
            }
        );
    }

    #[test]
    fn every_accepted_time_scale_retains_inverse_remainder() {
        let address = BluetoothControllerHalInitConfig::reviewed_standalone().scheduler_sram;
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
            let time_scale = BluetoothControllerHalInitConfig::new(scale, 11, 33, period, address)
                .controller_time_scale();
            let scheduler_delta = 0x1234_567b;
            let projection = time_scale.raw_delta_from_scheduler(scheduler_delta);
            let shift = u32::from(shift_image - 1);

            assert_eq!(time_scale.shift_image(), shift_image);
            assert_eq!(projection.whole, scheduler_delta >> shift);
            assert_eq!(
                u32::from(projection.remainder),
                scheduler_delta & ((1_u32 << shift) - 1)
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
                HalInitOperation::PublishSchedulerSramPrefix(0xbc),
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
                HalInitOperation::PublishSchedulerConfig16To20(0x10),
                HalInitOperation::EnableSchedulerControl,
                HalInitOperation::ClearLowHalf,
                HalInitOperation::FillLowHalf,
                HalInitOperation::ClearSchedulerByte1,
                HalInitOperation::PublishSchedulerByte1(0x20),
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
}
