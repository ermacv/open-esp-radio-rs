//! FT authentication and over-the-DS Action bodies. The MAC header is external.
use super::*;
use crate::management::{MAC_ADDRESS_LEN, MacAddress};

pub const FT_AUTHENTICATION_ALGORITHM: u16 = 2;
pub const FT_ACTION_CATEGORY: u8 = 6;
pub const AUTHENTICATION_FIXED_BODY_LEN: usize = 3 * core::mem::size_of::<u16>();
pub const FT_ACTION_REQUEST_FIXED_LEN: usize = 2 + 2 * MAC_ADDRESS_LEN;
pub const FT_ACTION_RESPONSE_FIXED_LEN: usize =
    FT_ACTION_REQUEST_FIXED_LEN + core::mem::size_of::<u16>();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u16)]
pub enum AuthenticationTransaction {
    Request = 1,
    Response = 2,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum MicTransaction {
    ReassociationRequest = 5,
    ReassociationResponse = 6,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Authentication<'a> {
    pub transaction: AuthenticationTransaction,
    pub status: u16,
    pub elements: Elements<'a>,
}
impl<'a> Authentication<'a> {
    pub fn parse(body: &'a [u8]) -> Result<Self, WireError> {
        let fixed = body
            .get(..AUTHENTICATION_FIXED_BODY_LEN)
            .ok_or(WireError::WrongFrame)?;
        if u16::from_le_bytes([fixed[0], fixed[1]]) != FT_AUTHENTICATION_ALGORITHM {
            return Err(WireError::WrongFrame);
        }
        let transaction = match u16::from_le_bytes([fixed[2], fixed[3]]) {
            value if value == AuthenticationTransaction::Request as u16 => {
                AuthenticationTransaction::Request
            }
            value if value == AuthenticationTransaction::Response as u16 => {
                AuthenticationTransaction::Response
            }
            _ => return Err(WireError::WrongFrame),
        };
        Ok(Self {
            transaction,
            status: u16::from_le_bytes([fixed[4], fixed[5]]),
            elements: Elements::parse(&body[AUTHENTICATION_FIXED_BODY_LEN..])?,
        })
    }
    pub fn encode(self, bytes: &mut [u8]) -> Result<usize, WireError> {
        let length = AUTHENTICATION_FIXED_BODY_LEN + self.elements.as_bytes().len();
        let bytes = output(bytes, length)?;
        bytes[..2].copy_from_slice(&FT_AUTHENTICATION_ALGORITHM.to_le_bytes());
        bytes[2..4].copy_from_slice(&(self.transaction as u16).to_le_bytes());
        bytes[4..AUTHENTICATION_FIXED_BODY_LEN].copy_from_slice(&self.status.to_le_bytes());
        bytes[AUTHENTICATION_FIXED_BODY_LEN..].copy_from_slice(self.elements.as_bytes());
        Ok(length)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum ActionKind {
    Request = 1,
    Response = 2,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Action<'a> {
    pub station: MacAddress,
    pub target: MacAddress,
    /// `None` is Request; `Some(status)` is Response, including failures.
    pub status: Option<u16>,
    pub elements: Elements<'a>,
}
impl<'a> Action<'a> {
    pub fn parse(body: &'a [u8]) -> Result<Self, WireError> {
        let fixed = body
            .get(..FT_ACTION_REQUEST_FIXED_LEN)
            .ok_or(WireError::WrongFrame)?;
        if fixed[0] != FT_ACTION_CATEGORY {
            return Err(WireError::WrongFrame);
        }
        let station = fixed[2..2 + MAC_ADDRESS_LEN]
            .try_into()
            .expect("checked station address");
        let target = fixed[2 + MAC_ADDRESS_LEN..]
            .try_into()
            .expect("checked target address");
        let (status, length) = match fixed[1] {
            value if value == ActionKind::Request as u8 => (None, FT_ACTION_REQUEST_FIXED_LEN),
            value if value == ActionKind::Response as u8 => {
                let status = body
                    .get(FT_ACTION_REQUEST_FIXED_LEN..FT_ACTION_RESPONSE_FIXED_LEN)
                    .ok_or(WireError::WrongFrame)?;
                (
                    Some(u16::from_le_bytes([status[0], status[1]])),
                    FT_ACTION_RESPONSE_FIXED_LEN,
                )
            }
            _ => return Err(WireError::WrongFrame),
        };
        Ok(Self {
            station,
            target,
            status,
            elements: Elements::parse(&body[length..])?,
        })
    }
    pub fn encode(self, bytes: &mut [u8]) -> Result<usize, WireError> {
        let fixed = if self.status.is_some() {
            FT_ACTION_RESPONSE_FIXED_LEN
        } else {
            FT_ACTION_REQUEST_FIXED_LEN
        };
        let length = fixed + self.elements.as_bytes().len();
        let bytes = output(bytes, length)?;
        bytes[0] = FT_ACTION_CATEGORY;
        bytes[1] = if self.status.is_some() {
            ActionKind::Response as u8
        } else {
            ActionKind::Request as u8
        };
        bytes[2..2 + MAC_ADDRESS_LEN].copy_from_slice(&self.station);
        bytes[2 + MAC_ADDRESS_LEN..FT_ACTION_REQUEST_FIXED_LEN].copy_from_slice(&self.target);
        if let Some(status) = self.status {
            bytes[FT_ACTION_REQUEST_FIXED_LEN..fixed].copy_from_slice(&status.to_le_bytes());
        }
        bytes[fixed..].copy_from_slice(self.elements.as_bytes());
        Ok(length)
    }
}
