//! Shared semantic identity for portable advertising lifecycles.
//!
//! Generation identifies one successful Enable epoch. Event sequence
//! identifies one scheduled advertising event within that epoch. Hardware
//! backends may copy the resulting identity for diagnostics and affinity
//! checks, but only the portable lifecycle allocates or advances it.

/// Monotonic identity of one advertising Enable epoch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyAdvertisingGeneration(u32);

impl LegacyAdvertisingGeneration {
    /// Numeric identity for diagnostics and lower-layer affinity checks.
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// Monotonic event sequence within one advertising Enable epoch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyAdvertisingEventSequence(u32);

impl LegacyAdvertisingEventSequence {
    /// Numeric event sequence for diagnostics and deadline affinity checks.
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// Exact portable identity of one advertising event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyAdvertisingEventIdentity {
    generation: LegacyAdvertisingGeneration,
    event: LegacyAdvertisingEventSequence,
}

impl LegacyAdvertisingEventIdentity {
    pub(crate) const fn new(
        generation: LegacyAdvertisingGeneration,
        event: LegacyAdvertisingEventSequence,
    ) -> Self {
        Self { generation, event }
    }

    pub const fn generation(self) -> LegacyAdvertisingGeneration {
        self.generation
    }

    pub const fn event(self) -> LegacyAdvertisingEventSequence {
        self.event
    }

    pub(crate) const fn next_event(self) -> Option<Self> {
        match self.event.0.checked_add(1) {
            Some(event) => Some(Self {
                generation: self.generation,
                event: LegacyAdvertisingEventSequence(event),
            }),
            None => None,
        }
    }

    #[cfg(test)]
    pub(crate) const fn from_parts(generation: u32, event: u32) -> Self {
        Self::new(
            LegacyAdvertisingGeneration(generation),
            LegacyAdvertisingEventSequence(event),
        )
    }
}

/// Affine source of unique advertising Enable generations.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct LegacyAdvertisingGenerationAllocator {
    next_generation: Option<u32>,
}

impl LegacyAdvertisingGenerationAllocator {
    pub(crate) const fn new() -> Self {
        Self {
            next_generation: Some(1),
        }
    }

    pub(crate) fn begin_enable(self) -> Result<(Self, LegacyAdvertisingEventIdentity), Self> {
        let Some(generation) = self.next_generation else {
            return Err(self);
        };
        Ok((
            Self {
                next_generation: generation.checked_add(1),
            },
            LegacyAdvertisingEventIdentity::new(
                LegacyAdvertisingGeneration(generation),
                LegacyAdvertisingEventSequence(0),
            ),
        ))
    }

    #[cfg(test)]
    pub(crate) const fn from_next_generation(next_generation: Option<u32>) -> Self {
        Self { next_generation }
    }
}
