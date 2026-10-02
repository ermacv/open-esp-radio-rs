//! NAI Realm records and EAP authentication parameters. Credential matching
//! belongs to a separate policy owner; this module owns their wire structure.
use super::*;
use crate::management::elements::Elements as Parameters;

pub mod authentication_id {
    pub const EXPANDED_EAP: u8 = 1;
    pub const NON_EAP_INNER: u8 = 2;
    pub const INNER_EAP: u8 = 3;
    pub const EXPANDED_INNER_EAP: u8 = 4;
    pub const CREDENTIAL: u8 = 5;
    pub const TUNNELED_CREDENTIAL: u8 = 6;
    pub const VENDOR: u8 = 221;
}
const EXPANDED_METHOD_LEN: usize = 3 + core::mem::size_of::<u32>();
pub const NAI_ENCODING: u8 = 0;
pub const UTF8_ENCODING: u8 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EapMethod<'a> {
    pub method: u8,
    pub parameters: Parameters<'a>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EapMethods<'a> {
    count: u8,
    bytes: &'a [u8],
}
impl<'a> EapMethods<'a> {
    pub const EMPTY: Self = Self {
        count: 0,
        bytes: &[],
    };
    pub fn parse(count: u8, bytes: &'a [u8]) -> Result<Self, WireError> {
        let mut fields = BodyReader::new(bytes);
        for _ in 0..count {
            read_method(&mut fields)?;
        }
        end(&fields)?;
        Ok(Self { count, bytes })
    }
    pub const fn count(self) -> u8 {
        self.count
    }
    pub const fn as_bytes(self) -> &'a [u8] {
        self.bytes
    }
    pub fn iter(self) -> impl Iterator<Item = EapMethod<'a>> + Clone {
        let mut rest = self.bytes;
        (0..self.count).map(move |_| {
            let mut fields = BodyReader::new(rest);
            let value = read_method(&mut fields).expect("validated EAP methods");
            rest = fields.remaining();
            value
        })
    }
}
fn read_method<'a>(fields: &mut BodyReader<'a>) -> Result<EapMethod<'a>, WireError> {
    let mut method = BodyReader::new(short_bytes(fields)?);
    let method_id = method.u8()?;
    let count = method.u8()?;
    let parameters = Parameters::parse(method.remaining()).map_err(|_| WireError::InvalidLength)?;
    if parameters.iter().count() != usize::from(count) {
        return Err(WireError::InvalidLength);
    }
    for parameter in parameters.iter() {
        let expected = match parameter.id {
            authentication_id::EXPANDED_EAP | authentication_id::EXPANDED_INNER_EAP => {
                Some(EXPANDED_METHOD_LEN)
            }
            authentication_id::NON_EAP_INNER
            | authentication_id::INNER_EAP
            | authentication_id::CREDENTIAL
            | authentication_id::TUNNELED_CREDENTIAL => Some(1),
            _ => None,
        };
        if expected.is_some_and(|length| parameter.body.len() != length) {
            return Err(WireError::InvalidLength);
        }
    }
    Ok(EapMethod {
        method: method_id,
        parameters,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RealmData<'a> {
    pub encoding: u8,
    pub realms: &'a [u8],
    pub methods: EapMethods<'a>,
}
impl RealmData<'_> {
    fn body_len(self) -> Result<usize, WireError> {
        validate_text(self.encoding, self.realms)?;
        if self.realms.len() > u8::MAX as usize {
            return Err(WireError::ElementTooLong);
        }
        let length = 3 + self.realms.len() + self.methods.bytes.len(); // encoding, string length, EAP count
        if length > u16::MAX as usize {
            return Err(WireError::ElementTooLong);
        }
        Ok(length)
    }
    fn write(self, fields: &mut BodyWriter<'_>) {
        fields.put(&(self.body_len().expect("validated realm length") as u16).to_le_bytes());
        fields.put(&[self.encoding, self.realms.len() as u8]);
        fields.put(self.realms);
        fields.put(&[self.methods.count]);
        fields.put(self.methods.bytes);
    }
}
pub(super) fn validate_text(encoding: u8, bytes: &[u8]) -> Result<(), WireError> {
    // Bit 0 selects UTF-8 instead of RFC 4282 NAI ASCII. Retain reserved bits.
    if encoding & UTF8_ENCODING != 0 {
        utf8(bytes)?;
    } else if !bytes.is_ascii() {
        return Err(WireError::InvalidText);
    }
    Ok(())
}
fn read_realm<'a>(fields: &mut BodyReader<'a>) -> Result<RealmData<'a>, WireError> {
    let mut record = BodyReader::new(long_bytes(fields)?);
    let encoding = record.u8()?;
    let realms = short_bytes(&mut record)?;
    validate_text(encoding, realms)?;
    let count = record.u8()?;
    let methods = EapMethods::parse(count, record.remaining())?;
    Ok(RealmData {
        encoding,
        realms,
        methods,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Realms<'a> {
    count: u16,
    records: &'a [u8],
}
impl<'a> Realms<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, WireError> {
        let mut fields = BodyReader::new(bytes);
        let count = u16::from_le_bytes(fields.array()?);
        let records = fields.remaining();
        for _ in 0..count {
            read_realm(&mut fields)?;
        }
        end(&fields)?;
        Ok(Self { count, records })
    }
    pub const fn count(self) -> u16 {
        self.count
    }
    pub fn iter(self) -> impl Iterator<Item = RealmData<'a>> + Clone {
        let mut rest = self.records;
        (0..self.count).map(move |_| {
            let mut fields = BodyReader::new(rest);
            let value = read_realm(&mut fields).expect("validated realm records");
            rest = fields.remaining();
            value
        })
    }
    /// Encode a complete NAI Realm element body. Capacity and every record
    /// are checked before any write; the supplied slice is one stable snapshot.
    pub fn encode(records: &[RealmData<'_>], bytes: &mut [u8]) -> Result<usize, WireError> {
        u16::try_from(records.len()).map_err(|_| WireError::ElementTooLong)?;
        let mut length = core::mem::size_of::<u16>();
        for record in records {
            length = length
                .checked_add(core::mem::size_of::<u16>() + record.body_len()?)
                .ok_or(WireError::ElementTooLong)?;
            if length > u16::MAX as usize {
                return Err(WireError::ElementTooLong);
            }
        }
        let mut encoder = Encoder::new(output(bytes, length)?)?;
        for record in records {
            encoder.push(*record)?;
        }
        Ok(encoder.finish())
    }
}

/// Incremental NAI Realm body construction for bounded filtered snapshots.
/// Each push is atomic; the caller owns partially prepared output until finish.
pub struct Encoder<'a> {
    bytes: &'a mut [u8],
    count: u16,
    length: usize,
}
impl<'a> Encoder<'a> {
    pub fn new(bytes: &'a mut [u8]) -> Result<Self, WireError> {
        let length = core::mem::size_of::<u16>();
        output(bytes, length)?.copy_from_slice(&0u16.to_le_bytes());
        Ok(Self {
            bytes,
            count: 0,
            length,
        })
    }
    pub fn push(&mut self, record: RealmData<'_>) -> Result<(), WireError> {
        let count = self.count.checked_add(1).ok_or(WireError::ElementTooLong)?;
        let length = self
            .length
            .checked_add(core::mem::size_of::<u16>() + record.body_len()?)
            .ok_or(WireError::ElementTooLong)?;
        if length > u16::MAX as usize {
            return Err(WireError::ElementTooLong);
        }
        let bytes = output(self.bytes, length)?;
        record.write(&mut BodyWriter::new(&mut bytes[self.length..]));
        bytes[..core::mem::size_of::<u16>()].copy_from_slice(&count.to_le_bytes());
        self.count = count;
        self.length = length;
        Ok(())
    }
    pub fn finish(self) -> usize {
        self.length
    }
}
