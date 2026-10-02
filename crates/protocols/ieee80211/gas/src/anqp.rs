//! ANQP semantics over GAS protocol 0. Providers supply immutable facts; this
//! owner validates requests, correlates answers and filters Home Realm data.
//! It never selects credentials or performs network, file or radio I/O.
use super::storage::Bytes;
use oer_ieee80211_mac::anqp::{self as wire, Element, Elements, InfoId, Value, hs20, realm};
mod client;
pub use client::{Outcome, Requester};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Wire(wire::WireError),
    Gas(super::Error),
    EmptyQuery,
    WrongMessage,
    DuplicateHotspot(hs20::Subtype),
    AnswerCount { expected: usize, actual: usize },
    AnswerIdentity,
    UnrequestedAnswer,
    ProviderNaiCount,
}
impl From<wire::WireError> for Error {
    fn from(value: wire::WireError) -> Self {
        Self::Wire(value)
    }
}
impl From<super::Error> for Error {
    fn from(value: super::Error) -> Self {
        Self::Gas(value)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Request<'a> {
    Standard(InfoId),
    Hotspot(hs20::Subtype),
    HomeRealms(hs20::HomeRealms<'a>),
    Icon(&'a str),
    /// Future/vendor requests are exposed to the provider, never discarded.
    Extension(Element<'a>),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Query<'a>(Elements<'a>);
impl<'a> Query<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        let elements = Elements::parse(bytes)?;
        elements.validate()?;
        if bytes.is_empty() {
            return Err(Error::EmptyQuery);
        }
        validate_hotspot_duplicates(elements)?;
        for element in elements.iter() {
            match element.value()? {
                Value::QueryList(list) => {
                    if list.iter().any(|id| id == InfoId::QUERY_LIST) {
                        return Err(Error::WrongMessage);
                    }
                }
                Value::Hotspot(value) => match value.subtype {
                    hs20::Subtype::QUERY_LIST => {
                        let hs20::Value::Ids(ids) = value.value()? else {
                            unreachable!()
                        };
                        if ids.iter().any(|id| {
                            id == hs20::Subtype::QUERY_LIST
                                || id == hs20::Subtype::NAI_HOME_REALM_QUERY
                                || id == hs20::Subtype::ICON_REQUEST
                        }) {
                            return Err(Error::WrongMessage);
                        }
                    }
                    hs20::Subtype::NAI_HOME_REALM_QUERY => (),
                    hs20::Subtype::ICON_REQUEST => (),
                    _ if matches!(value.value()?, hs20::Value::Unknown(_)) => (),
                    _ => return Err(Error::WrongMessage),
                },
                Value::Vendor(_) | Value::Unknown(_) | Value::TdlsCapability(_) => (),
                _ => return Err(Error::WrongMessage),
            }
        }
        let query = Self(elements);
        if query.requests().next().is_none() {
            return Err(Error::EmptyQuery);
        }
        Ok(query)
    }
    pub const fn as_bytes(self) -> &'a [u8] {
        self.0.as_bytes()
    }
    pub fn requests(self) -> impl Iterator<Item = Request<'a>> + Clone {
        let full_realms = self.0.iter().any(|element| matches!(element.value(), Ok(Value::QueryList(list)) if list.iter().any(|id| id == InfoId::NAI_REALM)));
        // Base IDs and HS2 subtypes use different envelopes. Flatten only at
        // this semantic boundary, retaining parameterized vendor requests.
        self.0.iter().flat_map(move |element| {
            let (base, hotspot, direct) = match element.value().expect("validated query") {
                Value::QueryList(list) => (Some(list), None, None),
                Value::Hotspot(value) if value.subtype == hs20::Subtype::QUERY_LIST => {
                    let hs20::Value::Ids(ids) = value.value().expect("validated HS2 query") else {
                        unreachable!()
                    };
                    (None, Some(ids), None)
                }
                Value::Hotspot(value) if value.subtype == hs20::Subtype::NAI_HOME_REALM_QUERY => {
                    let hs20::Value::HomeRealms(realms) =
                        value.value().expect("validated home realms")
                    else {
                        unreachable!()
                    };
                    (
                        None,
                        None,
                        (!full_realms).then_some(Request::HomeRealms(realms)),
                    )
                }
                Value::Hotspot(value) if value.subtype == hs20::Subtype::ICON_REQUEST => {
                    let hs20::Value::IconRequest(filename) =
                        value.value().expect("validated icon request")
                    else {
                        unreachable!()
                    };
                    (None, None, Some(Request::Icon(filename)))
                }
                _ => (None, None, Some(Request::Extension(element))),
            };
            base.into_iter()
                .flat_map(wire::QueryList::iter)
                .map(Request::Standard)
                .chain(
                    hotspot
                        .into_iter()
                        .flat_map(hs20::Subtypes::iter)
                        .map(Request::Hotspot),
                )
                .chain(direct)
        })
    }
}

/// One explicit provider resolution per request, in request order. Unsupported
/// means the service does not implement that identifier. For implemented but
/// unconfigured information the provider must supply the standard's empty
/// optional fields and any mandatory facts; this owner invents no defaults.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Resolution<'a> {
    Record(Element<'a>),
    Unsupported,
}

/// Complete validated response snapshot. Construction is atomic; the GAS
/// responder copies these bytes once and keeps them fixed across fragments.
pub struct Response<const SIZE: usize>(Bytes<SIZE>);
impl<const SIZE: usize> Response<SIZE> {
    pub fn prepare(query: Query<'_>, answers: &[Resolution<'_>]) -> Result<Self, Error> {
        let expected = query.requests().count();
        if answers.len() != expected {
            return Err(Error::AnswerCount {
                expected,
                actual: answers.len(),
            });
        }
        let mut result = Bytes::empty();
        for (request, answer) in query.requests().zip(answers) {
            let Resolution::Record(element) = *answer else {
                continue;
            };
            element.encoded_len()?;
            if !matches_answer(request, element)? {
                return Err(Error::AnswerIdentity);
            }
            if let Request::HomeRealms(home) = request {
                let Value::NaiRealms(realms) = element.value()? else {
                    return Err(Error::AnswerIdentity);
                };
                let matches = realms.iter().flat_map(|record| {
                    record
                        .realms
                        .split(|octet| *octet == b';')
                        .filter(move |name| home_matches(home, record.encoding, name))
                        .map(move |name| realm::RealmData {
                            realms: name,
                            ..record
                        })
                });
                let mut filtered = [0u8; SIZE];
                let mut encoder = realm::Encoder::new(&mut filtered)?;
                for record in matches {
                    encoder.push(record)?;
                }
                let length = encoder.finish();
                append(
                    &mut result,
                    Element {
                        id: InfoId::NAI_REALM,
                        body: &filtered[..length],
                    },
                )?;
            } else {
                append(&mut result, element)?;
            }
        }
        let response = Elements::parse(result.bytes())?;
        response.validate()?;
        validate_hotspot_duplicates(response)?;
        validate_provider_nai_count(response)?;
        Ok(Self(result))
    }
    pub fn as_bytes(&self) -> &[u8] {
        self.0.bytes()
    }
    pub fn elements(&self) -> Elements<'_> {
        Elements::parse(self.as_bytes()).expect("validated response snapshot")
    }
}
fn append<const SIZE: usize>(result: &mut Bytes<SIZE>, element: Element<'_>) -> Result<(), Error> {
    // Every append is bounded and private until the entire response succeeds.
    result.append(&element.id.0.to_le_bytes())?;
    result.append(&(element.body.len() as u16).to_le_bytes())?;
    result.append(element.body)?;
    Ok(())
}
fn matches_answer(request: Request<'_>, element: Element<'_>) -> Result<bool, Error> {
    Ok(match request {
        Request::Standard(id) => element.id == id,
        Request::HomeRealms(_) => element.id == InfoId::NAI_REALM,
        Request::Icon(_) => {
            matches!(element.value()?, Value::Hotspot(value) if value.subtype == hs20::Subtype::ICON_BINARY_FILE)
        }
        Request::Hotspot(id) => {
            matches!(element.value()?, Value::Hotspot(value) if value.subtype == id)
        }
        Request::Extension(request) => {
            request.id == element.id
                && match (request.value()?, element.value()?) {
                    (Value::Hotspot(request), Value::Hotspot(answer)) => {
                        request.subtype == answer.subtype
                    }
                    (Value::Vendor(request), Value::Vendor(answer)) => request.oi == answer.oi,
                    (Value::Unknown(_), _) | (Value::TdlsCapability(_), _) => true,
                    _ => false,
                }
        }
    })
}
fn home_matches(home: hs20::HomeRealms<'_>, encoding: u8, name: &[u8]) -> bool {
    home.iter().any(|wanted| {
        wanted.encoding == encoding
            && wanted
                .realms
                .split(|octet| *octet == b';')
                .any(|wanted| wanted == name)
    })
}
fn validate_hotspot_duplicates(elements: Elements<'_>) -> Result<(), Error> {
    const SUBTYPE_COUNT: usize = u8::MAX as usize + 1;
    const WORD_BITS: usize = u64::BITS as usize;
    let mut seen = [0u64; SUBTYPE_COUNT / WORD_BITS];
    for element in elements.iter() {
        if let Value::Hotspot(value) = element.value()? {
            let id = usize::from(value.subtype.0);
            let word = &mut seen[id / WORD_BITS];
            let bit = 1u64 << (id % WORD_BITS);
            if *word & bit != 0 {
                return Err(Error::DuplicateHotspot(value.subtype));
            }
            *word |= bit;
        }
    }
    Ok(())
}

fn validate_provider_nai_count(elements: Elements<'_>) -> Result<(), Error> {
    let mut providers = None;
    let mut nais = None;
    for element in elements.iter() {
        if let Value::Hotspot(value) = element.value()? {
            match value.value()? {
                hs20::Value::OsuProviders(list) => providers = Some(usize::from(list.count())),
                hs20::Value::OsuProvidersNai(list) => nais = Some(list.iter().count()),
                _ => (),
            }
        }
    }
    if let (Some(providers), Some(nais)) = (providers, nais)
        && providers != nais
    {
        return Err(Error::ProviderNaiCount);
    }
    Ok(())
}

/// Validate a completed GAS response before publishing ANQP records. Missing
/// answers remain visible through the query; unsolicited records are rejected.
pub fn validate_response<'a>(query: Query<'_>, bytes: &'a [u8]) -> Result<Elements<'a>, Error> {
    let response = Elements::parse(bytes)?;
    response.validate()?;
    validate_hotspot_duplicates(response)?;
    validate_provider_nai_count(response)?;
    for element in response.iter() {
        if !query
            .requests()
            .any(|request| matches_answer(request, element).unwrap_or(false))
        {
            return Err(Error::UnrequestedAnswer);
        }
        if let Value::NaiRealms(realms) = element.value()? {
            for request in query.requests() {
                if let Request::HomeRealms(home) = request {
                    for record in realms.iter() {
                        if !record
                            .realms
                            .split(|octet| *octet == b';')
                            .all(|name| home_matches(home, record.encoding, name))
                        {
                            return Err(Error::UnrequestedAnswer);
                        }
                    }
                }
            }
        }
        if matches!(element.value()?, Value::QueryList(_))
            || matches!(element.value()?, Value::Hotspot(value) if value.subtype == hs20::Subtype::QUERY_LIST || value.subtype == hs20::Subtype::NAI_HOME_REALM_QUERY || value.subtype == hs20::Subtype::ICON_REQUEST)
        {
            return Err(Error::WrongMessage);
        }
    }
    Ok(response)
}

#[cfg(test)]
mod tests;
