use super::*;
use oer_ieee80211_lower_mac::{AggregateAttempt, MpduStartSpacing};

#[derive(Clone, Copy, Debug)]
enum Form {
    Single,
    Aggregate,
}

struct Results {
    form: Form,
    result: AirResult,
}

impl Work for Results {
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
        let mut ticket = tickets.remove(0);
        let mut buffer = small.remove(0);
        let original_ticket = ticket.index();
        let original_buffer = buffer.slot.index();
        let air = phase.held.air.clone();
        match self.result {
            AirResult::Acknowledged => air.answer(true),
            AirResult::AckTimeout => air.answer(false),
            AirResult::Collision => air.collision(),
            AirResult::HardwareTimeout => air.timeout(),
        }
        // The second operation has no injection and must get the default ACK,
        // including when the first operation was an aggregate.
        for result in [self.result, AirResult::Acknowledged] {
            let rate = match self.form {
                Form::Single => {
                    let TxFrame::Individual { frame, plan } = data_frame(phase, false) else {
                        panic!()
                    };
                    let rate = plan.first().rate;
                    let frame = ClassFrame::new(TxFrame::Individual {
                        frame,
                        plan: RetryPlan::new(RetryStage {
                            rate,
                            transmissions: NonZeroU8::new(1).unwrap(),
                            protection: Protection::None,
                        }),
                    })
                    .ok()
                    .unwrap();
                    submit_frame(
                        phase,
                        ticket,
                        TxSource::Fresh {
                            buffer,
                            frame,
                            body: None,
                            protection: TxProtection::Open,
                        },
                    );
                    PhyRate::Legacy(LegacyRate::Ofdm6M)
                }
                Form::Aggregate => {
                    let rate = PhyRate::Ht(
                        oer_ieee80211_mac::phy::HtRate::new(
                            HtMcs::new(7).unwrap(),
                            oer_ieee80211_mac::phy::PpduBandwidth::Mhz20,
                            false,
                        )
                        .unwrap(),
                    );
                    let checked_rate = phase.rate(rate).unwrap();
                    let mut station = phase.station();
                    let checked_rate = station.ampdu_rate(checked_rate).ok().unwrap();
                    let aggregate = station.aggregate(
                        IndividualAddress::new(PEER).unwrap(),
                        Tid::new(0).unwrap(),
                        checked_rate,
                        TxProtection::Open,
                    );
                    let aggregate = push_subframe(aggregate, buffer, subframe_frame(0), None);
                    station
                        .submit_ampdu(AggregateAttempt {
                            context: (),
                            ticket,
                            aggregate,
                            protection: Protection::None,
                            power: SupportedPower::new(TxPower::Calibrated).unwrap(),
                            coex: SupportedCoex::new(CoexPriority::Normal).unwrap(),
                            min_mpdu_start_spacing: MpduStartSpacing::new(0).unwrap(),
                        })
                        .unwrap();
                    rate
                }
            };
            let Ok(Ok(LowerMacEvent::TxCompleted(completed))) = phase.next_event().await else {
                panic!()
            };
            let (returned_ticket, returned_buffer, outcome) = match (self.form, completed) {
                (
                    Form::Single,
                    TxCompleted::Single {
                        ticket,
                        buffer,
                        completion,
                        body,
                        ..
                    },
                ) => {
                    assert!(body.is_none());
                    (ticket, buffer, completion)
                }
                (
                    Form::Aggregate,
                    TxCompleted::Aggregate {
                        ticket,
                        mut buffers,
                        completion,
                        bodies,
                        ..
                    },
                ) => {
                    assert_eq!(buffers.len(), 1);
                    assert!(bodies.is_empty());
                    (ticket, buffers.remove(0), completion)
                }
                _ => panic!("terminal changed operation kind"),
            };
            let expected = match result {
                AirResult::HardwareTimeout => {
                    TxOutcome::Indeterminate(IndeterminateCause::HardwareTimeout)
                }
                AirResult::Acknowledged => TxOutcome::Ended {
                    status: TxEndStatus::Delivered,
                    last: Some((NonZeroU16::MIN, rate)),
                    ack: Some(TxAck {
                        block_ack: match self.form {
                            Form::Single => None,
                            Form::Aggregate => Some(BlockAckReport {
                                start_sequence: SequenceNumber::new(0).unwrap(),
                                bitmap: 1,
                            }),
                        },
                        ..TxAck::default()
                    }),
                },
                AirResult::AckTimeout => TxOutcome::Ended {
                    status: TxEndStatus::AckTimeout,
                    last: Some((NonZeroU16::MIN, rate)),
                    ack: None,
                },
                AirResult::Collision => TxOutcome::Ended {
                    status: TxEndStatus::ContentionLimit,
                    last: None,
                    ack: None,
                },
            };
            assert_eq!(outcome, expected, "{:?}", self.form);
            ticket = returned_ticket;
            let ModelAnyBuffer::SmallSent(sent) = returned_buffer else {
                panic!("publication must return Sent")
            };
            let (fresh, body) = <ModelTypes as TxBufferPool<ModelSmall>>::renew(sent);
            assert!(body.is_none());
            buffer = fresh;
            assert_eq!(ticket.index(), original_ticket);
            assert_eq!(buffer.slot.index(), original_buffer);
        }
        assert_eq!(air.sent().len(), 2, "one publication per attempt");
    }
}

#[test]
fn every_result_is_consumed_by_exactly_one_single_or_aggregate_attempt() {
    for form in [Form::Single, Form::Aggregate] {
        for result in [
            AirResult::Acknowledged,
            AirResult::AckTimeout,
            AirResult::Collision,
            AirResult::HardwareTimeout,
        ] {
            let (backend, _) = backend();
            block_on(run_port(backend, Body(Results { form, result })));
        }
    }
}

#[test]
fn review_aggregate_honors_timeout_injection() {
    let (backend, _) = backend();
    block_on(run_port(
        backend,
        Body(Results {
            form: Form::Aggregate,
            result: AirResult::HardwareTimeout,
        }),
    ));
}
