//! Pluggable, execution-time peripheral behavior.
//!
//! The generic machine records raw bus transactions. Device models only own
//! the state needed to answer reads and consume writes; they do not rename or
//! normalize observable effects.

mod spec;
mod standard;

use std::{collections::BTreeMap, fmt::Debug, sync::Arc};

use serde::Serialize;

use super::{Error, MemoryRange, Result};

pub use spec::DeviceModelSpec;

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
/// to the architecture-specific backend. Every execution instantiates fresh
/// mutable state, so vendor and Rust runs cannot influence one another.
pub trait DeviceModel: Debug + Send + Sync {
    fn descriptor(&self) -> DeviceModelDescriptor;

    fn instantiate(&self) -> Result<Box<dyn DeviceModelInstance>>;
}

/// Named compiled-addon models supplied by one selected knowledge provider.
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
            return Err(Error::invalid("compiled device model registry ID is empty"));
        }
        if self.models.contains_key(&id) {
            return Err(Error::invalid(format!(
                "duplicate compiled device model registry ID {id:?}"
            )));
        }
        self.models.insert(id, model);
        Ok(())
    }

    pub fn resolve(&self, id: &str) -> Result<Arc<dyn DeviceModel>> {
        self.models.get(id).cloned().ok_or_else(|| {
            Error::invalid(format!("unknown compiled device model registry ID {id:?}"))
        })
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
