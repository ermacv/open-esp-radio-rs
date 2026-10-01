//! Common Event/Diagnostic Action framing and an optional alternate report URI.
use super::element_id;
use super::{
    ACTION_HEADER_LEN, BodyReader, BodyWriter, DIALOG_TOKEN_OFFSET, ELEMENT_HEADER_LEN, Elements,
    MAX_ELEMENT_BODY_LEN, WNM_CATEGORY, WireError, WnmAction, output, token,
};
use core::mem::size_of;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DestinationUri<'a> {
    pub ess_detection_minutes: u8,
    /// RFC 3986 URI bytes; the caller owns URI parsing and the transport.
    pub uri: &'a [u8],
}
impl<'a> DestinationUri<'a> {
    // The original Destination URI permits 1..=253 URI octets (8.4.2.92).
    pub const MAX_URI_LEN: usize = 253;
    const FIXED_LEN: usize = size_of::<u8>();
    pub fn parse(elements: Elements<'a>) -> Result<Option<Self>, WireError> {
        let Some(body) = elements.unique(element_id::DESTINATION_URI)? else {
            return Ok(None);
        };
        if !(Self::FIXED_LEN + 1..=Self::FIXED_LEN + Self::MAX_URI_LEN).contains(&body.len()) {
            return Err(WireError::InvalidElementLength(element_id::DESTINATION_URI));
        }
        if elements
            .iter()
            .last()
            .is_none_or(|e| e.id != element_id::DESTINATION_URI)
        {
            return Err(WireError::InconsistentFields);
        }
        let mut fields = BodyReader::new(body);
        Ok(Some(Self {
            ess_detection_minutes: fields.u8()?,
            uri: fields.remaining(),
        }))
    }
    pub fn encode(self, buffer: &mut [u8]) -> Result<usize, WireError> {
        if self.uri.is_empty() || self.uri.len() > Self::MAX_URI_LEN {
            return Err(WireError::InvalidElementLength(element_id::DESTINATION_URI));
        }
        let body_len = Self::FIXED_LEN + self.uri.len();
        let len = ELEMENT_HEADER_LEN + body_len;
        let mut fields = BodyWriter::new(output(buffer, len)?);
        fields.put(&[
            element_id::DESTINATION_URI,
            body_len as u8,
            self.ess_detection_minutes,
        ]);
        fields.put(self.uri);
        Ok(len)
    }
}
pub(super) fn action_elements(
    bytes: &[u8],
    action: WnmAction,
    zero_allowed: bool,
) -> Result<(u8, Elements<'_>), WireError> {
    if bytes.len() < ACTION_HEADER_LEN {
        return Err(WireError::Truncated);
    }
    if bytes[..DIALOG_TOKEN_OFFSET] != [WNM_CATEGORY, action as u8] {
        return Err(WireError::WrongAction);
    }
    if !zero_allowed {
        token(bytes[DIALOG_TOKEN_OFFSET])?;
    }
    Ok((
        bytes[DIALOG_TOKEN_OFFSET],
        Elements::parse(&bytes[ACTION_HEADER_LEN..])?,
    ))
}
pub(super) fn encode_action(
    action: WnmAction,
    dialog: u8,
    elements: Elements<'_>,
    buffer: &mut [u8],
) -> Result<usize, WireError> {
    let out = output(buffer, ACTION_HEADER_LEN + elements.as_bytes().len())?;
    out[..ACTION_HEADER_LEN].copy_from_slice(&[WNM_CATEGORY, action as u8, dialog]);
    out[ACTION_HEADER_LEN..].copy_from_slice(elements.as_bytes());
    Ok(out.len())
}
pub(super) fn encode_element(id: u8, body: &[u8], buffer: &mut [u8]) -> Result<usize, WireError> {
    if body.len() > MAX_ELEMENT_BODY_LEN {
        return Err(WireError::ElementTooLong);
    }
    let out = output(buffer, ELEMENT_HEADER_LEN + body.len())?;
    out[..ELEMENT_HEADER_LEN].copy_from_slice(&[id, body.len() as u8]);
    out[ELEMENT_HEADER_LEN..].copy_from_slice(body);
    Ok(out.len())
}
/// Complete EAP type octets, including the expanded vendor ID and vendor type.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EapMethod {
    Legacy(u8),
    Expanded {
        vendor_id: [u8; EapMethod::VENDOR_ID_LEN],
        vendor_type: [u8; EapMethod::VENDOR_TYPE_LEN],
    },
}
impl EapMethod {
    pub const EXPANDED_TYPE: u8 = 254;
    pub const VENDOR_ID_LEN: usize = 3;
    pub const VENDOR_TYPE_LEN: usize = size_of::<u32>();
    pub const MAX_ENCODED_LEN: usize =
        size_of::<u8>() + Self::VENDOR_ID_LEN + Self::VENDOR_TYPE_LEN;
    pub fn parse_prefix(bytes: &[u8]) -> Result<Self, WireError> {
        let kind = *bytes.first().ok_or(WireError::Truncated)?;
        let len = if kind == Self::EXPANDED_TYPE {
            Self::MAX_ENCODED_LEN
        } else {
            size_of::<u8>()
        };
        Self::parse(bytes.get(..len).ok_or(WireError::Truncated)?)
    }
    pub fn parse(bytes: &[u8]) -> Result<Self, WireError> {
        match bytes {
            [Self::EXPANDED_TYPE, rest @ ..]
                if rest.len() == Self::VENDOR_ID_LEN + Self::VENDOR_TYPE_LEN =>
            {
                Ok(Self::Expanded {
                    vendor_id: rest[..Self::VENDOR_ID_LEN].try_into().expect("width"),
                    vendor_type: rest[Self::VENDOR_ID_LEN..].try_into().expect("width"),
                })
            }
            [kind] if *kind != Self::EXPANDED_TYPE => Ok(Self::Legacy(*kind)),
            _ => Err(WireError::InconsistentFields),
        }
    }
    pub const fn encoded_len(self) -> usize {
        match self {
            Self::Legacy(_) => size_of::<u8>(),
            Self::Expanded { .. } => Self::MAX_ENCODED_LEN,
        }
    }
    pub fn encode(self, buffer: &mut [u8]) -> Result<usize, WireError> {
        if self == Self::Legacy(Self::EXPANDED_TYPE) {
            return Err(WireError::InconsistentFields);
        }
        let out = output(buffer, self.encoded_len())?;
        match self {
            Self::Legacy(kind) => out[0] = kind,
            Self::Expanded {
                vendor_id,
                vendor_type,
            } => {
                out[0] = Self::EXPANDED_TYPE;
                out[size_of::<u8>()..size_of::<u8>() + Self::VENDOR_ID_LEN]
                    .copy_from_slice(&vendor_id);
                out[size_of::<u8>() + Self::VENDOR_ID_LEN..].copy_from_slice(&vendor_type);
            }
        }
        Ok(out.len())
    }
}
