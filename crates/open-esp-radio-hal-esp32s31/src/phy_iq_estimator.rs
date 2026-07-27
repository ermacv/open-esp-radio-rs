//! Owned ESP32-S31 DC/IQ estimator register leaves.
//!
//! The methods in this module perform only finite PAC-described MMIO. Timer
//! boundaries, readiness repetition, estimator lifetime and calibration
//! arithmetic remain explicit in the PHY state machines.

#[cfg(target_arch = "riscv32")]
use open_esp_radio_pac_esp32s31::RadioRegisters;
#[cfg(any(test, target_arch = "riscv32"))]
use open_esp_radio_pac_esp32s31::{power::phy_iq_estimator_oracle as iq, Field32, Register32};

/// One observation of the estimator completion signals.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadinessSnapshot {
    pub ready: bool,
    pub activity: bool,
}

/// Three signed accumulators consumed by `phy_dc_iq_est`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DcIqAccumulatorSnapshot {
    pub i: i32,
    pub q: i32,
    pub power: i32,
}

/// Four signed signal-power words consumed by IQ mismatch calculations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SignalPowerSnapshot {
    pub sum_i: i32,
    pub difference_i: i32,
    pub difference_q: i32,
    pub sum_q: i32,
}

#[cfg(any(test, target_arch = "riscv32"))]
const fn field_value(field: Field32, value: u32) -> u32 {
    match field.checked_value(value) {
        Some(value) => value,
        None => panic!("value does not fit recovered register field"),
    }
}

#[cfg(any(test, target_arch = "riscv32"))]
trait RegisterIo {
    fn read(&mut self, register: Register32) -> u32;
    fn write(&mut self, register: Register32, value: u32);

    fn modify(&mut self, register: Register32, clear_mask: u32, set_bits: u32) {
        let previous = self.read(register);
        self.write(register, (previous & !clear_mask) | (set_bits & clear_mask));
    }

    fn replace(&mut self, register: Register32, field: Field32, value: u32) {
        self.modify(register, field.mask(), field_value(field, value));
    }

    fn replace_truncated(&mut self, register: Register32, field: Field32, value: u32) {
        self.replace(register, field, value & field.max_value());
    }

    fn set_enabled(&mut self, register: Register32, field: Field32, enabled: bool) {
        self.modify(
            register,
            field.mask(),
            if enabled { field.mask() } else { 0 },
        );
    }
}

#[cfg(target_arch = "riscv32")]
impl RegisterIo for RadioRegisters {
    fn read(&mut self, register: Register32) -> u32 {
        self.read32(register)
    }

    fn write(&mut self, register: Register32, value: u32) {
        self.write32(register, value);
    }
}

/// Apply the finite register prefix of complete rev0 ROM
/// `phy_iq_est_enable`.
///
/// The body at `0x2f82_89d4`, size `0xb4`, first replaces
/// `ESTIMATOR_CONFIG.CONFIG_MODE_UNKNOWN` with one, then replaces
/// `ESTIMATOR_CONTROL.MODE_UNKNOWN` with two, and finally publishes the
/// caller's low fifteen control bits. The diagnostic halfword write, delays
/// and readiness loop are deliberately owned by the PHY transition.
#[cfg(target_arch = "riscv32")]
pub fn configure(registers: &mut RadioRegisters, control: u16) {
    configure_with(registers, control);
}

#[cfg(any(test, target_arch = "riscv32"))]
fn configure_with(io: &mut impl RegisterIo, control: u16) {
    io.replace(
        iq::ESTIMATOR_CONFIG,
        iq::estimator_config::CONFIG_MODE_UNKNOWN,
        1,
    );
    io.replace(
        iq::ESTIMATOR_CONTROL,
        iq::estimator_control::MODE_UNKNOWN,
        2,
    );
    // ROM shifts a u16 and masks the result, so bit 15 is intentionally
    // discarded instead of being treated as an invalid Rust argument.
    io.replace_truncated(
        iq::ESTIMATOR_CONTROL,
        iq::estimator_control::CONTROL_WINDOW_UNKNOWN,
        u32::from(control),
    );
}

/// Set or clear the first estimator enable phase.
///
/// Complete rev0 ROM `phy_iq_est_enable` sets bit zero before its first
/// one-microsecond delay. Complete `phy_iq_est_disable` at `0x2f82_8a88`,
/// size `0x2c`, clears the same bit after its final delay.
#[cfg(target_arch = "riscv32")]
pub fn set_start_enabled(registers: &mut RadioRegisters, enabled: bool) {
    set_start_enabled_with(registers, enabled);
}

#[cfg(any(test, target_arch = "riscv32"))]
fn set_start_enabled_with(io: &mut impl RegisterIo, enabled: bool) {
    io.set_enabled(
        iq::ESTIMATOR_CONTROL,
        iq::estimator_control::START_ENABLE,
        enabled,
    );
}

/// Set or clear the second estimator enable phase.
///
/// Complete rev0 ROM `phy_iq_est_enable` sets bit one after the first
/// one-microsecond delay. Complete `phy_iq_est_disable` clears it before its
/// one-microsecond delay.
#[cfg(target_arch = "riscv32")]
pub fn set_measurement_enabled(registers: &mut RadioRegisters, enabled: bool) {
    set_measurement_enabled_with(registers, enabled);
}

#[cfg(any(test, target_arch = "riscv32"))]
fn set_measurement_enabled_with(io: &mut impl RegisterIo, enabled: bool) {
    io.set_enabled(
        iq::ESTIMATOR_CONTROL,
        iq::estimator_control::MEASUREMENT_ENABLE,
        enabled,
    );
}

/// Sample the ready word and shared activity word exactly once each.
///
/// Complete rev0 ROM `phy_iq_est_enable` tests the PAC ready field and, while
/// it is clear, tests the PAC activity field. Rust retains repeat policy
/// outside this finite observation leaf.
#[cfg(target_arch = "riscv32")]
pub fn sample_readiness(registers: &mut RadioRegisters) -> ReadinessSnapshot {
    sample_readiness_with(registers)
}

#[cfg(any(test, target_arch = "riscv32"))]
fn sample_readiness_with(io: &mut impl RegisterIo) -> ReadinessSnapshot {
    let ready = iq::estimator_ready_status::READY.extract(io.read(iq::ESTIMATOR_READY_STATUS)) != 0;
    let activity = iq::estimator_activity_status::ACTIVITY_UNKNOWN
        .extract(io.read(iq::ESTIMATOR_ACTIVITY_STATUS))
        != 0;
    ReadinessSnapshot { ready, activity }
}

/// Read the three signed accumulators in complete `phy_dc_iq_est` order.
///
/// The complete rev0 ROM body at `0x2f82_8ab4`, size `0x84`, reads
/// `DC_I_ACCUMULATOR`, `DC_Q_ACCUMULATOR`, then `POWER_ACCUMULATOR`.
#[cfg(target_arch = "riscv32")]
pub fn read_dc_iq_accumulators(registers: &mut RadioRegisters) -> DcIqAccumulatorSnapshot {
    read_dc_iq_accumulators_with(registers)
}

#[cfg(any(test, target_arch = "riscv32"))]
fn read_dc_iq_accumulators_with(io: &mut impl RegisterIo) -> DcIqAccumulatorSnapshot {
    let i = io.read(iq::DC_I_ACCUMULATOR) as i32;
    let q = io.read(iq::DC_Q_ACCUMULATOR) as i32;
    let power = io.read(iq::POWER_ACCUMULATOR) as i32;
    DcIqAccumulatorSnapshot { i, q, power }
}

/// Read the signed total-power accumulator exactly once.
///
/// Complete rev0 ROM `phy_set_rx_gain_cal_iq` at `0x2f82_964c`, size
/// `0x20c`, and complete `phy_rxiq_get_mis` both consume this identity.
#[cfg(target_arch = "riscv32")]
pub fn read_total_power(registers: &mut RadioRegisters) -> i32 {
    read_total_power_with(registers)
}

#[cfg(any(test, target_arch = "riscv32"))]
fn read_total_power_with(io: &mut impl RegisterIo) -> i32 {
    io.read(iq::POWER_ACCUMULATOR) as i32
}

/// Read four signed words in complete `phy_rxiq_get_mis` order.
///
/// The complete rev0 ROM body at `0x2f82_8b84`, size `0x13e`, reads the
/// physical addresses in order `0x0454`, `0x0460`, `0x045c`, `0x0458`.
/// Locals preserve that hardware order even though the returned structure is
/// arranged by semantic role.
#[cfg(target_arch = "riscv32")]
pub fn read_rxiq_mismatch(registers: &mut RadioRegisters) -> SignalPowerSnapshot {
    read_rxiq_mismatch_with(registers)
}

#[cfg(any(test, target_arch = "riscv32"))]
fn read_rxiq_mismatch_with(io: &mut impl RegisterIo) -> SignalPowerSnapshot {
    let sum_i = io.read(iq::SIGNAL_POWER_SUM_I) as i32;
    let sum_q = io.read(iq::SIGNAL_POWER_SUM_Q) as i32;
    let difference_q = io.read(iq::SIGNAL_POWER_DIFFERENCE_Q) as i32;
    let difference_i = io.read(iq::SIGNAL_POWER_DIFFERENCE_I) as i32;
    SignalPowerSnapshot {
        sum_i,
        difference_i,
        difference_q,
        sum_q,
    }
}

/// Read four signed words in complete `phy_get_rx_sig_pwr` order.
///
/// The complete rev0 ROM body at `0x2f82_9ea2`, size `0x76`, reads the
/// physical addresses in order `0x0454`, `0x0460`, `0x0458`, `0x045c`.
#[cfg(target_arch = "riscv32")]
pub fn read_signal_power(registers: &mut RadioRegisters) -> SignalPowerSnapshot {
    read_signal_power_with(registers)
}

#[cfg(any(test, target_arch = "riscv32"))]
fn read_signal_power_with(io: &mut impl RegisterIo) -> SignalPowerSnapshot {
    let sum_i = io.read(iq::SIGNAL_POWER_SUM_I) as i32;
    let sum_q = io.read(iq::SIGNAL_POWER_SUM_Q) as i32;
    let difference_i = io.read(iq::SIGNAL_POWER_DIFFERENCE_I) as i32;
    let difference_q = io.read(iq::SIGNAL_POWER_DIFFERENCE_Q) as i32;
    SignalPowerSnapshot {
        sum_i,
        difference_i,
        difference_q,
        sum_q,
    }
}

/// Read the shared activity/saturation status word exactly once.
///
/// Complete pinned `libphy.a[phy_rx_cal.o]::phy_check_rx_sat`, size `0x76`,
/// samples the PAC activity field exactly 100 times. The bounded repeat count
/// remains in the caller-driven PHY transition.
#[cfg(target_arch = "riscv32")]
pub fn read_activity_status(registers: &mut RadioRegisters) -> u32 {
    read_activity_status_with(registers)
}

#[cfg(any(test, target_arch = "riscv32"))]
fn read_activity_status_with(io: &mut impl RegisterIo) -> u32 {
    io.read(iq::ESTIMATOR_ACTIVITY_STATUS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Operation {
        Read(usize),
        Write(usize, u32),
    }

    struct Model {
        values: [(usize, u32); 11],
        operations: Vec<Operation>,
    }

    impl Model {
        fn new(fill: u32) -> Self {
            Self {
                values: [
                    (iq::ESTIMATOR_CONFIG.address(), fill),
                    (iq::ESTIMATOR_CONTROL.address(), fill),
                    (iq::SIGNAL_POWER_SUM_I.address(), fill),
                    (iq::SIGNAL_POWER_DIFFERENCE_I.address(), fill),
                    (iq::SIGNAL_POWER_DIFFERENCE_Q.address(), fill),
                    (iq::SIGNAL_POWER_SUM_Q.address(), fill),
                    (iq::DC_I_ACCUMULATOR.address(), fill),
                    (iq::DC_Q_ACCUMULATOR.address(), fill),
                    (iq::POWER_ACCUMULATOR.address(), fill),
                    (iq::ESTIMATOR_READY_STATUS.address(), fill),
                    (iq::ESTIMATOR_ACTIVITY_STATUS.address(), fill),
                ],
                operations: Vec::new(),
            }
        }

        fn set(&mut self, register: Register32, value: u32) {
            self.values
                .iter_mut()
                .find(|(address, _)| *address == register.address())
                .expect("modeled PAC register")
                .1 = value;
        }

        fn value(&self, register: Register32) -> u32 {
            self.values
                .iter()
                .find(|(address, _)| *address == register.address())
                .expect("modeled PAC register")
                .1
        }
    }

    impl RegisterIo for Model {
        fn read(&mut self, register: Register32) -> u32 {
            self.operations.push(Operation::Read(register.address()));
            self.value(register)
        }

        fn write(&mut self, register: Register32, value: u32) {
            self.operations
                .push(Operation::Write(register.address(), value));
            self.set(register, value);
        }
    }

    #[test]
    fn configure_preserves_the_three_rom_fresh_reads() {
        let mut model = Model::new(u32::MAX);
        configure_with(&mut model, 0x8fa0);

        assert_eq!(model.value(iq::ESTIMATOR_CONFIG), 0xf7ff_ffff);
        assert_eq!(model.value(iq::ESTIMATOR_CONTROL), 0xfff6_3e83);
        assert_eq!(
            model.operations,
            [
                Operation::Read(iq::ESTIMATOR_CONFIG.address()),
                Operation::Write(iq::ESTIMATOR_CONFIG.address(), 0xf7ff_ffff),
                Operation::Read(iq::ESTIMATOR_CONTROL.address()),
                Operation::Write(iq::ESTIMATOR_CONTROL.address(), 0xfff7_ffff),
                Operation::Read(iq::ESTIMATOR_CONTROL.address()),
                Operation::Write(iq::ESTIMATOR_CONTROL.address(), 0xfff6_3e83),
            ]
        );
    }

    #[test]
    fn enable_phases_are_independent_fresh_read_updates() {
        let mut model = Model::new(0);
        set_start_enabled_with(&mut model, true);
        set_measurement_enabled_with(&mut model, true);
        set_measurement_enabled_with(&mut model, false);
        set_start_enabled_with(&mut model, false);

        assert_eq!(model.value(iq::ESTIMATOR_CONTROL), 0);
        assert_eq!(model.operations.len(), 8);
        assert_eq!(
            model.operations,
            [
                Operation::Read(iq::ESTIMATOR_CONTROL.address()),
                Operation::Write(iq::ESTIMATOR_CONTROL.address(), 1),
                Operation::Read(iq::ESTIMATOR_CONTROL.address()),
                Operation::Write(iq::ESTIMATOR_CONTROL.address(), 3),
                Operation::Read(iq::ESTIMATOR_CONTROL.address()),
                Operation::Write(iq::ESTIMATOR_CONTROL.address(), 1),
                Operation::Read(iq::ESTIMATOR_CONTROL.address()),
                Operation::Write(iq::ESTIMATOR_CONTROL.address(), 0),
            ]
        );
    }

    #[test]
    fn readiness_and_dc_accumulators_keep_the_rom_read_order() {
        let mut model = Model::new(0);
        model.set(iq::ESTIMATOR_READY_STATUS, 1 << 16);
        model.set(iq::ESTIMATOR_ACTIVITY_STATUS, 2 << 20);
        model.set(iq::DC_I_ACCUMULATOR, (-3_i32) as u32);
        model.set(iq::DC_Q_ACCUMULATOR, 4);
        model.set(iq::POWER_ACCUMULATOR, (-5_i32) as u32);

        assert_eq!(
            sample_readiness_with(&mut model),
            ReadinessSnapshot {
                ready: true,
                activity: true,
            }
        );
        assert_eq!(
            read_dc_iq_accumulators_with(&mut model),
            DcIqAccumulatorSnapshot {
                i: -3,
                q: 4,
                power: -5,
            }
        );
        assert_eq!(
            model.operations,
            [
                Operation::Read(iq::ESTIMATOR_READY_STATUS.address()),
                Operation::Read(iq::ESTIMATOR_ACTIVITY_STATUS.address()),
                Operation::Read(iq::DC_I_ACCUMULATOR.address()),
                Operation::Read(iq::DC_Q_ACCUMULATOR.address()),
                Operation::Read(iq::POWER_ACCUMULATOR.address()),
            ]
        );
    }

    #[test]
    fn mismatch_and_signal_power_retain_distinct_complete_rom_orders() {
        let mut mismatch = Model::new(0);
        mismatch.set(iq::SIGNAL_POWER_SUM_I, 1);
        mismatch.set(iq::SIGNAL_POWER_DIFFERENCE_I, 2);
        mismatch.set(iq::SIGNAL_POWER_DIFFERENCE_Q, 3);
        mismatch.set(iq::SIGNAL_POWER_SUM_Q, 4);
        assert_eq!(
            read_rxiq_mismatch_with(&mut mismatch),
            SignalPowerSnapshot {
                sum_i: 1,
                difference_i: 2,
                difference_q: 3,
                sum_q: 4,
            }
        );
        assert_eq!(
            mismatch.operations,
            [
                Operation::Read(iq::SIGNAL_POWER_SUM_I.address()),
                Operation::Read(iq::SIGNAL_POWER_SUM_Q.address()),
                Operation::Read(iq::SIGNAL_POWER_DIFFERENCE_Q.address()),
                Operation::Read(iq::SIGNAL_POWER_DIFFERENCE_I.address()),
            ]
        );

        let mut signal = Model::new(0);
        let _ = read_signal_power_with(&mut signal);
        assert_eq!(
            signal.operations,
            [
                Operation::Read(iq::SIGNAL_POWER_SUM_I.address()),
                Operation::Read(iq::SIGNAL_POWER_SUM_Q.address()),
                Operation::Read(iq::SIGNAL_POWER_DIFFERENCE_I.address()),
                Operation::Read(iq::SIGNAL_POWER_DIFFERENCE_Q.address()),
            ]
        );
    }

    #[test]
    fn total_power_is_one_signed_pac_read() {
        let mut model = Model::new(0);
        model.set(iq::POWER_ACCUMULATOR, (-17_i32) as u32);
        assert_eq!(read_total_power_with(&mut model), -17);
        assert_eq!(
            model.operations,
            [Operation::Read(iq::POWER_ACCUMULATOR.address())]
        );
    }

    #[test]
    fn rx_saturation_uses_the_shared_activity_identity() {
        let mut model = Model::new(0);
        model.set(iq::ESTIMATOR_ACTIVITY_STATUS, 0x1234_5678);
        assert_eq!(read_activity_status_with(&mut model), 0x1234_5678);
        assert_eq!(
            model.operations,
            [Operation::Read(iq::ESTIMATOR_ACTIVITY_STATUS.address())]
        );
    }
}
