use super::*;
use oer_ieee80211_lower_mac::{AggregateAttempt, LayoutError, MpduStartSpacing, PoolSource};
use oer_ieee80211_sta::machine::TxRefusal;

fn frame<'p>(
    phase: &ModelPhase<'p, '_>,
    len: usize,
    protected_bit: bool,
) -> ClassFrame<'p, ModelSmall, ModelCaps> {
    let TxFrame::Individual { frame, plan } = data_frame(phase, protected_bit) else {
        unreachable!()
    };
    ClassFrame::new(TxFrame::Individual {
        frame: SizedFrame::new(*frame.header(), MpduLength::new(len).unwrap()).unwrap(),
        plan,
    })
    .ok()
    .unwrap()
}

type RefusedSingle<'id> = (
    TxRefusal,
    Slot<'id, ModelTickets>,
    ModelAnyBuffer<'id>,
    Option<ModelBody>,
);
#[expect(
    clippy::result_large_err,
    reason = "a refusal returns the whole operation by value"
)]
fn try_source<'k, 'p, 'id: 'p + 'k>(
    phase: &mut ModelPhase<'p, 'id>,
    ticket: Slot<'id, ModelTickets>,
    source: PoolSource<'k, 'p, 'id, ModelTypes, ModelSmall, ModelCaps, Station>,
) -> Result<(), RefusedSingle<'id>> {
    let checked = match source.check() {
        Ok(source) => {
            <ModelTypes as TxBufferPool<ModelSmall>>::prepare::<Station, ModelCaps>(source)
                .map_err(|refused| (TxRefusal::Preparation(refused.reason), refused.operation))
        }
        Err(refused) => Err((TxRefusal::Layout(refused.reason), refused.operation)),
    };
    let source = match checked {
        Ok(source) => source,
        Err((reason, operation)) => {
            let (buffer, body) = match operation {
                TxSource::Fresh { buffer, body, .. } => (ModelAnyBuffer::Small(buffer), body),
                TxSource::Retransmission { mpdu, .. } => (ModelAnyBuffer::SmallSent(mpdu), None),
            };
            return Err((reason, ticket, buffer, body));
        }
    };
    phase
        .station()
        .submit(oer_ieee80211_lower_mac::Attempt {
            context: (),
            ticket,
            source,
            access_category: WmmAccessCategory::BestEffort,
            protection: Protection::None,
            power: SupportedPower::new(TxPower::Calibrated).unwrap(),
            coex: SupportedCoex::new(CoexPriority::Normal).unwrap(),
        })
        .expect("preparation does not poison the model");
    Ok(())
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
        for (len, body_len, protected_bit, keyed, refusal) in [
            (
                40,
                16,
                true,
                true,
                Some(TxRefusal::Layout(LayoutError::BodyOverlap)),
            ),
            (
                31,
                0,
                true,
                true,
                Some(TxRefusal::Layout(LayoutError::CcmpHeaderTooShort)),
            ),
            (
                40,
                17,
                false,
                false,
                Some(TxRefusal::Layout(LayoutError::BodyOverlap)),
            ),
            // The bit is now derived solely from TxProtection in preparation.
            (40, 0, true, false, None),
            (40, 0, false, true, None),
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
            let frame = frame(phase, len, protected_bit);
            let result = try_source(
                phase,
                ticket,
                TxSource::Fresh {
                    buffer,
                    frame,
                    body: Some(body),
                    protection: if keyed {
                        TxProtection::Ccmp(&key)
                    } else {
                        TxProtection::Open
                    },
                },
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
                assert!(buffer.memory.iter().all(|byte| *byte == 0xa5));
                assert_eq!(body.unwrap().0.as_ptr(), body_pointer);
            } else {
                assert!(result.is_ok());
                let (returned_ticket, returned_buffer) = completion(phase).await;
                ticket = returned_ticket;
                let ModelAnyBuffer::SmallSent(mpdu) = returned_buffer else {
                    unreachable!()
                };
                assert_eq!(mpdu.body().unwrap().0.as_ptr(), body_pointer);
                assert_eq!(
                    &mpdu.buffer.memory[len - body_len..len],
                    &[0x5a; 16][..body_len]
                );
                assert_eq!(mpdu.packet_number(), keyed.then_some(pn_before + 1));
                assert_eq!(mpdu.buffer.memory[1] & 0x40 != 0, keyed);
                let (returned, body) = <ModelTypes as TxBufferPool<ModelSmall>>::renew(mpdu);
                assert_eq!(body.unwrap().0.as_ptr(), body_pointer);
                buffer = returned;
            }
            assert_eq!(ticket.index(), ticket_index);
            assert_eq!(buffer.slot.index(), slot);
            assert_eq!(buffer.memory.as_ptr(), pointer);
        }
    }
}
#[test]
fn body_and_ccmp_boundaries_are_checked_before_admission_or_packet_number_allocation() {
    let (backend, _) = backend();
    block_on(run_port(backend, Body(Boundaries)));
}

fn aggregate_rate<'p, 'id: 'p>(phase: &mut ModelPhase<'p, 'id>) -> AmpduRate<'p> {
    let ht = oer_ieee80211_mac::phy::HtRate::new(
        HtMcs::new(7).unwrap(),
        oer_ieee80211_mac::phy::PpduBandwidth::Mhz20,
        false,
    )
    .unwrap();
    let rate = phase.rate(PhyRate::Ht(ht)).unwrap();
    phase.station().ampdu_rate(rate).ok().unwrap()
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
            protection: TxProtection::Ccmp(&key),
        };
        submit_frame::<ModelSmall>(phase, tickets.remove(0), source);
        let (ticket, buffer) = completion(phase).await;
        let ModelAnyBuffer::SmallSent(mpdu) = buffer else {
            unreachable!()
        };
        let before = mpdu.buffer.memory[..40].to_vec();
        let pn = mpdu.packet_number();
        let plan = RetryPlan::new(RetryStage {
            rate: phase.rate(PhyRate::Legacy(LegacyRate::Ofdm24M)).unwrap(),
            transmissions: NonZeroU8::MIN,
            protection: Protection::None,
        });
        submit_frame::<ModelSmall>(
            phase,
            ticket,
            TxSource::Retransmission {
                mpdu,
                plan: RetransmissionPlan::Individual(plan),
            },
        );
        let (ticket, buffer) = completion(phase).await;
        let ModelAnyBuffer::SmallSent(mpdu) = buffer else {
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
            if let Ok(Ok(LowerMacEvent::KeyRemoved(Removed::Station(RemovedKey::Pairwise(slot))))) =
                phase.next_event().await
            {
                break slot;
            }
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
        let Err((reason, ticket, returned, body)) = try_source(
            phase,
            ticket,
            TxSource::Retransmission {
                mpdu,
                plan: RetransmissionPlan::Individual(plan),
            },
        ) else {
            panic!("retired key refuses preparation")
        };
        assert_eq!(
            reason,
            TxRefusal::Preparation(PreparationError::RetransmissionKey)
        );
        assert!(body.is_none());
        let ModelAnyBuffer::SmallSent(mpdu) = returned else {
            unreachable!()
        };
        assert_eq!(mpdu.packet_number(), pn);
        assert_eq!(mpdu.body().unwrap().0.as_ptr(), body_pointer);
        assert_eq!(replacement.identity.0.last.get(), 0);
        assert!(phase.held.events.is_empty());
        // The builder also refuses the retained installation, even when a slot
        // has been reinstalled with identical material. The whole Sent returns.
        let rate = aggregate_rate(phase);
        let aggregate = phase.station().aggregate(
            IndividualAddress::new(PEER).unwrap(),
            Tid::new(0).unwrap(),
            rate,
            TxProtection::Ccmp(&replacement),
        );
        let refused = <ModelTypes as TxBufferPool<ModelSmall>>::push_aggregate(
            CheckedAggregate::new(aggregate, SubframeSource::Sent(mpdu))
                .ok()
                .unwrap(),
        )
        .err()
        .unwrap();
        assert_eq!(refused.reason, PreparationError::RetransmissionKey);
        assert_eq!(refused.aggregate.subframes(), 0);
        let SubframeSource::Sent(mpdu) = refused.subframe else {
            unreachable!()
        };
        assert_eq!(mpdu.packet_number(), pn);
        assert_eq!(mpdu.body().unwrap().0.as_ptr(), body_pointer);
        assert_eq!(&mpdu.buffer.memory[..40], expected.as_slice());
        assert_eq!(replacement.identity.0.last.get(), 0);
        let _ticket = ticket;
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
        let rate = aggregate_rate(phase);
        let receiver = IndividualAddress::new(ACCESS_POINT).unwrap();
        let tid = Tid::new(5).unwrap();
        let aggregate = phase
            .station()
            .aggregate(receiver, tid, rate, TxProtection::Ccmp(&key));
        let mut buffer = small.remove(0);
        buffer.memory.fill(0xa5);
        let memory = buffer.memory.as_ptr();
        let body = ModelBody(alloc::vec![0x99; 16]);
        let body_pointer = body.0.as_ptr();
        let source: ModelSubframe<ModelSmall, ModelCaps> = SubframeSource::Fresh {
            buffer,
            frame: ClassSubframe::<ModelSmall, _>::new(subframe_frame(0))
                .ok()
                .unwrap(),
            body: Some(body),
        };
        let refused = CheckedAggregate::new(aggregate, source).err().unwrap();
        assert_eq!(refused.reason, LayoutError::BodyOverlap);
        let (aggregate, subframe) = refused.operation;
        assert_eq!(aggregate.subframes(), 0);
        let SubframeSource::Fresh {
            buffer,
            frame,
            body,
        } = subframe
        else {
            unreachable!()
        };
        assert_eq!(buffer.memory.as_ptr(), memory);
        assert!(buffer.memory.iter().all(|byte| *byte == 0xa5));
        assert_eq!(body.as_ref().unwrap().0.as_ptr(), body_pointer);
        assert_eq!(key.identity.0.last.get(), 0);
        assert!(phase.held.events.is_empty());
        // QoS header 26 + CCMP 8 + body 16: exact boundary is 50.
        let frame = SizedFrame::new(*frame.frame().header(), MpduLength::new(50).unwrap()).unwrap();
        let aggregate = push_subframe(aggregate, buffer, frame, body);
        let (mut buffers, mut bodies) = aggregate.into_parts();
        let ModelAnyBuffer::Small(buffer) = buffers.remove(0) else {
            unreachable!()
        };
        let (index, body) = bodies.remove(0);
        assert_eq!(index, 0);
        assert_eq!(buffer.memory.as_ptr(), memory);
        assert_eq!(body.0.as_ptr(), body_pointer);
        let aggregate = phase
            .station()
            .aggregate(receiver, tid, rate, TxProtection::Ccmp(&key));
        let aggregate = push_subframe(aggregate, buffer, frame, Some(body));
        phase
            .station()
            .submit_ampdu(AggregateAttempt {
                context: (),
                ticket: tickets.remove(0),
                aggregate,
                protection: Protection::None,
                power: SupportedPower::new(TxPower::Calibrated).unwrap(),
                coex: SupportedCoex::new(CoexPriority::Normal).unwrap(),
                min_mpdu_start_spacing: MpduStartSpacing::new(0).unwrap(),
            })
            .unwrap();
        let (_, mut buffers, bodies, _) = aggregate_completion(phase).await;
        assert!(bodies.is_empty());
        let ModelAnyBuffer::SmallSent(mpdu) = buffers.remove(0) else {
            unreachable!()
        };
        assert_eq!(mpdu.buffer.memory.as_ptr(), memory);
        assert_eq!(mpdu.body().unwrap().0.as_ptr(), body_pointer);
        assert_eq!(&mpdu.buffer.memory[34..50], &[0x99; 16]);
        assert_eq!(mpdu.packet_number(), Some(1));
        assert_eq!(&mpdu.buffer.memory[4..10], &receiver.get());
        assert_eq!(mpdu.buffer.memory[24] & 0xf, tid.get());
    }
}
#[test]
fn aggregate_builder_checks_ccmp_before_mutating_and_returns_all_owners() {
    let (backend, _) = backend();
    block_on(run_port(backend, Body(AggregateBoundaries)));
}

struct PreparedRemoval {
    cancel: bool,
}
impl Work for PreparedRemoval {
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
        submit(phase, tickets.remove(0), small.remove(0), Some(&key));
        let (ticket, buffer) = completion(phase).await;
        let ModelAnyBuffer::SmallSent(mpdu) = buffer else {
            unreachable!()
        };
        let pn = mpdu.packet_number();
        let pointer = mpdu.buffer.memory.as_ptr();
        let TxFrame::Individual { plan, .. } = data_frame(phase, true) else {
            unreachable!()
        };
        let prepared = <ModelTypes as TxBufferPool<ModelSmall>>::prepare::<Station, ModelCaps>(
            (TxSource::Retransmission {
                mpdu,
                plan: RetransmissionPlan::Individual(plan),
            })
            .check()
            .ok()
            .unwrap(),
        )
        .ok()
        .unwrap();
        let air = phase.held.air.clone();
        let sent_before = air.sent().len();
        phase.station().remove_pairwise_key(key);
        yield_now().await;
        assert!(
            phase.held.events.is_empty(),
            "removal waits for the externally held preparation"
        );
        if self.cancel {
            let (buffer, body) = prepared.into_parts();
            assert!(body.is_none());
            let ModelAnyBuffer::SmallSent(mpdu) = buffer else {
                unreachable!()
            };
            assert_eq!(mpdu.packet_number(), pn);
            assert_eq!(mpdu.buffer.memory.as_ptr(), pointer);
        } else {
            phase
                .station()
                .submit(oer_ieee80211_lower_mac::Attempt {
                    context: (),
                    ticket,
                    source: prepared,
                    access_category: WmmAccessCategory::BestEffort,
                    protection: Protection::None,
                    power: SupportedPower::new(TxPower::Calibrated).unwrap(),
                    coex: SupportedCoex::new(CoexPriority::Normal).unwrap(),
                })
                .unwrap();
            let Ok(Ok(LowerMacEvent::TxCompleted(TxCompleted::Single {
                buffer, completion, ..
            }))) = phase.next_event().await
            else {
                panic!("terminal before removal")
            };
            assert_eq!(completion, TxOutcome::Withdrawn(WithdrawCause::KeyRemoved));
            let ModelAnyBuffer::SmallSent(mpdu) = buffer else {
                unreachable!()
            };
            assert_eq!(mpdu.packet_number(), pn);
            assert_eq!(mpdu.buffer.memory.as_ptr(), pointer);
        }
        let Ok(Ok(LowerMacEvent::KeyRemoved(_))) = phase.next_event().await else {
            panic!("the preparation no longer refers to the key")
        };
        assert_eq!(air.sent().len(), sent_before, "retired key never publishes");
    }
}
#[test]
fn key_removal_waits_for_prepared_retransmission_until_cancellation_or_its_terminal() {
    for cancel in [true, false] {
        let (backend, _) = backend();
        block_on(run_port(backend, Body(PreparedRemoval { cancel })));
    }
}

struct RetryRole;
impl Work for RetryRole {
    async fn work<'p, 'id: 'p>(
        self,
        phase: &mut ModelPhase<'p, 'id>,
        resources: ModelResources<'id>,
    ) {
        let ModelResources {
            mut tickets,
            mut small,
            ..
        } = resources;
        let source = <ModelTypes as TxBufferPool<ModelSmall>>::prepare::<AccessPoint, ModelCaps>(
            (TxSource::Fresh {
                buffer: small.remove(0),
                frame: frame(phase, 40, false),
                body: None,
                protection: TxProtection::Open,
            })
            .check()
            .ok()
            .unwrap(),
        )
        .ok()
        .unwrap();
        phase
            .access_point()
            .submit(oer_ieee80211_lower_mac::Attempt {
                context: (),
                ticket: tickets.remove(0),
                source,
                access_category: WmmAccessCategory::BestEffort,
                protection: Protection::None,
                power: SupportedPower::new(TxPower::Calibrated).unwrap(),
                coex: SupportedCoex::new(CoexPriority::Normal).unwrap(),
            })
            .unwrap();
        let (_ticket, buffer) = completion(phase).await;
        let ModelAnyBuffer::SmallSent(mpdu) = buffer else {
            unreachable!()
        };
        let before = mpdu.buffer.memory.to_vec();
        let TxFrame::Individual { plan, .. } = data_frame(phase, false) else {
            unreachable!()
        };
        let refused = <ModelTypes as TxBufferPool<ModelSmall>>::prepare::<Station, ModelCaps>(
            (TxSource::Retransmission {
                mpdu,
                plan: RetransmissionPlan::Individual(plan),
            })
            .check()
            .ok()
            .unwrap(),
        )
        .err()
        .unwrap();
        assert_eq!(refused.reason, PreparationError::RetransmissionKind);
        let TxSource::Retransmission { mpdu, .. } = refused.operation else {
            unreachable!()
        };
        assert_eq!(mpdu.buffer.memory.as_ref(), before.as_slice());
        assert!(phase.held.events.is_empty());
    }
}
#[test]
fn a_sent_mpdu_cannot_change_its_interface_role_during_retransmission() {
    let (backend, _) = backend();
    block_on(run_port(backend, Body(RetryRole)));
}
