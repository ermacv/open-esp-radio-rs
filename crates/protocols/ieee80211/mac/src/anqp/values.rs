use super::*;
use crate::management::{MAX_SSID_LEN, elements::Elements as ManagementElements};

/// Three-octet ISO-639 language codes shared by Venue and Hotspot name duples.
pub const LANGUAGE_CODE_LEN: usize = 3;
pub const GEOSPATIAL_REPORT_LEN: usize = 18;
const VENDOR_OI_LEN: usize = 3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Name<'a> {
    pub language: [u8; LANGUAGE_CODE_LEN],
    pub text: &'a str,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Names<'a>(&'a [u8]);
impl<'a> Names<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, WireError> {
        let mut fields = BodyReader::new(bytes);
        while !fields.remaining().is_empty() {
            read_name(&mut fields)?;
        }
        Ok(Self(bytes))
    }
    pub fn iter(self) -> impl Iterator<Item = Name<'a>> {
        let mut fields = BodyReader::new(self.0);
        core::iter::from_fn(move || {
            if fields.remaining().is_empty() {
                None
            } else {
                Some(read_name(&mut fields).expect("validated name duples"))
            }
        })
    }
    pub const fn as_bytes(self) -> &'a [u8] {
        self.0
    }
    pub fn encode(names: &[Name<'_>], bytes: &mut [u8]) -> Result<usize, WireError> {
        let mut length = 0usize;
        for name in names {
            if !name.language.is_ascii() || name.text.len() > u8::MAX as usize - LANGUAGE_CODE_LEN {
                return Err(WireError::InvalidText);
            }
            length = length
                .checked_add(1 + LANGUAGE_CODE_LEN + name.text.len())
                .ok_or(WireError::ElementTooLong)?;
        }
        let mut fields = BodyWriter::new(output(bytes, length)?);
        for name in names {
            fields.put(&[(LANGUAGE_CODE_LEN + name.text.len()) as u8]);
            fields.put(&name.language);
            fields.put(name.text.as_bytes());
        }
        Ok(length)
    }
}
fn read_name<'a>(fields: &mut BodyReader<'a>) -> Result<Name<'a>, WireError> {
    let bytes = short_bytes(fields)?;
    let language: [u8; LANGUAGE_CODE_LEN] = bytes
        .get(..LANGUAGE_CODE_LEN)
        .ok_or(WireError::InvalidLength)?
        .try_into()
        .expect("language width");
    if !language.is_ascii() {
        return Err(WireError::InvalidText);
    }
    Ok(Name {
        language,
        text: utf8(&bytes[LANGUAGE_CODE_LEN..])?,
    })
}

/// Shared one-octet-length byte strings; the owning element decides text rules.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Strings<'a>(&'a [u8]);
impl<'a> Strings<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, WireError> {
        let mut fields = BodyReader::new(bytes);
        while !fields.remaining().is_empty() {
            short_bytes(&mut fields)?;
        }
        Ok(Self(bytes))
    }
    pub fn iter(self) -> impl Iterator<Item = &'a [u8]> {
        let mut fields = BodyReader::new(self.0);
        core::iter::from_fn(move || {
            if fields.remaining().is_empty() {
                None
            } else {
                Some(short_bytes(&mut fields).expect("validated strings"))
            }
        })
    }
    pub const fn as_bytes(self) -> &'a [u8] {
        self.0
    }
    /// Encode one-octet-length strings. Their owning element applies text,
    /// OI or domain rules when the complete ANQP element is encoded.
    pub fn encode(strings: &[&[u8]], bytes: &mut [u8]) -> Result<usize, WireError> {
        let mut length = 0usize;
        for string in strings {
            if string.len() > u8::MAX as usize {
                return Err(WireError::ElementTooLong);
            }
            length = length
                .checked_add(1 + string.len())
                .ok_or(WireError::ElementTooLong)?;
        }
        if length > u16::MAX as usize {
            return Err(WireError::ElementTooLong);
        }
        let mut fields = BodyWriter::new(output(bytes, length)?);
        for string in strings {
            fields.put(&[string.len() as u8]);
            fields.put(string);
        }
        Ok(length)
    }
}

pub mod network_authentication_kind {
    pub const TERMS_AND_CONDITIONS: u8 = 0;
    pub const ONLINE_ENROLLMENT: u8 = 1;
    pub const HTTP_REDIRECT: u8 = 2;
    pub const DNS_REDIRECT: u8 = 3;
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NetworkAuthentication<'a> {
    pub kind: u8,
    pub redirect_uri: &'a [u8],
}
impl NetworkAuthentication<'_> {
    fn encoded_len(self) -> Result<usize, WireError> {
        if self.redirect_uri.len() > u16::MAX as usize {
            return Err(WireError::ElementTooLong);
        }
        if matches!(
            self.kind,
            network_authentication_kind::ONLINE_ENROLLMENT
                | network_authentication_kind::DNS_REDIRECT
        ) && !self.redirect_uri.is_empty()
        {
            return Err(WireError::InvalidValue);
        }
        Ok(1 + core::mem::size_of::<u16>() + self.redirect_uri.len())
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NetworkAuthentications<'a>(&'a [u8]);
impl<'a> NetworkAuthentications<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, WireError> {
        let mut fields = BodyReader::new(bytes);
        while !fields.remaining().is_empty() {
            read_authentication(&mut fields)?;
        }
        Ok(Self(bytes))
    }
    pub fn iter(self) -> impl Iterator<Item = NetworkAuthentication<'a>> {
        let mut fields = BodyReader::new(self.0);
        core::iter::from_fn(move || {
            if fields.remaining().is_empty() {
                None
            } else {
                Some(read_authentication(&mut fields).expect("validated authentications"))
            }
        })
    }
    pub fn encode(
        records: &[NetworkAuthentication<'_>],
        bytes: &mut [u8],
    ) -> Result<usize, WireError> {
        let mut length = 0usize;
        for record in records {
            length = length
                .checked_add(record.encoded_len()?)
                .ok_or(WireError::ElementTooLong)?;
        }
        if length > u16::MAX as usize {
            return Err(WireError::ElementTooLong);
        }
        let mut fields = BodyWriter::new(output(bytes, length)?);
        for record in records {
            fields.put(&[record.kind]);
            fields.put(&(record.redirect_uri.len() as u16).to_le_bytes());
            fields.put(record.redirect_uri);
        }
        Ok(length)
    }
}
fn read_authentication<'a>(
    fields: &mut BodyReader<'a>,
) -> Result<NetworkAuthentication<'a>, WireError> {
    let value = NetworkAuthentication {
        kind: fields.u8()?,
        redirect_uri: long_bytes(fields)?,
    };
    value.encoded_len()?;
    Ok(value)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Venue<'a> {
    pub group: u8,
    pub kind: u8,
    pub names: Names<'a>,
}
impl Venue<'_> {
    pub fn encode(self, bytes: &mut [u8]) -> Result<usize, WireError> {
        let length = 2 + self.names.as_bytes().len();
        if length > u16::MAX as usize {
            return Err(WireError::ElementTooLong);
        }
        let mut fields = BodyWriter::new(output(bytes, length)?);
        fields.put(&[self.group, self.kind]);
        fields.put(self.names.as_bytes());
        Ok(length)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VenueUrl<'a> {
    pub venue_number: u8,
    pub url: &'a str,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VenueUrls<'a>(Strings<'a>);
impl<'a> VenueUrls<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, WireError> {
        let strings = Strings::parse(bytes)?;
        for item in strings.iter() {
            let (_, url) = item.split_first().ok_or(WireError::InvalidLength)?;
            utf8(url)?;
        }
        Ok(Self(strings))
    }
    pub fn iter(self) -> impl Iterator<Item = VenueUrl<'a>> {
        self.0.iter().map(|bytes| VenueUrl {
            venue_number: bytes[0],
            url: utf8(&bytes[1..]).expect("validated venue URL"),
        })
    }
}

/// IPv4 has six bits and IPv6 two. Assigned and future values are preserved.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IpAvailability(u8);
impl IpAvailability {
    const IPV6_BITS: u8 = 2;
    const IPV6_MASK: u8 = (1 << Self::IPV6_BITS) - 1;
    pub fn new(ipv4: u8, ipv6: u8) -> Result<Self, WireError> {
        if ipv4 > u8::MAX >> Self::IPV6_BITS || ipv6 > Self::IPV6_MASK {
            return Err(WireError::InvalidValue);
        }
        Ok(Self((ipv4 << Self::IPV6_BITS) | ipv6))
    }
    pub const fn ipv4(self) -> u8 {
        self.0 >> Self::IPV6_BITS
    }
    pub const fn ipv6(self) -> u8 {
        self.0 & Self::IPV6_MASK
    }
    pub const fn octet(self) -> u8 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Vendor<'a> {
    pub oi: [u8; VENDOR_OI_LEN],
    pub payload: &'a [u8],
}
impl<'a> Vendor<'a> {
    pub fn parse(body: &'a [u8]) -> Result<Self, WireError> {
        let mut fields = BodyReader::new(body);
        Ok(Self {
            oi: fields.array()?,
            payload: fields.remaining(),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Value<'a> {
    QueryList(QueryList<'a>),
    CapabilityList(CapabilityList<'a>),
    Venue(Venue<'a>),
    EmergencyNumbers(Strings<'a>),
    NetworkAuthentication(NetworkAuthentications<'a>),
    RoamingConsortium(Strings<'a>),
    IpAvailability(IpAvailability),
    NaiRealms(realm::Realms<'a>),
    /// 3GPP defines the content of this generic ANQP container externally.
    CellularNetwork(&'a [u8]),
    /// Original fixed LCI report, retained without geographic conversion.
    GeospatialLocation(&'a [u8; GEOSPATIAL_REPORT_LEN]),
    CivicLocation {
        kind: u8,
        report: &'a [u8],
    },
    LocationUri(&'a [u8]),
    Domains(Strings<'a>),
    EmergencyAlertUri(&'a [u8]),
    /// Peer Information is retained for the TDLS procedure's explicit owner.
    TdlsCapability(&'a [u8]),
    EmergencyNai(&'a str),
    NeighborReports(ManagementElements<'a>),
    VenueUrls(VenueUrls<'a>),
    Hotspot(hs20::Element<'a>),
    Vendor(Vendor<'a>),
    Unknown(Element<'a>),
}
impl<'a> Value<'a> {
    pub(super) fn parse(element: Element<'a>) -> Result<Self, WireError> {
        let body = element.body;
        Ok(match element.id {
            InfoId::QUERY_LIST => Self::QueryList(QueryList::parse(body)?),
            InfoId::CAPABILITY_LIST => Self::CapabilityList(CapabilityList::parse(body)?),
            InfoId::VENUE_NAME => {
                let mut fields = BodyReader::new(body);
                Self::Venue(Venue {
                    group: fields.u8()?,
                    kind: fields.u8()?,
                    names: Names::parse(fields.remaining())?,
                })
            }
            InfoId::EMERGENCY_CALL_NUMBER => {
                let strings = Strings::parse(body)?;
                for string in strings.iter() {
                    utf8(string)?;
                }
                Self::EmergencyNumbers(strings)
            }
            InfoId::NETWORK_AUTHENTICATION => {
                Self::NetworkAuthentication(NetworkAuthentications::parse(body)?)
            }
            InfoId::ROAMING_CONSORTIUM => Self::RoamingConsortium(Strings::parse(body)?),
            InfoId::IP_ADDRESS_AVAILABILITY => {
                let [octet] = body else {
                    return Err(WireError::InvalidLength);
                };
                Self::IpAvailability(IpAvailability(*octet))
            }
            InfoId::NAI_REALM => Self::NaiRealms(realm::Realms::parse(body)?),
            InfoId::CELLULAR_NETWORK => Self::CellularNetwork(body),
            InfoId::GEOSPATIAL_LOCATION => {
                Self::GeospatialLocation(body.try_into().map_err(|_| WireError::InvalidLength)?)
            }
            InfoId::CIVIC_LOCATION => {
                let (&kind, report) = body.split_first().ok_or(WireError::Truncated)?;
                Self::CivicLocation { kind, report }
            }
            InfoId::LOCATION_URI => Self::LocationUri(body),
            InfoId::DOMAIN_NAME => {
                let strings = Strings::parse(body)?;
                for string in strings.iter() {
                    if !string.is_ascii() {
                        return Err(WireError::InvalidText);
                    }
                }
                Self::Domains(strings)
            }
            InfoId::EMERGENCY_ALERT_URI => Self::EmergencyAlertUri(body),
            InfoId::TDLS_CAPABILITY => Self::TdlsCapability(body),
            InfoId::EMERGENCY_NAI => Self::EmergencyNai(utf8(body)?),
            InfoId::NEIGHBOR_REPORT => {
                let list = ManagementElements::parse(body).map_err(|_| WireError::InvalidLength)?;
                for report in list.iter() {
                    if report.id != crate::roaming::element_id::NEIGHBOR_REPORT {
                        return Err(WireError::InvalidValue);
                    }
                    crate::roaming::NeighborReport::parse(report.body)
                        .map_err(|_| WireError::InvalidValue)?;
                }
                Self::NeighborReports(list)
            }
            InfoId::VENDOR => {
                let vendor = Vendor::parse(body)?;
                if vendor.oi == hs20::WFA_OI && vendor.payload.first() == Some(&hs20::ANQP_OUI_TYPE)
                {
                    Self::Hotspot(hs20::Element::parse(vendor.payload)?)
                } else {
                    Self::Vendor(vendor)
                }
            }
            InfoId::VENUE_URL => Self::VenueUrls(VenueUrls::parse(body)?),
            _ => Self::Unknown(element),
        })
    }
}

/// Validate an OSU SSID using the shared MAC limit without text conversion.
pub(super) fn ssid(bytes: &[u8]) -> Result<(), WireError> {
    if bytes.len() <= MAX_SSID_LEN {
        Ok(())
    } else {
        Err(WireError::InvalidLength)
    }
}
