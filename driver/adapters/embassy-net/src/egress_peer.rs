//! Bounded cross-core publication of radio-peer scheduling identities.
//!
//! The radio owner publishes only on peer-lifecycle changes. Network egress
//! classification reads one immutable-looking snapshot without taking a
//! critical section or spinning behind Core0. A read concurrent with a
//! publication fails closed as `None`; final radio admission remains the
//! authority for every packet.

use core::{
    num::{NonZeroU8, NonZeroU32},
    sync::atomic::{AtomicU32, Ordering},
};

/// Generation-bound identity of one radio peer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EgressPeerIdentity {
    destination: [u8; 6],
    slot: NonZeroU8,
    generation: NonZeroU32,
}

impl EgressPeerIdentity {
    /// Construct one unicast peer identity.
    ///
    /// Slot zero and generation zero are reserved for an unpublished entry.
    /// Group addresses are scheduling domains of their own and are therefore
    /// never accepted as peer identities.
    pub fn try_new(destination: [u8; 6], slot: u16, generation: u32) -> Option<Self> {
        if destination[0] & 1 != 0 {
            return None;
        }
        Some(Self {
            destination,
            slot: NonZeroU8::new(u8::try_from(slot).ok()?)?,
            generation: NonZeroU32::new(generation)?,
        })
    }

    pub const fn destination(self) -> [u8; 6] {
        self.destination
    }

    pub const fn slot(self) -> NonZeroU8 {
        self.slot
    }

    pub const fn generation(self) -> NonZeroU32 {
        self.generation
    }
}

/// Read-only route-to-peer lookup consumed by a network endpoint.
///
/// A successful lookup is queue identity, not transmit authority. The Core0
/// radio owner must still validate the peer generation at final admission.
pub trait EgressPeerResolver: Sync {
    fn resolve(&self, destination: [u8; 6]) -> Option<EgressPeerIdentity>;

    /// Validate one previously resolved slot/generation at final admission.
    ///
    /// This is deliberately independent from destination lookup. The network
    /// stack retains only the opaque device key after classification, while a
    /// peer-directory publication may race that key with reassociation. A
    /// successful result is still queue identity rather than radio authority;
    /// it only proves that the classified generation remains current.
    fn is_current(&self, slot: NonZeroU8, generation: NonZeroU32) -> bool;

    /// Monotonic completed-publication revision.
    fn revision(&self) -> u32;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EgressPeerDirectoryError {
    /// The array position must equal `peer.slot() - 1`.
    SlotMismatch,
    /// One snapshot cannot publish the same link destination twice.
    DuplicateDestination,
    /// The synchronization generation is intentionally not reusable.
    PublicationSequenceExhausted,
    /// A second publisher attempted to overlap the bounded transaction.
    PublicationInProgress,
}

struct PublishedPeer {
    destination_low: AtomicU32,
    destination_high: AtomicU32,
    slot: AtomicU32,
    generation: AtomicU32,
}

impl PublishedPeer {
    const fn new() -> Self {
        Self {
            destination_low: AtomicU32::new(0),
            destination_high: AtomicU32::new(0),
            slot: AtomicU32::new(0),
            generation: AtomicU32::new(0),
        }
    }

    fn load(&self) -> Option<EgressPeerIdentity> {
        let slot = NonZeroU8::new(u8::try_from(self.slot.load(Ordering::Relaxed)).ok()?)?;
        let generation = NonZeroU32::new(self.generation.load(Ordering::Relaxed))?;
        let low = self.destination_low.load(Ordering::Relaxed).to_le_bytes();
        let high = self.destination_high.load(Ordering::Relaxed).to_le_bytes();
        Some(EgressPeerIdentity {
            destination: [low[0], low[1], low[2], low[3], high[0], high[1]],
            slot,
            generation,
        })
    }

    fn store(&self, peer: Option<EgressPeerIdentity>) {
        let (destination_low, destination_high, slot, generation) = match peer {
            Some(peer) => {
                let destination = peer.destination();
                (
                    u32::from_le_bytes([
                        destination[0],
                        destination[1],
                        destination[2],
                        destination[3],
                    ]),
                    u32::from(u16::from_le_bytes([destination[4], destination[5]])),
                    u32::from(peer.slot().get()),
                    peer.generation().get(),
                )
            }
            None => (0, 0, 0, 0),
        };
        self.destination_low
            .store(destination_low, Ordering::Relaxed);
        self.destination_high
            .store(destination_high, Ordering::Relaxed);
        self.generation.store(generation, Ordering::Relaxed);
        // The slot is the validity marker and is written last inside the
        // publication transaction.
        self.slot.store(slot, Ordering::Relaxed);
    }
}

/// Fixed-capacity, allocation-free peer directory shared by Core0 and Core1.
///
/// All entry words are atomic so Rust never creates a cross-core data race.
/// The directory sequence is a non-blocking seqlock for readers: a lookup
/// concurrent with an update returns `None` instead of waiting for Core0.
pub struct EgressPeerDirectory<const CAPACITY: usize> {
    sequence: AtomicU32,
    peers: [PublishedPeer; CAPACITY],
}

impl<const CAPACITY: usize> EgressPeerDirectory<CAPACITY> {
    pub const fn new() -> Self {
        assert!(CAPACITY != 0, "an egress peer directory needs one slot");
        assert!(CAPACITY <= u8::MAX as usize, "peer slots are encoded as u8");
        Self {
            sequence: AtomicU32::new(0),
            peers: [const { PublishedPeer::new() }; CAPACITY],
        }
    }

    /// Atomically replace the complete peer snapshot.
    ///
    /// The caller supplies entries in radio-slot order. Publishing identical
    /// identity state performs no atomic writes, so unrelated AP status
    /// changes do not bounce the directory between cores.
    pub fn replace(
        &self,
        peers: &[Option<EgressPeerIdentity>; CAPACITY],
    ) -> Result<bool, EgressPeerDirectoryError> {
        Self::validate(peers)?;
        if self.matches(peers) {
            return Ok(false);
        }

        let sequence = self.begin_publication()?;
        for (published, peer) in self.peers.iter().zip(peers.iter().copied()) {
            published.store(peer);
        }
        self.sequence.store(sequence + 2, Ordering::Release);
        Ok(true)
    }

    pub fn clear(&self) -> Result<bool, EgressPeerDirectoryError> {
        self.replace(&[None; CAPACITY])
    }

    fn validate(
        peers: &[Option<EgressPeerIdentity>; CAPACITY],
    ) -> Result<(), EgressPeerDirectoryError> {
        for (index, peer) in peers.iter().enumerate() {
            let Some(peer) = peer else {
                continue;
            };
            if usize::from(peer.slot().get()) != index + 1 {
                return Err(EgressPeerDirectoryError::SlotMismatch);
            }
            if peers[..index]
                .iter()
                .flatten()
                .any(|existing| existing.destination() == peer.destination())
            {
                return Err(EgressPeerDirectoryError::DuplicateDestination);
            }
        }
        Ok(())
    }

    fn matches(&self, expected: &[Option<EgressPeerIdentity>; CAPACITY]) -> bool {
        let start = self.sequence.load(Ordering::Acquire);
        if start & 1 != 0 {
            return false;
        }
        let equal = self
            .peers
            .iter()
            .zip(expected)
            .all(|(published, expected)| published.load() == *expected);
        equal && self.sequence.load(Ordering::Acquire) == start
    }

    fn begin_publication(&self) -> Result<u32, EgressPeerDirectoryError> {
        let mut current = self.sequence.load(Ordering::Acquire);
        loop {
            if current & 1 != 0 {
                return Err(EgressPeerDirectoryError::PublicationInProgress);
            }
            if current > u32::MAX - 2 {
                return Err(EgressPeerDirectoryError::PublicationSequenceExhausted);
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

    fn lookup(&self, destination: [u8; 6]) -> Option<EgressPeerIdentity> {
        if destination[0] & 1 != 0 {
            return None;
        }
        let start = self.sequence.load(Ordering::Acquire);
        if start & 1 != 0 {
            return None;
        }
        let resolved = self
            .peers
            .iter()
            .filter_map(PublishedPeer::load)
            .find(|peer| peer.destination() == destination);
        (self.sequence.load(Ordering::Acquire) == start)
            .then_some(resolved)
            .flatten()
    }

    fn lookup_slot(&self, slot: NonZeroU8) -> Option<EgressPeerIdentity> {
        let start = self.sequence.load(Ordering::Acquire);
        if start & 1 != 0 {
            return None;
        }
        let resolved = self
            .peers
            .get(usize::from(slot.get()) - 1)
            .and_then(PublishedPeer::load);
        (self.sequence.load(Ordering::Acquire) == start)
            .then_some(resolved)
            .flatten()
    }
}

impl<const CAPACITY: usize> Default for EgressPeerDirectory<CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const CAPACITY: usize> EgressPeerResolver for EgressPeerDirectory<CAPACITY> {
    fn resolve(&self, destination: [u8; 6]) -> Option<EgressPeerIdentity> {
        self.lookup(destination)
    }

    fn is_current(&self, slot: NonZeroU8, generation: NonZeroU32) -> bool {
        self.lookup_slot(slot)
            .is_some_and(|peer| peer.generation() == generation)
    }

    fn revision(&self) -> u32 {
        let sequence = self.sequence.load(Ordering::Acquire);
        // An in-progress publication still exposes the previous completed
        // revision. Lookup fails closed until the matching even edge.
        (sequence & !1) / 2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: [u8; 6] = [0x02, 0, 0, 0, 0, 1];
    const B: [u8; 6] = [0x02, 0, 0, 0, 0, 2];

    fn peer(address: [u8; 6], slot: u16, generation: u32) -> EgressPeerIdentity {
        EgressPeerIdentity::try_new(address, slot, generation).expect("valid peer identity")
    }

    #[test]
    fn replaces_generation_bound_peers_without_republishing_unchanged_state() {
        let directory = EgressPeerDirectory::<2>::new();
        let first = [Some(peer(A, 1, 7)), Some(peer(B, 2, 9))];
        assert_eq!(directory.revision(), 0);
        assert_eq!(directory.replace(&first), Ok(true));
        assert_eq!(directory.revision(), 1);
        assert_eq!(directory.resolve(A), first[0]);
        assert_eq!(directory.resolve(B), first[1]);
        assert!(directory.is_current(peer(A, 1, 7).slot(), peer(A, 1, 7).generation()));

        assert_eq!(directory.replace(&first), Ok(false));
        assert_eq!(directory.revision(), 1);

        let replacement = [Some(peer(A, 1, 10)), None];
        assert_eq!(directory.replace(&replacement), Ok(true));
        assert_eq!(directory.revision(), 2);
        assert_eq!(directory.resolve(A), replacement[0]);
        assert_eq!(directory.resolve(B), None);
        assert!(!directory.is_current(peer(A, 1, 7).slot(), peer(A, 1, 7).generation()));
        assert!(directory.is_current(peer(A, 1, 10).slot(), peer(A, 1, 10).generation()));
    }

    #[test]
    fn rejects_malformed_snapshots_and_group_identities() {
        let directory = EgressPeerDirectory::<2>::new();
        assert_eq!(EgressPeerIdentity::try_new([0xff; 6], 1, 1), None);
        assert_eq!(EgressPeerIdentity::try_new(A, 0, 1), None);
        assert_eq!(EgressPeerIdentity::try_new(A, 1, 0), None);
        assert_eq!(
            directory.replace(&[None, Some(peer(B, 1, 1))]),
            Err(EgressPeerDirectoryError::SlotMismatch)
        );
        assert_eq!(
            directory.replace(&[Some(peer(A, 1, 1)), Some(peer(A, 2, 2))]),
            Err(EgressPeerDirectoryError::DuplicateDestination)
        );
    }

    #[test]
    fn lookup_fails_closed_while_publication_is_in_progress() {
        let directory = EgressPeerDirectory::<1>::new();
        directory.replace(&[Some(peer(A, 1, 1))]).unwrap();
        let even = directory.sequence.load(Ordering::Acquire);
        directory.sequence.store(even + 1, Ordering::Release);
        assert_eq!(directory.resolve(A), None);
        assert_eq!(directory.revision(), even / 2);
    }
}
