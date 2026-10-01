//! Directed Multicast Service descriptors; TCLAS is shared with TFS.
use super::element_id;
use super::management::{action_elements, encode_action};
use super::{
    BodyReader, BodyWriter, ELEMENT_HEADER_LEN, Elements, MAX_ELEMENT_BODY_LEN, WireError,
    WnmAction, output, token,
};
use crate::sequence::SequenceNumber;
use core::mem::size_of;
const DESCRIPTOR_FIXED_LEN: usize = size_of::<u8>();
const STATUS_FIXED_LEN: usize = size_of::<u8>() + size_of::<u16>();
pub const DMS_UNASSIGNED_ID: u8 = 0;
pub const DMS_REQUEST_ELEMENT_ID: u8 = element_id::DMS_REQUEST;
pub const DMS_RESPONSE_ELEMENT_ID: u8 = element_id::DMS_RESPONSE;
pub const TCLAS_ELEMENT_ID: u8 = element_id::TCLAS;
pub const TCLAS_PROCESSING_ELEMENT_ID: u8 = element_id::TCLAS_PROCESSING;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DmsRequestType(pub u8);
impl DmsRequestType {
    pub const ADD: Self = Self(0);
    pub const REMOVE: Self = Self(1);
    pub const CHANGE: Self = Self(2);
    pub const fn is_known(self) -> bool {
        matches!(self, Self::ADD | Self::REMOVE | Self::CHANGE)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DmsResponseType(pub u8);
impl DmsResponseType {
    pub const ACCEPT: Self = Self(0);
    pub const DENIED: Self = Self(1);
    pub const TERMINATE: Self = Self(2);
    pub const fn is_known(self) -> bool {
        matches!(self, Self::ACCEPT | Self::DENIED | Self::TERMINATE)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LastSequenceControl {
    Unsupported,
    /// The last individually delivered frame was never sent as a group frame.
    NotGroupTransmitted,
    Sequence(u16),
}
impl LastSequenceControl {
    pub const UNSUPPORTED_VALUE: u16 = u16::MAX;
    pub const NOT_GROUP_TRANSMITTED_VALUE: u16 = u16::MAX - 1;
    pub fn parse(value: u16) -> Result<Self, WireError> {
        match value {
            Self::UNSUPPORTED_VALUE => Ok(Self::Unsupported),
            Self::NOT_GROUP_TRANSMITTED_VALUE => Ok(Self::NotGroupTransmitted),
            value => {
                let sequence = SequenceNumber::from_sequence_control(value);
                if sequence.sequence_control() != value {
                    return Err(WireError::InconsistentFields);
                }
                Ok(Self::Sequence(sequence.get()))
            }
        }
    }
    pub fn encode(self) -> Result<u16, WireError> {
        match self {
            Self::Unsupported => Ok(Self::UNSUPPORTED_VALUE),
            Self::NotGroupTransmitted => Ok(Self::NOT_GROUP_TRANSMITTED_VALUE),
            Self::Sequence(seq) => SequenceNumber::new(seq)
                .map(SequenceNumber::sequence_control)
                .ok_or(WireError::InconsistentFields),
        }
    }
}

/// Validate all classifier fields while preserving unknown types and the
/// complete optional TSPEC/vendor elements. No resource admission is implied.
pub fn validate_dms_attributes(elements: Elements<'_>) -> Result<(), WireError> {
    super::validate_tclas_elements(elements)?;
    const TSPEC_BODY_LEN: usize = 55; // IEEE 802.11-2012, 8.4.2.32.
    if elements
        .unique(element_id::TSPEC)?
        .is_some_and(|body| body.len() != TSPEC_BODY_LEN)
    {
        return Err(WireError::InvalidElementLength(element_id::TSPEC));
    }
    Ok(())
}
/// Classifier identity is independent of TCLAS ordering. Multiplicity, User
/// Priority, masks and optional processing are part of the identity. QoS/vendor
/// attributes remain separately retained on each subscription.
pub fn equivalent_dms_classifiers(a: Elements<'_>, b: Elements<'_>) -> Result<bool, WireError> {
    validate_dms_attributes(a)?;
    validate_dms_attributes(b)?;
    if a.unique(element_id::TCLAS_PROCESSING)? != b.unique(element_id::TCLAS_PROCESSING)? {
        return Ok(false);
    }
    let count_a = a
        .iter()
        .filter(|element| element.id == TCLAS_ELEMENT_ID)
        .count();
    let count_b = b
        .iter()
        .filter(|element| element.id == TCLAS_ELEMENT_ID)
        .count();
    if count_a != count_b {
        return Ok(false);
    }
    for element in a.iter().filter(|element| element.id == TCLAS_ELEMENT_ID) {
        let first = a
            .iter()
            .filter(|other| other.id == TCLAS_ELEMENT_ID && other.body == element.body)
            .count();
        let second = b
            .iter()
            .filter(|other| other.id == TCLAS_ELEMENT_ID && other.body == element.body)
            .count();
        if first != second {
            return Ok(false);
        }
    }
    Ok(true)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DmsDescriptor<'a> {
    pub dms_id: u8,
    pub request_type: DmsRequestType,
    pub attributes: Elements<'a>,
}
impl<'a> DmsDescriptor<'a> {
    pub fn parse(id: u8, body: &'a [u8]) -> Result<Self, WireError> {
        let mut fields = BodyReader::new(body);
        let value = Self {
            dms_id: id,
            request_type: DmsRequestType(fields.u8()?),
            attributes: Elements::parse(fields.remaining())?,
        };
        value.validate()?;
        Ok(value)
    }
    pub fn validate(self) -> Result<(), WireError> {
        validate_dms_attributes(self.attributes)?;
        let count = self
            .attributes
            .iter()
            .filter(|element| element.id == TCLAS_ELEMENT_ID)
            .count();
        // 8.4.2.85: Add has Processing exactly when multiple TCLAS are present.
        if self.request_type == DmsRequestType::ADD
            && (count > 1)
                != self
                    .attributes
                    .unique(element_id::TCLAS_PROCESSING)?
                    .is_some()
        {
            return Err(WireError::InconsistentFields);
        }
        match self.request_type {
            DmsRequestType::ADD if self.dms_id != DMS_UNASSIGNED_ID || count == 0 => {
                Err(WireError::InconsistentFields)
            }
            DmsRequestType::REMOVE
                if self.dms_id == DMS_UNASSIGNED_ID || !self.attributes.as_bytes().is_empty() =>
            {
                Err(WireError::InconsistentFields)
            }
            DmsRequestType::CHANGE
                if self.dms_id == DMS_UNASSIGNED_ID
                    || count != 0
                    || self
                        .attributes
                        .unique(element_id::TCLAS_PROCESSING)?
                        .is_some() =>
            {
                Err(WireError::InconsistentFields)
            }
            _ => Ok(()),
        }
    }
    pub fn encode(self, buffer: &mut [u8]) -> Result<usize, WireError> {
        self.validate()?;
        let body_len = DESCRIPTOR_FIXED_LEN + self.attributes.as_bytes().len();
        if body_len > MAX_ELEMENT_BODY_LEN {
            return Err(WireError::ElementTooLong);
        }
        let len = ELEMENT_HEADER_LEN + body_len;
        let mut fields = BodyWriter::new(output(buffer, len)?);
        fields.put(&[self.dms_id, body_len as u8, self.request_type.0]);
        fields.put(self.attributes.as_bytes());
        Ok(len)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DmsStatus<'a> {
    pub dms_id: u8,
    pub response_type: DmsResponseType,
    pub last_sequence_control: u16,
    pub attributes: Elements<'a>,
}
impl<'a> DmsStatus<'a> {
    pub const FIXED_ENCODED_LEN: usize = ELEMENT_HEADER_LEN + STATUS_FIXED_LEN;
    pub fn parse(id: u8, body: &'a [u8]) -> Result<Self, WireError> {
        let mut fields = BodyReader::new(body);
        let value = Self {
            dms_id: id,
            response_type: DmsResponseType(fields.u8()?),
            last_sequence_control: u16::from_le_bytes(fields.array()?),
            attributes: Elements::parse(fields.remaining())?,
        };
        value.validate()?;
        Ok(value)
    }
    pub fn validate(self) -> Result<(), WireError> {
        validate_dms_attributes(self.attributes)?;
        // 8.4.2.86: response Processing is optional for multiple classifiers,
        // and absent for a single classifier. Request requirements differ.
        if self
            .attributes
            .iter()
            .filter(|element| element.id == TCLAS_ELEMENT_ID)
            .count()
            == 1
            && self
                .attributes
                .unique(element_id::TCLAS_PROCESSING)?
                .is_some()
        {
            return Err(WireError::InconsistentFields);
        }
        if self.response_type == DmsResponseType::TERMINATE {
            if self.dms_id == DMS_UNASSIGNED_ID || !self.attributes.as_bytes().is_empty() {
                return Err(WireError::InconsistentFields);
            }
            LastSequenceControl::parse(self.last_sequence_control)?;
        }
        if self.response_type == DmsResponseType::ACCEPT && self.dms_id == DMS_UNASSIGNED_ID {
            return Err(WireError::InconsistentFields);
        }
        Ok(())
    }
    pub fn encode(self, buffer: &mut [u8]) -> Result<usize, WireError> {
        self.validate()?;
        let body_len = STATUS_FIXED_LEN + self.attributes.as_bytes().len();
        if body_len > MAX_ELEMENT_BODY_LEN {
            return Err(WireError::ElementTooLong);
        }
        let len = ELEMENT_HEADER_LEN + body_len;
        let mut fields = BodyWriter::new(output(buffer, len)?);
        fields.put(&[self.dms_id, body_len as u8, self.response_type.0]);
        fields.put(&self.last_sequence_control.to_le_bytes());
        fields.put(self.attributes.as_bytes());
        Ok(len)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DmsRequest<'a> {
    pub dialog_token: u8,
    pub elements: Elements<'a>,
}
impl<'a> DmsRequest<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, WireError> {
        let (dialog_token, elements) = action_elements(bytes, WnmAction::DmsRequest, false)?;
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
        for element in self
            .elements
            .iter()
            .filter(|element| element.id == element_id::DMS_REQUEST)
        {
            for descriptor in Elements::parse(element.body)?.iter() {
                DmsDescriptor::parse(descriptor.id, descriptor.body)?;
                count += 1;
            }
        }
        if count == 0 {
            return Err(WireError::InconsistentFields);
        }
        Ok(())
    }
    pub fn descriptors(self) -> Result<impl Iterator<Item = DmsDescriptor<'a>>, WireError> {
        self.validate()?;
        Ok(self
            .elements
            .iter()
            .filter(|element| element.id == element_id::DMS_REQUEST)
            .flat_map(|element| {
                Elements::parse(element.body)
                    .expect("validated descriptors")
                    .iter()
            })
            .map(|element| {
                DmsDescriptor::parse(element.id, element.body).expect("validated descriptor")
            }))
    }
    pub fn encode(self, buffer: &mut [u8]) -> Result<usize, WireError> {
        self.validate()?;
        encode_action(
            WnmAction::DmsRequest,
            self.dialog_token,
            self.elements,
            buffer,
        )
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DmsResponse<'a> {
    pub dialog_token: u8,
    pub elements: Elements<'a>,
}
impl<'a> DmsResponse<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, WireError> {
        let (dialog_token, elements) = action_elements(bytes, WnmAction::DmsResponse, true)?;
        let value = Self {
            dialog_token,
            elements,
        };
        value.validate()?;
        Ok(value)
    }
    pub fn validate(self) -> Result<(), WireError> {
        let mut count = 0;
        for element in self
            .elements
            .iter()
            .filter(|element| element.id == element_id::DMS_RESPONSE)
        {
            for status in Elements::parse(element.body)?.iter() {
                DmsStatus::parse(status.id, status.body)?;
                count += 1;
            }
        }
        if count == 0 {
            return Err(WireError::InconsistentFields);
        }
        Ok(())
    }
    pub fn statuses(self) -> Result<impl Iterator<Item = DmsStatus<'a>>, WireError> {
        self.validate()?;
        Ok(self
            .elements
            .iter()
            .filter(|element| element.id == element_id::DMS_RESPONSE)
            .flat_map(|element| {
                Elements::parse(element.body)
                    .expect("validated statuses")
                    .iter()
            })
            .map(|element| DmsStatus::parse(element.id, element.body).expect("validated status")))
    }
    pub fn encode(self, buffer: &mut [u8]) -> Result<usize, WireError> {
        self.validate()?;
        encode_action(
            WnmAction::DmsResponse,
            self.dialog_token,
            self.elements,
            buffer,
        )
    }
}

#[cfg(test)]
mod tests;
