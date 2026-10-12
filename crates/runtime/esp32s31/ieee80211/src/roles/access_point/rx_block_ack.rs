//! The software owner of the ordinary RX Block Ack banks the station and the
//! access point share.

use core::cell::RefCell;

use embassy_sync::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex};

use oer_esp32s31_ieee80211_mac::{
    MacInterface,
    rx::ampdu::{
        RX_BLOCK_ACK_BANK_COUNT, RxBlockAckActivation, RxBlockAckRequest, RxBlockAckSessions,
        RxBlockAckSessionsError, RxBlockAckSnapshot,
    },
};


/// One upstream station peer plus every peer admitted by the SoftAP policy.
pub const STA_AP_RX_BLOCK_ACK_PEER_CAPACITY: usize =
    oer_esp32s31_ieee80211_ap::protocol::AP_MAX_CLIENTS + 1;

/// Serialized software owner of the eight physical ordinary RX BlockAck banks.
///
/// The ordinary station and primary SoftAP hardware paths share the same
/// direct banks. Both roles may therefore retain this shared reference, but
/// every mutation is serialized here and the session allocator exists only
/// once. The larger peer table tracks logical identities; it does not
/// manufacture more hardware banks.
pub struct StaApRxBlockAck {
    sessions: Mutex<
        CriticalSectionRawMutex,
        RefCell<RxBlockAckSessions<STA_AP_RX_BLOCK_ACK_PEER_CAPACITY>>,
    >,
}

impl StaApRxBlockAck {
    pub const fn new() -> Self {
        Self {
            sessions: Mutex::new(RefCell::new(RxBlockAckSessions::new())),
        }
    }

    pub const fn with_maximum_window(maximum_window: u16) -> Result<Self, RxBlockAckSessionsError> {
        match RxBlockAckSessions::with_maximum_window(maximum_window) {
            Ok(sessions) => Ok(Self {
                sessions: Mutex::new(RefCell::new(sessions)),
            }),
            Err(error) => Err(error),
        }
    }

    pub(crate) fn with_sessions<R>(
        &self,
        operation: impl FnOnce(&mut RxBlockAckSessions<STA_AP_RX_BLOCK_ACK_PEER_CAPACITY>) -> R,
    ) -> R {
        self.sessions
            .lock(|sessions| operation(&mut sessions.borrow_mut()))
    }

    pub fn maximum_window(&self) -> u16 {
        self.sessions
            .lock(|sessions| sessions.borrow().maximum_window())
    }

    pub fn reset_after_hardware_reset(&self) {
        self.with_sessions(RxBlockAckSessions::reset_after_hardware_reset);
    }

    pub fn prepare_interface(
        &self,
        interface: MacInterface,
    ) -> Result<(), RxBlockAckSessionsError> {
        self.with_sessions(|sessions| sessions.prepare_interface(interface))
    }

    pub fn offer(&self, request: RxBlockAckRequest) -> Result<(), RxBlockAckSessionsError> {
        self.with_sessions(|sessions| sessions.offer(request))
    }

    pub fn begin_pending(&self) -> Result<Option<RxBlockAckActivation>, RxBlockAckSessionsError> {
        self.with_sessions(RxBlockAckSessions::begin_pending)
    }

    pub fn commit(
        &self,
        activation: RxBlockAckActivation,
    ) -> Result<RxBlockAckSnapshot, RxBlockAckSessionsError> {
        self.with_sessions(|sessions| sessions.commit(activation))
    }

    pub fn cancel(&self, activation: RxBlockAckActivation) -> Result<(), RxBlockAckSessionsError> {
        self.with_sessions(|sessions| sessions.cancel(activation))
    }

    pub fn stop(
        &self,
        interface: MacInterface,
        peer: [u8; 6],
        tid: u8,
    ) -> Option<RxBlockAckSnapshot> {
        self.with_sessions(|sessions| sessions.stop(interface, peer, tid))
    }

    pub fn discard_pending(&self, interface: MacInterface, peer: [u8; 6], tid: u8) -> bool {
        self.with_sessions(|sessions| sessions.discard_pending(interface, peer, tid))
    }

    pub fn stop_peer(
        &self,
        interface: MacInterface,
        peer: [u8; 6],
    ) -> [Option<RxBlockAckSnapshot>; RX_BLOCK_ACK_BANK_COUNT] {
        self.with_sessions(|sessions| sessions.stop_peer(interface, peer))
    }

    pub fn snapshots(&self) -> [Option<RxBlockAckSnapshot>; RX_BLOCK_ACK_BANK_COUNT] {
        self.sessions.lock(|sessions| sessions.borrow().snapshots())
    }

    pub fn snapshots_for(
        &self,
        interface: MacInterface,
    ) -> [Option<RxBlockAckSnapshot>; RX_BLOCK_ACK_BANK_COUNT] {
        self.sessions
            .lock(|sessions| sessions.borrow().snapshots_for(interface))
    }
}

impl Default for StaApRxBlockAck {
    fn default() -> Self {
        Self::new()
    }
}
