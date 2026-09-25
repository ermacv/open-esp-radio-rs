//! Chip hooks and the HCI reply exchange around the portable fixed-key Host
//! policy. RF encryption stays in the production Controller.
use super::{Controller, Duration, Host, PERIPHERAL_HOST_EVENTS, SyncCmd, with_timeout};
use bt_hci::{
    ControllerToHostPacket,
    cmd::le::{LeLongTermKeyRequestNegativeReply, LeLongTermKeyRequestReply},
    param::ConnHandle,
};
use oer_esp32s31_bluetooth::le::peripheral::rx_fault;
use open_esp_radio_hil_protocol::{BluetoothEncryptionEvidence, BluetoothSecurityFailure};
use open_esp_radio_hil_target_core::bluetooth::security::{self as policy, KeyReply};

pub(super) use policy::enabled;

pub(super) fn configure(enabled: bool, failure: Option<BluetoothSecurityFailure>) {
    rx_fault::configure(enabled && failure == Some(BluetoothSecurityFailure::ActiveDataMic));
    policy::configure(enabled, failure);
}

pub(super) fn snapshot() -> BluetoothEncryptionEvidence {
    let injection = rx_fault::snapshot();
    BluetoothEncryptionEvidence {
        mic_injections: injection.injections,
        mic_injection_armed: injection.armed,
        ..policy::snapshot()
    }
}

/// Returns the requested handle and its phase-specific public fixture key, or a negative reply.
pub(super) fn observe(packet: &ControllerToHostPacket<'_>) -> Option<(ConnHandle, KeyReply)> {
    policy::observe_with(packet, || rx_fault::configure(false))
}

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
                    policy::record_fault();
                }
                PERIPHERAL_HOST_EVENTS.observe(packet);
            },
        ),
    )
    .await;
    if matches!(result, Ok(Ok(Ok(returned))) if returned == handle) {
        policy::record_reply(reply);
    } else {
        policy::record_fault();
    }
}
