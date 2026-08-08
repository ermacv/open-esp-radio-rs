use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{DeviceModel, DeviceModelDescriptor, DeviceModelInstance};
use crate::{Error, MemoryRange, Result, width_mask};

use super::standard::StandardDeviceInstance;

/// Serializable standard peripheral models.
///
/// These variants intentionally contain no chip vocabulary. More complicated
/// state machines stay behind [`DeviceModel`] as compiled platform addons.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum DeviceModelSpec {
    ConstantRead {
        id: String,
        address: u32,
        width: u8,
        value: u32,
    },
    SequenceRead {
        id: String,
        address: u32,
        width: u8,
        values: Vec<u32>,
    },
    W1c {
        id: String,
        address: u32,
        width: u8,
        initial_value: u32,
        clear_mask: u32,
        #[serde(default)]
        read_clear_mask: u32,
    },
    ReadToClear {
        id: String,
        address: u32,
        width: u8,
        initial_value: u32,
        clear_mask: u32,
    },
    SelfClearing {
        id: String,
        address: u32,
        width: u8,
        initial_value: u32,
        store_mask: u32,
        command_mask: u32,
    },
    Fifo {
        id: String,
        address: u32,
        width: u8,
        read_values: Vec<u32>,
        expected_writes: Vec<u32>,
    },
    IndexedBank {
        id: String,
        index_address: u32,
        data_address: u32,
        width: u8,
        initial_values: Vec<u32>,
    },
}

impl DeviceModelSpec {
    fn id(&self) -> &str {
        match self {
            Self::ConstantRead { id, .. }
            | Self::SequenceRead { id, .. }
            | Self::W1c { id, .. }
            | Self::ReadToClear { id, .. }
            | Self::SelfClearing { id, .. }
            | Self::Fifo { id, .. }
            | Self::IndexedBank { id, .. } => id,
        }
    }

    const fn kind(&self) -> &'static str {
        match self {
            Self::ConstantRead { .. } => "constant-read",
            Self::SequenceRead { .. } => "sequence-read",
            Self::W1c { .. } => "w1c",
            Self::ReadToClear { .. } => "read-to-clear",
            Self::SelfClearing { .. } => "self-clearing",
            Self::Fifo { .. } => "fifo",
            Self::IndexedBank { .. } => "indexed-bank",
        }
    }

    const fn width(&self) -> u8 {
        match self {
            Self::ConstantRead { width, .. }
            | Self::SequenceRead { width, .. }
            | Self::W1c { width, .. }
            | Self::ReadToClear { width, .. }
            | Self::SelfClearing { width, .. }
            | Self::Fifo { width, .. }
            | Self::IndexedBank { width, .. } => *width,
        }
    }

    fn range(&self) -> MemoryRange {
        match self {
            Self::ConstantRead { address, width, .. }
            | Self::SequenceRead { address, width, .. }
            | Self::W1c { address, width, .. }
            | Self::ReadToClear { address, width, .. }
            | Self::SelfClearing { address, width, .. }
            | Self::Fifo { address, width, .. } => MemoryRange {
                start: *address,
                length: u32::from(*width / 8),
            },
            Self::IndexedBank {
                index_address,
                data_address,
                width,
                ..
            } => {
                let start = (*index_address).min(*data_address);
                let end = (*index_address)
                    .max(*data_address)
                    .saturating_add(u32::from(*width / 8));
                MemoryRange {
                    start,
                    length: end.saturating_sub(start),
                }
            }
        }
    }

    fn validate(&self) -> Result<()> {
        if self.id().trim().is_empty() {
            return Err(Error::invalid("device model has an empty id"));
        }
        let width = self.width();
        if !matches!(width, 8 | 16 | 32) {
            return Err(Error::invalid(format!(
                "device model {} has unsupported width {width}",
                self.id()
            )));
        }
        let byte_width = u32::from(width / 8);
        let aligned = |address: u32| address.is_multiple_of(byte_width);
        let (first, second) = match self {
            Self::IndexedBank {
                index_address,
                data_address,
                initial_values,
                ..
            } => {
                if index_address == data_address {
                    return Err(Error::invalid(format!(
                        "indexed-bank device model {} uses the same index and data address",
                        self.id()
                    )));
                }
                if initial_values.is_empty() {
                    return Err(Error::invalid(format!(
                        "indexed-bank device model {} has no registers",
                        self.id()
                    )));
                }
                (*index_address, Some(*data_address))
            }
            Self::SequenceRead {
                address, values, ..
            } => {
                if values.is_empty() {
                    return Err(Error::invalid(format!(
                        "sequence-read device model {} has no values",
                        self.id()
                    )));
                }
                (*address, None)
            }
            Self::ConstantRead { address, .. }
            | Self::W1c { address, .. }
            | Self::ReadToClear { address, .. }
            | Self::SelfClearing { address, .. }
            | Self::Fifo { address, .. } => (*address, None),
        };
        if !aligned(first) || second.is_some_and(|address| !aligned(address)) {
            return Err(Error::invalid(format!(
                "device model {} address is not {width}-bit aligned",
                self.id()
            )));
        }
        let width_mask = width_mask(width);
        let check_value = |name: &str, value: u32| -> Result<()> {
            if value & !width_mask != 0 {
                return Err(Error::invalid(format!(
                    "device model {} {name} {value:#010x} exceeds its {width}-bit width",
                    self.id()
                )));
            }
            Ok(())
        };
        match self {
            Self::ConstantRead { value, .. } => check_value("value", *value)?,
            Self::SequenceRead { values, .. } => {
                for value in values {
                    check_value("sequence value", *value)?;
                }
            }
            Self::W1c {
                initial_value,
                clear_mask,
                read_clear_mask,
                ..
            } => {
                check_value("initial value", *initial_value)?;
                check_value("clear mask", *clear_mask)?;
                check_value("read-clear mask", *read_clear_mask)?;
            }
            Self::ReadToClear {
                initial_value,
                clear_mask,
                ..
            } => {
                check_value("initial value", *initial_value)?;
                check_value("clear mask", *clear_mask)?;
            }
            Self::SelfClearing {
                initial_value,
                store_mask,
                command_mask,
                ..
            } => {
                check_value("initial value", *initial_value)?;
                check_value("store mask", *store_mask)?;
                check_value("command mask", *command_mask)?;
                if store_mask & command_mask != 0 {
                    return Err(Error::invalid(format!(
                        "device model {} has overlapping store and self-clearing masks",
                        self.id()
                    )));
                }
            }
            Self::Fifo {
                read_values,
                expected_writes,
                ..
            } => {
                for value in read_values {
                    check_value("FIFO read value", *value)?;
                }
                for value in expected_writes {
                    check_value("FIFO expected write", *value)?;
                }
            }
            Self::IndexedBank { initial_values, .. } => {
                for value in initial_values {
                    check_value("bank value", *value)?;
                }
            }
        }
        Ok(())
    }

    fn configuration(&self) -> BTreeMap<String, String> {
        let mut values = BTreeMap::from([("width".to_owned(), self.width().to_string())]);
        match self {
            Self::ConstantRead { value, .. } => {
                values.insert("value".to_owned(), format!("{value:#010x}"));
            }
            Self::SequenceRead {
                values: sequence, ..
            } => {
                values.insert("values".to_owned(), sequence.len().to_string());
            }
            Self::W1c {
                initial_value,
                clear_mask,
                read_clear_mask,
                ..
            } => {
                values.insert("initial-value".to_owned(), format!("{initial_value:#010x}"));
                values.insert("clear-mask".to_owned(), format!("{clear_mask:#010x}"));
                values.insert(
                    "read-clear-mask".to_owned(),
                    format!("{read_clear_mask:#010x}"),
                );
            }
            Self::ReadToClear {
                initial_value,
                clear_mask,
                ..
            } => {
                values.insert("initial-value".to_owned(), format!("{initial_value:#010x}"));
                values.insert("clear-mask".to_owned(), format!("{clear_mask:#010x}"));
            }
            Self::SelfClearing {
                initial_value,
                store_mask,
                command_mask,
                ..
            } => {
                values.insert("initial-value".to_owned(), format!("{initial_value:#010x}"));
                values.insert("store-mask".to_owned(), format!("{store_mask:#010x}"));
                values.insert("command-mask".to_owned(), format!("{command_mask:#010x}"));
            }
            Self::Fifo {
                read_values,
                expected_writes,
                ..
            } => {
                values.insert("read-values".to_owned(), read_values.len().to_string());
                values.insert(
                    "expected-writes".to_owned(),
                    expected_writes.len().to_string(),
                );
            }
            Self::IndexedBank {
                index_address,
                data_address,
                initial_values,
                ..
            } => {
                values.insert("index-address".to_owned(), format!("{index_address:#010x}"));
                values.insert("data-address".to_owned(), format!("{data_address:#010x}"));
                values.insert("registers".to_owned(), initial_values.len().to_string());
            }
        }
        values
    }
}

impl DeviceModel for DeviceModelSpec {
    fn descriptor(&self) -> DeviceModelDescriptor {
        DeviceModelDescriptor {
            id: self.id().to_owned(),
            kind: self.kind().to_owned(),
            range: self.range(),
            configuration: self.configuration(),
        }
    }

    fn instantiate(&self) -> Result<Box<dyn DeviceModelInstance>> {
        self.validate()?;
        Ok(Box::new(StandardDeviceInstance::from(self.clone())))
    }
}
