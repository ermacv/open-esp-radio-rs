//! Standalone Traffic Filter Service actions and WNM Sleep elements.
use super::codec::{BodyReader, BodyWriter};
use super::management::{action_elements, encode_action, encode_element};
use super::{
    Elements, MAX_ELEMENT_BODY_LEN, WireError, WnmAction, element_id, output,
    validate_tclas_elements,
};
use core::mem::size_of;
const REQUEST_FIXED_LEN: usize = 2 * size_of::<u8>();
const STATUS_BODY_LEN: usize = 2 * size_of::<u8>();
const NOTIFY_HEADER_LEN: usize = 3 * size_of::<u8>();
pub const MAX_TFS_NOTIFY_IDS: usize = u8::MAX as usize;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TfsRequestFrame<'a> {
    /// Unlike Event/Diagnostic requests, the TFS dialog permits zero.
    pub dialog_token: u8,
    /// Empty requests cancel all filters; other complete elements are retained.
    pub elements: Elements<'a>,
}
impl<'a> TfsRequestFrame<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, WireError> {
        let (dialog_token, elements) = action_elements(bytes, WnmAction::TfsRequest, true)?;
        let value = Self {
            dialog_token,
            elements,
        };
        validate_tfs_requests(value.elements)?;
        Ok(value)
    }
    pub fn encode(self, buffer: &mut [u8]) -> Result<usize, WireError> {
        validate_tfs_requests(self.elements)?;
        encode_action(
            WnmAction::TfsRequest,
            self.dialog_token,
            self.elements,
            buffer,
        )
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TfsResponseFrame<'a> {
    pub dialog_token: u8,
    pub elements: Elements<'a>,
}
impl<'a> TfsResponseFrame<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, WireError> {
        let (dialog_token, elements) = action_elements(bytes, WnmAction::TfsResponse, true)?;
        let value = Self {
            dialog_token,
            elements,
        };
        validate_tfs_responses(value.elements)?;
        Ok(value)
    }
    pub fn encode(self, buffer: &mut [u8]) -> Result<usize, WireError> {
        validate_tfs_responses(self.elements)?;
        encode_action(
            WnmAction::TfsResponse,
            self.dialog_token,
            self.elements,
            buffer,
        )
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TfsRequest<'a> {
    pub id: u8,
    pub action: TfsAction,
    pub subelements: Elements<'a>,
}
impl<'a> TfsRequest<'a> {
    pub fn parse(body: &'a [u8]) -> Result<Self, WireError> {
        if body.len() < REQUEST_FIXED_LEN {
            return Err(WireError::InvalidElementLength(element_id::TFS_REQUEST));
        }
        let mut fields = BodyReader::new(body);
        let value = Self {
            id: fields.u8()?,
            action: TfsAction(fields.u8()?),
            subelements: Elements::parse(fields.remaining())?,
        };
        value.validate()?;
        Ok(value)
    }
    pub fn validate(self) -> Result<(), WireError> {
        if self.subelements.as_bytes().is_empty() {
            return Err(WireError::InconsistentFields);
        }
        for subelement in self
            .subelements
            .iter()
            .filter(|element| element.id == tfs_subelement_id::FILTER)
        {
            validate_tfs_classifiers(Elements::parse(subelement.body)?)?;
        }
        Ok(())
    }
    pub fn encode(self, buffer: &mut [u8]) -> Result<usize, WireError> {
        self.validate()?;
        let len = REQUEST_FIXED_LEN + self.subelements.as_bytes().len();
        if len > MAX_ELEMENT_BODY_LEN {
            return Err(WireError::ElementTooLong);
        }
        let mut body = [0; MAX_ELEMENT_BODY_LEN];
        let mut fields = BodyWriter::new(&mut body[..len]);
        fields.put(&[self.id, self.action.0]);
        fields.put(self.subelements.as_bytes());
        encode_element(element_id::TFS_REQUEST, &body[..len], buffer)
    }
}
fn validate_tfs_classifiers(elements: Elements<'_>) -> Result<(), WireError> {
    validate_tclas_elements(elements)?;
    if !elements
        .iter()
        .any(|element| element.id == element_id::TCLAS)
    {
        return Err(WireError::InconsistentFields);
    }
    Ok(())
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TfsStatus {
    pub status: TfsStatusCode,
    pub id: u8,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TfsResponse<'a> {
    pub subelements: Elements<'a>,
}
impl<'a> TfsResponse<'a> {
    pub fn parse(body: &'a [u8]) -> Result<Self, WireError> {
        let value = Self {
            subelements: Elements::parse(body)?,
        };
        value.validate()?;
        Ok(value)
    }
    pub fn validate(self) -> Result<(), WireError> {
        if self.subelements.as_bytes().is_empty() {
            return Err(WireError::InconsistentFields);
        }
        for element in self.subelements.iter() {
            match element.id {
                tfs_subelement_id::STATUS if element.body.len() != STATUS_BODY_LEN => {
                    return Err(WireError::InvalidElementLength(tfs_subelement_id::STATUS));
                }
                tfs_subelement_id::ALTERNATIVE_CLASSIFIERS => {
                    validate_tfs_classifiers(Elements::parse(element.body)?)?
                }
                _ => {}
            }
        }
        Ok(())
    }
    pub fn statuses(self) -> Result<impl Iterator<Item = TfsStatus> + 'a, WireError> {
        self.validate()?;
        Ok(self
            .subelements
            .iter()
            .filter(|element| element.id == tfs_subelement_id::STATUS)
            .map(|element| TfsStatus {
                status: TfsStatusCode(element.body[0]),
                id: element.body[1],
            }))
    }
    pub fn encode(self, buffer: &mut [u8]) -> Result<usize, WireError> {
        self.validate()?;
        encode_element(
            element_id::TFS_RESPONSE,
            self.subelements.as_bytes(),
            buffer,
        )
    }
}
pub fn validate_tfs_requests(elements: Elements<'_>) -> Result<(), WireError> {
    for (index, element) in elements
        .iter()
        .enumerate()
        .filter(|(_, element)| element.id == element_id::TFS_REQUEST)
    {
        let request = TfsRequest::parse(element.body)?;
        if elements.iter().take(index).any(|earlier| {
            earlier.id == element_id::TFS_REQUEST && earlier.body.first() == Some(&request.id)
        }) {
            return Err(WireError::DuplicateElement(element_id::TFS_REQUEST));
        }
    }
    Ok(())
}
pub fn validate_tfs_responses(elements: Elements<'_>) -> Result<(), WireError> {
    for element in elements
        .iter()
        .filter(|element| element.id == element_id::TFS_RESPONSE)
    {
        TfsResponse::parse(element.body)?;
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TfsNotify<'a> {
    pub ids: &'a [u8],
}
impl<'a> TfsNotify<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, WireError> {
        if bytes.len() < NOTIFY_HEADER_LEN {
            return Err(WireError::Truncated);
        }
        let mut fields = BodyReader::new(bytes);
        // Notify carries a count where other WNM actions carry a dialog token.
        if fields.array()? != [super::WNM_CATEGORY, WnmAction::TfsNotify as u8] {
            return Err(WireError::WrongAction);
        }
        let count = usize::from(fields.u8()?);
        if count == 0 || fields.remaining().len() != count {
            return Err(WireError::InconsistentFields);
        }
        let value = Self {
            ids: fields.remaining(),
        };
        value.validate()?;
        Ok(value)
    }
    pub fn validate(self) -> Result<(), WireError> {
        if self.ids.is_empty() || self.ids.len() > MAX_TFS_NOTIFY_IDS {
            return Err(WireError::InconsistentFields);
        }
        for (index, id) in self.ids.iter().enumerate() {
            if self.ids[..index].contains(id) {
                return Err(WireError::DuplicateElement(*id));
            }
        }
        Ok(())
    }
    pub fn encode(self, buffer: &mut [u8]) -> Result<usize, WireError> {
        self.validate()?;
        let out = output(buffer, NOTIFY_HEADER_LEN + self.ids.len())?;
        out[..NOTIFY_HEADER_LEN].copy_from_slice(&[
            super::WNM_CATEGORY,
            WnmAction::TfsNotify as u8,
            self.ids.len() as u8,
        ]);
        out[NOTIFY_HEADER_LEN..].copy_from_slice(self.ids);
        Ok(NOTIFY_HEADER_LEN + self.ids.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tfs_nested_validation_preserves_vendor_data_and_notify_count() {
        let wire = [7, 3, 1, 7, 14, 5, 0, 3, 0, 0, 0, 221, 2, 1, 2];
        let request = TfsRequest::parse(&wire).unwrap();
        assert_eq!(request.subelements.unique(221).unwrap(), Some(&[1, 2][..]));
        assert!(TfsRequest::parse(&[7, 3, 1, 1, 14]).is_err());
        let response = TfsResponse::parse(&[1, 2, 0, 7, 221, 1, 9]).unwrap();
        assert_eq!(
            response.statuses().unwrap().next(),
            Some(TfsStatus {
                status: TfsStatusCode::ACCEPT,
                id: 7
            })
        );
        assert!(TfsResponse::parse(&[1, 1, 0]).is_err());
        let mut out = [0; 4];
        assert_eq!(TfsNotify { ids: &[7] }.encode(&mut out).unwrap(), 4);
        assert_eq!(TfsNotify::parse(&out).unwrap().ids, [7]);
        assert!(TfsNotify::parse(&[10, 15, 2, 7]).is_err());
    }
    #[test]
    fn notify_count_is_validated_independently_of_dialog_tokens() {
        assert_eq!(
            TfsNotify::parse(&[10, 15, 0]),
            Err(WireError::InconsistentFields)
        );
        assert_eq!(TfsNotify::parse(&[10, 15]), Err(WireError::Truncated));
        assert_eq!(
            TfsNotify::parse(&[10, 14, 1, 7]),
            Err(WireError::WrongAction)
        );
        assert_eq!(TfsNotify::parse(&[10, 15, 1, 0]).unwrap().ids, [0]);
    }
    #[test]
    fn standalone_frames_preserve_optional_elements_and_allow_zero_dialog_cancellation() {
        let request = [
            10, 13, 0, 91, 11, 7, 3, 1, 7, 14, 5, 0, 3, 0, 0, 0, 221, 1, 9,
        ];
        let view = TfsRequestFrame::parse(&request).unwrap();
        let mut out = [0; 32];
        let n = view.encode(&mut out).unwrap();
        assert_eq!(&out[..n], request);
        assert_eq!(
            TfsRequestFrame::parse(&[10, 13, 0]).unwrap().elements,
            Elements::EMPTY
        );
        let response = [10, 14, 0, 92, 4, 1, 2, 0, 7, 221, 1, 9];
        let view = TfsResponseFrame::parse(&response).unwrap();
        let n = view.encode(&mut out).unwrap();
        assert_eq!(&out[..n], response);
        let mut small = [0xaa; 3];
        assert!(view.encode(&mut small).is_err());
        assert_eq!(small, [0xaa; 3]);
        let notice = TfsNotify { ids: &[7, 7] };
        let mut unchanged = [0xaa; 5];
        assert!(notice.encode(&mut unchanged).is_err());
        assert_eq!(unchanged, [0xaa; 5]);
        assert!(TfsNotify::parse(&[10, 15, 2, 7, 7]).is_err());
    }
}

/// Delete and Notify flags, IEEE 802.11-2012 Table 8-162.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TfsAction(pub u8);
impl TfsAction {
    pub const NONE: Self = Self(0);
    pub const DELETE_AFTER_MATCH: Self = Self(1);
    pub const NOTIFY: Self = Self(2);
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
    pub const fn contains(self, flag: Self) -> bool {
        self.0 & flag.0 == flag.0
    }
    pub const fn unsupported_bits(self) -> u8 {
        self.0 & !(Self::DELETE_AFTER_MATCH.0 | Self::NOTIFY.0)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TfsStatusCode(pub u8);
impl TfsStatusCode {
    pub const ACCEPT: Self = Self(0);
    pub const DENY_FORMAT: Self = Self(1);
    pub const DENY_RESOURCES: Self = Self(2);
    pub const DENY_CONFLICTING_STREAMS: Self = Self(3);
    pub const DENY_POLICY: Self = Self(4);
    pub const DENY_UNSPECIFIED: Self = Self(5);
    pub const ALTERNATE_EXISTING_INTERVAL: Self = Self(6);
    pub const ALTERNATE_POLICY: Self = Self(7);
    pub const ALTERNATE_INTERVAL: Self = Self(8);
    pub const ALTERNATE_MULTICAST_RATE: Self = Self(9);
    pub const TERMINATE_POLICY: Self = Self(10);
    pub const TERMINATE_RESOURCES: Self = Self(11);
    pub const TERMINATE_PRIORITY: Self = Self(12);
    pub const ALTERNATE_MAX_INTERVAL: Self = Self(13);
    pub const ALTERNATE_CLASSIFIER: Self = Self(14);
    pub const fn is_known(self) -> bool {
        self.0 <= Self::ALTERNATE_CLASSIFIER.0
    }
}
pub mod tfs_subelement_id {
    pub const FILTER: u8 = 1;
    pub const STATUS: u8 = 1;
    pub const ALTERNATIVE_CLASSIFIERS: u8 = 2;
}
