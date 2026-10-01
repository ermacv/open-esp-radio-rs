//! Original 802.11v Event Request/Report formats. Frequent-transition reports
//! explicitly support both conflicting descriptions in IEEE 802.11-2012.
use super::codec::{BodyReader, BodyWriter};
use super::{MAC_ADDRESS_LEN, MAX_ELEMENT_BODY_LEN, OCTET_VALUE_COUNT, WnmAction, element_id};
use core::mem::size_of;
mod conditions;
pub use conditions::*;
pub const MINIMUM_EVENT_HISTORY: usize = 5;
pub const AUTONOMOUS_EVENT_TOKEN: u8 = 0;
pub const ALL_RETAINED_EVENTS: u8 = 0;
pub const MAX_PEER_CONNECTION_SECONDS: u32 = (1 << 24) - 1;
const PEER_CONNECTION_TIME_LEN: usize = 3;
const UTC_OFFSET_LEN: usize = 10;
const TIME_ERROR_LEN: usize = 5;
const REQUEST_FIXED_LEN: usize = 3 * size_of::<u8>();
const REPORT_FIXED_LEN: usize = 3 * size_of::<u8>();
const TIMING_LEN: usize = size_of::<u64>() + UTC_OFFSET_LEN + TIME_ERROR_LEN;
const TRANSITION_DATA_LEN: usize = 2 * MAC_ADDRESS_LEN + 2 * size_of::<u16>() + 5 * size_of::<u8>();
const PEER_LINK_DATA_LEN: usize = MAC_ADDRESS_LEN + 4 * size_of::<u8>() + PEER_CONNECTION_TIME_LEN;
const RSNA_FIXED_DATA_LEN: usize = MAC_ADDRESS_LEN + size_of::<u32>() + size_of::<u16>();
use super::management::{action_elements, encode_action, encode_element};
use super::{EapMethod, Elements, MacAddress, WireError, token};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EventType(pub u8);
impl EventType {
    pub const TRANSITION: Self = Self(0);
    pub const RSNA: Self = Self(1);
    pub const PEER_LINK: Self = Self(2);
    pub const WNM_LOG: Self = Self(3);
    /// Event Type namespace (Table 8-133), independent of IE identifiers.
    pub const VENDOR: Self = Self(221);
    pub const fn original(self) -> bool {
        matches!(
            self,
            Self::TRANSITION | Self::RSNA | Self::PEER_LINK | Self::WNM_LOG
        )
    }
}
/// No default: applications must choose the frequent-transition transmit form.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrequentTransitionFormat {
    StatusOnly,
    WithLastEvent,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EventRequest<'a> {
    pub token: u8,
    pub kind: EventType,
    pub response_limit: u8,
    pub conditions: Elements<'a>,
}
impl<'a> EventRequest<'a> {
    pub fn parse(body: &'a [u8]) -> Result<Self, WireError> {
        if body.len() < REQUEST_FIXED_LEN {
            return Err(WireError::InvalidElementLength(element_id::EVENT_REQUEST));
        }
        let mut fields = BodyReader::new(body);
        let value = Self {
            token: fields.u8()?,
            kind: EventType(fields.u8()?),
            response_limit: fields.u8()?,
            conditions: Elements::parse(fields.remaining())?,
        };
        value.validate()?;
        Ok(value)
    }
    pub fn validate(self) -> Result<(), WireError> {
        token(self.token)?;
        if self.kind == EventType::WNM_LOG && !self.conditions.as_bytes().is_empty() {
            return Err(WireError::InconsistentFields);
        }
        for e in self.conditions.iter() {
            if !matches!(
                EventCondition::parse(self.kind, e)?,
                EventCondition::Unknown(_)
            ) {
                self.conditions.unique(e.id)?;
            }
        }
        Ok(())
    }
    pub fn typed_conditions(self) -> Result<impl Iterator<Item = EventCondition<'a>>, WireError> {
        self.validate()?;
        Ok(self
            .conditions
            .iter()
            .map(move |e| EventCondition::parse(self.kind, e).expect("validated event condition")))
    }
    pub fn frequent_transition(self) -> Result<Option<FrequentTransitionCondition>, WireError> {
        Ok(self.typed_conditions()?.find_map(|c| match c {
            EventCondition::FrequentTransition(value) => Some(value),
            _ => None,
        }))
    }
    pub fn encode(self, out: &mut [u8]) -> Result<usize, WireError> {
        self.validate()?;
        let len = REQUEST_FIXED_LEN + self.conditions.as_bytes().len();
        if len > MAX_ELEMENT_BODY_LEN {
            return Err(WireError::ElementTooLong);
        }
        let mut body = [0; MAX_ELEMENT_BODY_LEN];
        let mut fields = BodyWriter::new(&mut body[..len]);
        fields.put(&[self.token, self.kind.0, self.response_limit]);
        fields.put(self.conditions.as_bytes());
        encode_element(element_id::EVENT_REQUEST, &body[..len], out)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EventTiming {
    pub tsf: u64,
    pub utc_offset: [u8; UTC_OFFSET_LEN],
    pub time_error: [u8; TIME_ERROR_LEN],
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TransitionEvent {
    pub source: MacAddress,
    pub target: MacAddress,
    pub time_tu: u16,
    pub reason: u8,
    pub result: u16,
    pub source_rcpi: u8,
    pub source_rsni: u8,
    pub target_rcpi: u8,
    pub target_rsni: u8,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RsnaEvent<'a> {
    pub target: MacAddress,
    pub authentication: [u8; size_of::<u32>()],
    pub eap: EapMethod,
    pub result: u16,
    /// Complete available RSNE octets; the standard permits a truncated RSNE.
    pub rsne: &'a [u8],
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeerLinkEvent {
    pub peer: MacAddress,
    pub operating_class: u8,
    pub channel: u8,
    pub tx_power_dbm: i8,
    pub connection_seconds: u32,
    pub status: PeerLinkStatus,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventData<'a> {
    Empty,
    Transition(TransitionEvent),
    Rsna(RsnaEvent<'a>),
    PeerLink(PeerLinkEvent),
    /// Complete RFC 3164 message supplied by the logger, including STA MAC TAG.
    WnmLog(&'a [u8]),
    Vendor(Elements<'a>),
    /// Entire bytes after the three fixed fields for later/reserved event types.
    Other(&'a [u8]),
}
impl<'a> EventData<'a> {
    pub fn kind(self) -> Option<EventType> {
        Some(match self {
            Self::Empty => return None,
            Self::Transition(_) => EventType::TRANSITION,
            Self::Rsna(_) => EventType::RSNA,
            Self::PeerLink(_) => EventType::PEER_LINK,
            Self::WnmLog(_) => EventType::WNM_LOG,
            Self::Vendor(_) => EventType::VENDOR,
            Self::Other(_) => return None,
        })
    }
    fn len(self) -> usize {
        match self {
            Self::Empty => 0,
            Self::Transition(_) => TRANSITION_DATA_LEN,
            Self::Rsna(v) => RSNA_FIXED_DATA_LEN + v.eap.encoded_len() + v.rsne.len(),
            Self::PeerLink(_) => PEER_LINK_DATA_LEN,
            Self::WnmLog(v) | Self::Other(v) => v.len(),
            Self::Vendor(v) => v.as_bytes().len(),
        }
    }
    fn write(self, out: &mut [u8]) -> Result<(), WireError> {
        let mut fields = BodyWriter::new(out);
        match self {
            Self::Empty => {}
            Self::Transition(v) => {
                fields.put(&v.source);
                fields.put(&v.target);
                fields.put(&v.time_tu.to_le_bytes());
                fields.put(&[v.reason]);
                fields.put(&v.result.to_le_bytes());
                fields.put(&[v.source_rcpi, v.source_rsni, v.target_rcpi, v.target_rsni]);
            }
            Self::Rsna(v) => {
                fields.put(&v.target);
                fields.put(&v.authentication);
                let mut eap = [0; EapMethod::MAX_ENCODED_LEN];
                let len = v.eap.encode(&mut eap)?;
                fields.put(&eap[..len]);
                fields.put(&v.result.to_le_bytes());
                fields.put(v.rsne);
            }
            Self::PeerLink(v) => {
                if v.connection_seconds > MAX_PEER_CONNECTION_SECONDS {
                    return Err(WireError::InconsistentFields);
                }
                fields.put(&v.peer);
                fields.put(&[v.operating_class, v.channel, v.tx_power_dbm as u8]);
                fields.put(&v.connection_seconds.to_le_bytes()[..PEER_CONNECTION_TIME_LEN]);
                fields.put(&[v.status.0]);
            }
            Self::WnmLog(v) | Self::Other(v) => fields.put(v),
            Self::Vendor(v) => fields.put(v.as_bytes()),
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EventReport<'a> {
    pub token: u8,
    pub kind: EventType,
    pub status: EventReportStatus,
    pub timing: Option<EventTiming>,
    pub data: EventData<'a>,
}
impl<'a> EventReport<'a> {
    pub fn parse(body: &'a [u8]) -> Result<Self, WireError> {
        if body.len() < REPORT_FIXED_LEN {
            return Err(WireError::InvalidElementLength(element_id::EVENT_REPORT));
        }
        let mut fields = BodyReader::new(body);
        let token = fields.u8()?;
        let kind = EventType(fields.u8()?);
        let status = EventReportStatus(fields.u8()?);
        if !kind.original() && kind != EventType::VENDOR {
            return Ok(Self {
                token,
                kind,
                status,
                timing: None,
                data: if fields.remaining().is_empty() {
                    EventData::Empty
                } else {
                    EventData::Other(fields.remaining())
                },
            });
        }
        if fields.remaining().is_empty() {
            return Ok(Self {
                token,
                kind,
                status,
                timing: None,
                data: EventData::Empty,
            });
        }
        if status != EventReportStatus::SUCCESSFUL
            && !(status == EventReportStatus::FREQUENT_TRANSITION && kind == EventType::TRANSITION)
        {
            return Err(WireError::InconsistentFields);
        }
        if fields.remaining().len() < TIMING_LEN {
            return Err(WireError::InvalidElementLength(element_id::EVENT_REPORT));
        }
        let timing = EventTiming {
            tsf: u64::from_le_bytes(fields.array()?),
            utc_offset: fields.array()?,
            time_error: fields.array()?,
        };
        let data = match kind {
            EventType::TRANSITION => {
                if fields.remaining().len() != TRANSITION_DATA_LEN {
                    return Err(WireError::InvalidElementLength(element_id::EVENT_REPORT));
                }
                EventData::Transition(TransitionEvent {
                    source: fields.array()?,
                    target: fields.array()?,
                    time_tu: u16::from_le_bytes(fields.array()?),
                    reason: fields.u8()?,
                    result: u16::from_le_bytes(fields.array()?),
                    source_rcpi: fields.u8()?,
                    source_rsni: fields.u8()?,
                    target_rcpi: fields.u8()?,
                    target_rsni: fields.u8()?,
                })
            }
            EventType::RSNA => {
                if fields.remaining().len() < RSNA_FIXED_DATA_LEN + size_of::<u8>() {
                    return Err(WireError::InvalidElementLength(element_id::EVENT_REPORT));
                }
                let target = fields.array()?;
                let authentication = fields.array()?;
                let eap = EapMethod::parse_prefix(fields.remaining())?;
                fields.take(eap.encoded_len())?;
                EventData::Rsna(RsnaEvent {
                    target,
                    authentication,
                    eap,
                    result: u16::from_le_bytes(fields.array()?),
                    rsne: fields.remaining(),
                })
            }
            EventType::PEER_LINK => {
                if fields.remaining().len() != PEER_LINK_DATA_LEN {
                    return Err(WireError::InvalidElementLength(element_id::EVENT_REPORT));
                }
                let peer = fields.array()?;
                let operating_class = fields.u8()?;
                let channel = fields.u8()?;
                let tx_power_dbm = fields.u8()? as i8;
                let mut seconds = [0; size_of::<u32>()];
                seconds[..PEER_CONNECTION_TIME_LEN]
                    .copy_from_slice(fields.take(PEER_CONNECTION_TIME_LEN)?);
                EventData::PeerLink(PeerLinkEvent {
                    peer,
                    operating_class,
                    channel,
                    tx_power_dbm,
                    connection_seconds: u32::from_le_bytes(seconds),
                    status: PeerLinkStatus(fields.u8()?),
                })
            }
            EventType::WNM_LOG => {
                if !fields.remaining().is_ascii() {
                    return Err(WireError::InconsistentFields);
                }
                EventData::WnmLog(fields.remaining())
            }
            EventType::VENDOR => EventData::Vendor(Elements::parse(fields.remaining())?),
            _ => unreachable!("known event type"),
        };
        Ok(Self {
            token,
            kind,
            status,
            timing: Some(timing),
            data,
        })
    }
    /// Status 4 requires `encode_frequent` and an explicit format choice.
    pub fn encode(self, out: &mut [u8]) -> Result<usize, WireError> {
        if self.status == EventReportStatus::FREQUENT_TRANSITION {
            return Err(WireError::InconsistentFields);
        }
        self.encode_inner(out)
    }
    pub fn encode_frequent(
        self,
        format: FrequentTransitionFormat,
        out: &mut [u8],
    ) -> Result<usize, WireError> {
        if self.status != EventReportStatus::FREQUENT_TRANSITION
            || self.kind != EventType::TRANSITION
        {
            return Err(WireError::InconsistentFields);
        }
        match format {
            FrequentTransitionFormat::StatusOnly
                if self.timing.is_none() && self.data == EventData::Empty => {}
            FrequentTransitionFormat::WithLastEvent
                if self.timing.is_some() && matches!(self.data, EventData::Transition(_)) => {}
            _ => return Err(WireError::InconsistentFields),
        }
        self.encode_inner(out)
    }
    fn encode_inner(self, out: &mut [u8]) -> Result<usize, WireError> {
        if self.data.kind().is_some_and(|kind| kind != self.kind) {
            return Err(WireError::InconsistentFields);
        }
        if matches!(self.data, EventData::Other(_))
            && (self.timing.is_some() || (self.kind.original() || self.kind == EventType::VENDOR))
        {
            return Err(WireError::InconsistentFields);
        }
        let len =
            REPORT_FIXED_LEN + usize::from(self.timing.is_some()) * TIMING_LEN + self.data.len();
        if len > MAX_ELEMENT_BODY_LEN {
            return Err(WireError::ElementTooLong);
        }
        let mut body = [0; MAX_ELEMENT_BODY_LEN];
        let mut fields = BodyWriter::new(&mut body[..len]);
        fields.put(&[self.token, self.kind.0, self.status.0]);
        let offset = if let Some(t) = self.timing {
            fields.put(&t.tsf.to_le_bytes());
            fields.put(&t.utc_offset);
            fields.put(&t.time_error);
            REPORT_FIXED_LEN + TIMING_LEN
        } else {
            REPORT_FIXED_LEN
        };
        self.data.write(&mut body[offset..len])?;
        let parsed = EventReport::parse(&body[..len])?;
        if parsed != self {
            return Err(WireError::InconsistentFields);
        }
        encode_element(element_id::EVENT_REPORT, &body[..len], out)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EventRequestFrame<'a> {
    pub dialog_token: u8,
    pub elements: Elements<'a>,
}
impl<'a> EventRequestFrame<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, WireError> {
        let (dialog_token, elements) = action_elements(bytes, WnmAction::EventRequest, false)?;
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
        for e in self
            .elements
            .iter()
            .filter(|e| e.id == element_id::EVENT_REQUEST)
        {
            let r = EventRequest::parse(e.body)?;
            if seen[usize::from(r.token)] {
                return Err(WireError::DuplicateMeasurementToken(r.token));
            }
            seen[usize::from(r.token)] = true;
        }
        Ok(())
    }
    pub fn requests(self) -> Result<impl Iterator<Item = EventRequest<'a>>, WireError> {
        self.validate()?;
        Ok(self
            .elements
            .iter()
            .filter(|e| e.id == element_id::EVENT_REQUEST)
            .map(|e| EventRequest::parse(e.body).expect("validated request")))
    }
    pub fn encode(self, out: &mut [u8]) -> Result<usize, WireError> {
        self.validate()?;
        encode_action(
            WnmAction::EventRequest,
            self.dialog_token,
            self.elements,
            out,
        )
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EventReportFrame<'a> {
    pub dialog_token: u8,
    pub elements: Elements<'a>,
}
impl<'a> EventReportFrame<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, WireError> {
        let (dialog_token, elements) = action_elements(bytes, WnmAction::EventReport, true)?;
        let value = Self {
            dialog_token,
            elements,
        };
        value.validate()?;
        Ok(value)
    }
    pub fn validate(self) -> Result<(), WireError> {
        for e in self
            .elements
            .iter()
            .filter(|e| e.id == element_id::EVENT_REPORT)
        {
            let r = EventReport::parse(e.body)?;
            if self.dialog_token == 0 && r.token != 0 {
                return Err(WireError::InconsistentFields);
            }
        }
        Ok(())
    }
    pub fn reports(self) -> Result<impl Iterator<Item = EventReport<'a>>, WireError> {
        self.validate()?;
        Ok(self
            .elements
            .iter()
            .filter(|e| e.id == element_id::EVENT_REPORT)
            .map(|e| EventReport::parse(e.body).expect("validated report")))
    }
    /// Ordinary reports; status 4 requires `encode_frequent` even for raw IEs.
    pub fn encode(self, out: &mut [u8]) -> Result<usize, WireError> {
        if self
            .reports()?
            .any(|r| r.status == EventReportStatus::FREQUENT_TRANSITION)
        {
            return Err(WireError::InconsistentFields);
        }
        encode_action(
            WnmAction::EventReport,
            self.dialog_token,
            self.elements,
            out,
        )
    }
    /// Select the transmit form explicitly for every frequent-transition IE.
    pub fn encode_frequent(
        self,
        format: FrequentTransitionFormat,
        out: &mut [u8],
    ) -> Result<usize, WireError> {
        for report in self.reports()? {
            if report.status == EventReportStatus::FREQUENT_TRANSITION
                && (report.kind != EventType::TRANSITION
                    || report.timing.is_some()
                        != (format == FrequentTransitionFormat::WithLastEvent))
            {
                return Err(WireError::InconsistentFields);
            }
        }
        encode_action(
            WnmAction::EventReport,
            self.dialog_token,
            self.elements,
            out,
        )
    }
}
#[cfg(test)]
mod tests;

/// Event Report Status, IEEE 802.11-2012 Table 8-137. Unknown values are retained.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EventReportStatus(pub u8);
impl EventReportStatus {
    pub const SUCCESSFUL: Self = Self(0);
    pub const FAILED: Self = Self(1);
    pub const REFUSED: Self = Self(2);
    pub const INCAPABLE: Self = Self(3);
    pub const FREQUENT_TRANSITION: Self = Self(4);
    pub const fn is_known(self) -> bool {
        matches!(
            self,
            Self::SUCCESSFUL
                | Self::FAILED
                | Self::REFUSED
                | Self::INCAPABLE
                | Self::FREQUENT_TRANSITION
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeerLinkStatus(pub u8);
impl PeerLinkStatus {
    pub const DIRECT_TERMINATED: Self = Self(0);
    pub const DIRECT_ACTIVE: Self = Self(1);
    pub const IBSS_TERMINATED: Self = Self(2);
    pub const IBSS_ACTIVE: Self = Self(3);
    pub const fn active(self) -> bool {
        matches!(self, Self::DIRECT_ACTIVE | Self::IBSS_ACTIVE)
    }
    pub const fn terminated_initiation(self) -> Option<Self> {
        match self {
            Self::DIRECT_TERMINATED => Some(Self::DIRECT_ACTIVE),
            Self::IBSS_TERMINATED => Some(Self::IBSS_ACTIVE),
            _ => None,
        }
    }
    pub const fn is_known(self) -> bool {
        self.active() || self.terminated_initiation().is_some()
    }
}

/// Success value shared by the underlying transition/authentication status codes.
pub struct EventResultCode;
impl EventResultCode {
    pub const SUCCESS: u16 = 0;
}
