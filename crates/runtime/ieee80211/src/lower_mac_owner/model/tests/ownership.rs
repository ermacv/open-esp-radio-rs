use super::*;

// Deliberately neither Copy nor Clone. The backend can only move it.
struct Context {
    operation: usize,
    ticket: u8,
    buffer: u8,
    body: *const u8,
    dropped: Rc<Cell<usize>>,
}

impl ModelContext for Context {
    type Context<'id> = Self;
}

impl Drop for Context {
    fn drop(&mut self) {
        self.dropped.set(self.dropped.get() + 1);
    }
}

type Backend = ModelBackend<ModelCaps, Context>;

#[derive(Clone, Copy)]
enum End {
    Air,
    Gate,
    Disable,
    Poison,
}

struct Operations {
    air: Rc<ModelAir>,
    dropped: Rc<Cell<usize>>,
    seen: Rc<RefCell<Vec<(usize, u8)>>>,
    end: End,
}

fn returned(event: LowerMacEvent<'_, ModelTypes<Context>>, seen: &RefCell<Vec<(usize, u8)>>) {
    if let LowerMacEvent::TxCompleted {
        context,
        ticket,
        mut buffers,
        completion,
        mut bodies,
    } = event
    {
        assert_eq!(ticket.index(), context.ticket);
        assert_eq!(buffers.len(), 1);
        let buffer = buffers.remove(0);
        let ModelAnyBuffer::Small(buffer) = buffer else {
            panic!("an unprotected small MPDU")
        };
        assert_eq!(buffer.slot.index(), context.buffer);
        assert_eq!(bodies.len(), 1);
        let (index, body) = bodies.remove(0);
        assert_eq!(index, 0);
        assert_eq!(
            body.0.as_ptr(),
            context.body,
            "the original body owner returns"
        );
        assert_eq!(body.0, [context.operation as u8; 4]);
        assert_eq!(&buffer.memory[36..40], body.bytes());
        seen.borrow_mut()
            .push((context.operation, completion.transmissions));
    }
}

impl PortBody<Backend> for Operations {
    async fn run<'id>(
        self,
        owner: Owner<'_, 'id, Backend, Disabled>,
        resources: ModelResources<'id>,
    ) -> Ended<'id, Backend> {
        let Ok(mut owner) = owner.enable().await else {
            panic!("the model enables")
        };
        let ModelResources {
            mut tickets,
            mut small,
            gate,
            ..
        } = resources;
        {
            let mut phase = owner.phase();
            self.air.hold(WmmAccessCategory::BestEffort, true);
            for operation in 0..3 {
                let ticket = tickets.remove(0);
                let buffer = small.remove(0);
                let body = ModelBody(alloc::vec![operation as u8; 4]);
                let context = Context {
                    operation,
                    ticket: ticket.index(),
                    buffer: buffer.slot.index(),
                    body: body.0.as_ptr(),
                    dropped: self.dropped.clone(),
                };
                let frame = ClassFrame::new(data_frame(&phase, false)).ok().unwrap();
                phase
                    .station()
                    .submit::<ModelSmall>(oer_ieee80211_lower_mac::Attempt {
                        context,
                        ticket,
                        source: TxSource::Fresh {
                            buffer,
                            frame,
                            body: Some(body),
                        },
                        access_category: if operation == 2 {
                            WmmAccessCategory::Voice
                        } else {
                            WmmAccessCategory::BestEffort
                        },
                        protection: Protection::None,
                        key: None,
                        power: SupportedPower::new(TxPower::Calibrated).unwrap(),
                        coex: SupportedCoex::new(CoexPriority::Normal).unwrap(),
                    })
                    .unwrap();
            }
            yield_now().await;
            let Ok(Ok(event)) = phase.next_event().await else {
                panic!("Voice completes first")
            };
            returned(event, &self.seen);
            assert_eq!(&*self.seen.borrow(), &[(2, 1)]);
            assert_eq!(self.dropped.get(), 1);
            match self.end {
                End::Air => self.air.hold(WmmAccessCategory::BestEffort, false),
                End::Gate => phase.close_gate(gate),
                End::Disable => {}
                End::Poison => self.air.poison(),
            }
            if matches!(self.end, End::Air | End::Gate) {
                while self.seen.borrow().len() < 3 {
                    let Ok(Ok(event)) = phase.next_event().await else {
                        panic!("an operation terminal")
                    };
                    returned(event, &self.seen);
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

fn operations(end: End) -> Vec<(usize, u8)> {
    let (backend, air) = Backend::new(
        Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap(),
        [
            IndividualAddress::new(STATION).unwrap(),
            IndividualAddress::new(ACCESS_POINT).unwrap(),
        ],
    );
    let dropped = Rc::new(Cell::new(0));
    let seen = Rc::new(RefCell::new(Vec::new()));
    block_on(run_port(
        backend,
        Operations {
            air,
            dropped: dropped.clone(),
            seen: seen.clone(),
            end,
        },
    ));
    assert_eq!(dropped.get(), 3, "every owned context ends exactly once");
    let result = seen.borrow().clone();
    result
}

#[test]
fn interleaved_acs_return_their_own_context_ticket_buffer_and_body() {
    assert_eq!(operations(End::Air), [(2, 1), (0, 1), (1, 1)]);
}

#[test]
fn gate_returns_unpublished_operations_in_ac_order_with_all_their_resources() {
    assert_eq!(operations(End::Gate), [(2, 1), (0, 0), (1, 0)]);
}

#[test]
fn disable_returns_held_operations_before_uninstall() {
    assert_eq!(operations(End::Disable), [(2, 1), (0, 0), (1, 0)]);
}

#[test]
fn fail_stop_ends_held_ownership_with_the_install_without_inventing_terminals() {
    assert_eq!(operations(End::Poison), [(2, 1)]);
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
        use oer_ieee80211_lower_mac::{
            AggregateAttempt, AmpduPhase, MpduStartSpacing, PhaseAggregate, Tid,
        };
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
            let mut aggregate = station.aggregate(Tid::new(6).unwrap(), rate);
            let ticket = tickets.remove(0);
            let mut context = AggregateContext {
                ticket: ticket.index(),
                buffers: Vec::new(),
                bodies: Vec::new(),
                dropped: self.dropped.clone(),
            };
            for sequence in 0..3 {
                let buffer = small.remove(0);
                context.buffers.push(buffer.slot.index());
                // A missing body does not renumber the following body's index.
                let body = (sequence != 1).then(|| ModelBody(alloc::vec![sequence as u8; 4]));
                if let Some(body) = &body {
                    context.bodies.push((sequence as u8, body.0.as_ptr()));
                }
                aggregate
                    .push::<ModelSmall>(buffer, subframe_frame(sequence), body)
                    .ok()
                    .unwrap();
            }
            self.air.hold(WmmAccessCategory::Voice, self.withdraw);
            station
                .submit_ampdu(AggregateAttempt {
                    context,
                    ticket,
                    aggregate,
                    protection: Protection::None,
                    key: Some(&key),
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
                if matches!(&event, LowerMacEvent::TxCompleted { .. }) {
                    break event;
                }
            };
            let LowerMacEvent::TxCompleted {
                context,
                ticket,
                buffers,
                bodies,
                completion,
            } = event
            else {
                unreachable!()
            };
            assert_eq!(ticket.index(), context.ticket);
            assert_eq!(completion.transmissions, u8::from(!self.withdraw));
            assert_eq!(buffers.len(), 3);
            assert_eq!(bodies.len(), if self.withdraw { 2 } else { 0 });
            for (buffer, index) in buffers.iter().zip(&context.buffers) {
                let slot = match buffer {
                    ModelAnyBuffer::Small(buffer) if self.withdraw => buffer.slot.index(),
                    ModelAnyBuffer::SmallNumbered(buffer) if !self.withdraw => {
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
                    let ModelAnyBuffer::SmallNumbered(mpdu) = buffer else {
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
                !matches!(event, LowerMacEvent::TxCompleted { .. }),
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
