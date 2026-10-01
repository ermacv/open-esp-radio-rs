//! Original Event Request subelement namespaces and decoded conditions.
use super::*;
use crate::roaming::Element;
use crate::roaming::codec::BodyReader;

pub mod transition_condition_id {
    pub const TARGET_BSSID: u8 = 0;
    pub const SOURCE_BSSID: u8 = 1;
    pub const MINIMUM_TIME: u8 = 2;
    pub const OUTCOME: u8 = 3;
    pub const FREQUENT_TRANSITION: u8 = 4;
}
pub mod rsna_condition_id {
    pub const TARGET_BSSID: u8 = 0;
    pub const AKM_SUITE: u8 = 1;
    pub const EAP_METHOD: u8 = 2;
    pub const OUTCOME: u8 = 3;
}
pub mod peer_link_condition_id {
    pub const PEER_ADDRESS: u8 = 0;
    pub const OPERATING_CLASS_CHANNEL: u8 = 1;
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EventOutcomeFilter(pub u8);
impl EventOutcomeFilter {
    pub const SUCCESS: Self = Self(1);
    pub const FAILURE: Self = Self(2);
    pub const fn allows(self, successful: bool) -> bool {
        let flag = if successful {
            Self::SUCCESS
        } else {
            Self::FAILURE
        };
        self.0 & flag.0 != 0
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrequentTransitionCondition {
    pub minimum_count: u8,
    pub interval_tu: u16,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EventChannelCondition {
    pub operating_class: u8,
    pub channel: u8,
}
impl EventChannelCondition {
    pub const ANY_CHANNEL: u8 = 0;
    pub const fn matches(self, operating_class: u8, channel: u8) -> bool {
        self.operating_class == operating_class
            && (self.channel == Self::ANY_CHANNEL || self.channel == channel)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventCondition<'a> {
    TransitionTarget(MacAddress),
    TransitionSource(MacAddress),
    MinimumTransitionTime(u16),
    TransitionOutcome(EventOutcomeFilter),
    FrequentTransition(FrequentTransitionCondition),
    RsnaTarget(MacAddress),
    RsnaAkm([u8; size_of::<u32>()]),
    RsnaEap(EapMethod),
    RsnaOutcome(EventOutcomeFilter),
    PeerAddress(MacAddress),
    PeerChannel(EventChannelCondition),
    Unknown(Element<'a>),
}
impl<'a> EventCondition<'a> {
    pub(super) fn parse(kind: EventType, element: Element<'a>) -> Result<Self, WireError> {
        let mut fields = BodyReader::new(element.body);
        let value = match kind {
            EventType::TRANSITION => match element.id {
                transition_condition_id::TARGET_BSSID => Self::TransitionTarget(fields.array()?),
                transition_condition_id::SOURCE_BSSID => Self::TransitionSource(fields.array()?),
                transition_condition_id::MINIMUM_TIME => {
                    Self::MinimumTransitionTime(u16::from_le_bytes(fields.array()?))
                }
                transition_condition_id::OUTCOME => {
                    Self::TransitionOutcome(EventOutcomeFilter(fields.u8()?))
                }
                transition_condition_id::FREQUENT_TRANSITION => {
                    Self::FrequentTransition(FrequentTransitionCondition {
                        minimum_count: fields.u8()?,
                        interval_tu: u16::from_le_bytes(fields.array()?),
                    })
                }
                _ => Self::Unknown(element),
            },
            EventType::RSNA => match element.id {
                rsna_condition_id::TARGET_BSSID => Self::RsnaTarget(fields.array()?),
                rsna_condition_id::AKM_SUITE => Self::RsnaAkm(fields.array()?),
                rsna_condition_id::EAP_METHOD => {
                    let method = EapMethod::parse(fields.remaining())?;
                    fields.take(method.encoded_len())?;
                    Self::RsnaEap(method)
                }
                rsna_condition_id::OUTCOME => Self::RsnaOutcome(EventOutcomeFilter(fields.u8()?)),
                _ => Self::Unknown(element),
            },
            EventType::PEER_LINK => match element.id {
                peer_link_condition_id::PEER_ADDRESS => Self::PeerAddress(fields.array()?),
                peer_link_condition_id::OPERATING_CLASS_CHANNEL => {
                    Self::PeerChannel(EventChannelCondition {
                        operating_class: fields.u8()?,
                        channel: fields.u8()?,
                    })
                }
                _ => Self::Unknown(element),
            },
            _ => Self::Unknown(element),
        };
        if !matches!(value, Self::Unknown(_)) && !fields.remaining().is_empty() {
            return Err(WireError::InvalidElementLength(element.id));
        }
        Ok(value)
    }
}
