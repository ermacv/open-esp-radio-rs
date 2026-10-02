use super::*;
use crate::management::{
    PROTECTED_DUAL_ACTION_CATEGORY, PUBLIC_ACTION_CATEGORY,
    elements::{ELEMENT_HEADER_LEN, ELEMENT_LENGTH_OFFSET},
};

const ACTION_PREFIX_LEN: usize = 3; // Category, Action, Dialog Token.
const QUERY_LENGTH_LEN: usize = core::mem::size_of::<u16>();
const RESPONSE_FIXED_LEN: usize = 2 * core::mem::size_of::<u16>();
const MORE_FRAGMENTS_BIT: u8 = 0x80;
pub const MAX_FRAGMENT_ID: u8 = 127;
pub const MAX_FRAGMENT_COUNT: usize = MAX_FRAGMENT_ID as usize + 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Category {
    Public = PUBLIC_ACTION_CATEGORY,
    ProtectedDual = PROTECTED_DUAL_ACTION_CATEGORY,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Action {
    InitialRequest = 10,
    InitialResponse = 11,
    ComebackRequest = 12,
    ComebackResponse = 13,
}

/// GAS-specific status vocabulary; unknown status values remain explicit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Status(pub u16);
impl Status {
    pub const SUCCESS: Self = Self(0);
    pub const PROTOCOL_NOT_SUPPORTED: Self = Self(59);
    pub const NO_OUTSTANDING_REQUEST: Self = Self(60);
    pub const RESPONSE_NOT_RECEIVED: Self = Self(61);
    pub const RESPONSE_TIMEOUT: Self = Self(62);
    pub const RESPONSE_TOO_LARGE: Self = Self(63);
    pub const REFUSED_HOME: Self = Self(64);
    pub const SERVER_UNREACHABLE: Self = Self(65);
    pub const REFUSED_SSPN: Self = Self(67);
    pub const REFUSED_UNAUTHENTICATED: Self = Self(68);
    pub const RESPONSE_OUTSTANDING: Self = Self(95);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Fragment {
    id: u8,
    more: bool,
}
impl Fragment {
    pub fn new(id: u8, more: bool) -> Result<Self, WireError> {
        if id > MAX_FRAGMENT_ID || (id == MAX_FRAGMENT_ID && more) {
            return Err(WireError::InvalidFragment);
        }
        Ok(Self { id, more })
    }
    pub const fn id(self) -> u8 {
        self.id
    }
    pub const fn more(self) -> bool {
        self.more
    }
    fn octet(self) -> u8 {
        self.id | if self.more { MORE_FRAGMENTS_BIT } else { 0 }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Body<'a> {
    InitialRequest {
        advertisement: AdvertisementProtocol<'a>,
        query: &'a [u8],
    },
    InitialResponse {
        status: Status,
        comeback_delay_tu: u16,
        advertisement: AdvertisementProtocol<'a>,
        response: &'a [u8],
    },
    ComebackRequest,
    ComebackResponse {
        status: Status,
        fragment: Fragment,
        comeback_delay_tu: u16,
        advertisement: AdvertisementProtocol<'a>,
        response: &'a [u8],
    },
}
impl<'a> Body<'a> {
    pub const fn action(self) -> Action {
        match self {
            Self::InitialRequest { .. } => Action::InitialRequest,
            Self::InitialResponse { .. } => Action::InitialResponse,
            Self::ComebackRequest => Action::ComebackRequest,
            Self::ComebackResponse { .. } => Action::ComebackResponse,
        }
    }
    pub const fn advertisement(self) -> Option<AdvertisementProtocol<'a>> {
        match self {
            Self::InitialRequest { advertisement, .. }
            | Self::InitialResponse { advertisement, .. }
            | Self::ComebackResponse { advertisement, .. } => Some(advertisement),
            Self::ComebackRequest => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Frame<'a> {
    pub category: Category,
    /// GAS permits zero; this is not a k/v autonomous-report token.
    pub dialog_token: u8,
    pub body: Body<'a>,
}
impl<'a> Frame<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, WireError> {
        let mut fields = BodyReader::new(bytes);
        let category = match fields.u8()? {
            PUBLIC_ACTION_CATEGORY => Category::Public,
            PROTECTED_DUAL_ACTION_CATEGORY => Category::ProtectedDual,
            _ => return Err(WireError::WrongAction),
        };
        let action = fields.u8()?;
        let dialog_token = fields.u8()?;
        let body = match action {
            value if value == Action::ComebackRequest as u8 => Body::ComebackRequest,
            value if value == Action::InitialRequest as u8 => {
                let (advertisement, query) = read_query(&mut fields)?;
                Body::InitialRequest {
                    advertisement,
                    query,
                }
            }
            value if value == Action::InitialResponse as u8 => {
                let status = Status(u16::from_le_bytes(fields.array()?));
                let comeback_delay_tu = u16::from_le_bytes(fields.array()?);
                let (advertisement, response) = read_query(&mut fields)?;
                Body::InitialResponse {
                    status,
                    comeback_delay_tu,
                    advertisement,
                    response,
                }
            }
            value if value == Action::ComebackResponse as u8 => {
                let status = Status(u16::from_le_bytes(fields.array()?));
                let fragment = fields.u8()?;
                let fragment = Fragment::new(
                    fragment & MAX_FRAGMENT_ID,
                    fragment & MORE_FRAGMENTS_BIT != 0,
                )?;
                let comeback_delay_tu = u16::from_le_bytes(fields.array()?);
                let (advertisement, response) = read_query(&mut fields)?;
                Body::ComebackResponse {
                    status,
                    fragment,
                    comeback_delay_tu,
                    advertisement,
                    response,
                }
            }
            _ => return Err(WireError::WrongAction),
        };
        if !fields.remaining().is_empty() {
            return Err(WireError::InconsistentFields);
        }
        let frame = Self {
            category,
            dialog_token,
            body,
        };
        frame.encoded_len()?;
        Ok(frame)
    }
    pub fn encoded_len(self) -> Result<usize, WireError> {
        let (fixed, payload) = match self.body {
            Body::ComebackRequest => return Ok(ACTION_PREFIX_LEN),
            Body::InitialRequest { query, .. } => (0, query),
            Body::InitialResponse {
                comeback_delay_tu,
                response,
                ..
            } => {
                validate_response(comeback_delay_tu, response)?;
                (RESPONSE_FIXED_LEN, response)
            }
            Body::ComebackResponse {
                fragment,
                comeback_delay_tu,
                response,
                ..
            } => {
                validate_response(comeback_delay_tu, response)?;
                if comeback_delay_tu != 0 && (fragment.id() != 0 || fragment.more()) {
                    return Err(WireError::InconsistentFields);
                }
                (RESPONSE_FIXED_LEN + 1, response)
            }
        };
        if payload.len() > u16::MAX as usize {
            return Err(WireError::PayloadTooLong);
        }
        Ok(ACTION_PREFIX_LEN
            + fixed
            + self
                .body
                .advertisement()
                .expect("query body")
                .encoded_len()?
            + QUERY_LENGTH_LEN
            + payload.len())
    }
    pub fn encode(self, bytes: &mut [u8]) -> Result<usize, WireError> {
        let length = self.encoded_len()?;
        let mut fields = BodyWriter::new(output(bytes, length)?);
        fields.put(&[
            self.category as u8,
            self.body.action() as u8,
            self.dialog_token,
        ]);
        let payload = match self.body {
            Body::ComebackRequest => return Ok(length),
            Body::InitialRequest { query, .. } => query,
            Body::InitialResponse {
                status,
                comeback_delay_tu,
                response,
                ..
            } => {
                fields.put(&status.0.to_le_bytes());
                fields.put(&comeback_delay_tu.to_le_bytes());
                response
            }
            Body::ComebackResponse {
                status,
                fragment,
                comeback_delay_tu,
                response,
                ..
            } => {
                fields.put(&status.0.to_le_bytes());
                fields.put(&[fragment.octet()]);
                fields.put(&comeback_delay_tu.to_le_bytes());
                response
            }
        };
        self.body
            .advertisement()
            .expect("query body")
            .write(&mut fields);
        fields.put(&(payload.len() as u16).to_le_bytes());
        fields.put(payload);
        Ok(length)
    }
}
fn read_query<'a>(
    fields: &mut BodyReader<'a>,
) -> Result<(AdvertisementProtocol<'a>, &'a [u8]), WireError> {
    let header = fields
        .remaining()
        .get(..ELEMENT_HEADER_LEN)
        .ok_or(WireError::Truncated)?;
    let length = ELEMENT_HEADER_LEN + usize::from(header[ELEMENT_LENGTH_OFFSET]);
    let advertisement = AdvertisementProtocol::parse(fields.take(length)?)?;
    let length = usize::from(u16::from_le_bytes(fields.array()?));
    Ok((advertisement, fields.take(length)?))
}
fn validate_response(delay: u16, response: &[u8]) -> Result<(), WireError> {
    // Status semantics belong to the dialog. In particular, hostap accepts
    // status 95 in Comeback Responses with data as well as delayed replies.
    if delay != 0 && !response.is_empty() {
        return Err(WireError::InconsistentFields);
    }
    Ok(())
}
