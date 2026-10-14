use super::*;
use crate::lower_mac_owner::conformance::{
    self, Class, End, Harness, Operation, Resources, Scenario, Trace,
};

// Deliberately neither Copy nor Clone, and branded by the installation.
struct Context<'id> {
    epoch: Option<oer_ieee80211_sta::epoch::LinkEpoch<'id>>,
    operation: Operation,
    resources: Resources,
    dropped: Rc<Cell<usize>>,
}
struct Contexts;
impl ModelContext for Contexts {
    type Context<'id> = Context<'id>;
}
impl Drop for Context<'_> {
    fn drop(&mut self) {
        self.dropped.set(self.dropped.get() + 1);
    }
}
type Backend = ModelBackend<ModelCaps, Contexts>;

struct Operations {
    air: Rc<ModelAir>,
    dropped: Rc<Cell<usize>>,
    seen: Rc<RefCell<Vec<conformance::Returned>>>,
    scenario: Scenario,
}

fn observe(
    context: Context<'_>,
    ticket: Slot<'_, ModelTickets, ModelPool<1>>,
    buffer: ModelAnyBuffer<'_>,
    body: Option<ModelBody>,
    end: End,
    seen: &RefCell<Vec<conformance::Returned>>,
) {
    assert!(context.epoch.is_none());
    let (class, index, body) = match buffer {
        ModelAnyBuffer::Small(buffer) => (Class::Small, buffer.slot.index(), body),
        ModelAnyBuffer::Large(buffer) => (Class::Large, buffer.slot.index(), body),
        ModelAnyBuffer::SmallSent(mpdu) => {
            assert!(body.is_none());
            let (buffer, body) = <ModelTypes<Contexts> as TxBufferPool<ModelSmall>>::renew(mpdu);
            (Class::Small, buffer.slot.index(), body)
        }
        ModelAnyBuffer::LargeSent(mpdu) => {
            assert!(body.is_none());
            let (buffer, body) = <ModelTypes<Contexts> as TxBufferPool<ModelLarge>>::renew(mpdu);
            (Class::Large, buffer.slot.index(), body)
        }
    };
    let body = body.unwrap();
    assert!(body.0.iter().all(|byte| *byte == context.operation.id));
    seen.borrow_mut().push(conformance::Returned {
        operation: context.operation,
        expected: context.resources,
        actual: Resources {
            ticket: ticket.index(),
            buffer: index,
            class,
            body: body.0.as_ptr() as usize,
        },
        end,
    });
}

fn returned(
    event: LowerMacEvent<'_, ModelTypes<Contexts>>,
    seen: &RefCell<Vec<conformance::Returned>>,
) {
    match event {
        LowerMacEvent::TxCompleted(TxCompleted::Single {
            context,
            ticket,
            buffer,
            completion,
            body,
        }) => {
            let end = match completion {
                TxOutcome::Ended {
                    last: Some((core::num::NonZeroU16::MIN, _)),
                    status: TxEndStatus::Delivered,
                    ..
                } => End::Completed,
                TxOutcome::Withdrawn(_) => End::Withdrawn,
                other => panic!("unexpected outcome {other:?}"),
            };
            observe(context, ticket, buffer, body, end, seen);
        }
        LowerMacEvent::TxCompleted(TxCompleted::Aggregate { .. }) => {
            panic!("single submission must return Single")
        }
        _ => {}
    }
}

impl PortBody<Backend> for Operations {
    async fn run<'id>(
        self,
        owner: Owner<'_, 'id, Backend, Disabled>,
        resources: ModelResources<'id>,
    ) -> Ended<'id, Backend> {
        let Ok(mut owner) = owner.enable().await else {
            panic!("model enables")
        };
        let ModelResources {
            mut tickets,
            mut small,
            mut large,
            gate,
            ..
        } = resources;
        {
            let mut phase = owner.phase();
            self.air.hold(WmmAccessCategory::BestEffort, true);
            for operation in conformance::OPERATIONS {
                let ticket = tickets.remove(0);
                let body = ModelBody(
                    alloc::vec![operation.id; if self.scenario == Scenario::Refuse { 17 } else { 4 }],
                );
                let (index, source) = match operation.class {
                    Class::Small => {
                        let buffer = small.remove(0);
                        let index = buffer.slot.index();
                        let source = TxSource::Fresh {
                            buffer,
                            frame: ClassFrame::new(data_frame(&phase, false)).ok().unwrap(),
                            body: Some(body),
                            protection: TxProtection::Open,
                        }
                        .check()
                        .map(|source| {
                            <ModelTypes<Contexts> as TxBufferPool<ModelSmall>>::prepare::<
                                Station,
                                ModelCaps,
                            >(source)
                            .ok()
                            .unwrap()
                        })
                        .map_err(|refused| match refused.operation {
                            TxSource::Fresh { buffer, body, .. } => {
                                (ModelAnyBuffer::Small(buffer), body)
                            }
                            TxSource::Retransmission { .. } => unreachable!(),
                        });
                        (index, source)
                    }
                    Class::Large => {
                        let buffer = large.remove(0);
                        let index = buffer.slot.index();
                        let source = TxSource::Fresh {
                            buffer,
                            frame: ClassFrame::new(data_frame(&phase, false)).ok().unwrap(),
                            body: Some(body),
                            protection: TxProtection::Open,
                        }
                        .check()
                        .map(|source| {
                            <ModelTypes<Contexts> as TxBufferPool<ModelLarge>>::prepare::<
                                Station,
                                ModelCaps,
                            >(source)
                            .ok()
                            .unwrap()
                        })
                        .map_err(|refused| match refused.operation {
                            TxSource::Fresh { buffer, body, .. } => {
                                (ModelAnyBuffer::Large(buffer), body)
                            }
                            TxSource::Retransmission { .. } => unreachable!(),
                        });
                        (index, source)
                    }
                };
                // Identity comes from the actual prepared/refused owner, before transfer.
                let body_pointer = match &source {
                    Ok(source) => source.mpdu.body.as_ref().unwrap().0.as_ptr(),
                    Err((_, body)) => body.as_ref().unwrap().0.as_ptr(),
                };
                let context = Context {
                    epoch: None,
                    operation,
                    resources: Resources {
                        ticket: ticket.index(),
                        buffer: index,
                        class: operation.class,
                        body: body_pointer as usize,
                    },
                    dropped: self.dropped.clone(),
                };
                match source {
                    Ok(source) => {
                        assert_ne!(self.scenario, Scenario::Refuse);
                        phase
                            .station()
                            .submit(oer_ieee80211_lower_mac::Attempt {
                                context,
                                ticket,
                                source,
                                access_category: operation.category,
                                protection: Protection::None,
                                power: SupportedPower::new(TxPower::Calibrated).unwrap(),
                                coex: SupportedCoex::new(CoexPriority::Normal).unwrap(),
                            })
                            .unwrap();
                    }
                    Err((buffer, body)) => {
                        assert_eq!(self.scenario, Scenario::Refuse);
                        observe(context, ticket, buffer, body, End::Refused, &self.seen);
                    }
                }
            }
            if self.scenario != Scenario::Refuse {
                while self.seen.borrow().len() < 2 {
                    let Ok(Ok(event)) = phase.next_event().await else {
                        panic!("unheld AC completes")
                    };
                    returned(event, &self.seen);
                }
                assert!(
                    self.seen
                        .borrow()
                        .iter()
                        .all(|observation| observation.operation.category
                            != WmmAccessCategory::BestEffort)
                );
                match self.scenario {
                    Scenario::Complete => self.air.hold(WmmAccessCategory::BestEffort, false),
                    Scenario::CloseGate => phase.close_gate(gate),
                    Scenario::Disable => {}
                    Scenario::Poison => self.air.poison(),
                    Scenario::Refuse => unreachable!(),
                }
                if matches!(self.scenario, Scenario::Complete | Scenario::CloseGate) {
                    while self.seen.borrow().len() < 4 {
                        let Ok(Ok(event)) = phase.next_event().await else {
                            panic!("terminal")
                        };
                        returned(event, &self.seen);
                    }
                }
            }
        }
        let mut sink = |event| returned(event, &self.seen);
        match owner.disable(&mut sink).await {
            Ok(owner) => owner.uninstall(()),
            Err(poisoned) => poisoned.fail_stop(),
        }
    }
}

struct ModelHarness;
impl Harness for ModelHarness {
    async fn exercise(&mut self, scenario: Scenario) -> Trace {
        let (backend, air) = Backend::new(
            Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap(),
            [
                IndividualAddress::new(STATION).unwrap(),
                IndividualAddress::new(ACCESS_POINT).unwrap(),
            ],
        );
        let dropped = Rc::new(Cell::new(0));
        let seen = Rc::new(RefCell::new(Vec::new()));
        run_port(
            backend,
            Operations {
                air: air.clone(),
                dropped: dropped.clone(),
                seen: seen.clone(),
                scenario,
            },
        )
        .await;
        Trace {
            returned: core::mem::take(&mut *seen.borrow_mut()),
            contexts_dropped: dropped.get(),
            publications: air.sent().len(),
        }
    }
}

#[test]
fn common_backend_tx_ownership_suite() {
    block_on(async {
        conformance::check(&mut ModelHarness).await.unwrap();
    });
}

struct AggregateContext {
    ticket: u8,
    buffers: Vec<u8>,
    bodies: Vec<(u8, *const u8)>,
    dropped: Rc<Cell<usize>>,
}

impl ModelContext for AggregateContext {
    type Context<'id> = Self;
}

impl Drop for AggregateContext {
    fn drop(&mut self) {
        self.dropped.set(self.dropped.get() + 1);
    }
}

struct AggregateOwners {
    air: Rc<ModelAir>,
    dropped: Rc<Cell<usize>>,
    withdraw: bool,
}

impl PortBody<ModelBackend<ModelCaps, AggregateContext>> for AggregateOwners {
    async fn run<'id>(
        self,
        owner: Owner<'_, 'id, ModelBackend<ModelCaps, AggregateContext>, Disabled>,
        resources: ModelResources<'id>,
    ) -> Ended<'id, ModelBackend<ModelCaps, AggregateContext>> {
        use oer_ieee80211_lower_mac::{AggregateAttempt, AmpduPhase, MpduStartSpacing, Tid};
        let Ok(mut owner) = owner.enable().await else {
            panic!("the model enables")
        };
        let ModelResources {
            mut tickets,
            mut small,
            gate,
            station_pairwise: [slot, _],
            ..
        } = resources;
        {
            let mut phase = owner.phase();
            let ht = oer_ieee80211_mac::phy::HtRate::new(
                HtMcs::new(7).unwrap(),
                oer_ieee80211_mac::phy::PpduBandwidth::Mhz20,
                false,
            )
            .unwrap();
            let rate = phase.rate(PhyRate::Ht(ht)).unwrap();
            let mut station = phase.station();
            let key = station.install_pairwise_key(
                slot,
                IndividualAddress::new(PEER).unwrap(),
                Ccmp128Key([7; 16]),
            );
            let rate = station.ampdu_rate(rate).ok().unwrap();
            let aggregate = station.aggregate(
                IndividualAddress::new(PEER).unwrap(),
                Tid::new(6).unwrap(),
                rate,
                TxProtection::Ccmp(&key),
            );
            let ticket = tickets.remove(0);
            let mut context = AggregateContext {
                ticket: ticket.index(),
                buffers: Vec::new(),
                bodies: Vec::new(),
                dropped: self.dropped.clone(),
            };
            let first = small.remove(0);
            let body = ModelBody(alloc::vec![0; 4]);
            context.buffers.push(first.slot.index());
            context.bodies.push((0, body.0.as_ptr()));
            let mut aggregate = push_subframe(aggregate, first, subframe_frame(0), Some(body));
            for sequence in 1..3 {
                let buffer = small.remove(0);
                context.buffers.push(buffer.slot.index());
                // A missing body does not renumber the following body's index.
                let body = (sequence != 1).then(|| ModelBody(alloc::vec![sequence as u8; 4]));
                if let Some(body) = &body {
                    context.bodies.push((sequence as u8, body.0.as_ptr()));
                }
                aggregate = push_subframe(aggregate, buffer, subframe_frame(sequence), body);
            }
            self.air.hold(WmmAccessCategory::Voice, self.withdraw);
            station
                .submit_ampdu(AggregateAttempt {
                    context,
                    ticket,
                    aggregate,
                    protection: Protection::None,
                    power: SupportedPower::new(TxPower::Calibrated).unwrap(),
                    coex: SupportedCoex::new(CoexPriority::Normal).unwrap(),
                    min_mpdu_start_spacing: MpduStartSpacing::new(0).unwrap(),
                })
                .unwrap();
            if self.withdraw {
                phase.close_gate(gate);
            }
            let event = loop {
                let Ok(Ok(event)) = phase.next_event().await else {
                    panic!("the aggregate ends")
                };
                if matches!(&event, LowerMacEvent::TxCompleted(_)) {
                    break event;
                }
            };
            let LowerMacEvent::TxCompleted(TxCompleted::Aggregate {
                context,
                ticket,
                buffers,
                bodies,
                completion,
            }) = event
            else {
                unreachable!()
            };
            assert_eq!(ticket.index(), context.ticket);
            assert_eq!(matches!(completion, TxOutcome::Withdrawn(_)), self.withdraw);
            assert_eq!(buffers.len(), 3);
            assert_eq!(bodies.len(), if self.withdraw { 2 } else { 0 });
            for (buffer, index) in buffers.iter().zip(&context.buffers) {
                let slot = match buffer {
                    ModelAnyBuffer::Small(buffer) if self.withdraw => buffer.slot.index(),
                    ModelAnyBuffer::SmallSent(buffer) if !self.withdraw => {
                        buffer.buffer.slot.index()
                    }
                    _ => panic!("only published subframes are numbered"),
                };
                assert_eq!(slot, *index);
            }
            for (index, pointer) in &context.bodies {
                let buffer = &buffers[usize::from(*index)];
                let body = if self.withdraw {
                    &bodies.iter().find(|(at, _)| at == index).unwrap().1
                } else {
                    let ModelAnyBuffer::SmallSent(mpdu) = buffer else {
                        unreachable!()
                    };
                    mpdu.body().unwrap()
                };
                assert_eq!(body.0.as_ptr(), *pointer);
                assert_eq!(body.0, [*index; 4]);
                assert_eq!(&buffer.memory()[36..40], body.bytes());
            }
        }
        let mut sink = |event| {
            assert!(
                !matches!(event, LowerMacEvent::TxCompleted(_)),
                "each attempt has one terminal"
            )
        };
        let Ok(owner) = owner.disable(&mut sink).await else {
            panic!("the model disables")
        };
        owner.uninstall(())
    }
}

#[test]
fn aggregate_terminal_returns_its_context_and_each_subframes_resources_on_air_or_withdrawal() {
    for withdraw in [false, true] {
        let (backend, air) = ModelBackend::<ModelCaps, AggregateContext>::new(
            Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap(),
            [
                IndividualAddress::new(STATION).unwrap(),
                IndividualAddress::new(ACCESS_POINT).unwrap(),
            ],
        );
        let dropped = Rc::new(Cell::new(0));
        block_on(run_port(
            backend,
            AggregateOwners {
                air,
                dropped: dropped.clone(),
                withdraw,
            },
        ));
        assert_eq!(dropped.get(), 1);
    }
}
