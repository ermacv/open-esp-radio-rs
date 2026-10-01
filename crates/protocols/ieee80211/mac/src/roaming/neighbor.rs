use super::{
    ACTION_HEADER_LEN, BodyReader, BodyWriter, DIALOG_TOKEN_OFFSET, ELEMENT_HEADER_LEN, Elements,
    MAC_ADDRESS_LEN, MAX_ELEMENT_BODY_LEN, MacAddress, RADIO_MEASUREMENT_CATEGORY, RrmAction,
    WireError, element_id, header, output, token,
};
use crate::management::MAX_SSID_LEN;
use core::mem::size_of;

const NEIGHBOR_FIXED_LEN: usize = MAC_ADDRESS_LEN + size_of::<u32>() + 3 * size_of::<u8>();

pub const NEIGHBOR_REPORT_REQUEST_ACTION: u8 = RrmAction::NeighborRequest as u8;
pub const NEIGHBOR_REPORT_RESPONSE_ACTION: u8 = RrmAction::NeighborResponse as u8;
pub const NEIGHBOR_REPORT_ELEMENT_ID: u8 = element_id::NEIGHBOR_REPORT;
pub const CANDIDATE_PREFERENCE_SUBELEMENT_ID: u8 = 3;

/// Neighbor Report fields and all of their optional subelements.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NeighborReport<'a> {
    pub bssid: MacAddress,
    pub bssid_information: u32,
    pub operating_class: u8,
    pub channel_number: u8,
    pub phy_type: u8,
    pub subelements: Elements<'a>,
}

impl<'a> NeighborReport<'a> {
    pub fn parse(body: &'a [u8]) -> Result<Self, WireError> {
        if body.len() < NEIGHBOR_FIXED_LEN {
            return Err(WireError::InvalidElementLength(NEIGHBOR_REPORT_ELEMENT_ID));
        }
        let mut fields = BodyReader::new(body);
        let report = Self {
            bssid: fields.array()?,
            bssid_information: u32::from_le_bytes(fields.array()?),
            operating_class: fields.u8()?,
            channel_number: fields.u8()?,
            phy_type: fields.u8()?,
            subelements: Elements::parse(fields.remaining())?,
        };
        report.validate()?;
        Ok(report)
    }

    pub fn preference(self) -> Result<Option<u8>, WireError> {
        self.subelements
            .unique(CANDIDATE_PREFERENCE_SUBELEMENT_ID)?
            .map(|value| {
                if value.len() != size_of::<u8>() {
                    Err(WireError::InvalidElementLength(
                        CANDIDATE_PREFERENCE_SUBELEMENT_ID,
                    ))
                } else {
                    Ok(value[0])
                }
            })
            .transpose()
    }

    pub fn validate(self) -> Result<(), WireError> {
        if NEIGHBOR_FIXED_LEN + self.subelements.as_bytes().len() > MAX_ELEMENT_BODY_LEN {
            return Err(WireError::ElementTooLong);
        }
        self.preference()?;
        Ok(())
    }

    pub fn encode(self, buffer: &mut [u8]) -> Result<usize, WireError> {
        self.validate()?;
        let body_len = NEIGHBOR_FIXED_LEN + self.subelements.as_bytes().len();
        let len = ELEMENT_HEADER_LEN + body_len;
        let mut fields = BodyWriter::new(output(buffer, len)?);
        fields.put(&[NEIGHBOR_REPORT_ELEMENT_ID, body_len as u8]);
        fields.put(&self.bssid);
        fields.put(&self.bssid_information.to_le_bytes());
        fields.put(&[self.operating_class, self.channel_number, self.phy_type]);
        fields.put(self.subelements.as_bytes());
        Ok(len)
    }
}

pub(super) fn validate_reports(elements: Elements<'_>) -> Result<(), WireError> {
    for element in elements
        .iter()
        .filter(|element| element.id == NEIGHBOR_REPORT_ELEMENT_ID)
    {
        NeighborReport::parse(element.body)?;
    }
    Ok(())
}

/// An SSID of zero length is retained; an AP applies its own-BSS default.
/// Optional measurement requests and extension IEs remain in `elements`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NeighborReportRequest<'a> {
    pub dialog_token: u8,
    pub elements: Elements<'a>,
}

impl<'a> NeighborReportRequest<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, WireError> {
        header(
            bytes,
            RADIO_MEASUREMENT_CATEGORY,
            NEIGHBOR_REPORT_REQUEST_ACTION,
            ACTION_HEADER_LEN,
        )?;
        let request = Self {
            dialog_token: bytes[DIALOG_TOKEN_OFFSET],
            elements: Elements::parse(&bytes[ACTION_HEADER_LEN..])?,
        };
        request.validate()?;
        Ok(request)
    }
    pub fn ssid(self) -> Result<Option<&'a [u8]>, WireError> {
        self.elements.unique(element_id::SSID)
    }
    pub fn validate(self) -> Result<(), WireError> {
        token(self.dialog_token)?;
        if self.ssid()?.is_some_and(|ssid| ssid.len() > MAX_SSID_LEN) {
            return Err(WireError::SsidTooLong);
        }
        Ok(())
    }
    pub fn encode(self, buffer: &mut [u8]) -> Result<usize, WireError> {
        self.validate()?;
        let len = ACTION_HEADER_LEN + self.elements.as_bytes().len();
        let bytes = output(buffer, len)?;
        bytes[..ACTION_HEADER_LEN].copy_from_slice(&[
            RADIO_MEASUREMENT_CATEGORY,
            NEIGHBOR_REPORT_REQUEST_ACTION,
            self.dialog_token,
        ]);
        bytes[ACTION_HEADER_LEN..].copy_from_slice(self.elements.as_bytes());
        Ok(len)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NeighborReportResponse<'a> {
    pub dialog_token: u8,
    pub elements: Elements<'a>,
}

impl<'a> NeighborReportResponse<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, WireError> {
        header(
            bytes,
            RADIO_MEASUREMENT_CATEGORY,
            NEIGHBOR_REPORT_RESPONSE_ACTION,
            ACTION_HEADER_LEN,
        )?;
        let response = Self {
            dialog_token: bytes[DIALOG_TOKEN_OFFSET],
            elements: Elements::parse(&bytes[ACTION_HEADER_LEN..])?,
        };
        response.validate()?;
        Ok(response)
    }
    pub fn validate(self) -> Result<(), WireError> {
        token(self.dialog_token)?;
        validate_reports(self.elements)
    }
    pub fn reports(self) -> impl Iterator<Item = Result<NeighborReport<'a>, WireError>> {
        self.elements
            .iter()
            .filter(|element| element.id == NEIGHBOR_REPORT_ELEMENT_ID)
            .map(|element| NeighborReport::parse(element.body))
    }
    pub fn encode(self, buffer: &mut [u8]) -> Result<usize, WireError> {
        self.validate()?;
        let len = ACTION_HEADER_LEN + self.elements.as_bytes().len();
        let bytes = output(buffer, len)?;
        bytes[..ACTION_HEADER_LEN].copy_from_slice(&[
            RADIO_MEASUREMENT_CATEGORY,
            NEIGHBOR_REPORT_RESPONSE_ACTION,
            self.dialog_token,
        ]);
        bytes[ACTION_HEADER_LEN..].copy_from_slice(self.elements.as_bytes());
        Ok(len)
    }
}
