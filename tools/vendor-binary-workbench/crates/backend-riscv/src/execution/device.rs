//! Pluggable, execution-time peripheral behavior.
//!
//! The generic machine records raw bus transactions. Device models only own
//! the state needed to answer reads and consume writes; they do not rename or
//! normalize observable effects.

use std::{collections::BTreeMap, fmt::Debug, sync::Arc};

use serde::{Deserialize, Serialize};

use super::{MemoryRange, MmioValue};
use crate::Result;

/// Stable description retained by reports and diagnostics.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DeviceModelDescriptor {
    pub id: String,
    pub kind: String,
    pub range: MemoryRange,
    pub configuration: BTreeMap<String, String>,
}

/// Per-instance proof status after concrete execution.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DeviceModelCoverage {
    pub complete: bool,
    pub reason: Option<String>,
}

/// One model descriptor paired with the completeness of its concrete run.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DeviceModelOutcome {
    pub descriptor: DeviceModelDescriptor,
    pub coverage: DeviceModelCoverage,
}

impl DeviceModelCoverage {
    pub const fn complete() -> Self {
        Self {
            complete: true,
            reason: None,
        }
    }

    pub fn incomplete(reason: impl Into<String>) -> Self {
        Self {
            complete: false,
            reason: Some(reason.into()),
        }
    }
}

/// Cloneable scenario-level factory for one execution-time peripheral model.
///
/// Platform crates may implement this trait without adding their vocabulary
/// to the generic RISC-V backend. Every execution instantiates fresh mutable
/// state, so vendor and Rust runs cannot influence one another.
pub trait DeviceModel: Debug + Send + Sync {
    fn descriptor(&self) -> DeviceModelDescriptor;

    fn instantiate(&self) -> Result<Box<dyn DeviceModelInstance>>;
}

/// Named compiled-addon models supplied by one selected platform harness.
///
/// The registry does not infer a model from an address or semantic name. A
/// scenario or platform resolver must request an exact reviewed ID.
#[derive(Clone, Debug, Default)]
pub struct DeviceModelRegistry {
    models: BTreeMap<String, Arc<dyn DeviceModel>>,
}

impl DeviceModelRegistry {
    pub fn register(&mut self, id: impl Into<String>, model: Arc<dyn DeviceModel>) -> Result<()> {
        let id = id.into();
        if id.trim().is_empty() {
            return Err("compiled device model registry ID is empty".into());
        }
        if self.models.contains_key(&id) {
            return Err(format!("duplicate compiled device model registry ID {id:?}").into());
        }
        self.models.insert(id, model);
        Ok(())
    }

    pub fn resolve(&self, id: &str) -> Result<Arc<dyn DeviceModel>> {
        self.models
            .get(id)
            .cloned()
            .ok_or_else(|| format!("unknown compiled device model registry ID {id:?}").into())
    }

    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.models.keys().map(String::as_str)
    }
}

/// Mutable state owned by one concrete execution.
pub trait DeviceModelInstance: Debug + Send {
    fn read(&mut self, address: u32, width: u8) -> Result<u32>;

    fn write(&mut self, address: u32, width: u8, value: u32) -> Result<()>;

    /// Report unused scripted state or unmet expectations without converting
    /// an otherwise valid concrete trace into an executor failure.
    fn finish(&mut self) -> Result<DeviceModelCoverage> {
        Ok(DeviceModelCoverage::complete())
    }
}

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
            return Err("device model has an empty id".into());
        }
        let width = self.width();
        if !matches!(width, 8 | 16 | 32) {
            return Err(format!("device model {} has unsupported width {width}", self.id()).into());
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
                    return Err(format!(
                        "indexed-bank device model {} uses the same index and data address",
                        self.id()
                    )
                    .into());
                }
                if initial_values.is_empty() {
                    return Err(format!(
                        "indexed-bank device model {} has no registers",
                        self.id()
                    )
                    .into());
                }
                (*index_address, Some(*data_address))
            }
            Self::SequenceRead {
                address, values, ..
            } => {
                if values.is_empty() {
                    return Err(
                        format!("sequence-read device model {} has no values", self.id()).into(),
                    );
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
            return Err(format!(
                "device model {} address is not {width}-bit aligned",
                self.id()
            )
            .into());
        }
        let width_mask = MmioValue::mask(width);
        let check_value = |name: &str, value: u32| -> Result<()> {
            if value & !width_mask != 0 {
                return Err(format!(
                    "device model {} {name} {value:#010x} exceeds its {width}-bit width",
                    self.id()
                )
                .into());
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
                    return Err(format!(
                        "device model {} has overlapping store and self-clearing masks",
                        self.id()
                    )
                    .into());
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

#[derive(Debug)]
enum StandardDeviceInstance {
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
        return Err(format!(
            "device model requires an exact {expected_width}-bit access at {expected_address:#010x}, got {width} bits at {address:#010x}"
        )
        .into());
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
                    .ok_or_else(|| "sequence-read device model is exhausted".into())
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
                    .ok_or_else(|| "FIFO read sequence is exhausted".into())
            }
            Self::IndexedBank {
                index_address,
                data_address,
                width: expected_width,
                selected,
                values,
            } => {
                if width != *expected_width {
                    return Err(
                        format!("indexed bank requires {expected_width}-bit accesses").into(),
                    );
                }
                if address == *index_address {
                    Ok(u32::try_from(*selected).expect("selected index fits u32"))
                } else if address == *data_address {
                    Ok(values[*selected])
                } else {
                    Err(format!("unsupported indexed-bank read at {address:#010x}").into())
                }
            }
        }
    }

    fn write(&mut self, address: u32, width: u8, written: u32) -> Result<()> {
        match self {
            Self::ConstantRead { .. } | Self::SequenceRead { .. } | Self::ReadToClear { .. } => {
                Err(format!("read-only device model received a write at {address:#010x}").into())
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
                    return Err("FIFO received an unexpected write".into());
                };
                if expected != written {
                    return Err(format!(
                        "FIFO write mismatch: expected {expected:#010x}, got {written:#010x}"
                    )
                    .into());
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
                    return Err(
                        format!("indexed bank requires {expected_width}-bit accesses").into(),
                    );
                }
                if address == *index_address {
                    let index = usize::try_from(written)
                        .ok()
                        .filter(|index| *index < values.len())
                        .ok_or_else(|| {
                            format!("indexed-bank selection {written} is out of range")
                        })?;
                    *selected = index;
                    Ok(())
                } else if address == *data_address {
                    values[*selected] = written & MmioValue::mask(width);
                    Ok(())
                } else {
                    Err(format!("unsupported indexed-bank write at {address:#010x}").into())
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
