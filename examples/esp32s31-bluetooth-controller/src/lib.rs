#![no_std]
#![forbid(unsafe_code)]

//! Application configuration for the bounded Controller smoke sequences.

use bt_hci::{
    cmd::le::LeSetAdvParams,
    param::{AddrKind, AdvChannelMap, AdvFilterPolicy, AdvKind, BdAddr, Duration},
};

/// Local name published by the Trouble GATT application profile.
pub const TROUBLE_GATT_DEVICE_NAME: &[u8] = b"open-radio-gatt";
/// Vendor-specific primary service used by the writable smoke value.
pub const TROUBLE_GATT_SERVICE_UUID: u16 = 0xfff0;
/// One-byte read/write value exposed by the Trouble GATT service.
pub const TROUBLE_GATT_VALUE_UUID: u16 = 0xfff1;

/// Encode the complete legacy advertising and scan-response payloads used by
/// the Trouble GATT application.
///
/// Both destination buffers have the exact HCI legacy capacity, so the fixed
/// profile cannot be truncated or silently omit its local name.
pub fn encode_trouble_gatt_advertising(
    advertising: &mut [u8; 31],
    scan_response: &mut [u8; 31],
) -> (usize, usize) {
    advertising[..3].copy_from_slice(&[2, 0x01, 0x06]);
    let name_field_len = TROUBLE_GATT_DEVICE_NAME.len() + 1;
    scan_response[0] = name_field_len as u8;
    scan_response[1] = 0x09;
    scan_response[2..2 + TROUBLE_GATT_DEVICE_NAME.len()].copy_from_slice(TROUBLE_GATT_DEVICE_NAME);
    (3, 2 + TROUBLE_GATT_DEVICE_NAME.len())
}

/// Legacy advertising cases supported by the current S31 event graphs.
#[derive(Clone, Copy)]
pub enum AdvertisingSmokeCase {
    Nonconnectable,
    Connectable,
}

impl AdvertisingSmokeCase {
    pub const ALL: [Self; 2] = [Self::Nonconnectable, Self::Connectable];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Nonconnectable => "nonconnectable",
            Self::Connectable => "connectable",
        }
    }

    /// Build the typed command used for initial enable and reconfiguration.
    pub fn parameters(self) -> LeSetAdvParams {
        let (kind, channels) = match self {
            Self::Nonconnectable => (AdvKind::AdvNonconnInd, AdvChannelMap::ALL),
            // The response-capable S31 graph owns exactly one primary channel.
            Self::Connectable => (AdvKind::AdvInd, AdvChannelMap::CHANNEL_37),
        };
        LeSetAdvParams::new(
            Duration::from_millis(100),
            Duration::from_millis(100),
            kind,
            AddrKind::RANDOM,
            AddrKind::PUBLIC,
            BdAddr::default(),
            channels,
            AdvFilterPolicy::Unfiltered,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bt_hci::cmd::Cmd;

    #[test]
    fn smoke_commands_fit_their_radio_role_channel_capacity() {
        for case in AdvertisingSmokeCase::ALL {
            let command = case.parameters();
            let params = command.params();
            let channels = params.adv_channel_map;
            let selected = [
                channels.is_channel_37_enabled(),
                channels.is_channel_38_enabled(),
                channels.is_channel_39_enabled(),
            ]
            .into_iter()
            .filter(|enabled| *enabled)
            .count();
            match params.adv_kind {
                AdvKind::AdvInd => assert_eq!(selected, 1, "response graph has one channel"),
                AdvKind::AdvNonconnInd => assert_eq!(selected, 3, "exercise the full TX chain"),
                _ => panic!("the smoke selected an unsupported advertising role"),
            }
        }
    }

    #[test]
    fn trouble_gatt_profile_has_complete_bounded_legacy_payloads() {
        let mut advertising = [0_u8; 31];
        let mut scan_response = [0_u8; 31];
        let (advertising_len, scan_response_len) =
            encode_trouble_gatt_advertising(&mut advertising, &mut scan_response);

        assert_eq!(&advertising[..advertising_len], &[2, 0x01, 0x06]);
        assert_eq!(
            scan_response[0] as usize,
            TROUBLE_GATT_DEVICE_NAME.len() + 1
        );
        assert_eq!(scan_response[1], 0x09);
        assert_eq!(
            &scan_response[2..scan_response_len],
            TROUBLE_GATT_DEVICE_NAME
        );
        assert_eq!(TROUBLE_GATT_SERVICE_UUID, 0xfff0);
        assert_eq!(TROUBLE_GATT_VALUE_UUID, 0xfff1);
    }
}
