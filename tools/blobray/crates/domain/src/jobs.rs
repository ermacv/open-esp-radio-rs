//! Resource policy values, lifecycle outcomes and lossless origin paths.
use crate::*;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LimitMode {
    Kernel,
    Watchdog,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceBudget {
    pub mode: LimitMode,
    pub memory_bytes: u64,
    /// Algorithm capacity, distinct from the host process limit. Absent in old runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_memory_bytes: Option<u64>,
    pub timeout_ms: u64,
    pub grace_ms: u64,
    pub poll_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_work_units: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_policy: Option<u32>,
}
impl Default for ResourceBudget {
    fn default() -> Self {
        Self {
            mode: LimitMode::Kernel,
            memory_bytes: 4 * 1024 * 1024 * 1024,
            working_memory_bytes: Some(DEFAULT_WORKING_BYTES),
            timeout_ms: 900_000,
            grace_ms: 10_000,
            poll_ms: 100,
            max_work_units: Some(DEFAULT_WORK_UNITS),
            work_policy: Some(WORK_POLICY),
        }
    }
}
impl ResourceBudget {
    pub fn validate(&self) -> Result<()> {
        if self.work_policy != Some(WORK_POLICY) {
            return Err(Error::new(
                ErrorCode::Incompatible,
                "unsupported work accounting policy",
            ));
        }
        if self.max_work_units.is_none_or(|n| n == 0)
            || self.memory_bytes == 0
            || self.working_memory_bytes == Some(0)
            || self.timeout_ms == 0
            || self.poll_ms == 0
            || self.poll_ms > 1000
            || self.grace_ms > 60_000
        {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "positive work/memory/time budgets and work policy 1, poll 1..1000 ms and grace <=60000 ms required",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunState {
    Registered,
    Running,
    Validating,
    Completed,
    Cancelled,
    TimedOut,
    ResourceLimited,
    Failed,
    Abandoned,
}
impl RunState {
    pub fn terminal(self) -> bool {
        !matches!(self, Self::Registered | Self::Running | Self::Validating)
    }
}

impl OriginPath {
    pub fn from_path(path: &Path) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            Self::UnixBytes {
                bytes: path.as_os_str().as_bytes().to_vec(),
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            Self::WindowsWide {
                units: path.as_os_str().encode_wide().collect(),
            }
        }
    }
    pub fn to_path(&self) -> Result<PathBuf> {
        #[cfg(unix)]
        if let Self::UnixBytes { bytes } = self {
            use std::os::unix::ffi::OsStringExt;
            return Ok(std::ffi::OsString::from_vec(bytes.clone()).into());
        }
        #[cfg(windows)]
        if let Self::WindowsWide { units } = self {
            use std::os::windows::ffi::OsStringExt;
            return Ok(std::ffi::OsString::from_wide(units).into());
        }
        Err(Error::new(
            ErrorCode::Incompatible,
            "origin encoding does not match this host",
        ))
    }
}

/// Bounded observation stream; the application retains terminal outcomes separately.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunEvent {
    pub run: RunId,
    pub sequence: u64,
    pub state: RunState,
    pub progress: Option<RunProgress>,
}
