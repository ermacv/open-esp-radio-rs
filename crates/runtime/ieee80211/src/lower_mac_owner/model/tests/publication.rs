use super::*;

#[derive(Clone, Copy)]
enum Barrier {
    GateAck,
    GateRetry,
    GateTimeout,
    RemoveKey,
}
struct DuringPublication(Barrier);

impl Work for DuringPublication {
    async fn work<'p, 'id: 'p>(
        self,
        phase: &mut ModelPhase<'p, 'id>,
        resources: ModelResources<'id>,
    ) {
        let ModelResources {
            mut tickets,
            mut small,
            station_pairwise: [slot, _],
            gate,
            ..
        } = resources;
        let air = phase.held.air.clone();
        let mut key = Some(phase.station().install_pairwise_key(
            slot,
            IndividualAddress::new(PEER).unwrap(),
            Ccmp128Key([7; 16]),
        ));
        air.hold_completion(WmmAccessCategory::BestEffort, true);
        air.answer(matches!(self.0, Barrier::GateAck));
        let head_ticket = tickets[0].index();
        let tail_ticket = tickets[1].index();
        submit(phase, tickets.remove(0), small.remove(0), key.as_ref());
        submit(phase, tickets.remove(0), small.remove(0), key.as_ref());
        for _ in 0..6 {
            yield_now().await;
        }
        assert_eq!(
            air.sent().len(),
            1,
            "a queued successor cannot pass a published head"
        );
        assert!(
            phase.held.events.is_empty(),
            "publication is not completion"
        );
        let first = air.sent().remove(0);
        if matches!(self.0, Barrier::RemoveKey) {
            phase.station().remove_pairwise_key(key.take().unwrap());
        } else {
            phase.close_gate(gate);
        }
        for _ in 0..6 {
            yield_now().await;
        }
        assert_eq!(air.sent().len(), 1);
        assert!(
            phase.held.events.is_empty(),
            "no terminal or barrier before the current result"
        );
        if matches!(self.0, Barrier::GateTimeout) {
            air.set_time(oer_time::Instant::from_micros(
                MODEL_PUBLICATION_BOUND.as_micros() - 1,
            ));
            for _ in 0..3 {
                yield_now().await;
            }
            assert!(phase.held.events.is_empty());
            air.set_time(oer_time::Instant::from_micros(
                MODEL_PUBLICATION_BOUND.as_micros(),
            ));
        } else {
            air.hold_completion(WmmAccessCategory::BestEffort, false);
        }
        let Ok(Ok(LowerMacEvent::TxCompleted(TxCompleted::Single {
            ticket,
            buffer: ModelAnyBuffer::SmallSent(mpdu),
            completion,
            body,
            ..
        }))) = phase.next_event().await
        else {
            panic!("published head returns first, as Sent")
        };
        assert_eq!(ticket.index(), head_ticket);
        assert!(body.is_none());
        assert_eq!(mpdu.packet_number(), first.packet_number);
        match self.0 {
            Barrier::GateAck => assert!(matches!(
                completion,
                TxOutcome::Ended {
                    status: TxEndStatus::Delivered,
                    transmissions: 1,
                    ..
                }
            )),
            Barrier::GateRetry => {
                assert_eq!(completion, TxOutcome::Withdrawn(WithdrawCause::GateClosed))
            }
            Barrier::RemoveKey => {
                assert_eq!(completion, TxOutcome::Withdrawn(WithdrawCause::KeyRemoved))
            }
            Barrier::GateTimeout => assert_eq!(
                completion,
                TxOutcome::Indeterminate(IndeterminateCause::HardwareTimeout)
            ),
        }
        let Ok(Ok(LowerMacEvent::TxCompleted(TxCompleted::Single {
            ticket: tail,
            buffer: ModelAnyBuffer::Small(_),
            completion,
            ..
        }))) = phase.next_event().await
        else {
            panic!("unpublished tail returns second, as Fresh")
        };
        assert_eq!(tail.index(), tail_ticket);
        let cause = if matches!(self.0, Barrier::RemoveKey) {
            WithdrawCause::KeyRemoved
        } else {
            WithdrawCause::GateClosed
        };
        assert_eq!(completion, TxOutcome::Withdrawn(cause));
        assert_eq!(
            air.sent().len(),
            1,
            "the barrier waits one publication, not the two-entry plan"
        );
        if matches!(self.0, Barrier::RemoveKey) {
            assert!(matches!(
                phase.next_event().await,
                Ok(Ok(LowerMacEvent::KeyRemoved(_)))
            ));
        } else {
            let Ok(Ok(LowerMacEvent::GateClosed(closed))) = phase.next_event().await else {
                panic!("barrier follows both terminals")
            };
            let _gate = phase.open_gate(closed);
            air.hold_completion(WmmAccessCategory::BestEffort, false);
            if matches!(self.0, Barrier::GateRetry) {
                let TxFrame::Individual { plan, .. } = data_frame(phase, true) else {
                    unreachable!()
                };
                submit_frame::<ModelSmall>(
                    phase,
                    ticket,
                    TxSource::Retransmission {
                        mpdu,
                        plan: RetransmissionPlan::Individual(plan),
                    },
                );
                let (_, returned) = super::completion(phase).await;
                assert!(matches!(returned, ModelAnyBuffer::SmallSent(_)));
                let retried = air.sent().pop().unwrap();
                assert_eq!(retried.packet_number, first.packet_number);
                assert_eq!(&retried.frame[22..24], &first.frame[22..24]);
                assert_eq!(
                    &retried.frame[24..],
                    &first.frame[24..],
                    "CCMP and payload unchanged"
                );
                assert_eq!(first.frame[1] & 0x08, 0);
                assert_ne!(retried.frame[1] & 0x08, 0);
            }
        }
    }
}
#[test]
fn gate_waits_one_publication_and_returns_sent_for_retry_with_original_numbers() {
    let (backend, _) = backend();
    block_on(run_port(
        backend,
        Body(DuringPublication(Barrier::GateRetry)),
    ));
}
#[test]
fn ack_wins_while_gate_closes_and_barrier_follows_the_result() {
    let (backend, _) = backend();
    block_on(run_port(backend, Body(DuringPublication(Barrier::GateAck))));
}
#[test]
fn missing_result_ends_at_publication_bound_without_finishing_the_plan() {
    let (backend, _) = backend();
    block_on(run_port(
        backend,
        Body(DuringPublication(Barrier::GateTimeout)),
    ));
}
#[test]
fn key_removal_waits_for_the_publication_then_returns_sent_without_retry() {
    let (backend, _) = backend();
    block_on(run_port(
        backend,
        Body(DuringPublication(Barrier::RemoveKey)),
    ));
}
