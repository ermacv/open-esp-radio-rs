//! Scenario-owned PHY-I2C command completion for RFPLL execution comparisons.
//!
//! This model supplies peripheral responses, not a capacitor-search algorithm.
//! Status samples are explicit inputs; it cannot establish physical PLL lock.
//! Command encoding follows the reviewed S31 PHY-I2C port, independently of
//! either compared implementation. Unknown accesses and unused samples fail closed.

use crate::execution_model::{
    DeviceModel, DeviceModelCoverage, DeviceModelDescriptor, DeviceModelInstance, Error,
    MemoryRange, Result,
};
use std::collections::{BTreeMap, VecDeque};

fn invalid(message: impl Into<String>) -> Error {
    Error::Invalid {
        message: message.into(),
    }
}

pub const PORT_BASE: u32 = 0x2010_f800;

#[derive(Clone, Debug)]
pub struct Rfpll {
    initial_cap: u16,
    statuses: Vec<u8>,
    busy_reads: u16,
}

impl Rfpll {
    pub fn new(initial_cap: u16, statuses: Vec<u8>, busy_reads: u16) -> Result<Self> {
        if initial_cap > 511 || statuses.iter().any(|status| *status > 3) {
            return Err(invalid(
                "RFPLL scenario contains an invalid capacitor or status",
            ));
        }
        Ok(Self {
            initial_cap,
            statuses,
            busy_reads,
        })
    }
}

impl DeviceModel for Rfpll {
    fn descriptor(&self) -> DeviceModelDescriptor {
        DeviceModelDescriptor {
            id: "s31-rfpll-i2c".into(),
            kind: "s31-phy-i2c-scripted-status".into(),
            range: MemoryRange {
                start: PORT_BASE,
                length: 0x24,
            },
            configuration: BTreeMap::from([
                ("initial-cap".into(), self.initial_cap.to_string()),
                ("statuses".into(), format!("{:?}", self.statuses)),
                ("busy-reads-per-command".into(), self.busy_reads.to_string()),
            ]),
        }
    }

    fn instantiate(&self) -> Result<Box<dyn DeviceModelInstance>> {
        Ok(Box::new(Port {
            bbpll_control: None,
            registers: BTreeMap::from([
                ((0x62, 1), self.initial_cap as u8),
                ((0x62, 2), 0x95),
                ((0x62, 5), self.initial_cap as u8),
                ((0x62, 7), 0xc2 | (((self.initial_cap >> 8) as u8) << 2)),
                ((0x62, 11), 0x15),
            ]),
            reads: BTreeMap::from([(
                (0x62, 12),
                self.statuses
                    .iter()
                    .map(|value| 0xa3 | (value << 2))
                    .collect(),
            )]),
            command: [0; 2],
            pending: [0; 2],
            busy_reads: self.busy_reads,
            config: 0,
            host_selection: 0,
        }))
    }
}

/// Caller-owned RFPLL register contents and finite measurement sequences.
/// Unknown registers and exhausted samples remain errors; no RF algorithm runs here.
#[derive(Clone, Debug)]
pub struct RegisterBank {
    /// Optional independent BBPLL control in the same MMIO aperture.
    pub bbpll_control: Option<u32>,
    pub registers: BTreeMap<(u8, u8), u8>,
    pub reads: BTreeMap<(u8, u8), Vec<u8>>,
    pub busy_reads: u16,
}

impl DeviceModel for RegisterBank {
    fn descriptor(&self) -> DeviceModelDescriptor {
        DeviceModelDescriptor {
            id: "s31-rfpll-register-bank".into(),
            kind: "s31-phy-i2c-register-bank".into(),
            range: MemoryRange {
                start: PORT_BASE,
                length: 0x24,
            },
            configuration: BTreeMap::from([
                ("registers".into(), format!("{:?}", self.registers)),
                ("bbpll-control".into(), format!("{:?}", self.bbpll_control)),
                ("reads".into(), format!("{:?}", self.reads)),
                ("busy-reads-per-command".into(), self.busy_reads.to_string()),
            ]),
        }
    }

    fn instantiate(&self) -> Result<Box<dyn DeviceModelInstance>> {
        if self
            .registers
            .keys()
            .any(|key| self.reads.contains_key(key))
        {
            return Err(invalid(
                "analog register cannot be both retained and scripted",
            ));
        }
        Ok(Box::new(Port {
            bbpll_control: self.bbpll_control,
            registers: self.registers.clone(),
            reads: self
                .reads
                .iter()
                .map(|(key, values)| (*key, values.clone().into()))
                .collect(),
            command: [0; 2],
            pending: [0; 2],
            busy_reads: self.busy_reads,
            config: 0,
            host_selection: 0,
        }))
    }
}

#[derive(Debug)]
struct Port {
    bbpll_control: Option<u32>,
    registers: BTreeMap<(u8, u8), u8>,
    reads: BTreeMap<(u8, u8), VecDeque<u8>>,
    command: [u32; 2],
    pending: [u16; 2],
    busy_reads: u16,
    config: u32,
    host_selection: u32,
}

impl Port {
    fn port(address: u32, width: u8) -> Result<u32> {
        if width != 32 {
            return Err(invalid("PHY-I2C requires word access"));
        }
        address
            .checked_sub(PORT_BASE)
            .ok_or_else(|| invalid("PHY-I2C access outside model"))
    }
}

impl DeviceModelInstance for Port {
    fn read(&mut self, address: u32, width: u8) -> Result<u32> {
        match Self::port(address, width)? {
            offset @ (0 | 4) => {
                let host = (offset / 4) as usize;
                if self.pending[host] != 0 {
                    self.pending[host] -= 1;
                    Ok(self.command[host] | 0x0200_0000)
                } else {
                    Ok(self.command[host])
                }
            }
            0x18 => self
                .bbpll_control
                .ok_or_else(|| invalid("BBPLL control is not seeded")),
            0x1c => Ok(self.config),
            0x20 => Ok(self.host_selection),
            _ => Err(invalid("unexpected PHY-I2C register read")),
        }
    }

    fn write(&mut self, address: u32, width: u8, value: u32) -> Result<()> {
        match Self::port(address, width)? {
            offset @ (0 | 4) => {
                let host = (offset / 4) as usize;
                if self.pending[host] != 0 {
                    return Err(invalid("PHY-I2C command overwritten before completion"));
                }
                if value & 0xfe00_0000 != 0x0400_0000 {
                    return Err(invalid(format!("unsupported PHY-I2C command {value:#x}")));
                }
                let register = (value as u8, (value >> 8) as u8);
                let data = if value & 0x0100_0000 != 0 {
                    let slot = self.registers.get_mut(&register).ok_or_else(|| {
                        invalid(format!("write to unseeded analog register {register:#x?}"))
                    })?;
                    *slot = (value >> 16) as u8;
                    *slot
                } else if let Some(values) = self.reads.get_mut(&register) {
                    values.pop_front().ok_or_else(|| {
                        invalid(format!(
                            "analog register {register:?} sample sequence exhausted"
                        ))
                    })?
                } else {
                    *self.registers.get(&register).ok_or_else(|| {
                        invalid(format!("read from unseeded analog register {register:#x?}"))
                    })?
                };
                self.command[host] = (value & !0x00ff_0000) | (u32::from(data) << 16);
                self.pending[host] = self.busy_reads;
                Ok(())
            }
            0x18 => {
                *self
                    .bbpll_control
                    .as_mut()
                    .ok_or_else(|| invalid("BBPLL control is not seeded"))? = value;
                Ok(())
            }
            0x1c => {
                self.config = value;
                Ok(())
            }
            0x20 => {
                self.host_selection = value;
                Ok(())
            }
            _ => Err(invalid("unexpected PHY-I2C register write")),
        }
    }

    fn finish(&mut self) -> Result<DeviceModelCoverage> {
        if self.reads.values().all(VecDeque::is_empty) && self.pending == [0; 2] {
            Ok(DeviceModelCoverage::complete())
        } else {
            Ok(DeviceModelCoverage::incomplete(format!(
                "{} unused analog samples; pending reads {:?}",
                self.reads.values().map(VecDeque::len).sum::<usize>(),
                self.pending
            )))
        }
    }
}

#[cfg(test)]
mod tests;
