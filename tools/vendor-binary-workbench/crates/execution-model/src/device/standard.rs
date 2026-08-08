use super::{DeviceModelCoverage, DeviceModelInstance, DeviceModelSpec};
use crate::{Error, Result, width_mask};

#[derive(Debug)]
pub(super) enum StandardDeviceInstance {
    ConstantRead {
        address: u32,
        width: u8,
        value: u32,
    },
    SequenceRead {
        address: u32,
        width: u8,
        values: std::collections::VecDeque<u32>,
    },
    W1c {
        address: u32,
        width: u8,
        value: u32,
        clear_mask: u32,
        read_clear_mask: u32,
    },
    ReadToClear {
        address: u32,
        width: u8,
        value: u32,
        clear_mask: u32,
    },
    SelfClearing {
        address: u32,
        width: u8,
        value: u32,
        store_mask: u32,
    },
    Fifo {
        address: u32,
        width: u8,
        read_values: std::collections::VecDeque<u32>,
        expected_writes: std::collections::VecDeque<u32>,
    },
    IndexedBank {
        index_address: u32,
        data_address: u32,
        width: u8,
        selected: usize,
        values: Vec<u32>,
    },
}

impl From<DeviceModelSpec> for StandardDeviceInstance {
    fn from(spec: DeviceModelSpec) -> Self {
        match spec {
            DeviceModelSpec::ConstantRead {
                address,
                width,
                value,
                ..
            } => Self::ConstantRead {
                address,
                width,
                value,
            },
            DeviceModelSpec::SequenceRead {
                address,
                width,
                values,
                ..
            } => Self::SequenceRead {
                address,
                width,
                values: values.into(),
            },
            DeviceModelSpec::W1c {
                address,
                width,
                initial_value,
                clear_mask,
                read_clear_mask,
                ..
            } => Self::W1c {
                address,
                width,
                value: initial_value,
                clear_mask,
                read_clear_mask,
            },
            DeviceModelSpec::ReadToClear {
                address,
                width,
                initial_value,
                clear_mask,
                ..
            } => Self::ReadToClear {
                address,
                width,
                value: initial_value,
                clear_mask,
            },
            DeviceModelSpec::SelfClearing {
                address,
                width,
                initial_value,
                store_mask,
                ..
            } => Self::SelfClearing {
                address,
                width,
                value: initial_value,
                store_mask,
            },
            DeviceModelSpec::Fifo {
                address,
                width,
                read_values,
                expected_writes,
                ..
            } => Self::Fifo {
                address,
                width,
                read_values: read_values.into(),
                expected_writes: expected_writes.into(),
            },
            DeviceModelSpec::IndexedBank {
                index_address,
                data_address,
                width,
                initial_values,
                ..
            } => Self::IndexedBank {
                index_address,
                data_address,
                width,
                selected: 0,
                values: initial_values,
            },
        }
    }
}

fn require_access(
    expected_address: u32,
    expected_width: u8,
    address: u32,
    width: u8,
) -> Result<()> {
    if address != expected_address || width != expected_width {
        return Err(Error::invalid(format!(
            "device model requires an exact {expected_width}-bit access at {expected_address:#010x}, got {width} bits at {address:#010x}"
        )));
    }
    Ok(())
}

impl DeviceModelInstance for StandardDeviceInstance {
    fn read(&mut self, address: u32, width: u8) -> Result<u32> {
        match self {
            Self::ConstantRead {
                address: expected,
                width: expected_width,
                value,
            } => {
                require_access(*expected, *expected_width, address, width)?;
                Ok(*value)
            }
            Self::SequenceRead {
                address: expected,
                width: expected_width,
                values,
            } => {
                require_access(*expected, *expected_width, address, width)?;
                values
                    .pop_front()
                    .ok_or_else(|| Error::invalid("sequence-read device model is exhausted"))
            }
            Self::W1c {
                address: expected,
                width: expected_width,
                value,
                read_clear_mask,
                ..
            } => {
                require_access(*expected, *expected_width, address, width)?;
                let observed = *value;
                *value &= !*read_clear_mask;
                Ok(observed)
            }
            Self::SelfClearing {
                address: expected,
                width: expected_width,
                value,
                ..
            } => {
                require_access(*expected, *expected_width, address, width)?;
                Ok(*value)
            }
            Self::ReadToClear {
                address: expected,
                width: expected_width,
                value,
                clear_mask,
            } => {
                require_access(*expected, *expected_width, address, width)?;
                let observed = *value;
                *value &= !*clear_mask;
                Ok(observed)
            }
            Self::Fifo {
                address: expected,
                width: expected_width,
                read_values,
                ..
            } => {
                require_access(*expected, *expected_width, address, width)?;
                read_values
                    .pop_front()
                    .ok_or_else(|| Error::invalid("FIFO read sequence is exhausted"))
            }
            Self::IndexedBank {
                index_address,
                data_address,
                width: expected_width,
                selected,
                values,
            } => {
                if width != *expected_width {
                    return Err(Error::invalid(format!(
                        "indexed bank requires {expected_width}-bit accesses"
                    )));
                }
                if address == *index_address {
                    Ok(u32::try_from(*selected).expect("selected index fits u32"))
                } else if address == *data_address {
                    Ok(values[*selected])
                } else {
                    Err(Error::invalid(format!(
                        "unsupported indexed-bank read at {address:#010x}"
                    )))
                }
            }
        }
    }

    fn write(&mut self, address: u32, width: u8, written: u32) -> Result<()> {
        match self {
            Self::ConstantRead { .. } | Self::SequenceRead { .. } | Self::ReadToClear { .. } => {
                Err(Error::invalid(format!(
                    "read-only device model received a write at {address:#010x}"
                )))
            }
            Self::W1c {
                address: expected,
                width: expected_width,
                value,
                clear_mask,
                ..
            } => {
                require_access(*expected, *expected_width, address, width)?;
                *value &= !(written & *clear_mask);
                Ok(())
            }
            Self::SelfClearing {
                address: expected,
                width: expected_width,
                value,
                store_mask,
            } => {
                require_access(*expected, *expected_width, address, width)?;
                *value = (*value & !*store_mask) | (written & *store_mask);
                Ok(())
            }
            Self::Fifo {
                address: expected,
                width: expected_width,
                expected_writes,
                ..
            } => {
                require_access(*expected, *expected_width, address, width)?;
                let Some(expected) = expected_writes.pop_front() else {
                    return Err(Error::invalid("FIFO received an unexpected write"));
                };
                if expected != written {
                    return Err(Error::invalid(format!(
                        "FIFO write mismatch: expected {expected:#010x}, got {written:#010x}"
                    )));
                }
                Ok(())
            }
            Self::IndexedBank {
                index_address,
                data_address,
                width: expected_width,
                selected,
                values,
            } => {
                if width != *expected_width {
                    return Err(Error::invalid(format!(
                        "indexed bank requires {expected_width}-bit accesses"
                    )));
                }
                if address == *index_address {
                    let index = usize::try_from(written)
                        .ok()
                        .filter(|index| *index < values.len())
                        .ok_or_else(|| {
                            Error::invalid(format!(
                                "indexed-bank selection {written} is out of range"
                            ))
                        })?;
                    *selected = index;
                    Ok(())
                } else if address == *data_address {
                    values[*selected] = written & width_mask(width);
                    Ok(())
                } else {
                    Err(Error::invalid(format!(
                        "unsupported indexed-bank write at {address:#010x}"
                    )))
                }
            }
        }
    }

    fn finish(&mut self) -> Result<DeviceModelCoverage> {
        let reason = match self {
            Self::SequenceRead { values, .. } if !values.is_empty() => Some(format!(
                "{} sequence read values were not consumed",
                values.len()
            )),
            Self::Fifo {
                read_values,
                expected_writes,
                ..
            } if !read_values.is_empty() || !expected_writes.is_empty() => Some(format!(
                "FIFO left {} reads and {} expected writes unconsumed",
                read_values.len(),
                expected_writes.len()
            )),
            _ => None,
        };
        Ok(reason.map_or_else(
            DeviceModelCoverage::complete,
            DeviceModelCoverage::incomplete,
        ))
    }
}
