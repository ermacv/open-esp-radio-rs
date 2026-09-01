//! Diagnostic Core0-to-Core1 shadow of one bounded radio egress grant.
//!
//! This is deliberately not the production grant transport. Core0 publishes
//! the peer/TID window it selected from already-admitted work; Core1 compares
//! final keyed-admission requests against that window and spends a local copy
//! of its frame credits. No shadow result changes packet ownership or returns
//! `KeyDeferred`. The measurement establishes whether the two schedulers agree
//! before an affine SPSC grant protocol is allowed to control admission.

use core::{
    num::NonZeroU8,
    sync::atomic::{AtomicU32, Ordering},
};

use crate::EgressGrantKey;

/// One stable point-in-time shadow window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EgressShadowGrantSnapshot {
    serial: u32,
    key: EgressGrantKey,
    frame_credits: NonZeroU8,
}

impl EgressShadowGrantSnapshot {
    pub const fn serial(self) -> u32 {
        self.serial
    }

    pub const fn key(self) -> EgressGrantKey {
        self.key
    }

    pub const fn frame_credits(self) -> NonZeroU8 {
        self.frame_credits
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EgressShadowGrantError {
    PublicationInProgress,
    PublicationSequenceExhausted,
}

/// Single-writer, wait-free-reader publication of a diagnostic grant window.
pub struct EgressShadowGrant {
    sequence: AtomicU32,
    packed_key: AtomicU32,
    peer_generation: AtomicU32,
    frame_credits: AtomicU32,
}

impl EgressShadowGrant {
    pub const fn new() -> Self {
        Self {
            sequence: AtomicU32::new(0),
            packed_key: AtomicU32::new(0),
            peer_generation: AtomicU32::new(0),
            frame_credits: AtomicU32::new(0),
        }
    }

    /// Return the current stable publication serial without loading its key.
    ///
    /// Readers use this as the cheap common path and take a coherent snapshot
    /// only when the serial changes. `None` means a publication is in flight.
    #[inline(always)]
    pub fn serial(&self) -> Option<u32> {
        let sequence = self.sequence.load(Ordering::Acquire);
        (sequence & 1 == 0).then_some(sequence / 2)
    }

    /// Publish a new bounded window even when its key matches the prior one.
    ///
    /// Re-publication is semantically significant: it refills Core1's local
    /// diagnostic credit copy for a newly selected Core0 burst.
    #[inline(never)]
    pub fn publish(
        &self,
        key: EgressGrantKey,
        frame_credits: NonZeroU8,
    ) -> Result<(), EgressShadowGrantError> {
        let sequence = self.begin_publication()?;
        self.packed_key.store(key.packed(), Ordering::Relaxed);
        self.peer_generation
            .store(key.peer_generation().get(), Ordering::Relaxed);
        self.frame_credits
            .store(u32::from(frame_credits.get()), Ordering::Relaxed);
        self.sequence.store(sequence + 2, Ordering::Release);
        Ok(())
    }

    /// Revoke the current window. Concurrent readers fail closed.
    #[inline(never)]
    pub fn clear(&self) -> Result<(), EgressShadowGrantError> {
        if self.frame_credits.load(Ordering::Acquire) == 0 {
            return Ok(());
        }
        let sequence = self.begin_publication()?;
        self.frame_credits.store(0, Ordering::Relaxed);
        self.packed_key.store(0, Ordering::Relaxed);
        self.peer_generation.store(0, Ordering::Relaxed);
        self.sequence.store(sequence + 2, Ordering::Release);
        Ok(())
    }

    /// Read one coherent grant without waiting for the Core0 publisher.
    #[inline(never)]
    pub fn snapshot(&self) -> Option<EgressShadowGrantSnapshot> {
        let start = self.sequence.load(Ordering::Acquire);
        if start == 0 || start & 1 != 0 {
            return None;
        }
        let packed = self.packed_key.load(Ordering::Relaxed);
        let generation = self.peer_generation.load(Ordering::Relaxed);
        let credits = self.frame_credits.load(Ordering::Relaxed);
        if self.sequence.load(Ordering::Acquire) != start {
            return None;
        }
        Some(EgressShadowGrantSnapshot {
            serial: start / 2,
            key: EgressGrantKey::from_packed(packed, generation)?,
            frame_credits: NonZeroU8::new(u8::try_from(credits).ok()?)?,
        })
    }

    fn begin_publication(&self) -> Result<u32, EgressShadowGrantError> {
        let mut current = self.sequence.load(Ordering::Acquire);
        loop {
            if current & 1 != 0 {
                return Err(EgressShadowGrantError::PublicationInProgress);
            }
            if current > u32::MAX - 2 {
                return Err(EgressShadowGrantError::PublicationSequenceExhausted);
            }
            match self.sequence.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Ok(current),
                Err(observed) => current = observed,
            }
        }
    }
}

impl Default for EgressShadowGrant {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(generation: u32) -> EgressGrantKey {
        EgressGrantKey::new(
            1,
            core::num::NonZeroU8::new(2).unwrap(),
            core::num::NonZeroU32::new(generation).unwrap(),
            0,
        )
    }

    #[test]
    fn republication_refills_one_key_under_a_new_serial() {
        let grant = EgressShadowGrant::new();
        assert_eq!(grant.snapshot(), None);
        grant.publish(key(7), NonZeroU8::new(32).unwrap()).unwrap();
        let first = grant.snapshot().unwrap();
        assert_eq!(first.serial(), 1);
        assert_eq!(first.key(), key(7));
        assert_eq!(first.frame_credits().get(), 32);

        grant.publish(key(7), NonZeroU8::new(8).unwrap()).unwrap();
        let second = grant.snapshot().unwrap();
        assert_eq!(second.serial(), 2);
        assert_eq!(second.frame_credits().get(), 8);

        grant.clear().unwrap();
        assert_eq!(grant.snapshot(), None);
    }

    #[test]
    fn reader_fails_closed_during_publication() {
        let grant = EgressShadowGrant::new();
        grant.publish(key(7), NonZeroU8::new(1).unwrap()).unwrap();
        let even = grant.sequence.load(Ordering::Acquire);
        grant.sequence.store(even + 1, Ordering::Release);
        assert_eq!(grant.snapshot(), None);
    }
}
