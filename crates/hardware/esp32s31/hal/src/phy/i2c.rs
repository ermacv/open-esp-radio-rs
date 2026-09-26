//! Owned access to the ESP32-S31 PHY analog-register I2C master.
//!
//! The restricted PAC owns analog identities, command tables and single
//! command transactions. This module owns the multi-command configuration
//! and Bluetooth TX-power phase machines, the bounded retained-wake polling
//! loop and the Bluetooth restore obligation.

pub use oer_esp32s31_pac::{
    BluetoothTxPowerControlRegister, PhyAdcRate, PhyFilterDcapInputs, PhyI2cAccessError,
    PhyI2cAddress, PhyI2cBlock, PhyI2cCommandMemoryInputs, PhyI2cConfigurationCommand,
    PhyI2cConfigurationOperation, PhyI2cField, PhyI2cHost, PhyI2cInitializationStageOneInputs,
    PhyI2cParallelWrite, analog_registers,
};

use oer_esp32s31_pac::RadioPhyRegisters;

pub use crate::phy::restore::{
    BluetoothTxPowerControlPrepareError, BluetoothTxPowerControlRestoreError,
};
use crate::{
    owner::{SharedPhyAccess, phy_parts_mut},
    phy::restore::PhyRouteState,
    phy_pac, phy_pac_mut,
};

/// One finite failure while publishing current-vendor retained-wake analog
/// initialization through both PHY-I²C hosts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyI2cInitializationStageTwoError {
    BusyAtStart { host: PhyI2cHost },
    CompletionTimeout { host: PhyI2cHost, pair: u8 },
}

trait PhyI2cParallelAccess {
    fn select_parallel_host_map(&mut self);
    fn restore_radio_host_map(&mut self);
    fn is_busy(&self, host: PhyI2cHost) -> bool;
    fn start_pair(&mut self, pair: PhyI2cParallelWrite);
}

fn configure_initialization_stage_two_with(
    access: &mut impl PhyI2cParallelAccess,
    inputs: PhyI2cCommandMemoryInputs,
    maximum_observations: u32,
) -> Result<(), PhyI2cInitializationStageTwoError> {
    access.select_parallel_host_map();

    let result = (|| {
        for host in [PhyI2cHost::Host0, PhyI2cHost::Host1] {
            if access.is_busy(host) {
                return Err(PhyI2cInitializationStageTwoError::BusyAtStart { host });
            }
        }

        let mut pair_index = 0;
        while let Some(pair) = inputs.initialization_stage_two_pair(pair_index) {
            access.start_pair(pair);

            for host in [PhyI2cHost::Host0, PhyI2cHost::Host1] {
                let mut observations = 0;
                while access.is_busy(host) {
                    if observations == maximum_observations {
                        return Err(PhyI2cInitializationStageTwoError::CompletionTimeout {
                            host,
                            pair: pair_index as u8,
                        });
                    }
                    observations += 1;
                }
            }
            pair_index += 1;
        }
        Ok(())
    })();

    // The vendor leaf restores the normal 0x3fa0 host field after the
    // parallel sequence. OER also restores it on finite failure so diagnostic
    // access remains well-defined; the outer RF-wake epoch still fails closed.
    access.restore_radio_host_map();
    result
}

/// Current externally driven edge of a PHY-I²C configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyI2cConfigurationAction {
    StartCommand,
    AwaitCompletionEdge,
    Complete,
}

/// One independently delivered configuration-completion edge was consumed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyI2cConfigurationObservation {
    StillPending,
    EdgeConsumed,
}

/// A PHY-I²C configuration transaction could not advance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyI2cConfigurationError {
    BusyAtStart,
    WrongAction,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyI2cConfigurationPhase {
    Start,
    AwaitRead,
    AwaitWrite,
    Complete,
}

trait PhyI2cConfigurationAccess {
    fn start_read(&mut self, address: PhyI2cAddress) -> Result<(), ()>;
    fn start_write(&mut self, address: PhyI2cAddress, value: u8) -> Result<(), ()>;
    fn observe_read(&self) -> Result<u8, ()>;
    fn observe_write(&self) -> Result<(), ()>;
}

/// Non-cloneable owner of one complete recovered PHY-I²C write plan.
///
/// The PAC supplies the operation's commands with opaque analog identities;
/// this transaction owns the phase, read-modify-write value and completion
/// edges. Callers can only select and drive a finite semantic operation.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyI2cConfigurationTransaction {
    operation: PhyI2cConfigurationOperation,
    phase: PhyI2cConfigurationPhase,
    command_index: u8,
    pending_write: Option<u8>,
}

impl PhyI2cConfigurationTransaction {
    pub const fn new(operation: PhyI2cConfigurationOperation) -> Self {
        Self {
            operation,
            phase: PhyI2cConfigurationPhase::Start,
            command_index: 0,
            pending_write: None,
        }
    }

    pub const fn action(&self) -> PhyI2cConfigurationAction {
        match self.phase {
            PhyI2cConfigurationPhase::Start => PhyI2cConfigurationAction::StartCommand,
            PhyI2cConfigurationPhase::AwaitRead | PhyI2cConfigurationPhase::AwaitWrite => {
                PhyI2cConfigurationAction::AwaitCompletionEdge
            }
            PhyI2cConfigurationPhase::Complete => PhyI2cConfigurationAction::Complete,
        }
    }

    fn start_with(
        &mut self,
        access: &mut impl PhyI2cConfigurationAccess,
    ) -> Result<(), PhyI2cConfigurationError> {
        match self.phase {
            PhyI2cConfigurationPhase::Complete => {
                return Err(PhyI2cConfigurationError::AlreadyComplete);
            }
            PhyI2cConfigurationPhase::AwaitRead | PhyI2cConfigurationPhase::AwaitWrite => {
                return Err(PhyI2cConfigurationError::WrongAction);
            }
            PhyI2cConfigurationPhase::Start => {}
        }
        let command = self
            .operation
            .command(self.command_index)
            .ok_or(PhyI2cConfigurationError::WrongAction)?;
        let address = command.address();
        match (command, self.pending_write) {
            (
                PhyI2cConfigurationCommand::Read(_) | PhyI2cConfigurationCommand::Modify(..),
                None,
            ) => {
                access
                    .start_read(address)
                    .map_err(|()| PhyI2cConfigurationError::BusyAtStart)?;
                self.phase = PhyI2cConfigurationPhase::AwaitRead;
            }
            (PhyI2cConfigurationCommand::Write(_, value), None)
            | (PhyI2cConfigurationCommand::Modify(..), Some(value)) => {
                access
                    .start_write(address, value)
                    .map_err(|()| PhyI2cConfigurationError::BusyAtStart)?;
                self.phase = PhyI2cConfigurationPhase::AwaitWrite;
            }
            (PhyI2cConfigurationCommand::Write(..), Some(_)) => {
                return Err(PhyI2cConfigurationError::WrongAction);
            }
            (PhyI2cConfigurationCommand::Read(_), Some(_)) => {
                return Err(PhyI2cConfigurationError::WrongAction);
            }
        }
        Ok(())
    }

    fn observe_with(
        &mut self,
        access: &impl PhyI2cConfigurationAccess,
    ) -> Result<PhyI2cConfigurationObservation, PhyI2cConfigurationError> {
        match self.phase {
            PhyI2cConfigurationPhase::Complete => {
                return Err(PhyI2cConfigurationError::AlreadyComplete);
            }
            PhyI2cConfigurationPhase::Start => {
                return Err(PhyI2cConfigurationError::WrongAction);
            }
            PhyI2cConfigurationPhase::AwaitRead => {
                let current = match access.observe_read() {
                    Ok(value) => value,
                    Err(()) => return Ok(PhyI2cConfigurationObservation::StillPending),
                };
                match self.operation.command(self.command_index) {
                    Some(PhyI2cConfigurationCommand::Modify(field, value)) => {
                        self.pending_write = Some(field.replace(current, value));
                        self.phase = PhyI2cConfigurationPhase::Start;
                    }
                    Some(PhyI2cConfigurationCommand::Read(_)) => {
                        self.command_index += 1;
                        self.phase = if self.command_index == self.operation.command_count() {
                            PhyI2cConfigurationPhase::Complete
                        } else {
                            PhyI2cConfigurationPhase::Start
                        };
                    }
                    _ => return Err(PhyI2cConfigurationError::WrongAction),
                }
                return Ok(PhyI2cConfigurationObservation::EdgeConsumed);
            }
            PhyI2cConfigurationPhase::AwaitWrite => {}
        }
        if access.observe_write().is_err() {
            return Ok(PhyI2cConfigurationObservation::StillPending);
        }
        self.pending_write = None;
        self.command_index += 1;
        self.phase = if self.command_index == self.operation.command_count() {
            PhyI2cConfigurationPhase::Complete
        } else {
            PhyI2cConfigurationPhase::Start
        };
        Ok(PhyI2cConfigurationObservation::EdgeConsumed)
    }
}

/// One complete Bluetooth TX-power analog-control operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothTxPowerControlOperation {
    /// Capture the two reviewed source values into the route restore slot.
    PrepareRestore,
    /// Force the four reviewed analog-control registers for calibration.
    ConfigureCalibration,
    /// Restore all four analog-control registers and release the restore slot.
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

/// A Bluetooth TX-power control transaction could not advance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothTxPowerControlError {
    BusyAtStart,
    RestorePending,
    RestoreNotPending,
    WrongAction,
    AlreadyComplete,
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

/// Non-cloneable transaction over the four PAC-owned analog-control registers.
///
/// Saved values live in the route restore slot; field geometry stays in the
/// PAC's reviewed high-byte fields.
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
                analog_registers::BLUETOOTH_TX_POWER_HIGH_0
                    .replace(self.scratch, self.high_value(access)?),
            ),
            4 => access.start_read(BluetoothTxPowerControlRegister::High1),
            5 => access.start_write(
                BluetoothTxPowerControlRegister::High1,
                analog_registers::BLUETOOTH_TX_POWER_HIGH_1
                    .replace(self.scratch, self.high_value(access)?),
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
                .capture_high(analog_registers::BLUETOOTH_TX_POWER_HIGH_0.extract(value))
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

const fn map_restore_error(
    error: BluetoothTxPowerControlRestoreError,
) -> BluetoothTxPowerControlError {
    match error {
        BluetoothTxPowerControlRestoreError::RestoreNotPending => {
            BluetoothTxPowerControlError::RestoreNotPending
        }
    }
}

/// Start the current command of one PHY-I²C configuration.
pub fn start_configuration(
    transaction: &mut PhyI2cConfigurationTransaction,
    registers: &mut impl SharedPhyAccess,
) -> Result<(), PhyI2cConfigurationError> {
    transaction.start_with(phy_pac_mut(registers))
}

/// Consume one configuration-completion edge.
pub fn observe_configuration(
    transaction: &mut PhyI2cConfigurationTransaction,
    registers: &mut impl SharedPhyAccess,
) -> Result<PhyI2cConfigurationObservation, PhyI2cConfigurationError> {
    transaction.observe_with(phy_pac(registers))
}

/// Start the current command of one Bluetooth TX-power transaction.
pub fn start_bluetooth_tx_power_control(
    transaction: &mut BluetoothTxPowerControlTransaction,
    registers: &mut impl SharedPhyAccess,
) -> Result<(), BluetoothTxPowerControlError> {
    let (phy, restore) = phy_parts_mut(registers);
    transaction.start_with(&mut BluetoothTxPowerPort { phy, restore })
}

/// Consume one independently delivered Bluetooth TX-power completion edge.
pub fn observe_bluetooth_tx_power_control(
    transaction: &mut BluetoothTxPowerControlTransaction,
    registers: &mut impl SharedPhyAccess,
) -> Result<BluetoothTxPowerControlObservation, BluetoothTxPowerControlError> {
    let (phy, restore) = phy_parts_mut(registers);
    transaction.observe_with(&mut BluetoothTxPowerPort { phy, restore })
}

/// Run the complete 22-pair current-vendor retained-wake analog-I²C stage.
///
/// Both hardware waits are finite, and the normal host map is restored on
/// every return.
pub fn configure_initialization_stage_two(
    registers: &mut impl SharedPhyAccess,
    inputs: PhyI2cCommandMemoryInputs,
    maximum_observations: u32,
) -> Result<(), PhyI2cInitializationStageTwoError> {
    configure_initialization_stage_two_with(phy_pac_mut(registers), inputs, maximum_observations)
}

impl PhyI2cParallelAccess for RadioPhyRegisters {
    fn select_parallel_host_map(&mut self) {
        self.select_phy_i2c_parallel_host_map();
    }

    fn restore_radio_host_map(&mut self) {
        self.restore_phy_i2c_radio_host_map();
    }

    fn is_busy(&self, host: PhyI2cHost) -> bool {
        self.phy_i2c_master_is_busy(host)
    }

    fn start_pair(&mut self, pair: PhyI2cParallelWrite) {
        self.start_phy_i2c_parallel_pair(pair);
    }
}

impl PhyI2cConfigurationAccess for RadioPhyRegisters {
    fn start_read(&mut self, address: PhyI2cAddress) -> Result<(), ()> {
        self.start_phy_i2c_configuration_read(address)
            .map_err(|PhyI2cAccessError::Busy| ())
    }

    fn start_write(&mut self, address: PhyI2cAddress, value: u8) -> Result<(), ()> {
        self.start_phy_i2c_configuration_write(address, value)
            .map_err(|PhyI2cAccessError::Busy| ())
    }

    fn observe_read(&self) -> Result<u8, ()> {
        self.finish_phy_i2c_host_read(PhyI2cHost::Host1)
            .map_err(|PhyI2cAccessError::Busy| ())
    }

    fn observe_write(&self) -> Result<(), ()> {
        self.finish_phy_i2c_host_write(PhyI2cHost::Host1)
            .map_err(|PhyI2cAccessError::Busy| ())
    }
}

/// Shared PHY registers paired with the route restore slot.
struct BluetoothTxPowerPort<'a> {
    phy: &'a mut RadioPhyRegisters,
    restore: &'a mut PhyRouteState,
}

impl BluetoothTxPowerControlI2cAccess for BluetoothTxPowerPort<'_> {
    fn reserve_restore(&mut self) -> Result<(), BluetoothTxPowerControlPrepareError> {
        self.restore.prepare_bluetooth_tx_power_control()
    }

    fn capture_low(&mut self, value: u8) -> Result<(), BluetoothTxPowerControlRestoreError> {
        self.restore.capture_bluetooth_tx_power_control_low(value)
    }

    fn capture_high(&mut self, value: u8) -> Result<(), BluetoothTxPowerControlRestoreError> {
        self.restore.capture_bluetooth_tx_power_control_high(value)
    }

    fn restore_values(&self) -> Result<(u8, u8), BluetoothTxPowerControlRestoreError> {
        self.restore.bluetooth_tx_power_control_values()
    }

    fn finish_restore(&mut self) -> Result<(), BluetoothTxPowerControlRestoreError> {
        self.restore.finish_bluetooth_tx_power_control_restore()
    }

    fn start_read(&mut self, register: BluetoothTxPowerControlRegister) -> Result<(), ()> {
        self.phy
            .start_bluetooth_tx_power_control_read(register)
            .map_err(|PhyI2cAccessError::Busy| ())
    }

    fn start_write(
        &mut self,
        register: BluetoothTxPowerControlRegister,
        value: u8,
    ) -> Result<(), ()> {
        self.phy
            .start_bluetooth_tx_power_control_write(register, value)
            .map_err(|PhyI2cAccessError::Busy| ())
    }

    fn observe_read(&self) -> Result<u8, ()> {
        self.phy
            .finish_phy_i2c_host_read(PhyI2cHost::Host1)
            .map_err(|PhyI2cAccessError::Busy| ())
    }

    fn observe_write(&self) -> Result<(), ()> {
        self.phy
            .finish_phy_i2c_host_write(PhyI2cHost::Host1)
            .map_err(|PhyI2cAccessError::Busy| ())
    }
}

/// Configure the PAC-owned host map and return its typed selection.
pub fn configure_and_select_host(
    registers: &mut impl SharedPhyAccess,
    block: PhyI2cBlock,
) -> PhyI2cHost {
    phy_pac_mut(registers).configure_and_select_phy_i2c_host(block)
}

/// Start one PAC-encoded analog-register read.
pub fn try_start_read(
    registers: &mut impl SharedPhyAccess,
    address: PhyI2cAddress,
) -> Result<(), PhyI2cAccessError> {
    phy_pac_mut(registers).try_start_phy_i2c_read(address)
}

/// Consume one externally delivered completion edge for a PAC-encoded read.
pub fn try_finish_read(
    registers: &impl SharedPhyAccess,
    address: PhyI2cAddress,
) -> Result<u8, PhyI2cAccessError> {
    phy_pac(registers).try_finish_phy_i2c_read(address)
}

/// Start one PAC-encoded analog-register write.
pub fn try_start_write(
    registers: &mut impl SharedPhyAccess,
    address: PhyI2cAddress,
    value: u8,
) -> Result<(), PhyI2cAccessError> {
    phy_pac_mut(registers).try_start_phy_i2c_write(address, value)
}

/// Consume one externally delivered completion edge for a PAC-encoded write.
pub fn try_finish_write(
    registers: &impl SharedPhyAccess,
    address: PhyI2cAddress,
) -> Result<(), PhyI2cAccessError> {
    phy_pac(registers).try_finish_phy_i2c_write(address)
}

/// Publish one full-word PHY-I2C master reset command.
///
/// The complete ROM parent writes only bit 26 to a busy host and then polls
/// bit 25. This finite method publishes that one edge; retry and timeout
/// ownership remain in the Rust transition.
pub fn pulse_master_reset(registers: &mut impl SharedPhyAccess, host: PhyI2cHost) {
    phy_pac_mut(registers).pulse_phy_i2c_master_reset(host);
}

/// Sample one PHY-I2C master reset busy edge without retrying.
pub fn sample_master_reset_busy(registers: &impl SharedPhyAccess, host: PhyI2cHost) -> bool {
    phy_pac(registers).phy_i2c_master_is_busy(host)
}

/// Apply all six writes of the recovered PHY-I2C clock selection.
///
/// Basis: complete rev0 ROM `phy_i2c_clk_sel` at `0x2f829f1c`, size `0x68`.
/// Each of three registers receives a high-field update followed by a fresh
/// read and low-field update, preserving all instruction-evidenced
/// intermediate states.
pub fn configure_clock_selection(registers: &mut impl SharedPhyAccess) {
    phy_pac_mut(registers).configure_phy_i2c_clock_selection();
}

/// Configure the PHY-I2C master register mode and enable bit.
///
/// Basis: complete rev0 ROM `phy_i2cmst_reg_init` at `0x2f8276c4`, size
/// `0x22`. It writes `MASTER_CONTROL.REGISTER_MODE = 2`, then sets
/// `REGISTER_ENABLE`, using a fresh read for each update.
pub fn configure_master_registers(registers: &mut impl SharedPhyAccess) {
    phy_pac_mut(registers).configure_phy_i2c_master_registers();
}

/// Select the complete rev0 ROM `phy_bbpll_cal` mode.
///
/// The body at `0x2f82_7dbc`, size `0x1c`, performs one fresh-read
/// replacement of `MASTER_CONTROL` bits 3:2. Zero selects encoded mode one;
/// every nonzero input selects encoded mode two. The boolean API makes that
/// two-state contract explicit while preserving all unrelated shared fields.
pub fn configure_bbpll_calibration(registers: &mut impl SharedPhyAccess, enabled: bool) {
    phy_pac_mut(registers).set_phy_i2c_bbpll_calibration(enabled);
}

/// Program the complete PAC-owned PHY-I²C command memory.
///
/// Basis: complete S31
/// `libphy.a[phy_i2c.o]::phy_i2c_master_cmd_mem_init`. The SVD `dim=45`
/// array localizes every destination; its indices, internal analog addresses,
/// fixed values and derived images remain private to the PAC.
pub fn configure_command_memory(
    registers: &mut impl SharedPhyAccess,
    inputs: PhyI2cCommandMemoryInputs,
) {
    phy_pac_mut(registers).configure_phy_i2c_command_memory(inputs);
}

#[cfg(test)]
mod tests;
