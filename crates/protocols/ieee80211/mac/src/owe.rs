//! OWE association and Enhanced Open transition information elements.
//!
//! RFC 8110 section 4.3 defines compact ECC public keys and group numbers.
//! This owner validates framing; curve validation and secret keys belong to
//! RSN. Transition advertisement carries a binary SSID, not a UTF-8 name.
use crate::{
    codec::{BodyReader, BodyWriter, Truncated},
    management::{
        EXTENSION_ELEMENT_ID, MAC_ADDRESS_LEN, MAX_SSID_LEN, MacAddress, VENDOR_ELEMENT_ID,
        WFA_OUI,
        elements::{ELEMENT_HEADER_LEN, Elements},
        is_group_address,
    },
};

pub const DH_PARAMETER_EXTENSION_ID: u8 = 32;
pub const TRANSITION_OUI_TYPE: u8 = 28;
pub use crate::security::rsn::RSN_AKM_OWE as OWE_AKM_TYPE;
const GROUP_LEN: usize = core::mem::size_of::<u16>();
const DH_BODY_PREFIX_LEN: usize = 1 + GROUP_LEN;
const TRANSITION_PREFIX_LEN: usize = WFA_OUI.len() + 1 + MAC_ADDRESS_LEN + 1;

/// NIST ECC groups shared with the SAE wire contract.
pub use crate::security::NistEccGroup as Group;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WireError {
    Truncated,
    WrongElement,
    InvalidLength,
    InvalidAddress,
    UnsupportedGroup(u16),
    DuplicateElement,
    OutputTooSmall { required: usize },
}
impl From<Truncated> for WireError {
    fn from(_: Truncated) -> Self {
        Self::Truncated
    }
}

/// Borrowed DH Parameter body. Unknown groups remain observable so an AP can
/// reject them with status 77; this parser does not invent their key geometry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DhParameter<'a> {
    pub group: u16,
    pub public_key: &'a [u8],
}
impl<'a> DhParameter<'a> {
    /// Body includes the extension selector, but excludes the IE header.
    pub fn parse(body: &'a [u8]) -> Result<Self, WireError> {
        let mut fields = BodyReader::new(body);
        if fields.u8()? != DH_PARAMETER_EXTENSION_ID {
            return Err(WireError::WrongElement);
        }
        let value = Self {
            group: u16::from_le_bytes(fields.array()?),
            public_key: fields.remaining(),
        };
        if value.public_key.is_empty() || body.len() > u8::MAX as usize {
            return Err(WireError::InvalidLength);
        }
        if let Some(group) = Group::from_number(value.group)
            && value.public_key.len() != group.coordinate_len()
        {
            return Err(WireError::InvalidLength);
        }
        Ok(value)
    }
    pub fn supported_group(self) -> Result<Group, WireError> {
        let group =
            Group::from_number(self.group).ok_or(WireError::UnsupportedGroup(self.group))?;
        if self.public_key.len() != group.coordinate_len() {
            return Err(WireError::InvalidLength);
        }
        Ok(group)
    }
    /// Find the single OWE DH IE without rejecting unrelated extension IEs.
    pub fn from_elements(elements: Elements<'a>) -> Result<Option<Self>, WireError> {
        let mut value = None;
        for element in elements.iter().filter(|element| {
            element.id == EXTENSION_ELEMENT_ID
                && element.body.first() == Some(&DH_PARAMETER_EXTENSION_ID)
        }) {
            if value.replace(Self::parse(element.body)?).is_some() {
                return Err(WireError::DuplicateElement);
            }
        }
        Ok(value)
    }
    /// Encode a complete IE. No bytes change if validation or capacity fails.
    pub fn encode(self, output: &mut [u8]) -> Result<usize, WireError> {
        self.supported_group()?;
        let body_len = DH_BODY_PREFIX_LEN + self.public_key.len();
        let length = ELEMENT_HEADER_LEN + body_len;
        let mut fields = BodyWriter::new(
            output
                .get_mut(..length)
                .ok_or(WireError::OutputTooSmall { required: length })?,
        );
        fields.put(&[
            EXTENSION_ELEMENT_ID,
            body_len as u8,
            DH_PARAMETER_EXTENSION_ID,
        ]);
        fields.put(&self.group.to_le_bytes());
        fields.put(self.public_key);
        Ok(length)
    }
}

/// OWE Transition Mode identifies the counterpart BSS. Its optional channel
/// hint is an operating class/channel pair, never an inferred local channel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Transition<'a> {
    pub bssid: MacAddress,
    pub ssid: &'a [u8],
    pub channel_hint: Option<(u8, u8)>,
}
impl<'a> Transition<'a> {
    pub fn parse(body: &'a [u8]) -> Result<Self, WireError> {
        let mut fields = BodyReader::new(body);
        if fields.array::<3>()? != WFA_OUI || fields.u8()? != TRANSITION_OUI_TYPE {
            return Err(WireError::WrongElement);
        }
        let bssid = fields.array()?;
        let ssid_len = usize::from(fields.u8()?);
        let ssid = fields.take(ssid_len)?;
        let channel_hint = match fields.remaining() {
            [] => None,
            [class, channel] => Some((*class, *channel)),
            _ => return Err(WireError::InvalidLength),
        };
        let value = Self {
            bssid,
            ssid,
            channel_hint,
        };
        value.validate()?;
        Ok(value)
    }
    fn validate(self) -> Result<(), WireError> {
        if self.bssid == [0; MAC_ADDRESS_LEN] || is_group_address(self.bssid) {
            return Err(WireError::InvalidAddress);
        }
        if self.ssid.is_empty() || self.ssid.len() > MAX_SSID_LEN {
            return Err(WireError::InvalidLength);
        }
        Ok(())
    }
    pub fn from_elements(elements: Elements<'a>) -> Result<Option<Self>, WireError> {
        let mut value = None;
        for element in elements.iter().filter(|element| {
            element.id == VENDOR_ELEMENT_ID
                && element.body.starts_with(&WFA_OUI)
                && element.body.get(WFA_OUI.len()) == Some(&TRANSITION_OUI_TYPE)
        }) {
            if value.replace(Self::parse(element.body)?).is_some() {
                return Err(WireError::DuplicateElement);
            }
        }
        Ok(value)
    }
    pub fn encode(self, output: &mut [u8]) -> Result<usize, WireError> {
        self.validate()?;
        let body_len = TRANSITION_PREFIX_LEN
            + self.ssid.len()
            + if self.channel_hint.is_some() { 2 } else { 0 };
        let length = ELEMENT_HEADER_LEN + body_len;
        let mut fields = BodyWriter::new(
            output
                .get_mut(..length)
                .ok_or(WireError::OutputTooSmall { required: length })?,
        );
        fields.put(&[VENDOR_ELEMENT_ID, body_len as u8]);
        fields.put(&WFA_OUI);
        fields.put(&[TRANSITION_OUI_TYPE]);
        fields.put(&self.bssid);
        fields.put(&[self.ssid.len() as u8]);
        fields.put(self.ssid);
        if let Some((class, channel)) = self.channel_hint {
            fields.put(&[class, channel]);
        }
        Ok(length)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compact_keys_are_exact_and_duplicate_owe_extensions_are_rejected() {
        for group in [Group::P256, Group::P384, Group::P521] {
            let key = [0x42; 66];
            let value = DhParameter {
                group: group.number(),
                public_key: &key[..group.coordinate_len()],
            };
            let mut encoded = [0; 160];
            let len = value.encode(&mut encoded).unwrap();
            assert_eq!(
                DhParameter::from_elements(Elements::parse(&encoded[..len]).unwrap()).unwrap(),
                Some(value)
            );
            let one = encoded;
            encoded[len..len * 2].copy_from_slice(&one[..len]);
            assert_eq!(
                DhParameter::from_elements(Elements::parse(&encoded[..len * 2]).unwrap()),
                Err(WireError::DuplicateElement)
            );
            assert!(DhParameter::parse(&one[2..len - 1]).is_err());
        }
        let future = DhParameter::parse(&[32, 250, 0, 1]).unwrap();
        assert_eq!(
            future.supported_group(),
            Err(WireError::UnsupportedGroup(250))
        );
    }
    #[test]
    fn transition_keeps_binary_ssid_channel_hint_and_checks_the_complete_tail() {
        let value = Transition {
            bssid: [2, 3, 4, 5, 6, 7],
            ssid: &[0xff, 0, b'x'],
            channel_hint: Some((81, 6)),
        };
        let mut encoded = [0; 64];
        let len = value.encode(&mut encoded).unwrap();
        assert_eq!(&encoded[..6], &[221, 16, 0x50, 0x6f, 0x9a, 28]);
        assert_eq!(Transition::parse(&encoded[2..len]).unwrap(), value);
        assert!(Transition::parse(&encoded[2..len - 1]).is_err());
        let mut too_short = [0x55; 4];
        assert!(value.encode(&mut too_short).is_err());
        assert_eq!(too_short, [0x55; 4]);
        assert!(
            Transition {
                bssid: [0xff; 6],
                ..value
            }
            .encode(&mut encoded)
            .is_err()
        );
    }
}
