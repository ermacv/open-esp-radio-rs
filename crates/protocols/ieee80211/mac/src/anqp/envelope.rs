use super::*;

pub const ANQP_HEADER_LEN: usize = 2 * core::mem::size_of::<u16>();
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct InfoId(pub u16);
impl InfoId {
    pub const QUERY_LIST: Self = Self(256);
    pub const CAPABILITY_LIST: Self = Self(257);
    pub const VENUE_NAME: Self = Self(258);
    pub const EMERGENCY_CALL_NUMBER: Self = Self(259);
    pub const NETWORK_AUTHENTICATION: Self = Self(260);
    pub const ROAMING_CONSORTIUM: Self = Self(261);
    pub const IP_ADDRESS_AVAILABILITY: Self = Self(262);
    pub const NAI_REALM: Self = Self(263);
    pub const CELLULAR_NETWORK: Self = Self(264);
    pub const GEOSPATIAL_LOCATION: Self = Self(265);
    pub const CIVIC_LOCATION: Self = Self(266);
    pub const LOCATION_URI: Self = Self(267);
    pub const DOMAIN_NAME: Self = Self(268);
    pub const EMERGENCY_ALERT_URI: Self = Self(269);
    pub const TDLS_CAPABILITY: Self = Self(270);
    pub const EMERGENCY_NAI: Self = Self(271);
    pub const NEIGHBOR_REPORT: Self = Self(272);
    pub const VENUE_URL: Self = Self(277);
    pub const VENDOR: Self = Self(56797);
}

/// ANQP uses a 16-bit ID/Length envelope; it is not an 8-bit management IE.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Element<'a> {
    pub id: InfoId,
    pub body: &'a [u8],
}
impl<'a> Element<'a> {
    pub fn value(self) -> Result<Value<'a>, WireError> {
        Value::parse(self)
    }
    pub fn encoded_len(self) -> Result<usize, WireError> {
        if self.body.len() > u16::MAX as usize {
            return Err(WireError::ElementTooLong);
        }
        self.value()?;
        Ok(ANQP_HEADER_LEN + self.body.len())
    }
    pub fn encode(self, bytes: &mut [u8]) -> Result<usize, WireError> {
        let length = self.encoded_len()?;
        self.write(&mut BodyWriter::new(output(bytes, length)?));
        Ok(length)
    }
    pub(super) fn write(self, fields: &mut BodyWriter<'_>) {
        fields.put(&self.id.0.to_le_bytes());
        fields.put(&(self.body.len() as u16).to_le_bytes());
        fields.put(self.body);
    }
}

/// Complete borrowed envelope list. Values are validated separately so unknown
/// extensions and future 3GPP payloads can be retained without reinterpretation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Elements<'a>(&'a [u8]);
impl<'a> Elements<'a> {
    pub const EMPTY: Self = Self(&[]);
    pub fn parse(bytes: &'a [u8]) -> Result<Self, WireError> {
        let mut fields = BodyReader::new(bytes);
        while !fields.remaining().is_empty() {
            read_element(&mut fields)?;
        }
        Ok(Self(bytes))
    }
    pub const fn as_bytes(self) -> &'a [u8] {
        self.0
    }
    pub fn iter(self) -> impl Iterator<Item = Element<'a>> + Clone {
        let mut fields = BodyReader::new(self.0);
        core::iter::from_fn(move || {
            if fields.remaining().is_empty() {
                None
            } else {
                Some(read_element(&mut fields).expect("validated ANQP envelopes"))
            }
        })
    }
    pub fn validate(self) -> Result<(), WireError> {
        // Current base singleton IDs fit in one small set. Unknown extension
        // cardinality is not guessed; vendor envelopes remain repeatable.
        let mut seen = 0u32;
        for element in self.iter() {
            element.value()?;
            let id = element.id;
            if (InfoId::QUERY_LIST..=InfoId::NEIGHBOR_REPORT).contains(&id)
                || id == InfoId::VENUE_URL
            {
                let bit = 1u32 << (id.0 - InfoId::QUERY_LIST.0);
                if seen & bit != 0 {
                    return Err(WireError::Duplicate(id));
                }
                seen |= bit;
            }
        }
        Ok(())
    }
}
fn read_element<'a>(fields: &mut BodyReader<'a>) -> Result<Element<'a>, WireError> {
    let id = InfoId(u16::from_le_bytes(fields.array()?));
    let body = long_bytes(fields)?;
    Ok(Element { id, body })
}

/// Validated, strictly increasing IEEE Query List identifiers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QueryList<'a>(&'a [u8]);
impl<'a> QueryList<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, WireError> {
        if bytes.is_empty() || !bytes.len().is_multiple_of(core::mem::size_of::<u16>()) {
            return Err(WireError::InvalidLength);
        }
        let list = Self(bytes);
        let mut previous = None;
        for id in list.iter() {
            if previous.is_some_and(|old| old >= id) {
                return Err(WireError::UnorderedIds);
            }
            previous = Some(id);
        }
        Ok(list)
    }
    pub fn iter(self) -> impl Iterator<Item = InfoId> + Clone + 'a {
        self.0
            .chunks_exact(core::mem::size_of::<u16>())
            .map(|bytes| {
                InfoId(u16::from_le_bytes(
                    bytes.try_into().expect("identifier width"),
                ))
            })
    }
    pub const fn as_bytes(self) -> &'a [u8] {
        self.0
    }
    pub fn encode(ids: &[InfoId], bytes: &mut [u8]) -> Result<usize, WireError> {
        if ids.is_empty() || ids.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(WireError::UnorderedIds);
        }
        let length = ids
            .len()
            .checked_mul(core::mem::size_of::<u16>())
            .ok_or(WireError::ElementTooLong)?;
        if length > u16::MAX as usize {
            return Err(WireError::ElementTooLong);
        }
        let mut fields = BodyWriter::new(output(bytes, ANQP_HEADER_LEN + length)?);
        fields.put(&InfoId::QUERY_LIST.0.to_le_bytes());
        fields.put(&(length as u16).to_le_bytes());
        for id in ids {
            fields.put(&id.0.to_le_bytes());
        }
        Ok(ANQP_HEADER_LEN + length)
    }
}

/// Capability IDs followed by complete vendor-specific ANQP elements.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CapabilityList<'a> {
    pub ids: &'a [u8],
    pub vendor: Elements<'a>,
}
impl<'a> CapabilityList<'a> {
    /// Encode the complete Capability List element, including nested vendor
    /// ANQP envelopes. Standard IDs precede vendor records and are ordered.
    pub fn encode(
        ids: &[InfoId],
        vendor: Elements<'_>,
        bytes: &mut [u8],
    ) -> Result<usize, WireError> {
        if ids.windows(2).any(|pair| pair[0] >= pair[1]) || ids.contains(&InfoId::VENDOR) {
            return Err(WireError::UnorderedIds);
        }
        for element in vendor.iter() {
            if element.id != InfoId::VENDOR {
                return Err(WireError::InvalidValue);
            }
            element.value()?;
        }
        let length = ids
            .len()
            .checked_mul(core::mem::size_of::<u16>())
            .and_then(|length| length.checked_add(vendor.as_bytes().len()))
            .ok_or(WireError::ElementTooLong)?;
        if length > u16::MAX as usize {
            return Err(WireError::ElementTooLong);
        }
        let total = ANQP_HEADER_LEN + length;
        let mut fields = BodyWriter::new(output(bytes, total)?);
        fields.put(&InfoId::CAPABILITY_LIST.0.to_le_bytes());
        fields.put(&(length as u16).to_le_bytes());
        for id in ids {
            fields.put(&id.0.to_le_bytes());
        }
        fields.put(vendor.as_bytes());
        Ok(total)
    }
    pub fn parse(bytes: &'a [u8]) -> Result<Self, WireError> {
        let mut fields = BodyReader::new(bytes);
        let mut count = 0;
        let mut previous = None;
        while !fields.remaining().is_empty() {
            let id = InfoId(u16::from_le_bytes(
                fields
                    .remaining()
                    .get(..core::mem::size_of::<u16>())
                    .ok_or(WireError::Truncated)?
                    .try_into()
                    .expect("identifier width"),
            ));
            if id == InfoId::VENDOR {
                break;
            }
            if previous.is_some_and(|old| old >= id) {
                return Err(WireError::UnorderedIds);
            }
            fields.take(core::mem::size_of::<u16>())?;
            count += core::mem::size_of::<u16>();
            previous = Some(id);
        }
        let vendor = Elements::parse(fields.remaining())?;
        for element in vendor.iter() {
            if element.id != InfoId::VENDOR {
                return Err(WireError::InvalidValue);
            }
            element.value()?;
        }
        Ok(Self {
            ids: &bytes[..count],
            vendor,
        })
    }
    pub fn ids(self) -> impl Iterator<Item = InfoId> + Clone + 'a {
        self.ids
            .chunks_exact(core::mem::size_of::<u16>())
            .map(|bytes| {
                InfoId(u16::from_le_bytes(
                    bytes.try_into().expect("identifier width"),
                ))
            })
    }
}
