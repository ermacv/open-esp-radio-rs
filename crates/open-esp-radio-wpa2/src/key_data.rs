//! Fixed WPA2-CCMP GTK key-data parsing.
//!
//! This is the hardware-independent parser moved from
//! `migration/esp32s31-hybrid-runtime/src/wpa2_frames.rs`.

use zeroize::Zeroize;

pub const WPA2_GTK_LEN: usize = 16;

const RSN_ELEMENT_ID: u8 = 0x30;
const RSNXE_ELEMENT_ID: u8 = 0xf4;
const VENDOR_ELEMENT_ID: u8 = 0xdd;
const RSN_OUI: [u8; 3] = [0x00, 0x0f, 0xac];
const GTK_KDE_TYPE: u8 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Wpa2KeyDataError {
    InvalidKeyId,
    MalformedKeyData,
    UnsupportedKeyData,
    DuplicateRsnIe,
    DuplicateRsnxe,
    DuplicateGtk,
    MissingRsnIe,
    MissingRsnxe,
    UnexpectedRsnxe,
    MissingGtk,
    RsnIeMismatch,
    RsnxeMismatch,
}

pub struct Wpa2Gtk {
    key_id: u8,
    transmit: bool,
    key: [u8; WPA2_GTK_LEN],
}

impl Wpa2Gtk {
    pub fn new(
        key_id: u8,
        transmit: bool,
        key: [u8; WPA2_GTK_LEN],
    ) -> Result<Self, Wpa2KeyDataError> {
        if key_id > 3 {
            return Err(Wpa2KeyDataError::InvalidKeyId);
        }
        Ok(Self {
            key_id,
            transmit,
            key,
        })
    }

    pub const fn key_id(&self) -> u8 {
        self.key_id
    }

    pub const fn transmit(&self) -> bool {
        self.transmit
    }

    pub const fn key(&self) -> &[u8; WPA2_GTK_LEN] {
        &self.key
    }
}

impl Drop for Wpa2Gtk {
    fn drop(&mut self) {
        self.key.zeroize();
    }
}

pub fn parse_gtk_key_data(
    bytes: &[u8],
    expected_rsn_ie: &[u8],
    expected_rsnxe: &[u8],
) -> Result<Wpa2Gtk, Wpa2KeyDataError> {
    let mut offset = 0;
    let mut saw_rsn = false;
    let mut saw_rsnxe = false;
    let mut gtk = None;

    while offset < bytes.len() {
        let remaining = &bytes[offset..];
        if remaining.iter().all(|byte| *byte == 0)
            || (remaining[0] == VENDOR_ELEMENT_ID && remaining[1..].iter().all(|byte| *byte == 0))
        {
            break;
        }
        if remaining.len() < 2 {
            return Err(Wpa2KeyDataError::MalformedKeyData);
        }
        let element_len = remaining[1] as usize + 2;
        if element_len > remaining.len() {
            return Err(Wpa2KeyDataError::MalformedKeyData);
        }
        let element = &remaining[..element_len];
        match element[0] {
            RSN_ELEMENT_ID => {
                if saw_rsn {
                    return Err(Wpa2KeyDataError::DuplicateRsnIe);
                }
                if element != expected_rsn_ie {
                    return Err(Wpa2KeyDataError::RsnIeMismatch);
                }
                saw_rsn = true;
            }
            RSNXE_ELEMENT_ID => {
                if saw_rsnxe {
                    return Err(Wpa2KeyDataError::DuplicateRsnxe);
                }
                if expected_rsnxe.is_empty() {
                    return Err(Wpa2KeyDataError::UnexpectedRsnxe);
                }
                if element != expected_rsnxe {
                    return Err(Wpa2KeyDataError::RsnxeMismatch);
                }
                saw_rsnxe = true;
            }
            VENDOR_ELEMENT_ID => {
                if element.len() != 24
                    || element[2..5] != RSN_OUI
                    || element[5] != GTK_KDE_TYPE
                    || element[7] != 0
                    || element[6] & !0x07 != 0
                {
                    return Err(Wpa2KeyDataError::UnsupportedKeyData);
                }
                if gtk.is_some() {
                    return Err(Wpa2KeyDataError::DuplicateGtk);
                }
                let mut key = [0; WPA2_GTK_LEN];
                key.copy_from_slice(&element[8..24]);
                gtk = Some(Wpa2Gtk::new(
                    element[6] & 0x03,
                    element[6] & 0x04 != 0,
                    key,
                )?);
            }
            _ => return Err(Wpa2KeyDataError::UnsupportedKeyData),
        }
        offset += element_len;
    }

    if !saw_rsn {
        return Err(Wpa2KeyDataError::MissingRsnIe);
    }
    if !expected_rsnxe.is_empty() && !saw_rsnxe {
        return Err(Wpa2KeyDataError::MissingRsnxe);
    }
    gtk.ok_or(Wpa2KeyDataError::MissingGtk)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RSN: [u8; 22] = [
        0x30, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0, 0,
    ];

    fn key_data() -> [u8; 48] {
        let mut data = [0; 48];
        data[..RSN.len()].copy_from_slice(&RSN);
        data[22..46].copy_from_slice(&[
            0xdd, 22, 0, 0x0f, 0xac, 1, 1, 0, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
        ]);
        data[46] = VENDOR_ELEMENT_ID;
        data
    }

    #[test]
    fn parser_validates_rsn_and_owns_the_gtk() {
        let parsed = parse_gtk_key_data(&key_data(), &RSN, &[]).unwrap();
        assert_eq!(parsed.key_id(), 1);
        assert!(!parsed.transmit());
        assert_eq!(parsed.key(), &[7; WPA2_GTK_LEN]);

        let mut changed = RSN;
        changed[5] ^= 1;
        assert_eq!(
            parse_gtk_key_data(&key_data(), &changed, &[]).err(),
            Some(Wpa2KeyDataError::RsnIeMismatch)
        );
    }

    #[test]
    fn parser_rejects_duplicate_gtk() {
        let source = key_data();
        let mut duplicate = [0; 70];
        duplicate[..46].copy_from_slice(&source[..46]);
        duplicate[46..70].copy_from_slice(&source[22..46]);
        assert_eq!(
            parse_gtk_key_data(&duplicate, &RSN, &[]).err(),
            Some(Wpa2KeyDataError::DuplicateGtk)
        );
    }
}
