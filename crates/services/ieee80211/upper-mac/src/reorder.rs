//! Receive Block Ack reordering of a service's agreements.
//!
//! [`RxReorder`] keeps the reorder window of each receive Block Ack
//! agreement, by peer and TID, and copies of the out-of-order MPDUs the
//! windows keep in [`PORT_REORDER_SLOTS`] slots they share. An MPDU a window
//! releases at once is not copied: [`RxReorder::offer`] reports it as the
//! release's [`CURRENT_SLOT`], and the caller delivers it from the port's
//! buffer. The caller takes a released copy with [`RxReorder::take`] and
//! delivers each MPDU of a release in its order; the port's agreement setting,
//! replay checks and delivery stay the caller's. A window that keeps an MPDU
//! behind a missing one releases its run past the gap once the caller's gap
//! time passed ([`RxReorder::arm_gaps`], [`RxReorder::expire_due_gap`]).

use oer_ieee80211_mac::{
    block_ack::{RxReorderBuffer, RxReorderError, RxReorderMpdu, RxReorderRelease},
    sequence::SequenceNumber,
};
use oer_time::{Duration, Instant};

use crate::queue::PORT_FRAME_CAPACITY;

/// MPDUs the reorder windows of all agreements hold together.
pub const PORT_REORDER_SLOTS: usize = 8;
/// The widest receive Block Ack window a service accepts.
pub const PORT_REORDER_WINDOW: usize = 64;
/// The reorder identity of the MPDU being offered, which the window
/// releases at once and the caller delivers from the port's buffer; the
/// storage slots are the identities below it.
pub const CURRENT_SLOT: u8 = PORT_REORDER_SLOTS as u8;
/// Every reorder identity: the storage slots and the current MPDU.
const IDENTITIES: usize = PORT_REORDER_SLOTS + 1;

/// The MPDUs one window released, in order.
pub type ReorderRelease = RxReorderRelease<PORT_REORDER_WINDOW>;

/// An out-of-order MPDU a reorder window keeps: a copy, so that the port's
/// buffer goes back at once.
#[derive(Clone)]
pub struct StoredFrame {
    bytes: [u8; PORT_FRAME_CAPACITY],
    len: usize,
}

impl StoredFrame {
    fn copy(frame: &[u8]) -> Option<Self> {
        let mut bytes = [0; PORT_FRAME_CAPACITY];
        bytes.get_mut(..frame.len())?.copy_from_slice(frame);
        Some(Self {
            bytes,
            len: frame.len(),
        })
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

/// What one offered MPDU did.
#[derive(Debug)]
pub enum Offer {
    /// No agreement of the peer and TID runs.
    NoAgreement,
    /// The window released `release`, in order; it includes the offered
    /// MPDU, as [`CURRENT_SLOT`], when `current`.
    Released {
        release: ReorderRelease,
        current: bool,
    },
    /// Every storage slot is taken: deliver `release`, the window's oldest
    /// run, then offer the MPDU again without making room.
    MakeRoom(ReorderRelease),
    /// The window already holds or released this sequence.
    Duplicate,
    /// The MPDU is behind the window.
    Behind,
    /// The window keeps the MPDU, but it is longer than a storage slot.
    Unbuffered,
    /// The window keeps the MPDU, but no storage slot is free.
    Dropped,
}

struct Session {
    peer: [u8; 6],
    tid: u8,
    window: RxReorderBuffer<PORT_REORDER_WINDOW, IDENTITIES>,
    /// When the window, which keeps an MPDU behind a missing one, releases
    /// its run past the gap.
    gap: Option<Instant>,
}

/// The reorder windows of up to `AGREEMENTS` receive Block Ack agreements
/// and their stored MPDUs: see the [module](self).
pub struct RxReorder<const AGREEMENTS: usize> {
    sessions: [Option<Session>; AGREEMENTS],
    slots: [Option<StoredFrame>; PORT_REORDER_SLOTS],
}

impl<const AGREEMENTS: usize> Default for RxReorder<AGREEMENTS> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const AGREEMENTS: usize> RxReorder<AGREEMENTS> {
    pub const fn new() -> Self {
        Self {
            sessions: [const { None }; AGREEMENTS],
            slots: [const { None }; PORT_REORDER_SLOTS],
        }
    }

    /// End every agreement and drop every stored MPDU.
    pub fn reset(&mut self) {
        self.sessions.iter_mut().for_each(|session| *session = None);
        self.slots.iter_mut().for_each(|slot| *slot = None);
    }

    fn index(&self, peer: [u8; 6], tid: u8) -> Option<usize> {
        self.sessions.iter().position(|session| {
            session
                .as_ref()
                .is_some_and(|session| session.peer == peer && session.tid == tid)
        })
    }

    /// Whether an agreement of `peer` and `tid` runs.
    pub fn is_active(&self, peer: [u8; 6], tid: u8) -> bool {
        self.index(peer, tid).is_some()
    }

    /// The TIDs of `peer`'s agreements.
    pub fn agreements(&self, peer: [u8; 6]) -> impl Iterator<Item = u8> + '_ {
        self.sessions
            .iter()
            .flatten()
            .filter(move |session| session.peer == peer)
            .map(|session| session.tid)
    }

    /// Start `peer`'s agreement of `tid` at `start` with a window of
    /// `window` MPDUs; `false` when it runs already, no agreement is free or
    /// the window is not one this reorders.
    pub fn accept(&mut self, peer: [u8; 6], tid: u8, start: SequenceNumber, window: u16) -> bool {
        if self.is_active(peer, tid) || usize::from(window) > PORT_REORDER_WINDOW {
            return false;
        }
        let Ok(window) = RxReorderBuffer::new(start, window) else {
            return false;
        };
        let Some(free) = self.sessions.iter_mut().find(|session| session.is_none()) else {
            return false;
        };
        *free = Some(Session {
            peer,
            tid,
            window,
            gap: None,
        });
        true
    }

    /// End `peer`'s agreement of `tid`, dropping the MPDUs it kept; `false`
    /// when none ran.
    pub fn stop(&mut self, peer: [u8; 6], tid: u8) -> bool {
        let Some(index) = self.index(peer, tid) else {
            return false;
        };
        if let Some(mut session) = self.sessions[index].take() {
            let release = session.window.stop();
            for mpdu in release.iter() {
                if let Some(slot) = self.slots.get_mut(usize::from(mpdu.slot)) {
                    *slot = None;
                }
            }
        }
        true
    }

    /// Offer one MPDU of `peer`'s agreement of `tid`, header to end of
    /// body. With `make_room`, a full storage first releases the window's
    /// oldest run ([`Offer::MakeRoom`]).
    pub fn offer(&mut self, peer: [u8; 6], tid: u8, bytes: &[u8], make_room: bool) -> Offer {
        let Some(index) = self.index(peer, tid) else {
            return Offer::NoAgreement;
        };
        let Some(sequence) = bytes.get(22..24).map(|field| {
            SequenceNumber::from_sequence_control(u16::from_le_bytes([field[0], field[1]]))
        }) else {
            return Offer::Dropped;
        };
        let Some(session) = self.sessions[index].as_mut() else {
            return Offer::NoAgreement;
        };
        let kept = match session.window.retains_on_ingest(sequence) {
            Ok(kept) => kept,
            Err(RxReorderError::DuplicateSequence(_)) => return Offer::Duplicate,
            Err(_) => return Offer::Dropped,
        };
        let slot = if kept {
            let Some(free) = self.slots.iter().position(Option::is_none) else {
                return if make_room {
                    Offer::MakeRoom(session.window.expire_gap())
                } else {
                    Offer::Dropped
                };
            };
            let Some(stored) = StoredFrame::copy(bytes) else {
                return Offer::Unbuffered;
            };
            self.slots[free] = Some(stored);
            free as u8
        } else {
            CURRENT_SLOT
        };
        match session.window.ingest(RxReorderMpdu { sequence, slot }) {
            Ok(release) if release.rejected.is_some() => {
                self.clear(slot);
                Offer::Behind
            }
            Ok(release) => Offer::Released {
                release,
                current: slot == CURRENT_SLOT,
            },
            Err(RxReorderError::DuplicateSequence(_)) => {
                self.clear(slot);
                Offer::Duplicate
            }
            Err(_) => {
                self.clear(slot);
                Offer::Dropped
            }
        }
    }

    fn clear(&mut self, slot: u8) {
        if let Some(stored) = self.slots.get_mut(usize::from(slot)) {
            *stored = None;
        }
    }

    /// Move the window of `peer`'s agreement of `tid` to `start`, as a
    /// BlockAckReq does, releasing what it passes.
    pub fn move_window(
        &mut self,
        peer: [u8; 6],
        tid: u8,
        start: SequenceNumber,
    ) -> Option<ReorderRelease> {
        let index = self.index(peer, tid)?;
        self.sessions[index].as_mut()?.window.move_window_to(start)
    }

    /// Take a released MPDU out of its storage slot.
    pub fn take(&mut self, slot: u8) -> Option<StoredFrame> {
        self.slots.get_mut(usize::from(slot))?.take()
    }

    /// The earliest instant a window releases its run past a gap.
    pub fn next_gap_deadline(&self) -> Option<Instant> {
        self.sessions
            .iter()
            .flatten()
            .filter_map(|session| session.gap)
            .min()
    }

    /// Release the buffered run of one window whose gap time passed at
    /// `now`, if any, with the window's peer.
    pub fn expire_due_gap(&mut self, now: Instant) -> Option<([u8; 6], ReorderRelease)> {
        let session = self
            .sessions
            .iter_mut()
            .flatten()
            .find(|session| session.gap.is_some_and(|due| due <= now))?;
        session.gap = None;
        Some((session.peer, session.window.expire_gap()))
    }

    /// Time the gap of every window that keeps an MPDU from the first one
    /// it kept, `gap` after `now`; a window that keeps nothing has none.
    pub fn arm_gaps(&mut self, now: Instant, gap: Duration) {
        for session in self.sessions.iter_mut().flatten() {
            session.gap = if session.window.occupied() != 0 {
                // An unrepresentable deadline is never reached.
                Some(session.gap.unwrap_or_else(|| {
                    now.checked_add(gap)
                        .unwrap_or(Instant::from_micros(u64::MAX))
                }))
            } else {
                None
            };
        }
    }
}

#[cfg(test)]
mod tests;
