use super::*;

#[derive(Clone)]
struct Subscription<const ATTR_BYTES: usize> {
    peer: LinkIdentity,
    id: u8,
    attributes: Body<ATTR_BYTES>,
}
struct Change<const ATTR_BYTES: usize> {
    index: usize,
    value: Option<Subscription<ATTR_BYTES>>,
}

/// AP admission of QoS/vendor attributes remains an explicit caller decision.
/// Removal always terminates; its supplied sequence is actual group delivery
/// evidence, or an explicit Unsupported/NotGroupTransmitted wire value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DmsAdmission<'a> {
    Accept,
    Deny { alternative: Option<Elements<'a>> },
    Remove { last_sequence: LastSequenceControl },
}
pub struct DmsTransaction<
    const ATTR_BYTES: usize,
    const OPERATIONS: usize,
    const RESPONSE_BYTES: usize,
> {
    pub request: OperationId,
    bss: BssIdentity,
    capacity: usize,
    revision: u64,
    changes: [Option<Change<ATTR_BYTES>>; OPERATIONS],
    response: Body<RESPONSE_BYTES>,
}
impl<const A: usize, const O: usize, const R: usize> DmsTransaction<A, O, R> {
    pub fn response(&self) -> DmsResponse<'_> {
        DmsResponse::parse(self.response.bytes()).expect("prepared response")
    }
    pub fn bytes(&self) -> &[u8] {
        self.response.bytes()
    }
}

/// BSS-wide registry: identical classifier sets share a DMSID across peers;
/// complete QoS/vendor attributes stay on each peer's subscription.
// CAPABILITY: wifi-tsf-beacon-monitoring-and-power-saving-directed-multicast-service-802-11v
pub struct DmsRegistry<const MEMBERS: usize, const ATTR_BYTES: usize> {
    bss: BssIdentity,
    members: [Option<Subscription<ATTR_BYTES>>; MEMBERS],
    revision: u64,
}
impl<const MEMBERS: usize, const ATTR_BYTES: usize> DmsRegistry<MEMBERS, ATTR_BYTES> {
    pub fn new(bss: BssIdentity) -> Result<Self, DmsError> {
        if !crate::valid_peer_address(bss.bssid) {
            return Err(Error::InvalidTarget.into());
        }
        Ok(Self {
            bss,
            members: core::array::from_fn(|_| None),
            revision: 0,
        })
    }
    pub const fn bss(&self) -> BssIdentity {
        self.bss
    }
    fn effective<'a>(
        &'a self,
        changes: &'a [Option<Change<ATTR_BYTES>>],
        index: usize,
    ) -> Option<&'a Subscription<ATTR_BYTES>> {
        if let Some(change) = changes
            .iter()
            .flatten()
            .rev()
            .find(|change| change.index == index)
        {
            change.value.as_ref()
        } else {
            self.members[index].as_ref()
        }
    }
    pub fn subscriptions(&self) -> impl Iterator<Item = (LinkIdentity, u8, Elements<'_>)> {
        self.members.iter().flatten().map(|member| {
            (
                member.peer,
                member.id,
                Elements::parse(member.attributes.bytes()).expect("validated attributes"),
            )
        })
    }
    /// Prepare the entire response and all changes without mutating the BSS.
    /// Multiple wire DMS elements are emitted as needed; no descriptor is cut.
    pub fn prepare<'a, const OPERATIONS: usize, const RESPONSE_BYTES: usize>(
        &self,
        id: OperationId,
        request: DmsRequest<'a>,
        mut decide: impl FnMut(DmsDescriptor<'a>) -> DmsAdmission<'a>,
    ) -> Result<DmsTransaction<ATTR_BYTES, OPERATIONS, RESPONSE_BYTES>, DmsError> {
        request.validate()?;
        self.revision
            .checked_add(1)
            .ok_or(Error::IdentityExhausted)?;
        if !crate::valid_peer_address(id.link.peer) {
            return Err(Error::InvalidTarget.into());
        }
        if self.members.iter().flatten().any(|member| {
            member.peer.peer == id.link.peer && member.peer.generation != id.link.generation
        }) {
            return Err(DmsError::StalePeer);
        }
        if request.descriptors()?.count() > OPERATIONS {
            return Err(DmsError::OperationCapacity);
        }
        let mut changes: [Option<Change<ATTR_BYTES>>; OPERATIONS] = core::array::from_fn(|_| None);
        let mut response = Body::<RESPONSE_BYTES>::empty();
        if RESPONSE_BYTES < ACTION_HEADER_LEN {
            return Err(Error::FrameTooLarge {
                required: ACTION_HEADER_LEN,
                capacity: RESPONSE_BYTES,
            }
            .into());
        }
        response.bytes[..ACTION_HEADER_LEN].copy_from_slice(&[
            WNM_CATEGORY,
            WnmAction::DmsResponse as u8,
            request.dialog_token,
        ]);
        response.len = ACTION_HEADER_LEN;
        let mut outer = None;
        for (number, descriptor) in request.descriptors()?.enumerate() {
            let decision = decide(descriptor);
            if !descriptor.request_type.is_known() {
                return Err(DmsError::UnsupportedRequest(descriptor.request_type.0));
            }
            for earlier in request.descriptors()?.take(number) {
                if (descriptor.dms_id != 0 && earlier.dms_id == descriptor.dms_id)
                    || (descriptor.request_type == DmsRequestType::ADD
                        && earlier.request_type == DmsRequestType::ADD
                        && equivalent_dms_classifiers(descriptor.attributes, earlier.attributes)?)
                {
                    return Err(DmsError::ConflictingRequest);
                }
            }
            let existing = (0..MEMBERS).find(|index| {
                self.effective(&changes, *index)
                    .is_some_and(|member| member.peer == id.link && member.id == descriptor.dms_id)
            });
            let status = match descriptor.request_type {
                DmsRequestType::REMOVE => {
                    let DmsAdmission::Remove { last_sequence } = decision else {
                        return Err(DmsError::UnexpectedResponse);
                    };
                    if let Some(index) = existing {
                        changes[number] = Some(Change { index, value: None });
                    }
                    DmsStatus {
                        dms_id: descriptor.dms_id,
                        response_type: DmsResponseType::TERMINATE,
                        last_sequence_control: last_sequence.encode()?,
                        attributes: Elements::EMPTY,
                    }
                }
                DmsRequestType::CHANGE => {
                    let index = existing.ok_or(DmsError::UnknownStream(descriptor.dms_id))?;
                    let member = self.effective(&changes, index).expect("existing member");
                    let attributes = merge_attributes::<ATTR_BYTES>(
                        Elements::parse(member.attributes.bytes())?,
                        descriptor.attributes,
                    )?;
                    if attributes.bytes() == member.attributes.bytes() {
                        return Err(DmsError::NoChange);
                    }
                    match decision {
                        DmsAdmission::Accept => {
                            changes[number] = Some(Change {
                                index,
                                value: Some(Subscription {
                                    peer: id.link,
                                    id: descriptor.dms_id,
                                    attributes,
                                }),
                            });
                            DmsStatus {
                                dms_id: descriptor.dms_id,
                                response_type: DmsResponseType::ACCEPT,
                                last_sequence_control: LastSequenceControl::UNSUPPORTED_VALUE,
                                attributes: descriptor.attributes,
                            }
                        }
                        DmsAdmission::Deny { alternative } => DmsStatus {
                            dms_id: descriptor.dms_id,
                            response_type: DmsResponseType::DENIED,
                            last_sequence_control: LastSequenceControl::UNSUPPORTED_VALUE,
                            attributes: alternative.unwrap_or(descriptor.attributes),
                        },
                        _ => return Err(DmsError::UnexpectedResponse),
                    }
                }
                _ => match decision {
                    DmsAdmission::Accept => {
                        admit_classifiers(descriptor.attributes)?;
                        let mut shared = None;
                        let mut same_peer = None;
                        for index in 0..MEMBERS {
                            if let Some(member) = self.effective(&changes, index)
                                && equivalent_dms_classifiers(
                                    Elements::parse(member.attributes.bytes())?,
                                    descriptor.attributes,
                                )?
                            {
                                shared = Some(member.id);
                                if member.peer == id.link {
                                    same_peer = Some(index);
                                }
                            }
                        }
                        let assigned = if let Some(shared) = shared {
                            shared
                        } else {
                            (1..=u8::MAX)
                                .find(|candidate| {
                                    !(0..MEMBERS).any(|index| {
                                        self.effective(&changes, index)
                                            .is_some_and(|member| member.id == *candidate)
                                    })
                                })
                                .ok_or(DmsError::Full)?
                        };
                        let index = same_peer
                            .or_else(|| {
                                (0..MEMBERS)
                                    .find(|index| self.effective(&changes, *index).is_none())
                            })
                            .ok_or(DmsError::Full)?;
                        let attributes = attributes_body(descriptor.attributes)?;
                        if let Some(member) = self.effective(&changes, index)
                            && member.peer == id.link
                            && member.attributes.bytes() != attributes.bytes()
                        {
                            return Err(DmsError::ConflictingStream(assigned));
                        }
                        changes[number] = Some(Change {
                            index,
                            value: Some(Subscription {
                                peer: id.link,
                                id: assigned,
                                attributes,
                            }),
                        });
                        DmsStatus {
                            dms_id: assigned,
                            response_type: DmsResponseType::ACCEPT,
                            last_sequence_control: LastSequenceControl::UNSUPPORTED_VALUE,
                            attributes: descriptor.attributes,
                        }
                    }
                    DmsAdmission::Deny { alternative } => DmsStatus {
                        dms_id: 0,
                        response_type: DmsResponseType::DENIED,
                        last_sequence_control: LastSequenceControl::UNSUPPORTED_VALUE,
                        attributes: alternative.unwrap_or(descriptor.attributes),
                    },
                    _ => return Err(DmsError::UnexpectedResponse),
                },
            };
            append_status(&mut response, &mut outer, status)?;
        }
        Ok(DmsTransaction {
            request: id,
            bss: self.bss,
            capacity: MEMBERS,
            revision: self.revision,
            changes,
            response,
        })
    }
    pub(crate) fn validate_transaction<const O: usize, const R: usize>(
        &self,
        transaction: &DmsTransaction<ATTR_BYTES, O, R>,
    ) -> Result<(), DmsError> {
        if self.bss != transaction.bss
            || MEMBERS != transaction.capacity
            || self.revision != transaction.revision
        {
            return Err(DmsError::StaleTransaction);
        }
        Ok(())
    }
    pub(crate) fn prepare_termination<const RESPONSE_BYTES: usize>(
        &self,
        request: OperationId,
        dms_id: u8,
        last_sequence: LastSequenceControl,
    ) -> Result<DmsTransaction<ATTR_BYTES, 1, RESPONSE_BYTES>, DmsError> {
        self.revision
            .checked_add(1)
            .ok_or(Error::IdentityExhausted)?;
        let index = self
            .members
            .iter()
            .position(|member| {
                member
                    .as_ref()
                    .is_some_and(|member| member.peer == request.link && member.id == dms_id)
            })
            .ok_or(DmsError::UnknownStream(dms_id))?;
        let mut response = Body::copy(&[
            WNM_CATEGORY,
            WnmAction::DmsResponse as u8,
            AUTONOMOUS_DIALOG_TOKEN,
        ])?;
        append_status(
            &mut response,
            &mut None,
            DmsStatus {
                dms_id,
                response_type: DmsResponseType::TERMINATE,
                last_sequence_control: last_sequence.encode()?,
                attributes: Elements::EMPTY,
            },
        )?;
        Ok(DmsTransaction {
            request,
            bss: self.bss,
            capacity: MEMBERS,
            revision: self.revision,
            changes: [Some(Change { index, value: None })],
            response,
        })
    }
    pub(crate) fn apply<const O: usize, const R: usize>(
        &mut self,
        transaction: DmsTransaction<ATTR_BYTES, O, R>,
    ) {
        for change in transaction.changes.into_iter().flatten() {
            self.members[change.index] = change.value;
        }
        self.revision = transaction.revision + 1;
    }
    pub fn commit<const O: usize, const R: usize>(
        &mut self,
        transaction: DmsTransaction<ATTR_BYTES, O, R>,
    ) -> Result<(), DmsError> {
        self.validate_transaction(&transaction)?;
        self.apply(transaction);
        Ok(())
    }
    /// Association retirement removes only that peer's exact generation.
    pub fn remove_peer(&mut self, link: LinkIdentity) -> Result<usize, DmsError> {
        let revision = self
            .revision
            .checked_add(1)
            .ok_or(Error::IdentityExhausted)?;
        let mut count = 0;
        for slot in &mut self.members {
            if slot.as_ref().is_some_and(|member| member.peer == link) {
                *slot = None;
                count += 1;
            }
        }
        self.revision = revision;
        Ok(count)
    }
    pub fn recipients(
        &self,
        packet: DmsPacket,
    ) -> Result<impl Iterator<Item = LinkIdentity>, DmsError> {
        // Validate every stored set before exposing the iterator; membership
        // changes cannot overlap its immutable borrow.
        for member in self.members.iter().flatten() {
            admit_classifiers(Elements::parse(member.attributes.bytes())?)?;
        }
        Ok(self
            .members
            .iter()
            .flatten()
            .enumerate()
            .filter_map(move |(index, member)| {
                let matches = matches_classifiers(
                    Elements::parse(member.attributes.bytes()).expect("validated attributes"),
                    packet,
                )
                .expect("admitted classifiers");
                if !matches {
                    return None;
                }
                // One station gets one copy even if several subscriptions match.
                let seen = self.members.iter().flatten().take(index).any(|previous| {
                    previous.peer == member.peer
                        && matches_classifiers(
                            Elements::parse(previous.attributes.bytes())
                                .expect("validated attributes"),
                            packet,
                        )
                        .expect("admitted classifiers")
                });
                (!seen).then_some(member.peer)
            }))
    }
    /// The complete current associated-peer list decides whether a group copy
    /// is still required in addition to individually addressed A-MSDUs.
    pub fn group_delivery_required(
        &self,
        packet: DmsPacket,
        associated: &[LinkIdentity],
    ) -> Result<bool, DmsError> {
        for peer in associated {
            let mut covered = false;
            for member in self
                .members
                .iter()
                .flatten()
                .filter(|member| member.peer == *peer)
            {
                covered |=
                    matches_classifiers(Elements::parse(member.attributes.bytes())?, packet)?;
            }
            if !covered {
                return Ok(true);
            }
        }
        Ok(false)
    }
}
