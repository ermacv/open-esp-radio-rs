//! Architecture-neutral execution environment contracts.

mod device;
mod goal;
mod service;
mod table;
#[cfg(test)]
mod tests;

pub use device::*;
pub use goal::*;
pub use service::*;
pub use table::*;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid execution environment: {message}")]
    Invalid { message: String },
}

impl Error {
    fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid {
            message: message.into(),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub struct MemoryRange {
    pub start: u32,
    pub length: u32,
}

impl MemoryRange {
    pub fn contains(self, address: u32) -> bool {
        address
            .checked_sub(self.start)
            .is_some_and(|offset| offset < self.length)
    }

    pub fn contains_access(self, address: u32, width: u8) -> bool {
        let byte_width = u32::from(width / 8);
        byte_width != 0
            && address.checked_add(byte_width).is_some_and(|access_end| {
                self.start
                    .checked_add(self.length)
                    .is_some_and(|range_end| address >= self.start && access_end <= range_end)
            })
    }

    pub fn overlaps(self, other: Self) -> bool {
        let self_end = self.start.saturating_add(self.length);
        let other_end = other.start.saturating_add(other.length);
        self.start < other_end && other.start < self_end
    }
}

const fn width_mask(width: u8) -> u32 {
    match width {
        8 => 0xff,
        16 => 0xffff,
        _ => u32::MAX,
    }
}
