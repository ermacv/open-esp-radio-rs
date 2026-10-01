//! WNM idle and sleep formats. Key material has a redacted Debug view.
use super::management::{action_elements, encode_action};
use super::{
    ACTION_HEADER_LEN, BodyReader, BodyWriter, DIALOG_TOKEN_OFFSET, ELEMENT_HEADER_LEN, Elements,
    WireError, WnmAction, element_id, header, output, token,
};
use core::mem::size_of;

const IDLE_BODY_LEN: usize = size_of::<u16>() + size_of::<u8>();
const SLEEP_BODY_LEN: usize = 2 * size_of::<u8>() + size_of::<u16>();
const SLEEP_RESPONSE_FIXED_LEN: usize = ACTION_HEADER_LEN + size_of::<u16>();
pub const GTK_RSC_LEN: usize = size_of::<u64>();
pub const INTEGRITY_PACKET_NUMBER_LEN: usize = crate::security::BIP_PACKET_NUMBER_LEN;
pub const GTK_MIN_KEY_LEN: usize = 5;
pub const GTK_MAX_KEY_LEN: usize = 32;
/// Fixed BIP-CMAC-128 key field in the original WNM Sleep integrity subelement.
pub const INTEGRITY_KEY_LEN: usize = crate::security::BIP_CMAC_128_KEY_LEN;
const GTK_FIXED_LEN: usize = size_of::<u16>() + size_of::<u8>() + GTK_RSC_LEN;
const INTEGRITY_FIXED_LEN: usize = size_of::<u16>() + INTEGRITY_PACKET_NUMBER_LEN;
pub const GTK_KEY_ID_MASK: u16 = 0b11;
pub const GTK_TX_BIT: u16 = 1 << 2;
pub const GTK_KEY_INFO_MASK: u16 = GTK_KEY_ID_MASK | GTK_TX_BIT;
pub mod sleep_key_id {
    pub const GTK: u8 = 0;
    pub const IGTK: u8 = 1;
    pub const BIGTK: u8 = 2;
}

pub const BSS_MAX_IDLE_ELEMENT_ID: u8 = element_id::BSS_MAX_IDLE;
pub const WNM_SLEEP_ELEMENT_ID: u8 = element_id::WNM_SLEEP;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BssMaxIdle {
    /// Units of 1000 TU (1.024 seconds); zero is reserved.
    pub period: u16,
    pub options: u8,
}
impl BssMaxIdle {
    pub const PROTECTED_KEEP_ALIVE: u8 = 1;
    pub const PERIOD_UNIT_TU: u64 = 1000;
    pub fn parse(elements: Elements<'_>) -> Result<Option<Self>, WireError> {
        let Some(body) = elements.unique(BSS_MAX_IDLE_ELEMENT_ID)? else {
            return Ok(None);
        };
        let [low, high, options] = body else {
            return Err(WireError::InvalidElementLength(BSS_MAX_IDLE_ELEMENT_ID));
        };
        let value = Self {
            period: u16::from_le_bytes([*low, *high]),
            options: *options,
        };
        value.validate()?;
        Ok(Some(value))
    }
    pub const fn protected_keep_alive(self) -> bool {
        self.options & Self::PROTECTED_KEEP_ALIVE != 0
    }
    pub fn validate(self) -> Result<(), WireError> {
        if self.period == 0 {
            Err(WireError::InconsistentFields)
        } else {
            Ok(())
        }
    }
    pub fn encode(self, buffer: &mut [u8]) -> Result<usize, WireError> {
        self.validate()?;
        let len = ELEMENT_HEADER_LEN + IDLE_BODY_LEN;
        let mut fields = BodyWriter::new(output(buffer, len)?);
        fields.put(&[BSS_MAX_IDLE_ELEMENT_ID, IDLE_BODY_LEN as u8]);
        fields.put(&self.period.to_le_bytes());
        fields.put(&[self.options]);
        Ok(len)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SleepAction(pub u8);
impl SleepAction {
    pub const ENTER: Self = Self(0);
    pub const EXIT: Self = Self(1);
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SleepStatus(pub u8);
impl SleepStatus {
    pub const ACCEPT: Self = Self(0);
    pub const ACCEPT_KEY_UPDATE: Self = Self(1);
    pub const DENIED: Self = Self(2);
    pub const DENIED_TEMPORARILY: Self = Self(3);
    pub const DENIED_KEY: Self = Self(4);
    pub const DENIED_OTHER_SERVICE: Self = Self(5);
    pub const fn is_known(self) -> bool {
        matches!(
            self,
            Self::ACCEPT
                | Self::ACCEPT_KEY_UPDATE
                | Self::DENIED
                | Self::DENIED_TEMPORARILY
                | Self::DENIED_KEY
                | Self::DENIED_OTHER_SERVICE
        )
    }
    pub const fn accepted(self) -> bool {
        matches!(self, Self::ACCEPT | Self::ACCEPT_KEY_UPDATE)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WnmSleepElement {
    pub action: SleepAction,
    pub status: SleepStatus,
    /// DTIM intervals, not TU or beacon intervals. Zero has no fixed wake time.
    pub interval_dtim: u16,
}
impl WnmSleepElement {
    pub fn parse(elements: Elements<'_>) -> Result<Self, WireError> {
        let body = elements
            .unique(WNM_SLEEP_ELEMENT_ID)?
            .ok_or(WireError::InconsistentFields)?;
        let [action, status, low, high] = body else {
            return Err(WireError::InvalidElementLength(WNM_SLEEP_ELEMENT_ID));
        };
        Ok(Self {
            action: SleepAction(*action),
            status: SleepStatus(*status),
            interval_dtim: u16::from_le_bytes([*low, *high]),
        })
    }
    pub fn encode(self, buffer: &mut [u8]) -> Result<usize, WireError> {
        let len = ELEMENT_HEADER_LEN + SLEEP_BODY_LEN;
        let mut fields = BodyWriter::new(output(buffer, len)?);
        fields.put(&[
            WNM_SLEEP_ELEMENT_ID,
            SLEEP_BODY_LEN as u8,
            self.action.0,
            self.status.0,
        ]);
        fields.put(&self.interval_dtim.to_le_bytes());
        Ok(len)
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct SleepKeyData<'a>(Elements<'a>);
impl core::fmt::Debug for SleepKeyData<'_> {
    fn fmt(&self, out: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        out.debug_struct("SleepKeyData")
            .field("bytes", &self.0.as_bytes().len())
            .finish_non_exhaustive()
    }
}
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum SleepKey<'a> {
    Gtk {
        key_info: u16,
        rsc: [u8; GTK_RSC_LEN],
        key: &'a [u8],
    },
    Integrity {
        kind: u8,
        key_id: u16,
        packet_number: [u8; INTEGRITY_PACKET_NUMBER_LEN],
        key: &'a [u8],
    },
    Other {
        kind: u8,
        body: &'a [u8],
    },
}
impl core::fmt::Debug for SleepKey<'_> {
    fn fmt(&self, out: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Gtk { key_info, .. } => out
                .debug_struct("Gtk")
                .field("key_info", key_info)
                .finish_non_exhaustive(),
            Self::Integrity { kind, key_id, .. } => out
                .debug_struct("Integrity")
                .field("kind", kind)
                .field("key_id", key_id)
                .finish_non_exhaustive(),
            Self::Other { kind, .. } => out
                .debug_struct("Other")
                .field("kind", kind)
                .finish_non_exhaustive(),
        }
    }
}
impl<'a> SleepKeyData<'a> {
    pub const EMPTY: Self = Self(Elements::EMPTY);
    pub fn parse(bytes: &'a [u8]) -> Result<Self, WireError> {
        let elements = Elements::parse(bytes)?;
        for element in elements.iter() {
            parse_key(element.id, element.body)?;
        }
        Ok(Self(elements))
    }
    pub const fn as_bytes(self) -> &'a [u8] {
        self.0.as_bytes()
    }
    pub fn keys(self) -> impl Iterator<Item = SleepKey<'a>> {
        self.0
            .iter()
            .map(|element| parse_key(element.id, element.body).expect("validated key data"))
    }
}
fn parse_key(kind: u8, body: &[u8]) -> Result<SleepKey<'_>, WireError> {
    match kind {
        sleep_key_id::GTK => {
            if body.len() < GTK_FIXED_LEN {
                return Err(WireError::InvalidElementLength(kind));
            }
            let mut fields = BodyReader::new(body);
            let key_info = u16::from_le_bytes(fields.array()?);
            let length = usize::from(fields.u8()?);
            let rsc = fields.array()?;
            if !(GTK_MIN_KEY_LEN..=GTK_MAX_KEY_LEN).contains(&length)
                || fields.remaining().len() != length
            {
                return Err(WireError::InvalidElementLength(kind));
            }
            Ok(SleepKey::Gtk {
                key_info,
                rsc,
                key: fields.remaining(),
            })
        }
        sleep_key_id::IGTK | sleep_key_id::BIGTK => {
            if body.len() != INTEGRITY_FIXED_LEN + INTEGRITY_KEY_LEN {
                return Err(WireError::InvalidElementLength(kind));
            }
            let mut fields = BodyReader::new(body);
            Ok(SleepKey::Integrity {
                kind,
                key_id: u16::from_le_bytes(fields.array()?),
                packet_number: fields.array()?,
                key: fields.remaining(),
            })
        }
        _ => Ok(SleepKey::Other { kind, body }),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WnmSleepRequest<'a> {
    pub dialog_token: u8,
    pub elements: Elements<'a>,
}
impl<'a> WnmSleepRequest<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, WireError> {
        let (dialog_token, elements) = action_elements(bytes, WnmAction::WnmSleepRequest, false)?;
        let value = Self {
            dialog_token,
            elements,
        };
        value.validate()?;
        Ok(value)
    }
    pub fn sleep(self) -> Result<WnmSleepElement, WireError> {
        WnmSleepElement::parse(self.elements)
    }
    pub fn validate(self) -> Result<(), WireError> {
        token(self.dialog_token)?;
        self.sleep()?;
        super::validate_tfs_requests(self.elements)?;
        Ok(())
    }
    pub fn encode(self, buffer: &mut [u8]) -> Result<usize, WireError> {
        self.validate()?;
        encode_action(
            WnmAction::WnmSleepRequest,
            self.dialog_token,
            self.elements,
            buffer,
        )
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WnmSleepResponse<'a> {
    pub dialog_token: u8,
    pub keys: SleepKeyData<'a>,
    pub elements: Elements<'a>,
}
impl<'a> WnmSleepResponse<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, WireError> {
        header(
            bytes,
            super::WNM_CATEGORY,
            WnmAction::WnmSleepResponse as u8,
            SLEEP_RESPONSE_FIXED_LEN,
        )?;
        let mut fields = BodyReader::new(&bytes[DIALOG_TOKEN_OFFSET..]);
        let dialog_token = fields.u8()?;
        let key_length = usize::from(u16::from_le_bytes(fields.array()?));
        let keys = SleepKeyData::parse(fields.take(key_length)?)?;
        let value = Self {
            dialog_token,
            keys,
            elements: Elements::parse(fields.remaining())?,
        };
        value.validate()?;
        Ok(value)
    }
    pub fn sleep(self) -> Result<WnmSleepElement, WireError> {
        WnmSleepElement::parse(self.elements)
    }
    pub fn validate(self) -> Result<(), WireError> {
        token(self.dialog_token)?;
        self.sleep()?;
        if self.keys.as_bytes().len() > usize::from(u16::MAX) {
            return Err(WireError::ElementTooLong);
        }
        super::validate_tfs_responses(self.elements)?;
        Ok(())
    }
    pub fn encode(self, buffer: &mut [u8]) -> Result<usize, WireError> {
        self.validate()?;
        let key_length = self.keys.as_bytes().len();
        let len = SLEEP_RESPONSE_FIXED_LEN + key_length + self.elements.as_bytes().len();
        let mut fields = BodyWriter::new(output(buffer, len)?);
        fields.put(&[
            super::WNM_CATEGORY,
            WnmAction::WnmSleepResponse as u8,
            self.dialog_token,
        ]);
        fields.put(&(key_length as u16).to_le_bytes());
        fields.put(self.keys.as_bytes());
        fields.put(self.elements.as_bytes());
        Ok(len)
    }
}

#[cfg(test)]
mod tests;
