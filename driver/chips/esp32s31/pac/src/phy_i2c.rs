//! Ownership-bound access to the shared PHY analog-I²C master.

#![forbid(unsafe_code)]

use super::{
    BluetoothTxPowerControlPrepareError, BluetoothTxPowerControlRestoreError, RadioPhyRegisters,
};

/// One of the two reviewed analog-register command hosts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyI2cHost {
    Host0,
    Host1,
}

/// One complete PAC-owned Bluetooth TX-power analog-control operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothTxPowerControlOperation {
    /// Capture the two reviewed source values into the private PAC restore slot.
    PrepareRestore,
    /// Force the four reviewed analog-control registers for calibration.
    ConfigureCalibration,
    /// Restore all four analog-control registers and release the PAC slot.
    Restore,
}

/// Current externally driven edge of a Bluetooth TX-power control transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothTxPowerControlAction {
    StartCommand,
    AwaitCompletionEdge,
    Complete(BluetoothTxPowerControlCompletion),
}

/// Semantic result of a complete Bluetooth TX-power control transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothTxPowerControlCompletion {
    RestorePrepared,
    CalibrationConfigured,
    Restored,
}

/// One independently delivered analog-I²C completion edge was consumed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothTxPowerControlObservation {
    StillPending,
    EdgeConsumed,
}

/// A PAC-owned Bluetooth TX-power control transaction could not advance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothTxPowerControlError {
    BusyAtStart,
    RestorePending,
    RestoreNotPending,
    WrongAction,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BluetoothTxPowerControlRegister {
    Low0,
    Low1,
    High0,
    High1,
}

impl BluetoothTxPowerControlRegister {
    const fn address(self) -> u8 {
        match self {
            Self::Low0 => 0x1c,
            Self::Low1 => 0x1d,
            Self::High0 => 0x1e,
            Self::High1 => 0x1f,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BluetoothTxPowerControlPhase {
    Start,
    Await,
    Complete,
}

trait BluetoothTxPowerControlI2cAccess {
    fn reserve_restore(&mut self) -> Result<(), BluetoothTxPowerControlPrepareError>;
    fn capture_low(&mut self, value: u8) -> Result<(), BluetoothTxPowerControlRestoreError>;
    fn capture_high(&mut self, value: u8) -> Result<(), BluetoothTxPowerControlRestoreError>;
    fn restore_values(&self) -> Result<(u8, u8), BluetoothTxPowerControlRestoreError>;
    fn finish_restore(&mut self) -> Result<(), BluetoothTxPowerControlRestoreError>;
    fn start_read(&mut self, register: BluetoothTxPowerControlRegister) -> Result<(), ()>;
    fn start_write(
        &mut self,
        register: BluetoothTxPowerControlRegister,
        value: u8,
    ) -> Result<(), ()>;
    fn observe_read(&self) -> Result<u8, ()>;
    fn observe_write(&self) -> Result<(), ()>;
}

/// Non-cloneable transaction which keeps all four analog-register identities,
/// field geometry and saved values inside the PAC.
#[derive(Debug, Eq, PartialEq)]
pub struct BluetoothTxPowerControlTransaction {
    operation: BluetoothTxPowerControlOperation,
    phase: BluetoothTxPowerControlPhase,
    step: u8,
    scratch: u8,
    restore_reserved: bool,
}

impl BluetoothTxPowerControlTransaction {
    pub const fn new(operation: BluetoothTxPowerControlOperation) -> Self {
        Self {
            operation,
            phase: BluetoothTxPowerControlPhase::Start,
            step: 0,
            scratch: 0,
            restore_reserved: false,
        }
    }

    pub const fn action(&self) -> BluetoothTxPowerControlAction {
        match self.phase {
            BluetoothTxPowerControlPhase::Start => BluetoothTxPowerControlAction::StartCommand,
            BluetoothTxPowerControlPhase::Await => {
                BluetoothTxPowerControlAction::AwaitCompletionEdge
            }
            BluetoothTxPowerControlPhase::Complete => {
                BluetoothTxPowerControlAction::Complete(match self.operation {
                    BluetoothTxPowerControlOperation::PrepareRestore => {
                        BluetoothTxPowerControlCompletion::RestorePrepared
                    }
                    BluetoothTxPowerControlOperation::ConfigureCalibration => {
                        BluetoothTxPowerControlCompletion::CalibrationConfigured
                    }
                    BluetoothTxPowerControlOperation::Restore => {
                        BluetoothTxPowerControlCompletion::Restored
                    }
                })
            }
        }
    }

    pub fn start(
        &mut self,
        registers: &mut RadioPhyRegisters,
    ) -> Result<(), BluetoothTxPowerControlError> {
        self.start_with(registers)
    }

    pub fn observe_completion_edge(
        &mut self,
        registers: &mut RadioPhyRegisters,
    ) -> Result<BluetoothTxPowerControlObservation, BluetoothTxPowerControlError> {
        self.observe_with(registers)
    }

    fn start_with(
        &mut self,
        access: &mut impl BluetoothTxPowerControlI2cAccess,
    ) -> Result<(), BluetoothTxPowerControlError> {
        match self.phase {
            BluetoothTxPowerControlPhase::Complete => {
                return Err(BluetoothTxPowerControlError::AlreadyComplete);
            }
            BluetoothTxPowerControlPhase::Await => {
                return Err(BluetoothTxPowerControlError::WrongAction);
            }
            BluetoothTxPowerControlPhase::Start => {}
        }
        let start_result = match self.operation {
            BluetoothTxPowerControlOperation::PrepareRestore => self.start_prepare(access)?,
            BluetoothTxPowerControlOperation::ConfigureCalibration
            | BluetoothTxPowerControlOperation::Restore => self.start_write_plan(access)?,
        };
        if start_result.is_err() {
            return Err(BluetoothTxPowerControlError::BusyAtStart);
        }
        self.phase = BluetoothTxPowerControlPhase::Await;
        Ok(())
    }

    fn observe_with(
        &mut self,
        access: &mut impl BluetoothTxPowerControlI2cAccess,
    ) -> Result<BluetoothTxPowerControlObservation, BluetoothTxPowerControlError> {
        match self.phase {
            BluetoothTxPowerControlPhase::Complete => {
                return Err(BluetoothTxPowerControlError::AlreadyComplete);
            }
            BluetoothTxPowerControlPhase::Start => {
                return Err(BluetoothTxPowerControlError::WrongAction);
            }
            BluetoothTxPowerControlPhase::Await => {}
        }
        let observation = match self.operation {
            BluetoothTxPowerControlOperation::PrepareRestore => self.observe_prepare(access)?,
            BluetoothTxPowerControlOperation::ConfigureCalibration
            | BluetoothTxPowerControlOperation::Restore => self.observe_write_plan(access)?,
        };
        let Some(complete) = observation else {
            return Ok(BluetoothTxPowerControlObservation::StillPending);
        };
        if complete {
            self.phase = BluetoothTxPowerControlPhase::Complete;
        } else {
            self.step += 1;
            self.phase = BluetoothTxPowerControlPhase::Start;
        }
        Ok(BluetoothTxPowerControlObservation::EdgeConsumed)
    }

    fn start_prepare(
        &mut self,
        access: &mut impl BluetoothTxPowerControlI2cAccess,
    ) -> Result<Result<(), ()>, BluetoothTxPowerControlError> {
        if self.step == 0 && !self.restore_reserved {
            access.reserve_restore().map_err(|error| match error {
                BluetoothTxPowerControlPrepareError::RestorePending => {
                    BluetoothTxPowerControlError::RestorePending
                }
            })?;
            self.restore_reserved = true;
        }
        Ok(access.start_read(if self.step == 0 {
            BluetoothTxPowerControlRegister::Low0
        } else {
            BluetoothTxPowerControlRegister::High0
        }))
    }

    fn start_write_plan(
        &self,
        access: &mut impl BluetoothTxPowerControlI2cAccess,
    ) -> Result<Result<(), ()>, BluetoothTxPowerControlError> {
        self.require_restore(access)?;
        Ok(match self.step {
            0 => access.start_write(
                BluetoothTxPowerControlRegister::Low0,
                self.low_value(access)?,
            ),
            1 => access.start_write(
                BluetoothTxPowerControlRegister::Low1,
                self.low_value(access)?,
            ),
            2 => access.start_read(BluetoothTxPowerControlRegister::High0),
            3 => access.start_write(
                BluetoothTxPowerControlRegister::High0,
                replace_bluetooth_tx_power_control_field(self.scratch, self.high_value(access)?),
            ),
            4 => access.start_read(BluetoothTxPowerControlRegister::High1),
            5 => access.start_write(
                BluetoothTxPowerControlRegister::High1,
                replace_bluetooth_tx_power_control_field(self.scratch, self.high_value(access)?),
            ),
            _ => return Err(BluetoothTxPowerControlError::WrongAction),
        })
    }

    fn observe_prepare(
        &mut self,
        access: &mut impl BluetoothTxPowerControlI2cAccess,
    ) -> Result<Option<bool>, BluetoothTxPowerControlError> {
        let value = match access.observe_read() {
            Ok(value) => value,
            Err(()) => return Ok(None),
        };
        if self.step == 0 {
            access.capture_low(value).map_err(map_restore_error)?;
            Ok(Some(false))
        } else {
            access
                .capture_high(extract_bluetooth_tx_power_control_field(value))
                .map_err(map_restore_error)?;
            Ok(Some(true))
        }
    }

    fn observe_write_plan(
        &mut self,
        access: &mut impl BluetoothTxPowerControlI2cAccess,
    ) -> Result<Option<bool>, BluetoothTxPowerControlError> {
        match self.step {
            2 | 4 => {
                self.scratch = match access.observe_read() {
                    Ok(value) => value,
                    Err(()) => return Ok(None),
                };
            }
            _ => {
                if access.observe_write().is_err() {
                    return Ok(None);
                }
            }
        }
        if self.step != 5 {
            return Ok(Some(false));
        }
        if self.operation == BluetoothTxPowerControlOperation::Restore {
            access.finish_restore().map_err(map_restore_error)?;
        }
        Ok(Some(true))
    }

    fn require_restore(
        &self,
        access: &impl BluetoothTxPowerControlI2cAccess,
    ) -> Result<(), BluetoothTxPowerControlError> {
        access
            .restore_values()
            .map(|_| ())
            .map_err(map_restore_error)
    }

    fn low_value(
        &self,
        access: &impl BluetoothTxPowerControlI2cAccess,
    ) -> Result<u8, BluetoothTxPowerControlError> {
        match self.operation {
            BluetoothTxPowerControlOperation::ConfigureCalibration => {
                self.require_restore(access)?;
                Ok(2)
            }
            BluetoothTxPowerControlOperation::Restore => access
                .restore_values()
                .map(|values| values.0)
                .map_err(map_restore_error),
            BluetoothTxPowerControlOperation::PrepareRestore => {
                Err(BluetoothTxPowerControlError::WrongAction)
            }
        }
    }

    fn high_value(
        &self,
        access: &impl BluetoothTxPowerControlI2cAccess,
    ) -> Result<u8, BluetoothTxPowerControlError> {
        match self.operation {
            BluetoothTxPowerControlOperation::ConfigureCalibration => {
                self.require_restore(access)?;
                Ok(2)
            }
            BluetoothTxPowerControlOperation::Restore => access
                .restore_values()
                .map(|values| values.1)
                .map_err(map_restore_error),
            BluetoothTxPowerControlOperation::PrepareRestore => {
                Err(BluetoothTxPowerControlError::WrongAction)
            }
        }
    }
}

const fn extract_bluetooth_tx_power_control_field(value: u8) -> u8 {
    value & 0x3f
}

const fn replace_bluetooth_tx_power_control_field(register: u8, field: u8) -> u8 {
    (register & !0x3f) | (field & 0x3f)
}

const fn map_restore_error(
    error: BluetoothTxPowerControlRestoreError,
) -> BluetoothTxPowerControlError {
    match error {
        BluetoothTxPowerControlRestoreError::RestoreNotPending => {
            BluetoothTxPowerControlError::RestoreNotPending
        }
    }
}

impl RadioPhyRegisters {
    /// Install the complete reviewed PHY-I²C host map with one fresh RMW.
    pub fn configure_phy_i2c_host_map(&mut self) {
        self.peripherals
            .i2c_ana_mst
            .ana_conf2()
            .modify(|_, w| w.phy_host_map().reviewed_radio_map());
    }

    /// Publish the finite reset command for one analog-I²C host.
    pub fn pulse_phy_i2c_master_reset(&mut self, host: PhyI2cHost) {
        match host {
            PhyI2cHost::Host0 => self
                .peripherals
                .i2c_ana_mst
                .i2c0_ctrl()
                .write(|w| w.start_or_reset().set_bit()),
            PhyI2cHost::Host1 => self
                .peripherals
                .i2c_ana_mst
                .i2c1_ctrl()
                .write(|w| w.start_or_reset().set_bit()),
        };
    }

    /// Sample the reviewed completion predicate for one host.
    pub fn phy_i2c_master_is_busy(&self, host: PhyI2cHost) -> bool {
        match host {
            PhyI2cHost::Host0 => self
                .peripherals
                .i2c_ana_mst
                .i2c0_ctrl()
                .read()
                .busy()
                .bit_is_set(),
            PhyI2cHost::Host1 => self
                .peripherals
                .i2c_ana_mst
                .i2c1_ctrl()
                .read()
                .busy()
                .bit_is_set(),
        }
    }

    /// Publish the complete complemented read mask used by the vendor leaf.
    pub fn publish_phy_i2c_read_mask(&mut self, read_mask: u16) {
        let complement = !u32::from(read_mask);
        self.peripherals.i2c_ana_mst.ana_conf1().write(|w| {
            w.read_mask_complement_low()
                .set(complement & 0x00ff_ffff)
                .read_mask_complement_high()
                .set((complement >> 24) as u8)
        });
    }

    /// Publish one complete host command in the reviewed vendor order.
    pub fn publish_phy_i2c_command(
        &mut self,
        host: PhyI2cHost,
        block: u8,
        register: u8,
        value: u8,
        write: bool,
    ) {
        match host {
            PhyI2cHost::Host0 => self.peripherals.i2c_ana_mst.i2c0_ctrl().write(|w| {
                w.slave_addr()
                    .set(block)
                    .slave_reg_addr()
                    .set(register)
                    .data()
                    .set(value)
                    .read_write()
                    .bit(write)
                    .start_or_reset()
                    .set_bit()
            }),
            PhyI2cHost::Host1 => self.peripherals.i2c_ana_mst.i2c1_ctrl().write(|w| {
                w.slave_addr()
                    .set(block)
                    .slave_reg_addr()
                    .set(register)
                    .data()
                    .set(value)
                    .read_write()
                    .bit(write)
                    .start_or_reset()
                    .set_bit()
            }),
        };
    }

    /// Sample the completed data byte from one host.
    pub fn sample_phy_i2c_result(&self, host: PhyI2cHost) -> u8 {
        match host {
            PhyI2cHost::Host0 => self
                .peripherals
                .i2c_ana_mst
                .i2c0_ctrl()
                .read()
                .data()
                .bits(),
            PhyI2cHost::Host1 => self
                .peripherals
                .i2c_ana_mst
                .i2c1_ctrl()
                .read()
                .data()
                .bits(),
        }
    }

    /// Apply all six timing RMWs in the complete vendor order.
    pub fn configure_phy_i2c_clock_selection(&mut self, selection: u32) {
        let side_guard = ((selection >> 2) & 0x1f) as u8;
        let pulse_duration = ((selection >> 1) & 0x3f) as u8;
        let registers = &self.peripherals.i2c_ana_mst;

        registers
            .i2c0_ctrl1()
            .modify(|_, w| w.sda_side_guard().set(side_guard));
        registers
            .i2c0_ctrl1()
            .modify(|_, w| w.scl_pulse_duration().set(pulse_duration));
        registers
            .i2c1_ctrl1()
            .modify(|_, w| w.sda_side_guard().set(side_guard));
        registers
            .i2c1_ctrl1()
            .modify(|_, w| w.scl_pulse_duration().set(pulse_duration));
        registers
            .hw_i2c_ctrl()
            .modify(|_, w| w.sda_side_guard().set(side_guard));
        registers
            .hw_i2c_ctrl()
            .modify(|_, w| w.scl_pulse_duration().set(pulse_duration));
    }

    /// Select register mode two, then enable it with a separate fresh RMW.
    pub fn configure_phy_i2c_master_registers(&mut self) {
        let control = self.peripherals.i2c_ana_mst.ana_conf0();
        control.modify(|_, w| w.phy_register_mode().register_mode());
        control.modify(|_, w| w.phy_register_enable().set_bit());
    }

    /// Select one of the two complete-ROM BBPLL calibration encodings.
    pub fn set_phy_i2c_bbpll_calibration(&mut self, enabled: bool) {
        self.peripherals.i2c_ana_mst.ana_conf0().modify(|_, w| {
            if enabled {
                w.bbpll_cal_mode().enabled()
            } else {
                w.bbpll_cal_mode().disabled()
            }
        });
    }

    /// Publish one recovered command-RAM entry.
    ///
    /// Returns false for an invalid index. The PAC owns the command-memory
    /// field geometry and publishes zero to every unreviewed register bit.
    pub fn write_phy_i2c_command_memory(
        &mut self,
        index: usize,
        block: u8,
        register: u8,
        value: u8,
    ) -> bool {
        if index >= 45 {
            return false;
        }
        super::svd::zero_based_field_write::phy_i2c_command_memory(
            &self.peripherals.phy_i2c_command_ram,
            index,
            block,
            register,
            value,
        );
        true
    }
}

impl BluetoothTxPowerControlI2cAccess for RadioPhyRegisters {
    fn reserve_restore(&mut self) -> Result<(), BluetoothTxPowerControlPrepareError> {
        self.restore_slot.prepare_bluetooth_tx_power_control()
    }

    fn capture_low(&mut self, value: u8) -> Result<(), BluetoothTxPowerControlRestoreError> {
        self.restore_slot
            .capture_bluetooth_tx_power_control_low(value)
    }

    fn capture_high(&mut self, value: u8) -> Result<(), BluetoothTxPowerControlRestoreError> {
        self.restore_slot
            .capture_bluetooth_tx_power_control_high(value)
    }

    fn restore_values(&self) -> Result<(u8, u8), BluetoothTxPowerControlRestoreError> {
        self.restore_slot.bluetooth_tx_power_control_values()
    }

    fn finish_restore(&mut self) -> Result<(), BluetoothTxPowerControlRestoreError> {
        self.restore_slot
            .finish_bluetooth_tx_power_control_restore()
    }

    fn start_read(&mut self, register: BluetoothTxPowerControlRegister) -> Result<(), ()> {
        self.configure_phy_i2c_host_map();
        if self.phy_i2c_master_is_busy(PhyI2cHost::Host1) {
            return Err(());
        }
        self.publish_phy_i2c_read_mask(0x0004);
        self.publish_phy_i2c_command(PhyI2cHost::Host1, 0x67, register.address(), 0, false);
        Ok(())
    }

    fn start_write(
        &mut self,
        register: BluetoothTxPowerControlRegister,
        value: u8,
    ) -> Result<(), ()> {
        self.configure_phy_i2c_host_map();
        if self.phy_i2c_master_is_busy(PhyI2cHost::Host1) {
            return Err(());
        }
        self.publish_phy_i2c_command(PhyI2cHost::Host1, 0x67, register.address(), value, true);
        Ok(())
    }

    fn observe_read(&self) -> Result<u8, ()> {
        if self.phy_i2c_master_is_busy(PhyI2cHost::Host1) {
            Err(())
        } else {
            Ok(self.sample_phy_i2c_result(PhyI2cHost::Host1))
        }
    }

    fn observe_write(&self) -> Result<(), ()> {
        if self.phy_i2c_master_is_busy(PhyI2cHost::Host1) {
            Err(())
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, collections::VecDeque, vec::Vec};

    use super::{
        BluetoothTxPowerControlAction, BluetoothTxPowerControlCompletion,
        BluetoothTxPowerControlError, BluetoothTxPowerControlI2cAccess,
        BluetoothTxPowerControlObservation, BluetoothTxPowerControlOperation,
        BluetoothTxPowerControlPrepareError, BluetoothTxPowerControlRegister,
        BluetoothTxPowerControlRestoreError, BluetoothTxPowerControlTransaction,
    };

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Operation {
        Read(BluetoothTxPowerControlRegister),
        Write(BluetoothTxPowerControlRegister, u8),
    }

    struct FakeI2c {
        restore: Option<(u8, u8)>,
        reads: RefCell<VecDeque<u8>>,
        operations: Vec<Operation>,
        busy_starts: u8,
    }

    impl FakeI2c {
        fn new(reads: impl IntoIterator<Item = u8>) -> Self {
            Self {
                restore: None,
                reads: RefCell::new(reads.into_iter().collect()),
                operations: Vec::new(),
                busy_starts: 0,
            }
        }

        fn accept_start(&mut self) -> Result<(), ()> {
            if self.busy_starts == 0 {
                Ok(())
            } else {
                self.busy_starts -= 1;
                Err(())
            }
        }
    }

    impl BluetoothTxPowerControlI2cAccess for FakeI2c {
        fn reserve_restore(&mut self) -> Result<(), BluetoothTxPowerControlPrepareError> {
            if self.restore.is_some() {
                Err(BluetoothTxPowerControlPrepareError::RestorePending)
            } else {
                self.restore = Some((0, 0));
                Ok(())
            }
        }

        fn capture_low(&mut self, value: u8) -> Result<(), BluetoothTxPowerControlRestoreError> {
            let Some((_, high)) = self.restore else {
                return Err(BluetoothTxPowerControlRestoreError::RestoreNotPending);
            };
            self.restore = Some((value, high));
            Ok(())
        }

        fn capture_high(&mut self, value: u8) -> Result<(), BluetoothTxPowerControlRestoreError> {
            let Some((low, _)) = self.restore else {
                return Err(BluetoothTxPowerControlRestoreError::RestoreNotPending);
            };
            self.restore = Some((low, value));
            Ok(())
        }

        fn restore_values(&self) -> Result<(u8, u8), BluetoothTxPowerControlRestoreError> {
            self.restore
                .ok_or(BluetoothTxPowerControlRestoreError::RestoreNotPending)
        }

        fn finish_restore(&mut self) -> Result<(), BluetoothTxPowerControlRestoreError> {
            self.restore_values()?;
            self.restore = None;
            Ok(())
        }

        fn start_read(&mut self, register: BluetoothTxPowerControlRegister) -> Result<(), ()> {
            self.accept_start()?;
            self.operations.push(Operation::Read(register));
            Ok(())
        }

        fn start_write(
            &mut self,
            register: BluetoothTxPowerControlRegister,
            value: u8,
        ) -> Result<(), ()> {
            self.accept_start()?;
            self.operations.push(Operation::Write(register, value));
            Ok(())
        }

        fn observe_read(&self) -> Result<u8, ()> {
            self.reads.borrow_mut().pop_front().ok_or(())
        }

        fn observe_write(&self) -> Result<(), ()> {
            Ok(())
        }
    }

    fn drive(
        transaction: &mut BluetoothTxPowerControlTransaction,
        access: &mut FakeI2c,
    ) -> BluetoothTxPowerControlCompletion {
        for _ in 0..32 {
            match transaction.action() {
                BluetoothTxPowerControlAction::StartCommand => {
                    transaction.start_with(access).unwrap();
                }
                BluetoothTxPowerControlAction::AwaitCompletionEdge => {
                    assert_eq!(
                        transaction.observe_with(access).unwrap(),
                        BluetoothTxPowerControlObservation::EdgeConsumed
                    );
                }
                BluetoothTxPowerControlAction::Complete(completion) => return completion,
            }
        }
        panic!("Bluetooth TX-power control transaction exceeded its finite plan")
    }

    #[test]
    fn prepare_configure_and_restore_keep_analog_geometry_and_snapshot_in_pac() {
        let mut access = FakeI2c::new([0xa5, 0xd5]);
        let mut prepare = BluetoothTxPowerControlTransaction::new(
            BluetoothTxPowerControlOperation::PrepareRestore,
        );
        assert_eq!(
            drive(&mut prepare, &mut access),
            BluetoothTxPowerControlCompletion::RestorePrepared
        );
        assert_eq!(access.restore, Some((0xa5, 0x15)));

        access.operations.clear();
        *access.reads.borrow_mut() = [0xc0, 0x80].into_iter().collect();
        let mut configure = BluetoothTxPowerControlTransaction::new(
            BluetoothTxPowerControlOperation::ConfigureCalibration,
        );
        assert_eq!(
            drive(&mut configure, &mut access),
            BluetoothTxPowerControlCompletion::CalibrationConfigured
        );
        assert_eq!(
            access.operations,
            [
                Operation::Write(BluetoothTxPowerControlRegister::Low0, 2),
                Operation::Write(BluetoothTxPowerControlRegister::Low1, 2),
                Operation::Read(BluetoothTxPowerControlRegister::High0),
                Operation::Write(BluetoothTxPowerControlRegister::High0, 0xc2),
                Operation::Read(BluetoothTxPowerControlRegister::High1),
                Operation::Write(BluetoothTxPowerControlRegister::High1, 0x82),
            ]
        );

        access.operations.clear();
        *access.reads.borrow_mut() = [0xc0, 0x80].into_iter().collect();
        let mut restore =
            BluetoothTxPowerControlTransaction::new(BluetoothTxPowerControlOperation::Restore);
        assert_eq!(
            drive(&mut restore, &mut access),
            BluetoothTxPowerControlCompletion::Restored
        );
        assert_eq!(
            access.operations,
            [
                Operation::Write(BluetoothTxPowerControlRegister::Low0, 0xa5),
                Operation::Write(BluetoothTxPowerControlRegister::Low1, 0xa5),
                Operation::Read(BluetoothTxPowerControlRegister::High0),
                Operation::Write(BluetoothTxPowerControlRegister::High0, 0xd5),
                Operation::Read(BluetoothTxPowerControlRegister::High1),
                Operation::Write(BluetoothTxPowerControlRegister::High1, 0x95),
            ]
        );
        assert_eq!(access.restore, None);
    }

    #[test]
    fn prepare_rejects_an_owned_restore_before_starting_i2c() {
        let mut access = FakeI2c::new([]);
        access.restore = Some((0, 0));
        let mut transaction = BluetoothTxPowerControlTransaction::new(
            BluetoothTxPowerControlOperation::PrepareRestore,
        );
        assert_eq!(
            transaction.start_with(&mut access),
            Err(BluetoothTxPowerControlError::RestorePending)
        );
        assert!(access.operations.is_empty());
    }

    #[test]
    fn prepare_retries_a_busy_first_command_without_reserving_restore_twice() {
        let mut access = FakeI2c::new([0xa5, 0xd5]);
        access.busy_starts = 1;
        let mut transaction = BluetoothTxPowerControlTransaction::new(
            BluetoothTxPowerControlOperation::PrepareRestore,
        );

        assert_eq!(
            transaction.start_with(&mut access),
            Err(BluetoothTxPowerControlError::BusyAtStart)
        );
        assert_eq!(
            transaction.action(),
            BluetoothTxPowerControlAction::StartCommand
        );
        assert!(access.operations.is_empty());

        assert_eq!(
            drive(&mut transaction, &mut access),
            BluetoothTxPowerControlCompletion::RestorePrepared
        );
        assert_eq!(access.restore, Some((0xa5, 0x15)));
    }
}
