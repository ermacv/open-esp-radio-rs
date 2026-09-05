//! Zeroizing group-key owners, key-data construction and parsing.

use super::*;

#[derive(Zeroize, ZeroizeOnDrop)]
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
    ) -> Result<Self, Wpa2FrameError> {
        if key_id > 3 {
            return Err(Wpa2FrameError::InvalidKeyId);
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

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct Wpa2PlainKeyData<const N: usize = WPA2_PLAIN_KEY_DATA_CAPACITY> {
    len: usize,
    bytes: [u8; N],
}

impl<const N: usize> Wpa2PlainKeyData<N> {
    pub fn build<const R: usize>(
        rsn_ie: &OwnedRsnIe<R>,
        gtk: &Wpa2Gtk,
    ) -> Result<Self, Wpa2FrameError> {
        let rsn = rsn_ie.as_bytes();
        let unpadded_len = rsn
            .len()
            .checked_add(24)
            .ok_or(Wpa2FrameError::CapacityExceeded)?;
        let padding = if unpadded_len < 16 {
            16 - unpadded_len
        } else {
            (8 - unpadded_len % 8) % 8
        };
        let len = unpadded_len
            .checked_add(padding)
            .ok_or(Wpa2FrameError::CapacityExceeded)?;
        if len > N {
            return Err(Wpa2FrameError::CapacityExceeded);
        }

        let mut bytes = [0; N];
        bytes[..rsn.len()].copy_from_slice(rsn);
        let kde = &mut bytes[rsn.len()..unpadded_len];
        kde[0] = VENDOR_ELEMENT_ID;
        kde[1] = 22;
        kde[2..5].copy_from_slice(&RSN_OUI);
        kde[5] = GTK_KDE_TYPE;
        kde[6] = gtk.key_id | u8::from(gtk.transmit) << 2;
        kde[7] = 0;
        kde[8..24].copy_from_slice(gtk.key());
        if padding != 0 {
            bytes[unpadded_len] = VENDOR_ELEMENT_ID;
        }
        Ok(Self { len, bytes })
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

pub fn parse_gtk_key_data(
    bytes: &[u8],
    expected_rsn_ie: &[u8],
    expected_rsnxe: &[u8],
) -> Result<Wpa2Gtk, Wpa2FrameError> {
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
            return Err(Wpa2FrameError::MalformedKeyData);
        }
        let element_len = remaining[1] as usize + 2;
        if element_len > remaining.len() {
            return Err(Wpa2FrameError::MalformedKeyData);
        }
        let element = &remaining[..element_len];
        match element[0] {
            RSN_ELEMENT_ID => {
                if saw_rsn {
                    return Err(Wpa2FrameError::DuplicateRsnIe);
                }
                if element != expected_rsn_ie {
                    return Err(Wpa2FrameError::RsnIeMismatch);
                }
                saw_rsn = true;
            }
            RSNXE_ELEMENT_ID => {
                if saw_rsnxe {
                    return Err(Wpa2FrameError::DuplicateRsnxe);
                }
                if expected_rsnxe.is_empty() {
                    return Err(Wpa2FrameError::UnexpectedRsnxe);
                }
                if element != expected_rsnxe {
                    return Err(Wpa2FrameError::RsnxeMismatch);
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
                    return Err(Wpa2FrameError::UnsupportedKeyData);
                }
                if gtk.is_some() {
                    return Err(Wpa2FrameError::DuplicateGtk);
                }
                let mut key = [0; WPA2_GTK_LEN];
                key.copy_from_slice(&element[8..24]);
                gtk = Some(Wpa2Gtk::new(
                    element[6] & 0x03,
                    element[6] & 0x04 != 0,
                    key,
                )?);
            }
            _ => return Err(Wpa2FrameError::UnsupportedKeyData),
        }
        offset += element_len;
    }

    if !saw_rsn {
        return Err(Wpa2FrameError::MissingRsnIe);
    }
    if !expected_rsnxe.is_empty() && !saw_rsnxe {
        return Err(Wpa2FrameError::MissingRsnxe);
    }
    gtk.ok_or(Wpa2FrameError::MissingGtk)
}

/// Parse the encrypted key-data body of a connected-state Group Message 1.
///
/// Unlike pairwise Message 3, a group rekey carries a GTK KDE without a copy
/// of the association RSN IEs. Only the recovered RSN GTK KDE and key-wrap
/// padding are accepted; unrelated elements fail closed.
pub fn parse_group_gtk_key_data(bytes: &[u8]) -> Result<Wpa2Gtk, Wpa2FrameError> {
    let mut offset = 0;
    let mut gtk = None;
    while offset < bytes.len() {
        let remaining = &bytes[offset..];
        if remaining.iter().all(|byte| *byte == 0)
            || (remaining[0] == VENDOR_ELEMENT_ID && remaining[1..].iter().all(|byte| *byte == 0))
        {
            break;
        }
        if remaining.len() < 2 {
            return Err(Wpa2FrameError::MalformedKeyData);
        }
        let element_len = remaining[1] as usize + 2;
        if element_len > remaining.len() {
            return Err(Wpa2FrameError::MalformedKeyData);
        }
        let element = &remaining[..element_len];
        if element[0] != VENDOR_ELEMENT_ID
            || element.len() != 24
            || element[2..5] != RSN_OUI
            || element[5] != GTK_KDE_TYPE
            || element[7] != 0
            || element[6] & !0x07 != 0
            || gtk.is_some()
        {
            return Err(if gtk.is_some() {
                Wpa2FrameError::DuplicateGtk
            } else {
                Wpa2FrameError::UnsupportedKeyData
            });
        }
        let mut key = [0; WPA2_GTK_LEN];
        key.copy_from_slice(&element[8..24]);
        gtk = Some(Wpa2Gtk::new(
            element[6] & 0x03,
            element[6] & 0x04 != 0,
            key,
        )?);
        offset += element_len;
    }
    gtk.ok_or(Wpa2FrameError::MissingGtk)
}
