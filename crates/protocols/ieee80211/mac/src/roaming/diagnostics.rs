//! Original WNM Diagnostic Request/Report elements. Unknown subelements are
//! retained; accepted work uses the typed fields rather than guessing defaults.
use super::codec::{BodyReader, BodyWriter};
use super::management::{action_elements, encode_action, encode_element};
use super::{EapMethod, Elements, MacAddress, WireError, token};
use super::{MAC_ADDRESS_LEN, MAX_ELEMENT_BODY_LEN, OCTET_VALUE_COUNT, WnmAction, element_id};
use crate::management::MAX_SSID_LEN;
use core::mem::size_of;
const ANTENNA_FIXED_LEN: usize = 2 * size_of::<u8>();
const TX_POWER_FIXED_LEN: usize = 2 * size_of::<u8>();
const ORGANIZATION_IDENTIFIER_LEN: usize = 3;
const EXTENDED_ORGANIZATION_IDENTIFIER_LEN: usize = 5;
const TX_POWER_RANGE_INDICATOR: u8 = 1;
const DIAGNOSTIC_AP_BODY_LEN: usize = MAC_ADDRESS_LEN + 2 * size_of::<u8>();
const REQUEST_FIXED_LEN: usize = 2 * size_of::<u8>() + size_of::<u16>();
const REPORT_FIXED_LEN: usize = 3 * size_of::<u8>();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiagnosticType(pub u8);
impl DiagnosticType {
    pub const CANCEL: Self = Self(0);
    pub const MANUFACTURER: Self = Self(1);
    pub const CONFIGURATION: Self = Self(2);
    pub const ASSOCIATION: Self = Self(3);
    pub const IEEE8021X: Self = Self(4);
    /// Diagnostic Type namespace (Table 8-140), independent of IE identifiers.
    pub const VENDOR: Self = Self(221);
    pub const fn active(self) -> bool {
        matches!(self, Self::ASSOCIATION | Self::IEEE8021X)
    }
    pub const fn requires_no_information(self) -> bool {
        matches!(
            self,
            Self::CANCEL | Self::MANUFACTURER | Self::CONFIGURATION
        )
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiagnosticAp {
    pub bssid: MacAddress,
    pub operating_class: u8,
    pub channel: u8,
}
impl DiagnosticAp {
    pub fn parse(body: &[u8]) -> Result<Self, WireError> {
        if body.len() != DIAGNOSTIC_AP_BODY_LEN {
            return Err(WireError::InvalidElementLength(
                diagnostic_information_id::AP_DESCRIPTOR,
            ));
        }
        let mut fields = BodyReader::new(body);
        Ok(Self {
            bssid: fields.array()?,
            operating_class: fields.u8()?,
            channel: fields.u8()?,
        })
    }
    pub fn encode(self, buffer: &mut [u8]) -> Result<usize, WireError> {
        let mut body = [0; DIAGNOSTIC_AP_BODY_LEN];
        let mut fields = BodyWriter::new(&mut body);
        fields.put(&self.bssid);
        fields.put(&[self.operating_class, self.channel]);
        encode_element(diagnostic_information_id::AP_DESCRIPTOR, &body, buffer)
    }
}
/// All known diagnostic subelements are validated without dropping repetitions
/// (e.g. multiple antennas or collocated radios) or unknown/vendor values.
pub fn validate_diagnostic_information(elements: Elements<'_>) -> Result<(), WireError> {
    for e in elements.iter() {
        let n = e.body.len();
        let valid = match e.id {
            diagnostic_information_id::CREDENTIAL_TYPES => !e.body.is_empty(),
            diagnostic_information_id::COLLOCATED_RADIO_TYPE
            | diagnostic_information_id::DEVICE_TYPE
            | diagnostic_information_id::PROFILE_ID => n == size_of::<u8>(),
            diagnostic_information_id::AKM_SUITE
            | diagnostic_information_id::CIPHER_SUITE
            | diagnostic_information_id::POWER_SAVE_MODE => n == size_of::<u32>(),
            diagnostic_information_id::AP_DESCRIPTOR => n == DIAGNOSTIC_AP_BODY_LEN,
            diagnostic_information_id::ANTENNA_TYPE => {
                n >= ANTENNA_FIXED_LEN && e.body[0] != 0 && e.body[ANTENNA_FIXED_LEN..].is_ascii()
            }
            diagnostic_information_id::EAP_METHOD => {
                EapMethod::parse(e.body)?;
                true
            }
            diagnostic_information_id::FIRMWARE_VERSION
            | diagnostic_information_id::MANUFACTURER_ID
            | diagnostic_information_id::MANUFACTURER_MODEL
            | diagnostic_information_id::MANUFACTURER_SERIAL => {
                !e.body.is_empty() && e.body.is_ascii()
            }
            diagnostic_information_id::CERTIFICATE_ID => {
                !e.body.is_empty() && core::str::from_utf8(e.body).is_ok()
            }
            diagnostic_information_id::MAC_ADDRESS => n == MAC_ADDRESS_LEN,
            diagnostic_information_id::MANUFACTURER_OI => {
                n == ORGANIZATION_IDENTIFIER_LEN || n == EXTENDED_ORGANIZATION_IDENTIFIER_LEN
            }
            diagnostic_information_id::SUPPORTED_OPERATING_CLASSES => {
                let wrapped = Elements::parse(e.body)?;
                wrapped.iter().count() == 1
                    && wrapped
                        .unique(element_id::SUPPORTED_OPERATING_CLASSES)?
                        .is_some_and(|v| !v.is_empty())
            }
            diagnostic_information_id::STATUS_CODE => n == size_of::<u16>(),
            diagnostic_information_id::SSID => n <= MAX_SSID_LEN,
            diagnostic_information_id::TX_POWER => {
                n >= TX_POWER_FIXED_LEN
                    && (e.body[0] != TX_POWER_RANGE_INDICATOR
                        || n == TX_POWER_FIXED_LEN + size_of::<u8>())
            }
            _ => true,
        };
        if !valid {
            return Err(WireError::InvalidElementLength(e.id));
        }
    }
    Ok(())
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiagnosticRequest<'a> {
    pub token: u8,
    pub kind: DiagnosticType,
    pub timeout_seconds: u16,
    pub information: Elements<'a>,
}
impl<'a> DiagnosticRequest<'a> {
    pub const FIXED_ENCODED_LEN: usize = super::ELEMENT_HEADER_LEN + REQUEST_FIXED_LEN;
    pub fn parse(body: &'a [u8]) -> Result<Self, WireError> {
        if body.len() < REQUEST_FIXED_LEN {
            return Err(WireError::InvalidElementLength(
                element_id::DIAGNOSTIC_REQUEST,
            ));
        }
        let mut fields = BodyReader::new(body);
        let value = Self {
            token: fields.u8()?,
            kind: DiagnosticType(fields.u8()?),
            timeout_seconds: u16::from_le_bytes(fields.array()?),
            information: Elements::parse(fields.remaining())?,
        };
        value.validate()?;
        Ok(value)
    }
    pub fn validate(self) -> Result<(), WireError> {
        validate_diagnostic_information(self.information)?;
        if self.kind.requires_no_information() && !self.information.as_bytes().is_empty() {
            return Err(WireError::InconsistentFields);
        }
        if self.kind.active() {
            for id in [
                diagnostic_information_id::AP_DESCRIPTOR,
                diagnostic_information_id::PROFILE_ID,
            ] {
                if self.information.unique(id)?.is_none() {
                    return Err(WireError::InconsistentFields);
                }
            }
            if self.kind == DiagnosticType::IEEE8021X {
                for id in [
                    diagnostic_information_id::EAP_METHOD,
                    diagnostic_information_id::CREDENTIAL_TYPES,
                ] {
                    if self.information.unique(id)?.is_none() {
                        return Err(WireError::InconsistentFields);
                    }
                }
            }
        }
        Ok(())
    }
    pub fn target(self) -> Result<Option<DiagnosticAp>, WireError> {
        self.validate()?;
        self.information
            .unique(diagnostic_information_id::AP_DESCRIPTOR)?
            .map(DiagnosticAp::parse)
            .transpose()
    }
    pub fn profile_id(self) -> Result<Option<u8>, WireError> {
        self.validate()?;
        Ok(self
            .information
            .unique(diagnostic_information_id::PROFILE_ID)?
            .map(|b| b[0]))
    }
    pub fn encode(self, buffer: &mut [u8]) -> Result<usize, WireError> {
        self.validate()?;
        let len = REQUEST_FIXED_LEN + self.information.as_bytes().len();
        if len > MAX_ELEMENT_BODY_LEN {
            return Err(WireError::ElementTooLong);
        }
        let mut body = [0; MAX_ELEMENT_BODY_LEN];
        let mut fields = BodyWriter::new(&mut body[..len]);
        fields.put(&[self.token, self.kind.0]);
        fields.put(&self.timeout_seconds.to_le_bytes());
        fields.put(self.information.as_bytes());
        encode_element(element_id::DIAGNOSTIC_REQUEST, &body[..len], buffer)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiagnosticReport<'a> {
    pub token: u8,
    pub kind: DiagnosticType,
    pub status: DiagnosticReportStatus,
    pub information: Elements<'a>,
}
impl<'a> DiagnosticReport<'a> {
    pub const FIXED_ENCODED_LEN: usize = super::ELEMENT_HEADER_LEN + REPORT_FIXED_LEN;
    pub fn parse(body: &'a [u8]) -> Result<Self, WireError> {
        if body.len() < REPORT_FIXED_LEN {
            return Err(WireError::InvalidElementLength(
                element_id::DIAGNOSTIC_REPORT,
            ));
        }
        let mut fields = BodyReader::new(body);
        let value = Self {
            token: fields.u8()?,
            kind: DiagnosticType(fields.u8()?),
            status: DiagnosticReportStatus(fields.u8()?),
            information: Elements::parse(fields.remaining())?,
        };
        value.validate()?;
        Ok(value)
    }
    pub fn validate(self) -> Result<(), WireError> {
        validate_diagnostic_information(self.information)?;
        if (self.kind == DiagnosticType::CANCEL || self.status == DiagnosticReportStatus::CANCELLED)
            && !self.information.as_bytes().is_empty()
        {
            return Err(WireError::InconsistentFields);
        }
        Ok(())
    }
    pub fn encode(self, buffer: &mut [u8]) -> Result<usize, WireError> {
        self.validate()?;
        let len = REPORT_FIXED_LEN + self.information.as_bytes().len();
        if len > MAX_ELEMENT_BODY_LEN {
            return Err(WireError::ElementTooLong);
        }
        let mut body = [0; MAX_ELEMENT_BODY_LEN];
        let mut fields = BodyWriter::new(&mut body[..len]);
        fields.put(&[self.token, self.kind.0, self.status.0]);
        fields.put(self.information.as_bytes());
        encode_element(element_id::DIAGNOSTIC_REPORT, &body[..len], buffer)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiagnosticRequestFrame<'a> {
    pub dialog_token: u8,
    pub elements: Elements<'a>,
}
impl<'a> DiagnosticRequestFrame<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, WireError> {
        let (dialog_token, elements) = action_elements(bytes, WnmAction::DiagnosticRequest, false)?;
        let value = Self {
            dialog_token,
            elements,
        };
        value.validate()?;
        Ok(value)
    }
    pub fn validate(self) -> Result<(), WireError> {
        token(self.dialog_token)?;
        super::DestinationUri::parse(self.elements)?;
        let mut seen = [false; OCTET_VALUE_COUNT];
        let mut count = 0;
        for e in self
            .elements
            .iter()
            .filter(|e| e.id == element_id::DIAGNOSTIC_REQUEST)
        {
            let r = DiagnosticRequest::parse(e.body)?;
            if seen[usize::from(r.token)] {
                return Err(WireError::DuplicateMeasurementToken(r.token));
            }
            seen[usize::from(r.token)] = true;
            count += 1;
        }
        if count == 0 {
            return Err(WireError::InconsistentFields);
        }
        Ok(())
    }
    pub fn requests(self) -> Result<impl Iterator<Item = DiagnosticRequest<'a>>, WireError> {
        self.validate()?;
        Ok(self
            .elements
            .iter()
            .filter(|e| e.id == element_id::DIAGNOSTIC_REQUEST)
            .map(|e| DiagnosticRequest::parse(e.body).expect("validated diagnostic")))
    }
    pub fn encode(self, out: &mut [u8]) -> Result<usize, WireError> {
        self.validate()?;
        encode_action(
            WnmAction::DiagnosticRequest,
            self.dialog_token,
            self.elements,
            out,
        )
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiagnosticReportFrame<'a> {
    pub dialog_token: u8,
    pub elements: Elements<'a>,
}
impl<'a> DiagnosticReportFrame<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, WireError> {
        let (dialog_token, elements) = action_elements(bytes, WnmAction::DiagnosticReport, false)?;
        let value = Self {
            dialog_token,
            elements,
        };
        value.validate()?;
        Ok(value)
    }
    pub fn validate(self) -> Result<(), WireError> {
        token(self.dialog_token)?;
        let mut count = 0;
        for e in self
            .elements
            .iter()
            .filter(|e| e.id == element_id::DIAGNOSTIC_REPORT)
        {
            DiagnosticReport::parse(e.body)?;
            count += 1;
        }
        if count == 0 {
            return Err(WireError::InconsistentFields);
        }
        Ok(())
    }
    pub fn reports(self) -> Result<impl Iterator<Item = DiagnosticReport<'a>>, WireError> {
        self.validate()?;
        Ok(self
            .elements
            .iter()
            .filter(|e| e.id == element_id::DIAGNOSTIC_REPORT)
            .map(|e| DiagnosticReport::parse(e.body).expect("validated diagnostic")))
    }
    pub fn encode(self, out: &mut [u8]) -> Result<usize, WireError> {
        self.validate()?;
        encode_action(
            WnmAction::DiagnosticReport,
            self.dialog_token,
            self.elements,
            out,
        )
    }
}
#[cfg(test)]
mod tests;

/// Diagnostic status 4 means cancellation, independently of Event status 4.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiagnosticReportStatus(pub u8);
impl DiagnosticReportStatus {
    pub const SUCCESSFUL: Self = Self(0);
    pub const FAILED: Self = Self(1);
    pub const REFUSED: Self = Self(2);
    pub const INCAPABLE: Self = Self(3);
    pub const CANCELLED: Self = Self(4);
    pub const fn is_known(self) -> bool {
        matches!(
            self,
            Self::SUCCESSFUL | Self::FAILED | Self::REFUSED | Self::INCAPABLE | Self::CANCELLED
        )
    }
}
/// Diagnostic Information subelements (IEEE 802.11-2012 Table 8-143).
pub mod diagnostic_information_id {
    pub const CREDENTIAL_TYPES: u8 = 0;
    pub const AKM_SUITE: u8 = 1;
    pub const AP_DESCRIPTOR: u8 = 2;
    pub const ANTENNA_TYPE: u8 = 3;
    pub const CIPHER_SUITE: u8 = 4;
    pub const COLLOCATED_RADIO_TYPE: u8 = 5;
    pub const DEVICE_TYPE: u8 = 6;
    pub const EAP_METHOD: u8 = 7;
    pub const FIRMWARE_VERSION: u8 = 8;
    pub const MAC_ADDRESS: u8 = 9;
    pub const MANUFACTURER_ID: u8 = 10;
    pub const MANUFACTURER_MODEL: u8 = 11;
    pub const MANUFACTURER_OI: u8 = 12;
    pub const MANUFACTURER_SERIAL: u8 = 13;
    pub const POWER_SAVE_MODE: u8 = 14;
    pub const PROFILE_ID: u8 = 15;
    pub const SUPPORTED_OPERATING_CLASSES: u8 = 16;
    pub const STATUS_CODE: u8 = 17;
    pub const SSID: u8 = 18;
    pub const TX_POWER: u8 = 19;
    pub const CERTIFICATE_ID: u8 = 20;
}
