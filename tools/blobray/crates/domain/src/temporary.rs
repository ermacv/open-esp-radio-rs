//! Logical temporary-file capacity, independent of resident memory and plan identity.
use crate::*;

pub const TEMPORARY_CONTROL_BYTES: u64 = 1024 * 1024;
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemporaryUsage {
    pub limit_bytes: u64,
    pub current_bytes: u64,
    pub peak_bytes: u64,
    pub control_reserved_bytes: u64,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StorageFailure {
    pub requested_bytes: u64,
    pub available_bytes: u64,
    pub limit_bytes: u64,
    pub owner: Option<RunId>,
    pub position: RunPosition,
}
/// Charge growth before issuing a write. Release only after truncation/deletion.
pub trait TemporaryCapacity {
    fn reserve(&self, bytes: u64) -> Result<()>;
    fn release(&self, bytes: u64);
    fn usage(&self) -> TemporaryUsage;
}
/// Preserve typed capacity errors transported through std I/O adapters.
pub fn storage_io(error: std::io::Error) -> Error {
    if let Some(typed) = error.get_ref().and_then(|e| e.downcast_ref::<Error>()) {
        return typed.clone();
    }
    let code = match error.kind() {
        std::io::ErrorKind::StorageFull | std::io::ErrorKind::QuotaExceeded => ErrorCode::DiskFull,
        _ => ErrorCode::Io,
    };
    Error::new(code, error.to_string())
}
