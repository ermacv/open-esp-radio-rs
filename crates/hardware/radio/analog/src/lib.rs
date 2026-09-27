//! Chip-neutral analog register bus of the Espressif PHY.
//!
//! The PHY reaches its analog blocks (PLLs, bias, regulators, the RF front
//! end) through byte registers behind an analog I2C master. The vendor leaves
//! (`phy_chip_i2c_readReg`, `phy_chip_i2c_writeReg`) busy-wait for each
//! command; this crate splits every command into a start and an observed
//! completion, so an outer owner decides how to wait.
//!
//! A chip implements [`AnalogRegisterBus`] over its PAC. Its address type is
//! opaque: host selection, read masks and block aliases stay in the chip
//! implementation, and every completion is observed with the same address
//! that started the command. [`FieldRead`] and [`FieldWrite`] are the
//! vendor field transactions `phy_i2c_readReg_Mask` and
//! `phy_i2c_writeReg_Mask` as polled machines over that contract.
//!
//! This crate performs no MMIO and holds no delay or deadline; those belong
//! to the executor that polls it.

#![no_std]
#![forbid(unsafe_code)]

/// The host of an address is still executing a command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Busy;

/// Non-blocking access to one chip's analog byte registers.
///
/// Implementations must observe completion on the host that `address`
/// selected when the command started, and must not start a command while that
/// host is busy.
pub trait AnalogRegisterBus {
    /// Validated identity of one analog byte register.
    type Address: Copy + Eq + core::fmt::Debug;

    /// Start a read of `address`.
    ///
    /// # Errors
    ///
    /// The address's host is busy; no command was started.
    fn try_start_read(&mut self, address: Self::Address) -> Result<(), Busy>;

    /// The byte of the completed read of `address`.
    ///
    /// # Errors
    ///
    /// The read is still executing.
    fn try_finish_read(&self, address: Self::Address) -> Result<u8, Busy>;

    /// Start a write of `value` to `address`.
    ///
    /// # Errors
    ///
    /// The address's host is busy; no command was started.
    fn try_start_write(&mut self, address: Self::Address, value: u8) -> Result<(), Busy>;

    /// Observe the completion of the write of `address`.
    ///
    /// # Errors
    ///
    /// The write is still executing.
    fn try_finish_write(&self, address: Self::Address) -> Result<(), Busy>;
}

/// Bits `msb..=lsb` of one analog byte register.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AnalogField<Address> {
    address: Address,
    msb: u8,
    lsb: u8,
}

impl<Address: Copy> AnalogField<Address> {
    /// The field, when `lsb <= msb <= 7`.
    pub const fn new(address: Address, msb: u8, lsb: u8) -> Option<Self> {
        if lsb <= msb && msb < 8 {
            Some(Self { address, msb, lsb })
        } else {
            None
        }
    }

    pub const fn address(self) -> Address {
        self.address
    }

    /// The field's bits in place.
    const fn mask(self) -> u8 {
        let width = self.msb - self.lsb + 1;
        ((((1_u16) << width) - 1) as u8) << self.lsb
    }

    /// The field's value in `byte`, as `phy_i2c_readReg_Mask` extracts it.
    pub const fn extract(self, byte: u8) -> u8 {
        (byte & self.mask()) >> self.lsb
    }

    /// `byte` with the field replaced, as `phy_i2c_writeReg_Mask` composes
    /// it, when `value` fits the field; `None` otherwise.
    ///
    /// The vendor leaf ORs `value << lsb` in unmasked, so a wider value
    /// would also set bits above the field; such values are rejected here.
    pub const fn insert(self, byte: u8, value: u8) -> Option<u8> {
        let width = self.msb - self.lsb + 1;
        if width < 8 && value >> width != 0 {
            return None;
        }
        Some((byte & !self.mask()) | (value << self.lsb))
    }
}

/// Progress of a polled transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Step<T> {
    /// A command is executing or waits for its host; poll again.
    Pending,
    /// The transaction completed with this result.
    Ready(T),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReadPhase {
    Start,
    Finish,
}

/// Polled `phy_i2c_readReg_Mask`: read the register, extract the field.
#[must_use = "a field read does nothing until it is polled to completion"]
#[derive(Clone, Copy, Debug)]
pub struct FieldRead<Address> {
    field: AnalogField<Address>,
    phase: ReadPhase,
}

impl<Address: Copy> FieldRead<Address> {
    pub const fn new(field: AnalogField<Address>) -> Self {
        Self {
            field,
            phase: ReadPhase::Start,
        }
    }

    /// Advance the read by at most one bus action.
    pub fn poll<Bus>(&mut self, bus: &mut Bus) -> Step<u8>
    where
        Bus: AnalogRegisterBus<Address = Address>,
    {
        match self.phase {
            ReadPhase::Start => {
                if bus.try_start_read(self.field.address).is_ok() {
                    self.phase = ReadPhase::Finish;
                }
                Step::Pending
            }
            ReadPhase::Finish => match bus.try_finish_read(self.field.address) {
                Ok(byte) => Step::Ready(self.field.extract(byte)),
                Err(Busy) => Step::Pending,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WritePhase {
    StartRead,
    FinishRead,
    StartWrite(u8),
    FinishWrite,
}

/// Why a field write was not constructed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ValueTooWide;

/// Polled `phy_i2c_writeReg_Mask`: read the register, replace the field and
/// write the byte back.
#[must_use = "a field write does nothing until it is polled to completion"]
#[derive(Clone, Copy, Debug)]
pub struct FieldWrite<Address> {
    field: AnalogField<Address>,
    value: u8,
    phase: WritePhase,
}

impl<Address: Copy> FieldWrite<Address> {
    /// A write of `value` to `field`.
    ///
    /// # Errors
    ///
    /// `value` does not fit the field.
    pub const fn new(field: AnalogField<Address>, value: u8) -> Result<Self, ValueTooWide> {
        if field.insert(0, value).is_none() {
            return Err(ValueTooWide);
        }
        Ok(Self {
            field,
            value,
            phase: WritePhase::StartRead,
        })
    }

    /// Advance the write by at most one bus action.
    pub fn poll<Bus>(&mut self, bus: &mut Bus) -> Step<()>
    where
        Bus: AnalogRegisterBus<Address = Address>,
    {
        let address = self.field.address;
        match self.phase {
            WritePhase::StartRead => {
                if bus.try_start_read(address).is_ok() {
                    self.phase = WritePhase::FinishRead;
                }
                Step::Pending
            }
            WritePhase::FinishRead => {
                if let Ok(byte) = bus.try_finish_read(address) {
                    let byte = match self.field.insert(byte, self.value) {
                        Some(byte) => byte,
                        None => unreachable!("the value was checked at construction"),
                    };
                    self.phase = WritePhase::StartWrite(byte);
                }
                Step::Pending
            }
            WritePhase::StartWrite(byte) => {
                if bus.try_start_write(address, byte).is_ok() {
                    self.phase = WritePhase::FinishWrite;
                }
                Step::Pending
            }
            WritePhase::FinishWrite => match bus.try_finish_write(address) {
                Ok(()) => Step::Ready(()),
                Err(Busy) => Step::Pending,
            },
        }
    }
}

#[cfg(test)]
mod tests;
