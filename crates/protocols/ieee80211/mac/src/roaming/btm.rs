use super::neighbor::validate_reports;
use super::{
    ACTION_HEADER_LEN, BodyReader, BodyWriter, ELEMENT_HEADER_LEN, Elements, MAC_ADDRESS_LEN,
    MAX_ELEMENT_BODY_LEN, MacAddress, WNM_CATEGORY, WireError, WnmAction, header, output, token,
};
use core::mem::size_of;

const QUERY_FIXED_LEN: usize = ACTION_HEADER_LEN + size_of::<u8>();
const REQUEST_FIXED_LEN: usize = ACTION_HEADER_LEN + 2 * size_of::<u8>() + size_of::<u16>();
const RESPONSE_FIXED_LEN: usize = ACTION_HEADER_LEN + 2 * size_of::<u8>();
const TERMINATION_SUBELEMENT_ID: u8 = 4;
const TERMINATION_BODY_LEN: usize = size_of::<u64>() + size_of::<u16>();
const TERMINATION_ENCODED_LEN: usize = ELEMENT_HEADER_LEN + TERMINATION_BODY_LEN;

pub const BTM_QUERY_ACTION: u8 = WnmAction::BssTransitionQuery as u8;
pub const BTM_REQUEST_ACTION: u8 = WnmAction::BssTransitionRequest as u8;
pub const BTM_RESPONSE_ACTION: u8 = WnmAction::BssTransitionResponse as u8;

/// Preserve all mode bits, including extensions unknown to the base policy.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BtmRequestMode(pub u8);
impl BtmRequestMode {
    pub const CANDIDATES: Self = Self(1);
    pub const ABRIDGED: Self = Self(2);
    pub const DISASSOCIATION_IMMINENT: Self = Self(4);
    pub const TERMINATION: Self = Self(8);
    pub const ESS_DISASSOCIATION_IMMINENT: Self = Self(16);
    pub const KNOWN_BITS: u8 = Self::CANDIDATES.0
        | Self::ABRIDGED.0
        | Self::DISASSOCIATION_IMMINENT.0
        | Self::TERMINATION.0
        | Self::ESS_DISASSOCIATION_IMMINENT.0;
    pub const fn contains(self, flag: Self) -> bool {
        self.0 & flag.0 == flag.0
    }
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// Status octet is retained even when a peer uses a later assigned value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BtmStatus(pub u8);
impl BtmStatus {
    pub const ACCEPT: Self = Self(0);
    pub const REJECT_UNSPECIFIED: Self = Self(1);
    pub const REJECT_INSUFFICIENT_BEACON: Self = Self(2);
    pub const REJECT_INSUFFICIENT_CAPACITY: Self = Self(3);
    pub const REJECT_UNDESIRED: Self = Self(4);
    pub const REJECT_DELAY: Self = Self(5);
    pub const REJECT_CANDIDATES_PROVIDED: Self = Self(6);
    pub const REJECT_NO_SUITABLE_CANDIDATES: Self = Self(7);
    pub const REJECT_LEAVING_ESS: Self = Self(8);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BssTerminationDuration {
    pub tsf: u64,
    pub duration_minutes: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BtmQuery<'a> {
    pub dialog_token: u8,
    pub reason: u8,
    pub elements: Elements<'a>,
}
impl<'a> BtmQuery<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, WireError> {
        header(bytes, WNM_CATEGORY, BTM_QUERY_ACTION, QUERY_FIXED_LEN)?;
        let mut fields = BodyReader::new(&bytes[super::DIALOG_TOKEN_OFFSET..]);
        let query = Self {
            dialog_token: fields.u8()?,
            reason: fields.u8()?,
            elements: Elements::parse(fields.remaining())?,
        };
        query.validate()?;
        Ok(query)
    }
    pub fn validate(self) -> Result<(), WireError> {
        token(self.dialog_token)?;
        validate_reports(self.elements)
    }
    pub fn encode(self, buffer: &mut [u8]) -> Result<usize, WireError> {
        self.validate()?;
        let len = QUERY_FIXED_LEN + self.elements.as_bytes().len();
        let mut fields = BodyWriter::new(output(buffer, len)?);
        fields.put(&[
            WNM_CATEGORY,
            BTM_QUERY_ACTION,
            self.dialog_token,
            self.reason,
        ]);
        fields.put(self.elements.as_bytes());
        Ok(len)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BtmRequest<'a> {
    pub dialog_token: u8,
    pub mode: BtmRequestMode,
    /// Beacon intervals, not time units or milliseconds.
    pub disassociation_timer: u16,
    /// Beacon intervals for the candidate list's validity.
    pub validity_interval: u8,
    pub termination: Option<BssTerminationDuration>,
    /// The Session Information URL remains binary, up to 255 octets.
    pub session_url: Option<&'a [u8]>,
    pub elements: Elements<'a>,
}
impl<'a> BtmRequest<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, WireError> {
        header(bytes, WNM_CATEGORY, BTM_REQUEST_ACTION, REQUEST_FIXED_LEN)?;
        let mut fields = BodyReader::new(&bytes[super::DIALOG_TOKEN_OFFSET..]);
        let dialog_token = fields.u8()?;
        let mode = BtmRequestMode(fields.u8()?);
        let disassociation_timer = u16::from_le_bytes(fields.array()?);
        let validity_interval = fields.u8()?;
        let termination = if mode.contains(BtmRequestMode::TERMINATION) {
            let mut duration = BodyReader::new(fields.take(TERMINATION_ENCODED_LEN)?);
            if duration.u8()? != TERMINATION_SUBELEMENT_ID
                || usize::from(duration.u8()?) != TERMINATION_BODY_LEN
            {
                return Err(WireError::InvalidElementLength(TERMINATION_SUBELEMENT_ID));
            }
            Some(BssTerminationDuration {
                tsf: u64::from_le_bytes(duration.array()?),
                duration_minutes: u16::from_le_bytes(duration.array()?),
            })
        } else {
            None
        };
        let session_url = if mode.contains(BtmRequestMode::ESS_DISASSOCIATION_IMMINENT) {
            let len = usize::from(fields.u8()?);
            Some(fields.take(len)?)
        } else {
            None
        };
        let request = Self {
            dialog_token,
            mode,
            disassociation_timer,
            validity_interval,
            termination,
            session_url,
            elements: Elements::parse(fields.remaining())?,
        };
        request.validate()?;
        Ok(request)
    }
    pub fn validate(self) -> Result<(), WireError> {
        token(self.dialog_token)?;
        if self.mode.contains(BtmRequestMode::TERMINATION) != self.termination.is_some()
            || self
                .mode
                .contains(BtmRequestMode::ESS_DISASSOCIATION_IMMINENT)
                != self.session_url.is_some()
        {
            return Err(WireError::InconsistentFields);
        }
        if self
            .session_url
            .is_some_and(|url| url.len() > MAX_ELEMENT_BODY_LEN)
        {
            return Err(WireError::ElementTooLong);
        }
        validate_reports(self.elements)?;
        if !self.mode.contains(BtmRequestMode::CANDIDATES)
            && self
                .elements
                .iter()
                .any(|element| element.id == super::NEIGHBOR_REPORT_ELEMENT_ID)
        {
            return Err(WireError::InconsistentFields);
        }
        Ok(())
    }
    pub fn encode(self, buffer: &mut [u8]) -> Result<usize, WireError> {
        self.validate()?;
        let len = REQUEST_FIXED_LEN
            + self.termination.map_or(0, |_| TERMINATION_ENCODED_LEN)
            + self
                .session_url
                .map_or(0, |url| size_of::<u8>() + url.len())
            + self.elements.as_bytes().len();
        let mut fields = BodyWriter::new(output(buffer, len)?);
        fields.put(&[
            WNM_CATEGORY,
            BTM_REQUEST_ACTION,
            self.dialog_token,
            self.mode.0,
        ]);
        fields.put(&self.disassociation_timer.to_le_bytes());
        fields.put(&[self.validity_interval]);
        if let Some(termination) = self.termination {
            fields.put(&[TERMINATION_SUBELEMENT_ID, TERMINATION_BODY_LEN as u8]);
            fields.put(&termination.tsf.to_le_bytes());
            fields.put(&termination.duration_minutes.to_le_bytes());
        }
        if let Some(url) = self.session_url {
            fields.put(&[url.len() as u8]);
            fields.put(url);
        }
        fields.put(self.elements.as_bytes());
        Ok(len)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BtmResponse<'a> {
    pub dialog_token: u8,
    pub status: BtmStatus,
    pub termination_delay_minutes: u8,
    pub target_bssid: Option<MacAddress>,
    pub elements: Elements<'a>,
}
impl<'a> BtmResponse<'a> {
    pub const ACCEPT_FIXED_LEN: usize = RESPONSE_FIXED_LEN + MAC_ADDRESS_LEN;
    pub fn parse(bytes: &'a [u8]) -> Result<Self, WireError> {
        header(bytes, WNM_CATEGORY, BTM_RESPONSE_ACTION, RESPONSE_FIXED_LEN)?;
        let mut fields = BodyReader::new(&bytes[super::DIALOG_TOKEN_OFFSET..]);
        let dialog_token = fields.u8()?;
        let status = BtmStatus(fields.u8()?);
        let termination_delay_minutes = fields.u8()?;
        let target_bssid = if status == BtmStatus::ACCEPT {
            Some(fields.array()?)
        } else {
            None
        };
        let response = Self {
            dialog_token,
            status,
            termination_delay_minutes,
            target_bssid,
            elements: Elements::parse(fields.remaining())?,
        };
        response.validate()?;
        Ok(response)
    }
    pub fn validate(self) -> Result<(), WireError> {
        token(self.dialog_token)?;
        if (self.status == BtmStatus::ACCEPT) != self.target_bssid.is_some() {
            return Err(WireError::InconsistentFields);
        }
        validate_reports(self.elements)
    }
    pub fn encode(self, buffer: &mut [u8]) -> Result<usize, WireError> {
        self.validate()?;
        let len = RESPONSE_FIXED_LEN
            + self.target_bssid.map_or(0, |_| MAC_ADDRESS_LEN)
            + self.elements.as_bytes().len();
        let mut fields = BodyWriter::new(output(buffer, len)?);
        fields.put(&[
            WNM_CATEGORY,
            BTM_RESPONSE_ACTION,
            self.dialog_token,
            self.status.0,
            self.termination_delay_minutes,
        ]);
        if let Some(target) = self.target_bssid {
            fields.put(&target);
        }
        fields.put(self.elements.as_bytes());
        Ok(len)
    }
}
