//! Owned CCMP keys and a fixed-capacity software key table.
//!
//! This is the source-owned hardware-independent key boundary.
//! Hardware slot selection remains in the chip MAC crate; this module owns
//! key material, slot identity, replacement, and zeroization.

use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::{Ptk, Wpa2Interface, frames::Wpa2Gtk};

pub const WPA2_TK_LEN: usize = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Wpa2KeyKind {
    Pairwise,
    Group { key_id: u8, transmit: bool },
}

/// Owned CCMP temporal-key bytes, zeroized on drop.
///
/// Hardware adapters borrow bytes; the restricted PAC converts them into
/// register words without casting this owner or publishing its address to DMA.
/// The historical four-byte alignment is retained for layout compatibility.
#[derive(Zeroize, ZeroizeOnDrop)]
#[repr(C, align(4))]
pub struct CcmpKey {
    bytes: [u8; WPA2_TK_LEN],
}

/// Compatibility name for the original portable key owner.
pub type AlignedCcmpKey = CcmpKey;

impl CcmpKey {
    pub const fn new(bytes: [u8; WPA2_TK_LEN]) -> Self {
        Self { bytes }
    }

    pub fn from_ptk(ptk: &Ptk) -> Self {
        Self::new(*ptk.temporal_key())
    }

    pub fn from_gtk(gtk: &Wpa2Gtk) -> Self {
        Self::new(*gtk.key())
    }

    pub const fn as_bytes(&self) -> &[u8; WPA2_TK_LEN] {
        &self.bytes
    }

    pub fn is_word_aligned(&self) -> bool {
        self.bytes.as_ptr().addr() & 3 == 0
    }
}

/// One key installation request with all key material owned by the request.
pub struct Wpa2KeyInstall {
    interface: Wpa2Interface,
    peer: [u8; 6],
    kind: Wpa2KeyKind,
    receive_sequence: [u8; 8],
    key: CcmpKey,
}

impl Wpa2KeyInstall {
    pub fn pairwise(
        interface: Wpa2Interface,
        peer: [u8; 6],
        receive_sequence: [u8; 8],
        ptk: &Ptk,
    ) -> Self {
        Self {
            interface,
            peer,
            kind: Wpa2KeyKind::Pairwise,
            receive_sequence,
            key: CcmpKey::from_ptk(ptk),
        }
    }

    pub fn group(interface: Wpa2Interface, gtk: &Wpa2Gtk, receive_sequence: [u8; 8]) -> Self {
        Self {
            interface,
            peer: [0xff; 6],
            kind: Wpa2KeyKind::Group {
                key_id: gtk.key_id(),
                transmit: gtk.transmit(),
            },
            receive_sequence,
            key: CcmpKey::from_gtk(gtk),
        }
    }

    pub const fn interface(&self) -> Wpa2Interface {
        self.interface
    }

    pub const fn peer(&self) -> &[u8; 6] {
        &self.peer
    }

    pub const fn kind(&self) -> Wpa2KeyKind {
        self.kind
    }

    pub const fn receive_sequence(&self) -> &[u8; 8] {
        &self.receive_sequence
    }

    pub const fn key(&self) -> &CcmpKey {
        &self.key
    }

    fn same_slot(&self, other: &Self) -> bool {
        if self.interface != other.interface || self.peer != other.peer {
            return false;
        }
        match (self.kind, other.kind) {
            (Wpa2KeyKind::Pairwise, Wpa2KeyKind::Pairwise) => true,
            (Wpa2KeyKind::Group { key_id: left, .. }, Wpa2KeyKind::Group { key_id: right, .. }) => {
                left == right
            }
            (Wpa2KeyKind::Pairwise, Wpa2KeyKind::Group { .. })
            | (Wpa2KeyKind::Group { .. }, Wpa2KeyKind::Pairwise) => false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaticKeyTableError {
    Full,
}

/// Persistent fixed-capacity ownership for hardware or software key slots.
///
/// Replacing or removing an entry drops and zeroizes the old key.
pub struct StaticWpa2Keys<const N: usize> {
    slots: [Option<Wpa2KeyInstall>; N],
}

impl<const N: usize> StaticWpa2Keys<N> {
    pub const fn new() -> Self {
        Self {
            slots: [const { None }; N],
        }
    }

    pub fn insert(&mut self, key: Wpa2KeyInstall) -> Result<usize, StaticKeyTableError> {
        let index = self.slot_for(&key)?;
        self.slots[index] = Some(key);
        Ok(index)
    }

    pub fn slot_for(&self, key: &Wpa2KeyInstall) -> Result<usize, StaticKeyTableError> {
        self.slots
            .iter()
            .position(|slot| slot.as_ref().is_some_and(|old| old.same_slot(key)))
            .or_else(|| self.slots.iter().position(Option::is_none))
            .ok_or(StaticKeyTableError::Full)
    }

    pub fn replace_at(
        &mut self,
        index: usize,
        key: Wpa2KeyInstall,
    ) -> Result<&Wpa2KeyInstall, Wpa2KeyInstall> {
        let Some(slot) = self.slots.get_mut(index) else {
            return Err(key);
        };
        Ok(slot.insert(key))
    }

    pub fn get(&self, index: usize) -> Option<&Wpa2KeyInstall> {
        self.slots.get(index)?.as_ref()
    }

    pub fn remove(&mut self, index: usize) -> Option<Wpa2KeyInstall> {
        self.slots.get_mut(index)?.take()
    }
}

impl<const N: usize> Default for StaticWpa2Keys<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
