//! Shared TFS negotiation and traffic matching for standalone and Sleep services.
use crate::tclas::{ClassifierPacket, matches_network_rule};
use crate::{Body, Error, Identities, LinkIdentity, OperationId};
use oer_ieee80211_mac::management::is_group_address;
use oer_ieee80211_mac::qos::WmmUserPriority;
use oer_ieee80211_mac::roaming as wire;
use oer_ieee80211_mac::roaming::*;

// IEEE 802.1Q TCI bit positions. The TCLAS PCP parameter has four low
// significant bits (IEEE 802.11-2012, 8.4.2.33), whereas a tag has three.
const VLAN_PCP_SHIFT: u32 = 13;
const VLAN_CFI_SHIFT: u32 = 12;
const TCLAS_PCP_MASK: u8 = 0x0f;
const VLAN_CFI_MASK: u8 = 0x01;
const VLAN_VID_MASK: u16 = 0x0fff;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrafficError {
    Protocol(Error),
    Full,
    IncompleteResponse,
    UnexpectedResponse,
    UnsupportedStatus(u8),
    UnsupportedClassifier(u8),
    UnsupportedProcessing(u8),
    InvalidClassifier,
    NoAcceptedFilters,
    UnsupportedAction(u8),
}
impl From<WireError> for TrafficError {
    fn from(value: WireError) -> Self {
        Self::Protocol(Error::Wire(value))
    }
}
impl From<Error> for TrafficError {
    fn from(value: Error) -> Self {
        Self::Protocol(value)
    }
}

pub fn validate_sleep_filters(
    request: Elements<'_>,
    response: Elements<'_>,
) -> Result<(), TrafficError> {
    let accepted = validate_tfs_negotiation(request, response)?;
    if accepted == 0 {
        return Err(TrafficError::NoAcceptedFilters);
    }
    Ok(())
}

/// Complete one-status-per-request correlation; empty requests cancel the service.
pub fn validate_tfs_negotiation(
    request: Elements<'_>,
    response: Elements<'_>,
) -> Result<usize, TrafficError> {
    validate_tfs_requests(request)?;
    validate_tfs_responses(response)?;
    let mut accepted = 0;
    for element in request
        .iter()
        .filter(|element| element.id == element_id::TFS_REQUEST)
    {
        let request = TfsRequest::parse(element.body)?;
        let mut seen = 0;
        for element in response
            .iter()
            .filter(|element| element.id == element_id::TFS_RESPONSE)
        {
            for status in TfsResponse::parse(element.body)?.statuses()? {
                if !status.status.is_known() {
                    return Err(TrafficError::UnsupportedStatus(status.status.0));
                }
                if status.id == request.id {
                    seen += 1;
                    accepted += usize::from(status.status == TfsStatusCode::ACCEPT);
                }
            }
        }
        if seen != 1 {
            return Err(TrafficError::IncompleteResponse);
        }
    }
    for element in response
        .iter()
        .filter(|element| element.id == element_id::TFS_RESPONSE)
    {
        for status in TfsResponse::parse(element.body)?.statuses()? {
            if !request
                .iter()
                .filter(|element| element.id == element_id::TFS_REQUEST)
                .any(|element| element.body[0] == status.id)
            {
                return Err(TrafficError::UnexpectedResponse);
            }
        }
    }
    Ok(accepted)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TrafficInput<'a> {
    pub fields: ClassifierPacket,
    /// Original plaintext MSDU/MMPDU payload immediately after the MAC header.
    /// TCLAS type 3 offsets are relative to this slice, before encryption.
    pub payload: &'a [u8],
    pub vlan_tci: Option<u16>,
    /// Proven by the packet decoder, not merely EtherType == EAPOL.
    pub eapol_key: bool,
}
#[derive(Clone, Copy)]
struct Filter {
    id: u8,
    action: TfsAction,
    enabled: bool,
}
struct Pending<const F: usize> {
    id: OperationId,
    delete: [bool; F],
    notify: bool,
    queue_frame: bool,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrafficDecision<const FILTERS: usize> {
    pub id: OperationId,
    pub discard_individual: bool,
    /// A matched frame or a notification must set the sleeping STA's TIM bit.
    pub set_tim: bool,
    /// Group delivery continues for the BSS; this lists notifications for this STA.
    pub notifications: [Option<u8>; FILTERS],
}
impl<const F: usize> TrafficDecision<F> {
    pub fn encode_notification(&self, buffer: &mut [u8]) -> Result<usize, WireError> {
        let mut ids = [0; wire::MAX_ELEMENT_BODY_LEN];
        let mut count = 0;
        for &id in self.notifications.iter().flatten() {
            if count == MAX_TFS_NOTIFY_IDS {
                return Err(WireError::ElementTooLong);
            }
            ids[count] = id;
            count += 1;
        }
        TfsNotify { ids: &ids[..count] }.encode(buffer)
    }
}

pub(crate) struct Installation<const B: usize, const F: usize> {
    request: Body<B>,
    response: Body<B>,
    filters: [Option<Filter>; F],
    active: bool,
}

/// Per-association accepted filters, with mandatory EAPOL-Key delivery.
/// Installation retains complete request/response elements. Traffic evaluation
/// stages Delete-after-match; apply it only after queue admission, so a failed
/// notification/buffer publication cannot lose the wake trigger.
pub struct TrafficFilters<const BYTES: usize, const FILTERS: usize> {
    ids: Identities,
    request: Body<BYTES>,
    response: Body<BYTES>,
    filters: [Option<Filter>; FILTERS],
    pending: Option<Pending<FILTERS>>,
    active: bool,
}
impl<const B: usize, const F: usize> TrafficFilters<B, F> {
    pub fn new(link: LinkIdentity) -> Self {
        Self {
            ids: Identities::new(link),
            request: Body::empty(),
            response: Body::empty(),
            filters: [None; F],
            pending: None,
            active: false,
        }
    }
    pub fn install(
        &mut self,
        link: LinkIdentity,
        request: Elements<'_>,
        response: Elements<'_>,
    ) -> Result<(), TrafficError> {
        let plan = self.prepare_install(link, request, response)?;
        self.apply_install(plan);
        Ok(())
    }
    pub(crate) fn prepare_install(
        &self,
        link: LinkIdentity,
        request: Elements<'_>,
        response: Elements<'_>,
    ) -> Result<Installation<B, F>, TrafficError> {
        if link != self.ids.link {
            return Err(Error::WrongOperation.into());
        }
        if self.pending.is_some() {
            return Err(Error::Busy.into());
        }
        let accepted_count = validate_tfs_negotiation(request, response)?;
        let mut filters = [None; F];
        let mut count = 0;
        for element in request
            .iter()
            .filter(|element| element.id == element_id::TFS_REQUEST)
        {
            let filter = TfsRequest::parse(element.body)?;
            let accepted = response
                .iter()
                .filter(|element| element.id == element_id::TFS_RESPONSE)
                .any(|element| {
                    TfsResponse::parse(element.body)
                        .expect("validated response")
                        .statuses()
                        .expect("validated response")
                        .any(|status| {
                            status.id == filter.id && status.status == TfsStatusCode::ACCEPT
                        })
                });
            if !accepted {
                continue;
            }
            if count == F {
                return Err(TrafficError::Full);
            }
            admit_filter(filter)?;
            filters[count] = Some(Filter {
                id: filter.id,
                action: filter.action,
                enabled: true,
            });
            count += 1;
        }
        let request = Body::copy(request.as_bytes())?;
        let response = Body::copy(response.as_bytes())?;
        Ok(Installation {
            request,
            response,
            filters,
            active: accepted_count != 0,
        })
    }
    pub(crate) fn apply_install(&mut self, plan: Installation<B, F>) {
        self.request = plan.request;
        self.response = plan.response;
        self.filters = plan.filters;
        self.active = plan.active;
    }
    pub fn clear(&mut self, link: LinkIdentity) -> Result<(), TrafficError> {
        if link != self.ids.link {
            return Err(Error::WrongOperation.into());
        }
        if self.pending.is_some() {
            return Err(Error::Busy.into());
        }
        self.active = false;
        self.filters = [None; F];
        self.request = Body::empty();
        self.response = Body::empty();
        Ok(())
    }
    pub fn negotiation(&self) -> (Elements<'_>, Elements<'_>) {
        (
            Elements::parse(self.request.bytes()).expect("validated request"),
            Elements::parse(self.response.bytes()).expect("validated response"),
        )
    }
    pub fn evaluate(
        &mut self,
        link: LinkIdentity,
        input: TrafficInput<'_>,
    ) -> Result<TrafficDecision<F>, TrafficError> {
        if link != self.ids.link {
            return Err(Error::WrongOperation.into());
        }
        if self.pending.is_some() {
            return Err(Error::Busy.into());
        }
        let individual = !is_group_address(input.fields.destination);
        let mut matched = !self.active
            || (input.eapol_key && individual && input.fields.destination == link.peer);
        let mut notifications = [None; F];
        let mut count = 0;
        let mut delete = [false; F];
        for (index, filter) in self
            .filters
            .iter()
            .enumerate()
            .filter_map(|(index, filter)| {
                filter
                    .filter(|filter| filter.enabled)
                    .map(|filter| (index, filter))
            })
        {
            let request = Elements::parse(self.request.bytes())?
                .iter()
                .find(|element| {
                    element.id == element_id::TFS_REQUEST && element.body[0] == filter.id
                })
                .expect("retained filter");
            if filter_matches(TfsRequest::parse(request.body)?, input)? {
                matched = true;
                delete[index] = filter.action.contains(TfsAction::DELETE_AFTER_MATCH);
                if filter.action.contains(TfsAction::NOTIFY) {
                    notifications[count] = Some(filter.id);
                    count += 1;
                }
            }
        }
        let id = self.ids.issue()?;
        let notify = count != 0;
        let queue_frame = individual && matched;
        self.pending = Some(Pending {
            id,
            delete,
            notify,
            queue_frame,
        });
        Ok(TrafficDecision {
            id,
            discard_individual: individual && !matched,
            set_tim: self.active && matched,
            notifications,
        })
    }
    /// Queue results refer to the exact original decision; a stale completion
    /// cannot delete filters installed by a later operation.
    pub fn applied(
        &mut self,
        id: OperationId,
        notification_queued: bool,
        matched_frame_queued: bool,
    ) -> Result<(), TrafficError> {
        let pending = self.pending.as_ref().ok_or(Error::NoPendingOperation)?;
        if pending.id != id {
            return Err(Error::WrongOperation.into());
        }
        if (pending.notify && !notification_queued)
            || (pending.queue_frame && !matched_frame_queued)
        {
            return Err(Error::NotAdmitted.into());
        }
        for (index, delete) in pending.delete.iter().enumerate() {
            if *delete {
                self.filters[index]
                    .as_mut()
                    .expect("matched filter")
                    .enabled = false;
            }
        }
        self.pending = None;
        Ok(())
    }
    pub fn cancel(&mut self, id: OperationId) -> Result<(), TrafficError> {
        if self.pending.as_ref().is_none_or(|pending| pending.id != id) {
            return Err(Error::WrongOperation.into());
        }
        self.pending = None;
        Ok(())
    }
}
pub(crate) fn admit_filter(filter: TfsRequest<'_>) -> Result<(), TrafficError> {
    if filter.action.unsupported_bits() != 0 {
        return Err(TrafficError::UnsupportedAction(filter.action.0));
    }
    let mut found = false;
    for sub in filter
        .subelements
        .iter()
        .filter(|element| element.id == tfs_subelement_id::FILTER)
    {
        found = true;
        let attributes = Elements::parse(sub.body)?;
        if attributes
            .unique(element_id::TCLAS_PROCESSING)?
            .is_some_and(|body| !tclas_processing::supported(body[0]))
        {
            return Err(TrafficError::UnsupportedProcessing(
                attributes
                    .unique(element_id::TCLAS_PROCESSING)?
                    .expect("present")[0],
            ));
        }
        for element in attributes
            .iter()
            .filter(|element| element.id == element_id::TCLAS)
        {
            let rule = TclasRule::parse(element.body)?;
            if WmmUserPriority::new(rule.user_priority).is_none() {
                return Err(TrafficError::InvalidClassifier);
            }
            match rule.parameters {
                TclasParameters::Ethernet { .. }
                | TclasParameters::VlanTci(_)
                | TclasParameters::Filter { .. }
                | TclasParameters::Vlan { .. } => {}
                TclasParameters::Ipv4 { .. } | TclasParameters::Ipv6 { .. }
                    if rule.mask & ip_fields::VERSION != 0 => {}
                TclasParameters::Other { kind, .. } => {
                    return Err(TrafficError::UnsupportedClassifier(kind));
                }
                _ => return Err(TrafficError::InvalidClassifier),
            }
        }
    }
    if !found {
        return Err(TrafficError::InvalidClassifier);
    }
    Ok(())
}
pub(crate) fn filter_matches(
    filter: TfsRequest<'_>,
    input: TrafficInput<'_>,
) -> Result<bool, TrafficError> {
    let mut matched = false;
    for sub in filter
        .subelements
        .iter()
        .filter(|element| element.id == tfs_subelement_id::FILTER)
    {
        let attributes = Elements::parse(sub.body)?;
        let any = attributes
            .unique(element_id::TCLAS_PROCESSING)?
            .is_some_and(|body| body[0] == tclas_processing::ANY);
        let mut result = !any;
        for element in attributes
            .iter()
            .filter(|element| element.id == element_id::TCLAS)
        {
            let rule = TclasRule::parse(element.body)?;
            let matches = match rule.parameters {
                TclasParameters::VlanTci(tci) => input
                    .vlan_tci
                    .is_some_and(|actual| rule.mask & vlan_fields::TCI == 0 || actual == tci),
                TclasParameters::Vlan { pcp, cfi, vid } => input.vlan_tci.is_some_and(|actual| {
                    (rule.mask & vlan_fields::PCP == 0
                        || ((actual >> VLAN_PCP_SHIFT) as u8) == pcp & TCLAS_PCP_MASK)
                        && (rule.mask & vlan_fields::CFI == 0
                            || ((actual >> VLAN_CFI_SHIFT) as u8) & VLAN_CFI_MASK
                                == cfi & VLAN_CFI_MASK)
                        && (rule.mask & vlan_fields::VID == 0
                            || actual & VLAN_VID_MASK == vid & VLAN_VID_MASK)
                }),
                TclasParameters::Filter {
                    offset,
                    value,
                    mask,
                } => input
                    .payload
                    .get(usize::from(offset)..usize::from(offset) + value.len())
                    .is_some_and(|actual| {
                        actual
                            .iter()
                            .zip(value)
                            .zip(mask)
                            .all(|((&a, &b), &m)| a & m == b & m)
                    }),
                _ => matches_network_rule(rule, input.fields),
            };
            if any {
                result |= matches;
            } else {
                result &= matches;
            }
        }
        matched |= result;
    }
    Ok(matched)
}
