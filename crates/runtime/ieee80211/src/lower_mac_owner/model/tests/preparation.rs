use super::*;
use oer_ieee80211_lower_mac::{AggregateAttempt, AmpduPhase, MpduStartSpacing, PhaseAggregate};

fn frame<'p>(
    phase: &ModelPhase<'p, '_>,
    len: usize,
    protected: bool,
) -> ClassFrame<'p, ModelSmall, ModelCaps> {
    let TxFrame::Individual { frame, plan } = data_frame(phase, protected) else {
        unreachable!()
    };
    ClassFrame::new(TxFrame::Individual {
        frame: SizedFrame::new(*frame.header(), MpduLength::new(len).unwrap()).unwrap(),
        plan,
    })
    .ok()
    .unwrap()
}

fn try_source<'p, 'id: 'p>(
    phase: &mut ModelPhase<'p, 'id>,
    ticket: Slot<'id, ModelTickets>,
    source: PoolSource<'p, 'id, ModelTypes, ModelSmall, ModelCaps>,
    key: Option<&ModelPairwiseKey<'id, Station>>,
) -> Result<
    (),
    (
        PreparationError,
        Slot<'id, ModelTickets>,
        ModelAnyBuffer<'id>,
        Option<ModelBody>,
    ),
> {
    let source = match <ModelTypes as TxBufferPool<'id, ModelSmall>>::prepare(source) {
        Ok(source) => source,
        Err(refused) => {
            let (buffer, body) = match refused.operation {
                TxSource::Fresh { buffer, body, .. } => (ModelAnyBuffer::Small(buffer), body),
                TxSource::Retransmission { mpdu, .. } => {
                    (ModelAnyBuffer::SmallNumbered(mpdu), None)
                }
            };
            return Err((refused.reason, ticket, buffer, body));
        }
    };
    match phase.station().submit(oer_ieee80211_lower_mac::Attempt {
        context: (),
        ticket,
        source,
        access_category: WmmAccessCategory::BestEffort,
        protection: Protection::None,
        key: key.map(AttemptKey::Pairwise),
        power: SupportedPower::new(TxPower::Calibrated).unwrap(),
        coex: SupportedCoex::new(CoexPriority::Normal).unwrap(),
    }) {
        Ok(()) => Ok(()),
        Err(SubmitError::Refused(refused)) => {
            let operation = refused.operation;
            let (buffer, body) = operation.source.into_parts();
            Err((refused.reason, operation.ticket, buffer, body))
        }
        Err(SubmitError::Poisoned(_)) => panic!("preparation must not poison the model"),
    }
}

struct Boundaries;

impl Work for Boundaries {
    async fn work<'p, 'id: 'p>(
        self,
        phase: &mut ModelPhase<'p, 'id>,
        resources: ModelResources<'id>,
    ) {
        let ModelResources {
            mut tickets,
            mut small,
            station_pairwise: [slot, _],
            ..
        } = resources;
        let key = phase.station().install_pairwise_key(
            slot,
            IndividualAddress::new(PEER).unwrap(),
            Ccmp128Key([7; 16]),
        );
        let mut ticket = tickets.remove(0);
        let mut buffer = small.remove(0);
        for (len, body_len, protected, keyed, refusal) in [
            (40, 16, true, true, Some(PreparationError::BodyOverlap)),
            (
                31,
                0,
                true,
                true,
                Some(PreparationError::CcmpHeaderTooShort),
            ),
            (40, 17, false, false, Some(PreparationError::BodyOverlap)),
            (
                40,
                0,
                true,
                false,
                Some(PreparationError::ProtectionMismatch),
            ),
            (
                40,
                0,
                false,
                true,
                Some(PreparationError::ProtectionMismatch),
            ),
            (32, 0, true, true, None),
            (40, 8, true, true, None),
            (40, 16, false, false, None),
        ] {
            buffer.memory.fill(0xa5);
            let pointer = buffer.memory.as_ptr();
            let slot = buffer.slot.index();
            let ticket_index = ticket.index();
            let body = ModelBody(alloc::vec![0x5a; body_len]);
            let body_pointer = body.0.as_ptr();
            let before = phase.held.commands.items.borrow().len();
            let pn_before = key.identity.0.last.get();
            let frame = frame(phase, len, protected);
            let result = try_source(
                phase,
                ticket,
                TxSource::Fresh {
                    buffer,
                    frame,
                    body: Some(body),
                },
                keyed.then_some(&key),
            );
            if let Some(reason) = refusal {
                let Err((returned_reason, returned_ticket, returned, body)) = result else {
                    panic!("typed refusal")
                };
                assert_eq!(returned_reason, reason);
                assert_eq!(key.identity.0.last.get(), pn_before);
                assert_eq!(phase.held.commands.items.borrow().len(), before);
                assert!(phase.held.events.is_empty(), "no synthetic terminal");
                ticket = returned_ticket;
                let ModelAnyBuffer::Small(returned) = returned else {
                    unreachable!()
                };
                buffer = returned;
                if reason != PreparationError::ProtectionMismatch {
                    assert!(buffer.memory.iter().all(|byte| *byte == 0xa5));
                }
                assert_eq!(body.unwrap().0.as_ptr(), body_pointer);
            } else {
                assert!(result.is_ok());
                let (returned_ticket, mut buffers, bodies, _) = aggregate_completion(phase).await;
                ticket = returned_ticket;
                buffer = match buffers.remove(0) {
                    ModelAnyBuffer::SmallNumbered(mpdu) => {
                        assert_eq!(mpdu.body().unwrap().0.as_ptr(), body_pointer);
                        assert_eq!(
                            &mpdu.buffer.memory[len - body_len..len],
                            &[0x5a; 16][..body_len]
                        );
                        assert_eq!(mpdu.packet_number(), pn_before + 1);
                        let (buffer, body) =
                            <ModelTypes as TxBufferPool<'id, ModelSmall>>::renew(mpdu);
                        assert_eq!(body.unwrap().0.as_ptr(), body_pointer);
                        buffer
                    }
                    ModelAnyBuffer::Small(buffer) => {
                        assert_eq!(bodies.len(), 1);
                        assert_eq!(bodies[0].1.0.as_ptr(), body_pointer);
                        buffer
                    }
                    _ => unreachable!(),
                };
            }
            assert_eq!(ticket.index(), ticket_index);
            assert_eq!(buffer.slot.index(), slot);
            assert_eq!(buffer.memory.as_ptr(), pointer);
        }
        // The class proof is not a substitute for checking actual memory.
        buffer.memory = alloc::vec![0xa5; 30].into_boxed_slice();
        let pointer = buffer.memory.as_ptr();
        let pn = key.identity.0.last.get();
        let frame = frame(phase, 40, true);
        let result = try_source(
            phase,
            ticket,
            TxSource::Fresh {
                buffer,
                frame,
                body: None,
            },
            Some(&key),
        );
        let Err((reason, _ticket, buffer, _body)) = result else {
            panic!("actual memory refuses the frame")
        };
        assert_eq!(reason, PreparationError::BufferTooSmall);
        assert_eq!(buffer.memory().as_ptr(), pointer);
        assert_eq!(key.identity.0.last.get(), pn);
        assert!(phase.held.events.is_empty());
    }
}

#[test]
fn body_and_ccmp_boundaries_are_checked_before_admission_or_packet_number_allocation() {
    let (backend, _) = backend();
    block_on(run_port(backend, Body(Boundaries)));
}

struct RetryIdentity;

impl Work for RetryIdentity {
    async fn work<'p, 'id: 'p>(
        self,
        phase: &mut ModelPhase<'p, 'id>,
        resources: ModelResources<'id>,
    ) {
        let ModelResources {
            mut tickets,
            mut small,
            station_pairwise: [slot, _],
            ..
        } = resources;
        let key = phase.station().install_pairwise_key(
            slot,
            IndividualAddress::new(PEER).unwrap(),
            Ccmp128Key([7; 16]),
        );
        let body = ModelBody(alloc::vec![0x66; 8]);
        let body_pointer = body.0.as_ptr();
        let source = TxSource::Fresh {
            buffer: small.remove(0),
            frame: frame(phase, 40, true),
            body: Some(body),
        };
        submit_frame::<ModelSmall>(phase, tickets.remove(0), source, Some(&key));
        let (ticket, buffer) = completion(phase).await;
        let ModelAnyBuffer::SmallNumbered(mpdu) = buffer else {
            unreachable!()
        };
        let before = mpdu.buffer.memory[..40].to_vec();
        let pn = mpdu.packet_number();
        let plan = RetryPlan::new(RetryStage {
            rate: phase.rate(PhyRate::Legacy(LegacyRate::Ofdm24M)).unwrap(),
            transmissions: core::num::NonZeroU8::MIN,
            protection: Protection::None,
        });
        submit_frame::<ModelSmall>(
            phase,
            ticket,
            TxSource::Retransmission {
                mpdu,
                plan: RetransmissionPlan::Individual(plan),
            },
            Some(&key),
        );
        let (ticket, buffer) = completion(phase).await;
        let ModelAnyBuffer::SmallNumbered(mpdu) = buffer else {
            unreachable!()
        };
        let mut expected = before;
        expected[1] |= 0x08;
        assert_eq!(&mpdu.buffer.memory[..40], expected.as_slice());
        assert_eq!(mpdu.packet_number(), pn);
        assert_eq!(mpdu.body().unwrap().0.as_ptr(), body_pointer);
        let index = key.identity.0.index;
        phase.station().remove_pairwise_key(key);
        let slot = loop {
            let Ok(Ok(LowerMacEvent::KeyRemoved(Removed::Station(RemovedKey::Pairwise(slot))))) =
                phase.next_event().await
            else {
                continue;
            };
            break slot;
        };
        let replacement = phase.station().install_pairwise_key(
            slot,
            IndividualAddress::new(PEER).unwrap(),
            Ccmp128Key([7; 16]),
        );
        assert_eq!(replacement.identity.0.index, index);
        let TxFrame::Individual { plan, .. } = data_frame(phase, true) else {
            unreachable!()
        };
        let result = phase.station().submit(oer_ieee80211_lower_mac::Attempt {
            context: (),
            ticket,
            source: <ModelTypes as TxBufferPool<'id, ModelSmall>>::prepare(
                TxSource::Retransmission {
                    mpdu,
                    plan: RetransmissionPlan::Individual(plan),
                },
            )
            .ok()
            .unwrap(),
            access_category: WmmAccessCategory::Video,
            protection: Protection::None,
            key: Some(AttemptKey::Pairwise(&replacement)),
            power: SupportedPower::new(TxPower::Calibrated).unwrap(),
            coex: SupportedCoex::new(CoexPriority::Normal).unwrap(),
        });
        let Err(SubmitError::Refused(refused)) = result else {
            panic!("new key installation refuses the old MPDU")
        };
        assert_eq!(refused.reason, PreparationError::RetransmissionKey);
        let (returned, body) = refused.operation.source.into_parts();
        assert!(body.is_none());
        let ModelAnyBuffer::SmallNumbered(mpdu) = returned else {
            unreachable!()
        };
        assert_eq!(mpdu.packet_number(), pn);
        assert_eq!(mpdu.body().unwrap().0.as_ptr(), body_pointer);
        assert_eq!(replacement.identity.0.last.get(), 0);
        assert!(phase.held.events.is_empty());

        // The same installation check applies when the retry is aggregated.
        let ht = oer_ieee80211_mac::phy::HtRate::new(
            HtMcs::new(7).unwrap(),
            oer_ieee80211_mac::phy::PpduBandwidth::Mhz20,
            false,
        )
        .unwrap();
        let rate = phase.rate(PhyRate::Ht(ht)).unwrap();
        let mut station = phase.station();
        let rate = station.ampdu_rate(rate).ok().unwrap();
        let mut aggregate = station.aggregate(Tid::new(0).unwrap(), rate);
        aggregate.push(prepare_retry(mpdu, &rate)).ok().unwrap();
        let result = station.submit_ampdu(AggregateAttempt {
            context: (),
            ticket: refused.operation.ticket,
            aggregate,
            protection: Protection::None,
            key: Some(&replacement),
            power: SupportedPower::new(TxPower::Calibrated).unwrap(),
            coex: SupportedCoex::new(CoexPriority::Normal).unwrap(),
            min_mpdu_start_spacing: MpduStartSpacing::new(0).unwrap(),
        });
        let Err(SubmitError::Refused(refused)) = result else {
            panic!("aggregate keeps original key identity")
        };
        assert_eq!(refused.reason, PreparationError::RetransmissionKey);
        assert_eq!(refused.operation.aggregate.subframes(), 1);
        assert_eq!(replacement.identity.0.last.get(), 0);
        let (mut buffers, bodies) = refused.operation.aggregate.into_parts();
        assert!(bodies.is_empty());
        let ModelAnyBuffer::SmallNumbered(mpdu) = buffers.remove(0) else {
            unreachable!()
        };
        let (_, body) = <ModelTypes as TxBufferPool<'id, ModelSmall>>::renew(mpdu);
        assert_eq!(body.unwrap().0.as_ptr(), body_pointer);
        assert!(phase.held.events.is_empty());
    }
}

#[test]
fn retry_owns_its_mpdu_and_body_allows_a_new_rate_and_refuses_a_reinstalled_key() {
    let (backend, _) = backend();
    block_on(run_port(backend, Body(RetryIdentity)));
}

struct AggregateBoundaries;

impl Work for AggregateBoundaries {
    async fn work<'p, 'id: 'p>(
        self,
        phase: &mut ModelPhase<'p, 'id>,
        resources: ModelResources<'id>,
    ) {
        let ModelResources {
            mut tickets,
            mut small,
            station_pairwise: [slot, _],
            ..
        } = resources;
        let key = phase.station().install_pairwise_key(
            slot,
            IndividualAddress::new(PEER).unwrap(),
            Ccmp128Key([7; 16]),
        );
        let ht = oer_ieee80211_mac::phy::HtRate::new(
            HtMcs::new(7).unwrap(),
            oer_ieee80211_mac::phy::PpduBandwidth::Mhz20,
            false,
        )
        .unwrap();
        let rate = phase.rate(PhyRate::Ht(ht)).unwrap();
        let mut station = phase.station();
        let rate = station.ampdu_rate(rate).ok().unwrap();
        let mut aggregate = station.aggregate(Tid::new(0).unwrap(), rate);
        let mut buffer = small.remove(0);
        buffer.memory.fill(0xa5);
        let memory = buffer.memory.as_ptr();
        let body = ModelBody(alloc::vec![0x99; 16]);
        let body_pointer = body.0.as_ptr();
        let refused = prepare_subframe::<()>(buffer, subframe_frame(0), Some(body), &rate)
            .err()
            .unwrap();
        assert_eq!(refused.reason, PreparationError::BodyOverlap);
        assert_eq!(aggregate.subframes(), 0);
        let TxSource::Fresh {
            buffer,
            frame,
            body,
        } = refused.operation
        else {
            unreachable!()
        };
        let TxFrame::Individual {
            frame: original, ..
        } = frame.into_frame()
        else {
            unreachable!()
        };
        assert_eq!(buffer.memory.as_ptr(), memory);
        assert!(buffer.memory.iter().all(|byte| *byte == 0xa5));
        assert_eq!(body.as_ref().unwrap().0.as_ptr(), body_pointer);
        let frame = SizedFrame::new(*original.header(), MpduLength::new(48).unwrap()).unwrap();
        aggregate
            .push(prepare_subframe(buffer, frame, body, &rate).ok().unwrap())
            .ok()
            .unwrap();
        let refused = station.submit_ampdu(AggregateAttempt {
            context: (),
            ticket: tickets.remove(0),
            aggregate,
            protection: Protection::None,
            key: None,
            power: SupportedPower::new(TxPower::Calibrated).unwrap(),
            coex: SupportedCoex::new(CoexPriority::Normal).unwrap(),
            min_mpdu_start_spacing: MpduStartSpacing::new(0).unwrap(),
        });
        let Err(SubmitError::Refused(refused)) = refused else {
            panic!("protected aggregate needs its key")
        };
        assert_eq!(refused.reason, PreparationError::ProtectionMismatch);
        assert_eq!(refused.operation.aggregate.subframes(), 1);
        assert_eq!(key.identity.0.last.get(), 0);
        let mut operation = refused.operation;
        // Refusal does not force a retry to recover the subframe resources.
        let (mut buffers, mut bodies) = operation.aggregate.into_parts();
        let ModelAnyBuffer::Small(buffer) = buffers.remove(0) else {
            unreachable!()
        };
        let (index, body) = bodies.remove(0);
        assert_eq!(index, 0);
        assert_eq!(buffer.memory.as_ptr(), memory);
        assert_eq!(body.0.as_ptr(), body_pointer);
        assert!(phase.held.events.is_empty());
        let rate = phase.rate(PhyRate::Ht(ht)).unwrap();
        let mut station = phase.station();
        let rate = station.ampdu_rate(rate).ok().unwrap();
        let mut aggregate = station.aggregate(Tid::new(0).unwrap(), rate);
        aggregate
            .push(
                prepare_subframe(buffer, frame, Some(body), &rate)
                    .ok()
                    .unwrap(),
            )
            .ok()
            .unwrap();
        operation.aggregate = aggregate;
        operation.key = Some(&key);
        station.submit_ampdu(operation).unwrap();
        let (_, mut buffers, bodies, _) = aggregate_completion(phase).await;
        assert!(bodies.is_empty());
        let ModelAnyBuffer::SmallNumbered(mpdu) = buffers.remove(0) else {
            unreachable!()
        };
        assert_eq!(mpdu.buffer.memory.as_ptr(), memory);
        assert_eq!(mpdu.body().unwrap().0.as_ptr(), body_pointer);
        assert_eq!(&mpdu.buffer.memory[32..48], &[0x99; 16]);
        assert_eq!(mpdu.packet_number(), 1);
    }
}

#[test]
fn aggregate_push_and_submit_refuse_overlapping_bodies_and_return_all_owners() {
    let (backend, _) = backend();
    block_on(run_port(backend, Body(AggregateBoundaries)));
}
