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

    /// Complete scheduler-prefix image published by the HAL body.
    pub const fn scheduler_sram_prefix(self) -> u32 {
        self.scheduler_sram.address() & 0xffc0_0000
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
    SchedulerSramPointerPrefix,
    HalInitBytes,
    SleepTimerControl,
    HalInitControl0,
    HalInitControl1,
    HalInitLatch,
    HalInitLow20,
    HalInitSchedulerControl,
    HalInitLowHalf,
    HalInitSlotMap0,
    HalInitSlotMap1,
}

trait HalInitTransaction {
    fn write(&mut self, register: HalInitRegister, image: u32);
    fn modify(&mut self, register: HalInitRegister, preserve_mask: u32, set_mask: u32);
}

fn execute_hal_init(
    transaction: &mut impl HalInitTransaction,
    config: BluetoothControllerHalInitConfig,
) {
    transaction.write(
        HalInitRegister::SchedulerSramPointerPrefix,
        config.scheduler_sram_prefix(),
    );
    transaction.modify(
        HalInitRegister::SleepTimerControl,
        0xffff_fff8,
        u32::from(config.sleep_timer_shift()),
    );
    transaction.modify(
        HalInitRegister::HalInitBytes,
        0xffff_ff00,
        u32::from(config.transformed_value_0()),
    );
    transaction.modify(
        HalInitRegister::HalInitBytes,
        0xffff_00ff,
        u32::from(config.transformed_value_1()) << 8,
    );

    transaction.write(HalInitRegister::HalInitLatch, 0x0000_0040);
    transaction.write(HalInitRegister::HalInitLow20, 0x000f_ffff);
    transaction.modify(HalInitRegister::HalInitLatch, u32::MAX, 0x8000_0000);
    transaction.modify(HalInitRegister::HalInitControl1, u32::MAX, 0x000c_8000);
    transaction.modify(HalInitRegister::HalInitControl1, u32::MAX, 0x0000_00c8);
    transaction.modify(HalInitRegister::HalInitControl0, u32::MAX, 0x8000_0000);

    let config_24 = match config.scale {
        BluetoothHalInitScale::Eight => 0x0100_0000,
        BluetoothHalInitScale::Sixteen => 0,
    };
    transaction.modify(HalInitRegister::SleepTimerControl, 0x00ff_ffff, config_24);

    transaction.modify(HalInitRegister::HalInitSchedulerControl, 0xffe0_ffff, 0);
    transaction.modify(
        HalInitRegister::HalInitSchedulerControl,
        u32::MAX,
        0x0010_0000,
    );
    transaction.modify(
        HalInitRegister::HalInitSchedulerControl,
        u32::MAX,
        0x8000_0000,
    );
    transaction.modify(HalInitRegister::HalInitLowHalf, 0xffff_0000, 0);
    transaction.modify(HalInitRegister::HalInitLowHalf, u32::MAX, 0x0000_ffff);
    transaction.modify(HalInitRegister::HalInitSchedulerControl, 0xffff_00ff, 0);
    transaction.modify(
        HalInitRegister::HalInitSchedulerControl,
        u32::MAX,
        0x0000_2000,
    );

    for global_index in 0..16_u32 {
        let register = if global_index < 8 {
            HalInitRegister::HalInitSlotMap0
        } else {
            HalInitRegister::HalInitSlotMap1
        };
        let lane_shift = (global_index & 7) * 4;
        transaction.modify(register, !(0x0c_u32 << lane_shift), 0);
        transaction.modify(
            register,
            u32::MAX,
            (1_u32 << lane_shift) | ((global_index & 3) << (lane_shift + 1)),
        );
    }
}

struct MmioHalInit<'a> {
    registers: &'a super::svd::BluetoothControllerCore,
}

impl HalInitTransaction for MmioHalInit<'_> {
    #[allow(
        unsafe_code,
        reason = "this finite adapter emits reviewed complete images through svd2rust"
    )]
    fn write(&mut self, register: HalInitRegister, image: u32) {
        macro_rules! write_complete {
            ($register:expr) => {{
                // SAFETY: `execute_hal_init` supplies only the finite complete
                // images recovered for this exact ordinary register.
                unsafe { $register.write_with_zero(|writer| writer.bits(image)) };
            }};
        }

        match register {
            HalInitRegister::SchedulerSramPointerPrefix => {
                write_complete!(self.registers.scheduler_sram_pointer_prefix())
            }
            HalInitRegister::HalInitLatch => write_complete!(self.registers.hal_init_latch()),
            HalInitRegister::HalInitLow20 => write_complete!(self.registers.hal_init_low_20()),
            _ => unreachable!("HAL-init plan attempted an unreviewed complete write"),
        }
    }

    #[allow(
        unsafe_code,
        reason = "this finite adapter emits reviewed complete RMW images through svd2rust"
    )]
    fn modify(&mut self, register: HalInitRegister, preserve_mask: u32, set_mask: u32) {
        macro_rules! modify_complete {
            ($register:expr) => {{
                $register.modify(|reader, writer| {
                    let image = (reader.bits() & preserve_mask) | set_mask;
                    // SAFETY: masks and set images are closed inside the
                    // reviewed transaction and are regression-tested in order.
                    unsafe { writer.bits(image) }
                });
            }};
        }

        match register {
            HalInitRegister::HalInitBytes => modify_complete!(self.registers.hal_init_bytes()),
            HalInitRegister::SleepTimerControl => {
                modify_complete!(self.registers.sleep_timer_control())
            }
            HalInitRegister::HalInitControl0 => {
                modify_complete!(self.registers.hal_init_control_0())
            }
            HalInitRegister::HalInitControl1 => {
                modify_complete!(self.registers.hal_init_control_1())
            }
            HalInitRegister::HalInitLatch => modify_complete!(self.registers.hal_init_latch()),
            HalInitRegister::HalInitSchedulerControl => {
                modify_complete!(self.registers.hal_init_scheduler_control())
            }
            HalInitRegister::HalInitLowHalf => {
                modify_complete!(self.registers.hal_init_low_half())
            }
            HalInitRegister::HalInitSlotMap0 => {
                modify_complete!(self.registers.hal_init_slot_map(0))
            }
            HalInitRegister::HalInitSlotMap1 => {
                modify_complete!(self.registers.hal_init_slot_map(1))
            }
            _ => unreachable!("HAL-init plan attempted an unreviewed RMW"),
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
    /// Clocks, PHY, BTBB, software queues, SRAM and interrupt lifecycle are
    /// upper-layer ownership facts. They are deliberately not represented by
    /// a forgeable PAC proof token.
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
        HalInitRegister, HalInitTransaction, execute_hal_init,
    };
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Operation {
        Write(HalInitRegister, u32),
        Modify(HalInitRegister, u32, u32),
    }

    #[derive(Default)]
    struct Recorder {
        operations: Vec<Operation>,
    }

    impl HalInitTransaction for Recorder {
        fn write(&mut self, register: HalInitRegister, image: u32) {
            self.operations.push(Operation::Write(register, image));
        }

        fn modify(&mut self, register: HalInitRegister, preserve_mask: u32, set_mask: u32) {
            self.operations
                .push(Operation::Modify(register, preserve_mask, set_mask));
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
    fn complete_transaction_has_exact_prefix_and_thirty_two_lane_edges() {
        let mut recorder = Recorder::default();
        execute_hal_init(
            &mut recorder,
            BluetoothControllerHalInitConfig::reviewed_standalone(),
        );

        assert_eq!(recorder.operations.len(), 50);
        assert_eq!(
            recorder.operations[..18],
            [
                Operation::Write(HalInitRegister::SchedulerSramPointerPrefix, 0x2f00_0000),
                Operation::Modify(HalInitRegister::SleepTimerControl, 0xffff_fff8, 3),
                Operation::Modify(HalInitRegister::HalInitBytes, 0xffff_ff00, 22),
                Operation::Modify(HalInitRegister::HalInitBytes, 0xffff_00ff, 66 << 8),
                Operation::Write(HalInitRegister::HalInitLatch, 0x40),
                Operation::Write(HalInitRegister::HalInitLow20, 0x000f_ffff),
                Operation::Modify(HalInitRegister::HalInitLatch, u32::MAX, 0x8000_0000),
                Operation::Modify(HalInitRegister::HalInitControl1, u32::MAX, 0x000c_8000),
                Operation::Modify(HalInitRegister::HalInitControl1, u32::MAX, 0x0000_00c8),
                Operation::Modify(HalInitRegister::HalInitControl0, u32::MAX, 0x8000_0000),
                Operation::Modify(HalInitRegister::SleepTimerControl, 0x00ff_ffff, 0),
                Operation::Modify(HalInitRegister::HalInitSchedulerControl, 0xffe0_ffff, 0,),
                Operation::Modify(
                    HalInitRegister::HalInitSchedulerControl,
                    u32::MAX,
                    0x0010_0000,
                ),
                Operation::Modify(
                    HalInitRegister::HalInitSchedulerControl,
                    u32::MAX,
                    0x8000_0000,
                ),
                Operation::Modify(HalInitRegister::HalInitLowHalf, 0xffff_0000, 0),
                Operation::Modify(HalInitRegister::HalInitLowHalf, u32::MAX, 0x0000_ffff),
                Operation::Modify(HalInitRegister::HalInitSchedulerControl, 0xffff_00ff, 0,),
                Operation::Modify(
                    HalInitRegister::HalInitSchedulerControl,
                    u32::MAX,
                    0x0000_2000,
                ),
            ]
        );

        for (global_index, pair) in recorder.operations[18..].chunks_exact(2).enumerate() {
            let register = if global_index < 8 {
                HalInitRegister::HalInitSlotMap0
            } else {
                HalInitRegister::HalInitSlotMap1
            };
            let shift = (global_index & 7) * 4;
            assert_eq!(
                pair,
                [
                    Operation::Modify(register, !(0x0c_u32 << shift), 0),
                    Operation::Modify(
                        register,
                        u32::MAX,
                        (1_u32 << shift) | (((global_index as u32) & 3) << (shift + 1)),
                    ),
                ]
            );
        }
    }
}
