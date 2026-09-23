//! Explicit capacity admission. Reservations account for requested allocations,
//! not allocator bookkeeping, stacks, library internals or resident process pages.
use crate::*;
use std::{
    cell::Cell,
    ops::{Deref, DerefMut},
};

pub const DEFAULT_WORKING_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkingMemoryObservation {
    pub limit_bytes: u64,
    pub reserved_bytes: u64,
    pub peak_reserved_bytes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryFailure {
    pub requested_bytes: u64,
    pub available_bytes: u64,
    pub limit_bytes: u64,
    pub phase: RunPhase,
    pub input: Option<u64>,
    pub member: Option<u64>,
}

/// One operation's allocation authority. No global allocator or implicit growth.
pub struct WorkingMemory {
    limit: u64,
    used: Cell<u64>,
    peak: Cell<u64>,
}
impl WorkingMemory {
    pub fn new(limit: u64) -> Result<Self> {
        if limit == 0 {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "working memory must be positive",
            ));
        }
        Ok(Self {
            limit,
            used: Cell::new(0),
            peak: Cell::new(0),
        })
    }
    pub fn used(&self) -> u64 {
        self.used.get()
    }
    pub fn peak(&self) -> u64 {
        self.peak.get()
    }
    pub fn observation(&self) -> WorkingMemoryObservation {
        WorkingMemoryObservation {
            limit_bytes: self.limit,
            reserved_bytes: self.used(),
            peak_reserved_bytes: self.peak(),
        }
    }
    pub fn reserve(&self, bytes: u64, position: RunPosition) -> Result<MemoryReservation<'_>> {
        let available = self.limit - self.used.get();
        if bytes > available {
            let mut error = Error::new(
                ErrorCode::ResourceLimited,
                format!(
                    "working memory exhausted: requested {bytes} bytes; available {available} bytes; phase {:?}; input {:?}; member {:?}",
                    position.phase, position.input, position.member
                ),
            );
            error.memory = Some(MemoryFailure {
                requested_bytes: bytes,
                available_bytes: available,
                limit_bytes: self.limit,
                phase: position.phase,
                input: position.input,
                member: position.member,
            });
            return Err(error);
        }
        let used = self.used.get() + bytes;
        self.used.set(used);
        self.peak.set(self.peak.get().max(used));
        Ok(MemoryReservation {
            memory: self,
            bytes,
        })
    }
    /// Initialized, fallibly allocated scratch. Its capacity and lifetime remain
    /// attached to the reservation; dropping on any path returns the capacity.
    pub fn bytes(&self, size: usize, position: RunPosition) -> Result<ScratchBytes<'_>> {
        let reservation = self.reserve(size as u64, position)?;
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(size).map_err(|_| {
            Error::new(
                ErrorCode::ResourceLimited,
                "host allocation refused working buffer",
            )
        })?;
        bytes.resize(size, 0);
        Ok(ScratchBytes {
            bytes,
            _reservation: reservation,
        })
    }
}

pub struct MemoryReservation<'a> {
    memory: &'a WorkingMemory,
    bytes: u64,
}
impl Drop for MemoryReservation<'_> {
    fn drop(&mut self) {
        self.memory.used.set(self.memory.used.get() - self.bytes);
    }
}

/// Scratch cannot be detached from its accounting or outlive its operation.
///
/// ```compile_fail
/// use blobray_domain::{WorkingMemory, RunPosition};
/// let bytes = { let memory = WorkingMemory::new(8).unwrap();
///     memory.bytes(8, RunPosition::default()).unwrap() };
/// println!("{}", bytes.len());
/// ```
pub struct ScratchBytes<'a> {
    bytes: Vec<u8>,
    _reservation: MemoryReservation<'a>,
}
impl Deref for ScratchBytes<'_> {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.bytes
    }
}
impl DerefMut for ScratchBytes<'_> {
    fn deref_mut(&mut self) -> &mut [u8] {
        &mut self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overlapping_scopes_are_bounded_and_errors_release_capacity() {
        let memory = WorkingMemory::new(16).unwrap();
        let a = memory.bytes(10, RunPosition::default()).unwrap();
        assert!(memory.bytes(7, RunPosition::default()).is_err());
        {
            let _b = memory.bytes(6, RunPosition::default()).unwrap();
        }
        assert_eq!(memory.used(), 10);
        drop(a);
        assert_eq!(memory.used(), 0);
        assert_eq!(memory.peak(), 16);
    }
}

#[cfg(test)]
mod unwind_tests {
    use super::*;
    #[test]
    fn unwind_releases_scratch_before_next_scope() {
        let memory = WorkingMemory::new(8).unwrap();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _bytes = memory.bytes(8, RunPosition::default()).unwrap();
            panic!("phase failed");
        }));
        assert!(result.is_err());
        assert_eq!(memory.used(), 0);
        assert!(memory.bytes(8, RunPosition::default()).is_ok());
        assert_eq!(memory.used(), 0);
        assert_eq!(
            memory
                .reserve(u64::MAX, RunPosition::default())
                .err()
                .unwrap()
                .memory
                .unwrap()
                .requested_bytes,
            u64::MAX
        );
    }
}
