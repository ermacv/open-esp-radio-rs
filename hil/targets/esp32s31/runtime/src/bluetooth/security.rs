//! Fixed-key diagnostic Host policy. RF encryption stays in the production Controller.
#[cfg(target_arch = "riscv32")]
use super::{Controller, Duration, Host, PERIPHERAL_HOST_EVENTS, SyncCmd, with_timeout};
#[cfg(target_arch = "riscv32")]
use bt_hci::cmd::le::{LeLongTermKeyRequestNegativeReply, LeLongTermKeyRequestReply};
use bt_hci::{
    ControllerToHostPacket,
    event::{Event as HciEvent, le::LeEvent},
    param::{ConnHandle, Status},
};
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use open_esp_radio_hil_protocol::{
    BLUETOOTH_REFRESH_EDIV, BLUETOOTH_REFRESH_LTK, BLUETOOTH_REFRESH_RAND, BLUETOOTH_TEST_EDIV,
    BLUETOOTH_TEST_LTK, BLUETOOTH_TEST_RAND, BluetoothEncryptionEvidence, BluetoothSecurityFailure,
};

static ENABLED: AtomicBool = AtomicBool::new(false);
static ENCRYPTED: AtomicBool = AtomicBool::new(false);
static REQUESTS: AtomicU32 = AtomicU32::new(0);
static REPLIES: AtomicU32 = AtomicU32::new(0);
static NEGATIVE_REPLIES: AtomicU32 = AtomicU32::new(0);
static WRONG_REPLIES: AtomicU32 = AtomicU32::new(0);
static INJECTION: AtomicU8 = AtomicU8::new(0);
static CHANGES: AtomicU32 = AtomicU32::new(0);
static REFRESHES: AtomicU32 = AtomicU32::new(0);
// Written only by the diagnostic Host event pump; snapshots read counters separately.
const IDLE: u8 = 0;
const START_PENDING: u8 = 1;
const STARTED: u8 = 2;
const REFRESH_PENDING: u8 = 3;
const REFRESHED: u8 = 4;
static PHASE: AtomicU8 = AtomicU8::new(IDLE);
static FAULTS: AtomicU32 = AtomicU32::new(0);

fn increment(counter: &AtomicU32) {
    if counter
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
        .is_err()
    {
        FAULTS.store(u32::MAX, Ordering::Relaxed);
    }
}

pub(super) fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}
pub(super) fn configure(enabled: bool, failure: Option<BluetoothSecurityFailure>) {
    #[cfg(target_arch = "riscv32")]
    oer_esp32s31_bluetooth::le::peripheral::rx_fault::configure(
        enabled && failure == Some(BluetoothSecurityFailure::ActiveDataMic),
    );
    ENABLED.store(enabled, Ordering::Relaxed);
    INJECTION.store(
        match failure {
            None => 0,
            Some(BluetoothSecurityFailure::MissingKey) => 1,
            Some(BluetoothSecurityFailure::WrongKey) => 2,
            Some(BluetoothSecurityFailure::MissingRefreshKey) => 3,
            Some(BluetoothSecurityFailure::ActiveDataMic) => 0,
        },
        Ordering::Relaxed,
    );
}
pub(super) fn snapshot() -> BluetoothEncryptionEvidence {
    #[cfg(target_arch = "riscv32")]
    let injection = oer_esp32s31_bluetooth::le::peripheral::rx_fault::snapshot();
    BluetoothEncryptionEvidence {
        #[cfg(target_arch = "riscv32")]
        mic_injections: injection.injections,
        #[cfg(target_arch = "riscv32")]
        mic_injection_armed: injection.armed,
        #[cfg(not(target_arch = "riscv32"))]
        mic_injections: 0,
        #[cfg(not(target_arch = "riscv32"))]
        mic_injection_armed: false,
        enabled: enabled(),
        encrypted: ENCRYPTED.load(Ordering::Relaxed),
        key_requests: REQUESTS.load(Ordering::Relaxed),
        key_replies: REPLIES.load(Ordering::Relaxed),
        negative_replies: NEGATIVE_REPLIES.load(Ordering::Relaxed),
        wrong_key_replies: WRONG_REPLIES.load(Ordering::Relaxed),
        encryption_changes: CHANGES.load(Ordering::Relaxed),
        key_refreshes: REFRESHES.load(Ordering::Relaxed),
        faults: FAULTS.load(Ordering::Relaxed),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct KeyReply {
    key: Option<[u8; 16]>,
    injected: Option<BluetoothSecurityFailure>,
}

/// Returns the requested handle and its phase-specific public fixture key, or a negative reply.
pub(super) fn observe(packet: &ControllerToHostPacket<'_>) -> Option<(ConnHandle, KeyReply)> {
    if !enabled() {
        return None;
    }
    let ControllerToHostPacket::Event(event) = packet else {
        if !ENCRYPTED.load(Ordering::Relaxed) {
            increment(&FAULTS);
        }
        return None;
    };
    let Ok(event) = HciEvent::try_from(event.clone()) else {
        increment(&FAULTS);
        return None;
    };
    match event {
        HciEvent::Le(LeEvent::LeLongTermKeyRequest(request)) => {
            increment(&REQUESTS);
            let identity = (request.random_number, request.encrypted_diversifier);
            let key = if request.handle.raw() != 1 {
                None
            } else if PHASE.load(Ordering::Relaxed) == IDLE
                && identity == (BLUETOOTH_TEST_RAND, BLUETOOTH_TEST_EDIV)
            {
                PHASE.store(START_PENDING, Ordering::Relaxed);
                Some(BLUETOOTH_TEST_LTK)
            } else if PHASE.load(Ordering::Relaxed) == STARTED
                && identity == (BLUETOOTH_REFRESH_RAND, BLUETOOTH_REFRESH_EDIV)
            {
                PHASE.store(REFRESH_PENDING, Ordering::Relaxed);
                ENCRYPTED.store(false, Ordering::Relaxed);
                Some(BLUETOOTH_REFRESH_LTK)
            } else {
                None
            };
            if key.is_none() {
                increment(&FAULTS);
            }
            let injected = match (
                key.is_some(),
                PHASE.load(Ordering::Relaxed),
                INJECTION.load(Ordering::Relaxed),
            ) {
                (true, START_PENDING, 1) => Some(BluetoothSecurityFailure::MissingKey),
                (true, START_PENDING, 2) => Some(BluetoothSecurityFailure::WrongKey),
                (true, REFRESH_PENDING, 3) => Some(BluetoothSecurityFailure::MissingRefreshKey),
                _ => None,
            };
            if injected.is_some() {
                INJECTION.store(0, Ordering::Relaxed);
            }
            let key = match injected {
                Some(
                    BluetoothSecurityFailure::MissingKey
                    | BluetoothSecurityFailure::MissingRefreshKey,
                ) => None,
                Some(BluetoothSecurityFailure::WrongKey) => {
                    let mut wrong = BLUETOOTH_TEST_LTK;
                    wrong[0] ^= 1;
                    Some(wrong)
                }
                None | Some(BluetoothSecurityFailure::ActiveDataMic) => key,
            };
            Some((request.handle, KeyReply { key, injected }))
        }
        HciEvent::EncryptionChangeV1(change) => {
            if change.status == Status::SUCCESS
                && change.handle.raw() == 1
                && change.enabled == bt_hci::param::EncryptionEnabledLevel::OnE0OrAesCcm
                && PHASE.load(Ordering::Relaxed) == START_PENDING
            {
                PHASE.store(STARTED, Ordering::Relaxed);
                ENCRYPTED.store(true, Ordering::Relaxed);
                increment(&CHANGES);
            } else {
                increment(&FAULTS);
            }
            None
        }
        HciEvent::EncryptionKeyRefreshComplete(refresh) => {
            if refresh.status == Status::SUCCESS
                && refresh.handle.raw() == 1
                && PHASE.load(Ordering::Relaxed) == REFRESH_PENDING
            {
                PHASE.store(REFRESHED, Ordering::Relaxed);
                ENCRYPTED.store(true, Ordering::Relaxed);
                increment(&REFRESHES);
            } else {
                increment(&FAULTS);
            }
            None
        }
        HciEvent::Le(LeEvent::LeConnectionComplete(_)) | HciEvent::DisconnectionComplete(_) => {
            #[cfg(target_arch = "riscv32")]
            if matches!(event, HciEvent::DisconnectionComplete(_)) {
                oer_esp32s31_bluetooth::le::peripheral::rx_fault::configure(false);
            }
            ENCRYPTED.store(false, Ordering::Relaxed);
            PHASE.store(IDLE, Ordering::Relaxed);
            None
        }
        _ => None,
    }
}

#[cfg(target_arch = "riscv32")]
pub(super) async fn reply(
    hci: &Host,
    buffer: &mut <Host as Controller>::Buffer<'_>,
    handle: ConnHandle,
    reply: KeyReply,
) {
    let key = reply.key;
    let result = with_timeout(
        Duration::from_secs(2),
        super::command_pump::with_event_pump(
            hci,
            buffer,
            async {
                if let Some(key) = key {
                    LeLongTermKeyRequestReply::new(handle, key).exec(hci).await
                } else {
                    LeLongTermKeyRequestNegativeReply::new(handle)
                        .exec(hci)
                        .await
                }
            },
            |packet| {
                if observe(&packet).is_some() || matches!(packet, ControllerToHostPacket::Acl(_)) {
                    increment(&FAULTS);
                }
                PERIPHERAL_HOST_EVENTS.observe(packet);
            },
        ),
    )
    .await;
    if matches!(result, Ok(Ok(Ok(returned))) if returned == handle) {
        record_reply(reply);
    } else {
        increment(&FAULTS);
    }
}

#[cfg(any(target_arch = "riscv32", test))]
fn record_reply(reply: KeyReply) {
    match (reply.key, reply.injected) {
        (
            None,
            Some(
                BluetoothSecurityFailure::MissingKey | BluetoothSecurityFailure::MissingRefreshKey,
            ),
        ) => increment(&NEGATIVE_REPLIES),
        (Some(_), injected) => {
            increment(&REPLIES);
            if injected == Some(BluetoothSecurityFailure::WrongKey) {
                increment(&WRONG_REPLIES);
            }
        }
        _ => increment(&FAULTS),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bt_hci::{FromHciBytes, event::EventPacket};
    fn event(bytes: &[u8]) -> Option<(ConnHandle, KeyReply)> {
        let (event, rest) = EventPacket::from_hci_bytes(bytes).unwrap();
        assert!(rest.is_empty());
        observe(&ControllerToHostPacket::Event(event))
    }
    #[test]
    fn fixed_key_host_checks_request_and_encryption_and_retires_on_disconnect() {
        configure(true, None);
        let mut request = [0; 15];
        request[..5].copy_from_slice(&[0x3e, 13, 5, 1, 0]);
        request[5..13].copy_from_slice(&BLUETOOTH_TEST_RAND);
        request[13..].copy_from_slice(&BLUETOOTH_TEST_EDIV.to_le_bytes());
        assert_eq!(
            event(&request),
            Some((
                ConnHandle::new(1),
                KeyReply {
                    key: Some(BLUETOOTH_TEST_LTK),
                    injected: None
                }
            ))
        );
        assert_eq!(snapshot().key_requests, 1);
        assert_eq!(snapshot().key_replies, 0);
        event(&[8, 4, 0, 1, 0, 1]);
        assert!(snapshot().encrypted);
        assert_eq!(snapshot().encryption_changes, 1);
        request[5..13].copy_from_slice(&BLUETOOTH_REFRESH_RAND);
        request[13..].copy_from_slice(&BLUETOOTH_REFRESH_EDIV.to_le_bytes());
        assert_eq!(
            event(&request),
            Some((
                ConnHandle::new(1),
                KeyReply {
                    key: Some(BLUETOOTH_REFRESH_LTK),
                    injected: None
                }
            ))
        );
        assert!(!snapshot().encrypted);
        event(&[0x30, 3, 0, 1, 0]);
        assert!(snapshot().encrypted);
        assert_eq!(snapshot().key_refreshes, 1);
        assert_eq!(snapshot().key_requests, 2);
        event(&[5, 4, 0, 1, 0, 0x13]);
        assert!(!snapshot().encrypted);
        assert_eq!(snapshot().faults, 0);
        request[5] ^= 1;
        assert_eq!(
            event(&request),
            Some((
                ConnHandle::new(1),
                KeyReply {
                    key: None,
                    injected: None
                }
            ))
        );
        assert_eq!(snapshot().faults, 1);
        event(&[8, 4, 0, 1, 0, 0]);
        assert!(!snapshot().encrypted);
        assert_eq!(snapshot().encryption_changes, 1);
        assert_eq!(snapshot().faults, 2);
        // A new connection may use the initial key again, but cannot begin with the refresh key.
        request[5..13].copy_from_slice(&BLUETOOTH_REFRESH_RAND);
        assert_eq!(
            event(&request),
            Some((
                ConnHandle::new(1),
                KeyReply {
                    key: None,
                    injected: None
                }
            ))
        );
        assert_eq!(snapshot().faults, 3);
        request[5..13].copy_from_slice(&BLUETOOTH_TEST_RAND);
        request[13..].copy_from_slice(&BLUETOOTH_TEST_EDIV.to_le_bytes());
        assert_eq!(
            event(&request),
            Some((
                ConnHandle::new(1),
                KeyReply {
                    key: Some(BLUETOOTH_TEST_LTK),
                    injected: None
                }
            ))
        );
        event(&[0x30, 3, 0, 1, 0]); // refresh cannot replace the start event
        assert!(!snapshot().encrypted);
        assert_eq!(snapshot().key_refreshes, 1);
        event(&[8, 4, 0, 1, 0, 1]);
        assert!(snapshot().encrypted);
        assert_eq!(snapshot().encryption_changes, 2);
        request[5..13].copy_from_slice(&BLUETOOTH_REFRESH_RAND);
        request[13..].copy_from_slice(&BLUETOOTH_REFRESH_EDIV.to_le_bytes());
        assert_eq!(
            event(&request),
            Some((
                ConnHandle::new(1),
                KeyReply {
                    key: Some(BLUETOOTH_REFRESH_LTK),
                    injected: None
                }
            ))
        );
        for invalid in [[0x30, 3, 0, 2, 0], [0x30, 3, 0x06, 1, 0]] {
            event(&invalid);
            assert!(!snapshot().encrypted);
            assert_eq!(snapshot().key_refreshes, 1);
        }
        event(&[0x30, 3, 0, 1, 0]);
        assert!(snapshot().encrypted);
        assert_eq!(snapshot().key_refreshes, 2);
        event(&[0x30, 3, 0, 1, 0]); // duplicate completion
        assert_eq!(snapshot().key_refreshes, 2);
        assert_eq!(snapshot().faults, 7);
        configure(false, None);
        assert_eq!(event(&request), None);
        assert_eq!(snapshot().key_requests, 6);
        for failure in [
            BluetoothSecurityFailure::MissingKey,
            BluetoothSecurityFailure::WrongKey,
        ] {
            configure(true, Some(failure));
            event(&[5, 4, 0, 1, 0, 0x13]);
            request[5..13].copy_from_slice(&BLUETOOTH_TEST_RAND);
            request[13..].copy_from_slice(&BLUETOOTH_TEST_EDIV.to_le_bytes());
            let (_, reply) = event(&request).unwrap();
            assert_eq!(reply.injected, Some(failure));
            match failure {
                BluetoothSecurityFailure::MissingKey
                | BluetoothSecurityFailure::MissingRefreshKey => assert_eq!(reply.key, None),
                BluetoothSecurityFailure::ActiveDataMic => panic!("not a key injection"),
                BluetoothSecurityFailure::WrongKey => {
                    assert_ne!(reply.key, Some(BLUETOOTH_TEST_LTK))
                }
            }
            record_reply(reply);
            event(&[5, 4, 0, 1, 0, 0x13]);
            let (_, reply) = event(&request).unwrap();
            assert_eq!(reply.key, Some(BLUETOOTH_TEST_LTK));
            assert_eq!(reply.injected, None);
        }
        assert_eq!(snapshot().negative_replies, 1);
        assert_eq!(snapshot().wrong_key_replies, 1);
        assert_eq!(snapshot().faults, 7);
        // Refresh injection survives the initial valid reply and is consumed exactly once.
        let before = snapshot();
        event(&[5, 4, 0, 1, 0, 0x13]);
        configure(true, Some(BluetoothSecurityFailure::MissingRefreshKey));
        request[5..13].copy_from_slice(&BLUETOOTH_TEST_RAND);
        request[13..].copy_from_slice(&BLUETOOTH_TEST_EDIV.to_le_bytes());
        let (_, initial) = event(&request).unwrap();
        assert_eq!(initial.key, Some(BLUETOOTH_TEST_LTK));
        assert_eq!(initial.injected, None);
        record_reply(initial);
        event(&[8, 4, 0, 1, 0, 1]);
        request[5..13].copy_from_slice(&BLUETOOTH_REFRESH_RAND);
        request[13..].copy_from_slice(&BLUETOOTH_REFRESH_EDIV.to_le_bytes());
        let (_, rejected) = event(&request).unwrap();
        assert_eq!(rejected.key, None);
        assert_eq!(
            rejected.injected,
            Some(BluetoothSecurityFailure::MissingRefreshKey)
        );
        record_reply(rejected);
        assert!(!snapshot().encrypted);
        event(&[5, 4, 0, 1, 0, 6]);
        // A fresh connection recovers with the initial LTK and no residual injection.
        request[5..13].copy_from_slice(&BLUETOOTH_TEST_RAND);
        request[13..].copy_from_slice(&BLUETOOTH_TEST_EDIV.to_le_bytes());
        let (_, recovered) = event(&request).unwrap();
        assert_eq!(recovered.key, Some(BLUETOOTH_TEST_LTK));
        assert_eq!(recovered.injected, None);
        record_reply(recovered);
        event(&[8, 4, 0, 1, 0, 1]);
        assert!(snapshot().encrypted);
        assert_eq!(snapshot().negative_replies, before.negative_replies + 1);
        assert_eq!(snapshot().key_replies, before.key_replies + 2);
        assert_eq!(snapshot().encryption_changes, before.encryption_changes + 2);
        assert_eq!(snapshot().key_refreshes, before.key_refreshes);
        assert_eq!(snapshot().faults, before.faults);
        configure(false, None);
    }
}
