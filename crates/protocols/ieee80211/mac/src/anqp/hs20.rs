//! Wi-Fi Alliance Hotspot 2.0 ANQP vendor envelope and Release 1–3 records.
use super::*;
mod osu;
pub use osu::*;

pub use crate::management::WFA_OUI as WFA_OI;
pub const ANQP_OUI_TYPE: u8 = 17;
pub const VENDOR_HEADER_LEN: usize = WFA_OI.len() + 3; // OI, type, subtype, reserved
pub const WAN_METRICS_LEN: usize = 13;
pub const CONNECTION_LEN: usize = 4;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Subtype(pub u8);
impl Subtype {
    pub const QUERY_LIST: Self = Self(1);
    pub const CAPABILITY_LIST: Self = Self(2);
    pub const OPERATOR_FRIENDLY_NAME: Self = Self(3);
    pub const WAN_METRICS: Self = Self(4);
    pub const CONNECTION_CAPABILITY: Self = Self(5);
    pub const NAI_HOME_REALM_QUERY: Self = Self(6);
    pub const OPERATING_CLASS: Self = Self(7);
    pub const OSU_PROVIDERS: Self = Self(8);
    // Subtype 9 is reserved, not an OSU/icon message.
    pub const ICON_REQUEST: Self = Self(10);
    pub const ICON_BINARY_FILE: Self = Self(11);
    pub const OPERATOR_ICON_METADATA: Self = Self(12);
    pub const OSU_PROVIDERS_NAI: Self = Self(13);
}

/// `parse` consumes the vendor payload after its OI. The reserved octet is
/// retained on receive and encode. Senders explicitly supply zero for the
/// current format; future extensions are not silently normalized.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
// CAPABILITY: wifi-roaming-and-service-discovery-passpoint
pub struct Element<'a> {
    pub subtype: Subtype,
    pub reserved: u8,
    pub body: &'a [u8],
}
impl<'a> Element<'a> {
    pub fn parse(payload: &'a [u8]) -> Result<Self, WireError> {
        let mut fields = BodyReader::new(payload);
        if fields.u8()? != ANQP_OUI_TYPE {
            return Err(WireError::InvalidValue);
        }
        let value = Self {
            subtype: Subtype(fields.u8()?),
            reserved: fields.u8()?,
            body: fields.remaining(),
        };
        value.value()?;
        Ok(value)
    }
    pub fn value(self) -> Result<Value<'a>, WireError> {
        Ok(match self.subtype {
            Subtype::QUERY_LIST | Subtype::CAPABILITY_LIST => {
                Value::Ids(Subtypes::parse(self.body)?)
            }
            Subtype::OPERATOR_FRIENDLY_NAME => Value::Names(Names::parse(self.body)?),
            Subtype::WAN_METRICS => Value::WanMetrics(WanMetrics::parse(self.body)?),
            Subtype::CONNECTION_CAPABILITY => Value::Connections(Connections::parse(self.body)?),
            Subtype::NAI_HOME_REALM_QUERY => Value::HomeRealms(HomeRealms::parse(self.body)?),
            Subtype::OPERATING_CLASS => Value::OperatingClasses(self.body),
            Subtype::OSU_PROVIDERS => Value::OsuProviders(OsuProviders::parse(self.body)?),
            Subtype::ICON_REQUEST => Value::IconRequest(utf8(self.body)?),
            Subtype::ICON_BINARY_FILE => Value::IconBinaryFile(IconBinaryFile::parse(self.body)?),
            Subtype::OPERATOR_ICON_METADATA => Value::Icons(Icons::parse(self.body)?),
            Subtype::OSU_PROVIDERS_NAI => {
                let list = Strings::parse(self.body)?;
                for nai in list.iter() {
                    utf8(nai)?;
                }
                Value::OsuProvidersNai(list)
            }
            _ => Value::Unknown(self.body),
        })
    }
    pub fn encode(self, bytes: &mut [u8]) -> Result<usize, WireError> {
        self.value()?;
        let length = VENDOR_HEADER_LEN + self.body.len();
        if length > u16::MAX as usize {
            return Err(WireError::ElementTooLong);
        }
        let total = ANQP_HEADER_LEN + length;
        let mut fields = BodyWriter::new(output(bytes, total)?);
        fields.put(&InfoId::VENDOR.0.to_le_bytes());
        fields.put(&(length as u16).to_le_bytes());
        fields.put(&WFA_OI);
        fields.put(&[ANQP_OUI_TYPE, self.subtype.0, self.reserved]);
        fields.put(self.body);
        Ok(total)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Value<'a> {
    Ids(Subtypes<'a>),
    Names(Names<'a>),
    WanMetrics(WanMetrics),
    Connections(Connections<'a>),
    HomeRealms(HomeRealms<'a>),
    OperatingClasses(&'a [u8]),
    Unknown(&'a [u8]),
    OsuProviders(OsuProviders<'a>),
    IconRequest(&'a str),
    IconBinaryFile(IconBinaryFile<'a>),
    Icons(Icons<'a>),
    OsuProvidersNai(Strings<'a>),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Subtypes<'a>(&'a [u8]);
impl<'a> Subtypes<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, WireError> {
        // No ordering requirement in HS2; duplicate identifiers are ambiguous.
        for (index, id) in bytes.iter().enumerate() {
            if bytes[..index].contains(id) {
                return Err(WireError::InvalidValue);
            }
        }
        Ok(Self(bytes))
    }
    pub fn iter(self) -> impl Iterator<Item = Subtype> + Clone + 'a {
        self.0.iter().copied().map(Subtype)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WanMetrics {
    /// Link status, symmetric-link and at-capacity flags, including reserved bits.
    pub info: u8,
    pub downlink_kbps: u32,
    pub uplink_kbps: u32,
    pub downlink_load: u8,
    pub uplink_load: u8,
    /// Load Measurement Duration in 0.1-second units.
    pub measurement_duration: u16,
}
impl WanMetrics {
    pub fn parse(bytes: &[u8]) -> Result<Self, WireError> {
        if bytes.len() != WAN_METRICS_LEN {
            return Err(WireError::InvalidLength);
        }
        let mut fields = BodyReader::new(bytes);
        Ok(Self {
            info: fields.u8()?,
            downlink_kbps: u32::from_le_bytes(fields.array()?),
            uplink_kbps: u32::from_le_bytes(fields.array()?),
            downlink_load: fields.u8()?,
            uplink_load: fields.u8()?,
            measurement_duration: u16::from_le_bytes(fields.array()?),
        })
    }
    pub fn encode(self, bytes: &mut [u8]) -> Result<usize, WireError> {
        let mut fields = BodyWriter::new(output(bytes, WAN_METRICS_LEN)?);
        fields.put(&[self.info]);
        fields.put(&self.downlink_kbps.to_le_bytes());
        fields.put(&self.uplink_kbps.to_le_bytes());
        fields.put(&[self.downlink_load, self.uplink_load]);
        fields.put(&self.measurement_duration.to_le_bytes());
        Ok(WAN_METRICS_LEN)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Connection {
    pub protocol: u8,
    pub port: u16,
    pub status: u8,
}
pub mod connection_status {
    pub const CLOSED: u8 = 0;
    pub const OPEN: u8 = 1;
    pub const UNKNOWN: u8 = 2;
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Connections<'a>(&'a [u8]);
impl<'a> Connections<'a> {
    pub fn encode(records: &[Connection], bytes: &mut [u8]) -> Result<usize, WireError> {
        let length = records
            .len()
            .checked_mul(CONNECTION_LEN)
            .ok_or(WireError::ElementTooLong)?;
        if length > u16::MAX as usize - VENDOR_HEADER_LEN {
            return Err(WireError::ElementTooLong);
        }
        let mut fields = BodyWriter::new(output(bytes, length)?);
        for record in records {
            fields.put(&[record.protocol]);
            fields.put(&record.port.to_le_bytes());
            fields.put(&[record.status]);
        }
        Ok(length)
    }
    pub fn parse(bytes: &'a [u8]) -> Result<Self, WireError> {
        if !bytes.len().is_multiple_of(CONNECTION_LEN) {
            return Err(WireError::InvalidLength);
        }
        Ok(Self(bytes))
    }
    pub fn iter(self) -> impl Iterator<Item = Connection> + 'a {
        self.0.chunks_exact(CONNECTION_LEN).map(|bytes| Connection {
            protocol: bytes[0],
            port: u16::from_le_bytes([bytes[1], bytes[2]]),
            status: bytes[3],
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HomeRealm<'a> {
    pub encoding: u8,
    pub realms: &'a [u8],
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HomeRealms<'a> {
    count: u8,
    bytes: &'a [u8],
}
impl<'a> HomeRealms<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, WireError> {
        let mut fields = BodyReader::new(bytes);
        let count = fields.u8()?;
        let records = fields.remaining();
        for _ in 0..count {
            read_home_realm(&mut fields)?;
        }
        end(&fields)?;
        Ok(Self {
            count,
            bytes: records,
        })
    }
    pub fn iter(self) -> impl Iterator<Item = HomeRealm<'a>> + Clone {
        let mut rest = self.bytes;
        (0..self.count).map(move |_| {
            let mut fields = BodyReader::new(rest);
            let value = read_home_realm(&mut fields).expect("validated home realms");
            rest = fields.remaining();
            value
        })
    }
    pub fn encode(realms: &[HomeRealm<'_>], bytes: &mut [u8]) -> Result<usize, WireError> {
        let count = u8::try_from(realms.len()).map_err(|_| WireError::ElementTooLong)?;
        let mut length = 1usize;
        for realm in realms {
            realm::validate_text(realm.encoding, realm.realms)?;
            if realm.realms.len() > u8::MAX as usize {
                return Err(WireError::ElementTooLong);
            }
            length += 2 + realm.realms.len(); // encoding and length
        }
        let mut fields = BodyWriter::new(output(bytes, length)?);
        fields.put(&[count]);
        for realm in realms {
            fields.put(&[realm.encoding, realm.realms.len() as u8]);
            fields.put(realm.realms);
        }
        Ok(length)
    }
}
fn read_home_realm<'a>(fields: &mut BodyReader<'a>) -> Result<HomeRealm<'a>, WireError> {
    let encoding = fields.u8()?;
    let realms = short_bytes(fields)?;
    realm::validate_text(encoding, realms)?;
    Ok(HomeRealm { encoding, realms })
}
