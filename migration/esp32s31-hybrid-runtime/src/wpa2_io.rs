//! Owned, fail-fast I/O boundary for the WPA2-Personal state machines.
//!
//! This module deliberately does not call the vendor TX or key-install
//! wrappers. Those wrappers retain or allocate storage in the pinned S31
//! blobs. Instead, a single radio owner receives one fully owned command and
//! gives it to a backend exactly once. A strict backend must either accept the
//! command immediately into fixed storage or return it unchanged.

use core::{
    sync::atomic::{compiler_fence, Ordering},
    task::{Context, Poll},
};

use crate::{
    command::{PendingCommandAction, RadioCommandHandler, RadioCommandQueue},
    data_tx::OwnedWifiDataTxFrame,
    wpa2::Wpa2Interface,
    wpa2_crypto::{Wpa2Ptk, WPA2_TK_LEN},
    wpa2_frames::{Wpa2EthernetFrame, Wpa2Gtk, WPA2_TX_ETHERNET_CAPACITY},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Wpa2KeyKind {
    Pairwise,
    Group { key_id: u8, transmit: bool },
}

/// A CCMP key whose address always selects the allocation-free branch in the
/// pinned `hal_crypto_set_key_entry` implementation.
#[repr(C, align(4))]
pub struct AlignedCcmpKey {
    bytes: [u8; WPA2_TK_LEN],
}

impl AlignedCcmpKey {
    pub const fn new(bytes: [u8; WPA2_TK_LEN]) -> Self {
        Self { bytes }
    }

    pub fn from_ptk(ptk: &Wpa2Ptk) -> Self {
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

impl Drop for AlignedCcmpKey {
    fn drop(&mut self) {
        for byte in &mut self.bytes {
            unsafe { core::ptr::write_volatile(byte, 0) };
        }
        compiler_fence(Ordering::SeqCst);
    }
}

pub struct Wpa2KeyInstall {
    interface: Wpa2Interface,
    peer: [u8; 6],
    kind: Wpa2KeyKind,
    receive_sequence: [u8; 8],
    key: AlignedCcmpKey,
}

impl Wpa2KeyInstall {
    pub fn pairwise(
        interface: Wpa2Interface,
        peer: [u8; 6],
        receive_sequence: [u8; 8],
        ptk: &Wpa2Ptk,
    ) -> Self {
        Self {
            interface,
            peer,
            kind: Wpa2KeyKind::Pairwise,
            receive_sequence,
            key: AlignedCcmpKey::from_ptk(ptk),
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
            key: AlignedCcmpKey::from_gtk(gtk),
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

    pub const fn key(&self) -> &AlignedCcmpKey {
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

/// Persistent fixed-capacity ownership for hardware/software key slots.
/// Replacing or removing an entry runs the volatile key destructor.
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

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn replace_at(
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

pub enum Wpa2IoCommand<const N: usize = WPA2_TX_ETHERNET_CAPACITY> {
    Transmit(Wpa2EthernetFrame<N>),
    TransmitData(OwnedWifiDataTxFrame),
    InstallKey(Wpa2KeyInstall),
    SetPeerAuthorized {
        interface: Wpa2Interface,
        peer: [u8; 6],
        authorized: bool,
    },
    /// Close the STA controlled port and remove every Rust-owned key and
    /// association fact for `peer`.
    ///
    /// The S31 backend executes this as one bounded radio-owner transaction.
    /// Callers must first stop producing data and await the fixed TX ownership
    /// drain; this command never waits for outstanding hardware work.
    ResetStaLink {
        peer: [u8; 6],
    },
    #[cfg(feature = "hil-rx-ampdu")]
    ExpireRxAmpduGap {
        generation: usize,
    },
    #[cfg(feature = "hil-rx-ampdu")]
    RemoveRxAmpduPeer {
        peer: [u8; 6],
    },
}

pub struct Wpa2IoFailure<E, const N: usize = WPA2_TX_ETHERNET_CAPACITY> {
    pub error: E,
    pub command: Wpa2IoCommand<N>,
}

/// Immediate backend contract. Implementations must not sleep, spin, poll a
/// status register, acquire a contended lock, allocate, or retain a borrowed
/// pointer. On backpressure they return ownership in `Wpa2IoFailure`.
pub trait TryWpa2Io<const N: usize = WPA2_TX_ETHERNET_CAPACITY> {
    type Error;

    fn try_execute(
        &mut self,
        command: Wpa2IoCommand<N>,
    ) -> Result<(), Wpa2IoFailure<Self::Error, N>>;

    fn poll_internal(&mut self, _cx: &mut Context<'_>) -> bool {
        false
    }

    fn prepare_retry(&mut self, _error: &Self::Error) -> bool {
        false
    }

    fn poll_retry_ready(&mut self, _cx: &mut Context<'_>) -> Poll<PendingCommandAction> {
        Poll::Ready(PendingCommandAction::Retry)
    }

    fn cancel_retry(&mut self, _command: &Wpa2IoCommand<N>) {}
}

pub struct Wpa2IoHandler<B> {
    backend: B,
}

impl<B> Wpa2IoHandler<B> {
    pub const fn new(backend: B) -> Self {
        Self { backend }
    }

    pub const fn backend(&self) -> &B {
        &self.backend
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }
}

impl<B, const N: usize> RadioCommandHandler<Wpa2IoCommand<N>> for Wpa2IoHandler<B>
where
    B: TryWpa2Io<N>,
{
    type Error = Wpa2IoFailure<B::Error, N>;

    fn handle(&mut self, command: Wpa2IoCommand<N>) -> Result<(), Self::Error> {
        self.backend.try_execute(command)
    }

    fn poll_internal(&mut self, cx: &mut Context<'_>) -> bool {
        self.backend.poll_internal(cx)
    }

    fn recover_retry(&mut self, failure: Self::Error) -> Result<Wpa2IoCommand<N>, Self::Error> {
        if self.backend.prepare_retry(&failure.error) {
            Ok(failure.command)
        } else {
            Err(failure)
        }
    }

    fn poll_retry_ready(&mut self, cx: &mut Context<'_>) -> Poll<PendingCommandAction> {
        self.backend.poll_retry_ready(cx)
    }

    fn cancel_retry(&mut self, command: Wpa2IoCommand<N>) {
        self.backend.cancel_retry(&command);
    }
}

pub type Wpa2IoQueue<const DEPTH: usize, const FRAME_CAPACITY: usize = WPA2_TX_ETHERNET_CAPACITY> =
    RadioCommandQueue<Wpa2IoCommand<FRAME_CAPACITY>, DEPTH>;

#[cfg(test)]
mod tests {
    use super::*;

    fn key(peer: [u8; 6], value: u8) -> Wpa2KeyInstall {
        Wpa2KeyInstall {
            interface: Wpa2Interface::Station,
            peer,
            kind: Wpa2KeyKind::Pairwise,
            receive_sequence: [0; 8],
            key: AlignedCcmpKey::new([value; WPA2_TK_LEN]),
        }
    }

    #[test]
    fn ccmp_key_is_word_aligned() {
        let key = AlignedCcmpKey::new([7; WPA2_TK_LEN]);
        assert!(key.is_word_aligned());
        assert_eq!(core::mem::align_of::<AlignedCcmpKey>(), 4);
    }

    #[test]
    fn static_key_table_replaces_matching_slot_and_fails_when_full() {
        let mut keys = StaticWpa2Keys::<1>::new();
        assert_eq!(keys.insert(key([1; 6], 2)), Ok(0));
        assert_eq!(keys.insert(key([1; 6], 3)), Ok(0));
        assert_eq!(keys.get(0).unwrap().key().as_bytes(), &[3; WPA2_TK_LEN]);
        assert_eq!(keys.insert(key([2; 6], 4)), Err(StaticKeyTableError::Full));
    }

    #[test]
    fn group_transmit_flag_change_reuses_key_id_slot() {
        let mut keys = StaticWpa2Keys::<1>::new();
        let first = Wpa2Gtk::new(2, false, [6; WPA2_TK_LEN]).unwrap();
        let second = Wpa2Gtk::new(2, true, [7; WPA2_TK_LEN]).unwrap();
        assert_eq!(
            keys.insert(Wpa2KeyInstall::group(
                Wpa2Interface::AccessPoint,
                &first,
                [0; 8]
            )),
            Ok(0)
        );
        assert_eq!(
            keys.insert(Wpa2KeyInstall::group(
                Wpa2Interface::AccessPoint,
                &second,
                [1; 8]
            )),
            Ok(0)
        );
        assert_eq!(
            keys.get(0).unwrap().kind(),
            Wpa2KeyKind::Group {
                key_id: 2,
                transmit: true
            }
        );
    }

    struct RejectingBackend;

    impl TryWpa2Io<32> for RejectingBackend {
        type Error = u8;

        fn try_execute(
            &mut self,
            command: Wpa2IoCommand<32>,
        ) -> Result<(), Wpa2IoFailure<Self::Error, 32>> {
            Err(Wpa2IoFailure { error: 9, command })
        }
    }

    #[test]
    fn fail_fast_backend_returns_command_ownership() {
        let mut handler = Wpa2IoHandler::new(RejectingBackend);
        let command = Wpa2IoCommand::InstallKey(key([1; 6], 5));
        let failure = handler.handle(command).unwrap_err();
        assert_eq!(failure.error, 9);
        let Wpa2IoCommand::InstallKey(key) = failure.command else {
            panic!("backend changed command kind")
        };
        assert_eq!(key.key().as_bytes(), &[5; WPA2_TK_LEN]);
    }
}
