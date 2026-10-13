//! The model on the phase shape, under `run_port`: the conformance of its
//! order and the three tests of the owner decisions on #563
//! (https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6129037560).

use super::*;
use core::num::NonZeroU8;
use oer_ieee80211_lower_mac::{
    AckHeader, ChannelWidth, ClassFrame, FrameControl, HeaderSpec, MpduLength, Protection,
    RetryPlan, RetryStage, SizedFrame, SupportedCoex, SupportedPower, TxPower,
};
use oer_ieee80211_mac::ccmp::GroupKeyId;
use oer_ieee80211_mac::{phy::LegacyRate, qos::WmmAccessCategory};
use oer_ieee80211_port::{Disabled, Ended, Owner, PortBody, run_port};

const STATION: [u8; 6] = [2, 0, 0, 0, 0, 1];
const ACCESS_POINT: [u8; 6] = [2, 0, 0, 0, 0, 2];
const PEER: [u8; 6] = [2, 0, 0, 0, 0, 9];

fn backend() -> (ModelBackend, Rc<ModelAir>) {
    ModelBackend::new(
        Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap(),
        [
            IndividualAddress::new(STATION).unwrap(),
            IndividualAddress::new(ACCESS_POINT).unwrap(),
        ],
    )
}

/// Poll `future` with a waker that does nothing until it is ready: the
/// model's progress needs no wake, only polls.
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = core::pin::pin!(future);
    let mut context = core::task::Context::from_waker(Waker::noop());
    for _ in 0..10_000 {
        if let Poll::Ready(value) = future.as_mut().poll(&mut context) {
            return value;
        }
    }
    panic!("the port's future never ended");
}

/// Let the port's other futures, its driver among them, run once.
async fn yield_now() {
    let mut yielded = false;
    poll_fn(|cx| {
        if yielded {
            Poll::Ready(())
        } else {
            yielded = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    })
    .await;
}

/// A future that is pending until `open` is set.
fn until(open: &Cell<bool>) -> impl Future<Output = ()> + '_ {
    poll_fn(|_| {
        if open.get() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    })
}

/// A data frame to the peer, 40 octets, protected or not.
fn data_frame<'p, X: ModelContext>(
    phase: &ModelPhase<'p, '_, ModelCaps, X>,
    protected: bool,
) -> TxFrame<'p, ModelCaps> {
    let rate = phase
        .rate(PhyRate::Legacy(LegacyRate::Ofdm6M))
        .expect("the channel carries 6 Mbit/s");
    let control = if protected { 0x4108 } else { 0x0108 };
    let header = HeaderSpec::new(
        FrameControl::new(control).unwrap(),
        0,
        IndividualAddress::new(PEER).unwrap(),
        STATION,
        PEER,
        0,
        None,
        None,
        None,
    )
    .and_then(AckHeader::new)
    .unwrap();
    TxFrame::Individual {
        frame: SizedFrame::new(header, MpduLength::new(40).unwrap()).unwrap(),
        plan: RetryPlan::new(RetryStage {
            rate,
            transmissions: NonZeroU8::new(2).unwrap(),
            protection: Protection::None,
        }),
    }
}

/// Submit a frame and its class's memory at the same boundary.
fn submit_frame<'p, 'id: 'p, C: ModelClass>(
    phase: &mut ModelPhase<'p, 'id>,
    ticket: Slot<'id, ModelTickets>,
    source: TxSource<
        'p,
        ModelBuffer<'id, C>,
        ModelSent<'id, C>,
        ClassFrame<'p, C, ModelCaps>,
        ModelBody,
    >,
    key: Option<&ModelPairwiseKey<'id, Station>>,
) {
    let source = prepare_model(source).ok().unwrap();
    phase
        .station()
        .submit(oer_ieee80211_lower_mac::Attempt {
            context: (),
            ticket,
            source,
            access_category: WmmAccessCategory::BestEffort,
            protection: Protection::None,
            key: key.map(AttemptKey::Pairwise),
            power: SupportedPower::new(TxPower::Calibrated).unwrap(),
            coex: SupportedCoex::new(CoexPriority::Normal).unwrap(),
        })
        .unwrap();
}

fn submit<'p, 'id: 'p, C: ModelClass>(
    phase: &mut ModelPhase<'p, 'id>,
    ticket: Slot<'id, ModelTickets>,
    mut buffer: ModelBuffer<'id, C>,
    key: Option<&ModelPairwiseKey<'id, Station>>,
) {
    buffer.bytes_mut()[..40].fill(0x5a);
    let frame = ClassFrame::new(data_frame(phase, key.is_some()))
        .ok()
        .unwrap();
    submit_frame(
        phase,
        ticket,
        TxSource::Fresh {
            buffer,
            frame,
            body: None,
        },
        key,
    );
}

#[expect(
    clippy::result_large_err,
    reason = "a refusal returns the whole operation by value"
)]
fn prepare_subframe<'p, 'id, X: ModelContext>(
    buffer: ModelBuffer<'id, ModelSmall>,
    frame: SizedFrame<AckHeader, ModelCaps>,
    body: Option<ModelBody>,
    rate: &AmpduRate<'p>,
) -> ModelPreparation<'p, 'id, Station, ModelCaps, X, ModelSmall> {
    let frame = ClassFrame::new(TxFrame::Individual {
        frame,
        plan: RetryPlan::new(RetryStage {
            rate: *rate.rate(),
            transmissions: NonZeroU8::MIN,
            protection: Protection::None,
        }),
    })
    .ok()
    .unwrap();
    <ModelTypes<X> as TxBufferPool<'id, ModelSmall>>::prepare(TxSource::Fresh {
        buffer,
        frame,
        body,
    })
}

fn prepare_retry<'p, 'id, X: ModelContext>(
    mpdu: ModelSent<'id, ModelSmall>,
    rate: &AmpduRate<'p>,
) -> ModelPrepared<'p, 'id, Station, ModelCaps, X> {
    <ModelTypes<X> as TxBufferPool<'id, ModelSmall>>::prepare(TxSource::Retransmission {
        mpdu,
        plan: RetransmissionPlan::Individual(RetryPlan::new(RetryStage {
            rate: *rate.rate(),
            transmissions: NonZeroU8::MIN,
            protection: Protection::None,
        })),
    })
    .ok()
    .unwrap()
}

/// The next terminal's ticket and buffer.
async fn completion<'p, 'id: 'p>(
    phase: &mut ModelPhase<'p, 'id>,
) -> (Slot<'id, ModelTickets>, ModelAnyBuffer<'id>) {
    loop {
        yield_now().await;
        let event = {
            let event = core::pin::pin!(phase.next_event());
            let mut context = core::task::Context::from_waker(Waker::noop());
            Future::poll(event, &mut context)
        };
        if let Poll::Ready(Ok(Ok(LowerMacEvent::TxCompleted(TxCompleted::Single {
            ticket,
            buffer,
            ..
        })))) = event
        {
            return (ticket, buffer);
        }
    }
}

/// A protected attempt, its retransmission from the numbered buffer, then
/// a new MPDU in the renewed buffer.
struct Numbering {
    protected: bool,
}

impl Work for Numbering {
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
        let key = phase.station().install_pairwise_key(
            slot,
            IndividualAddress::new(PEER).unwrap(),
            Ccmp128Key([7; 16]),
        );
        let key = self.protected.then_some(&key);
        submit(phase, tickets.remove(0), small.remove(0), key);
        let (ticket, buffer) = completion(phase).await;
        let numbered = <ModelTypes as TxBufferPool<'id, ModelSmall>>::sent(buffer)
            .expect("a protected attempt's buffer comes back numbered");
        let number = numbered.packet_number();
        let air = phase.held.air.clone();
        air.hold(WmmAccessCategory::BestEffort, true);
        let TxFrame::Individual { plan, .. } = data_frame(phase, self.protected) else {
            unreachable!()
        };
        submit_frame::<ModelSmall>(
            phase,
            ticket,
            TxSource::Retransmission {
                mpdu: numbered,
                plan: RetransmissionPlan::Individual(plan),
            },
            key,
        );
        phase.close_gate(gate);
        let (ticket, buffer) = completion(phase).await;
        let numbered = <ModelTypes as TxBufferPool<'id, ModelSmall>>::sent(buffer)
            .expect("withdrawal preserves an already numbered MPDU");
        assert_eq!(numbered.packet_number(), number);
        assert_ne!(numbered.buffer.memory[1] & 0x08, 0);
        let Ok(Ok(LowerMacEvent::GateClosed(closed))) = phase.next_event().await else {
            panic!("gate closes after the attempt returns");
        };
        let _gate = phase.open_gate(closed);
        air.hold(WmmAccessCategory::BestEffort, false);
        // The same MPDU again: a retransmission keeps its number.
        let TxFrame::Individual { plan, .. } = data_frame(phase, self.protected) else {
            unreachable!()
        };
        submit_frame::<ModelSmall>(
            phase,
            ticket,
            TxSource::Retransmission {
                mpdu: numbered,
                plan: RetransmissionPlan::Individual(plan),
            },
            key,
        );
        let (ticket, buffer) = completion(phase).await;
        let numbered = <ModelTypes as TxBufferPool<'id, ModelSmall>>::sent(buffer)
            .expect("a retransmission's buffer comes back numbered");
        // Renewed, the memory holds a new MPDU, numbered anew.
        let (renewed, body) = <ModelTypes as TxBufferPool<'id, ModelSmall>>::renew(numbered);
        assert!(body.is_none());
        submit(phase, ticket, renewed, key);
        let _ = completion(phase).await;
        let _ = key;
    }
}

#[test]
fn a_retransmitted_mpdu_keeps_its_packet_number_and_a_renewed_buffer_takes_a_new_one() {
    let (backend, air) = backend();
    block_on(run_port(backend, Body(Numbering { protected: true })));
    let numbers: Vec<_> = air.sent().iter().map(|frame| frame.packet_number).collect();
    assert_eq!(numbers, [Some(1), Some(1), Some(2)]);
}

#[test]
fn an_open_sent_mpdu_retransmits_without_rebuilding_its_sn_or_payload() {
    let (backend, air) = backend();
    block_on(run_port(backend, Body(Numbering { protected: false })));
    let frames = air.sent();
    assert_eq!(frames.len(), 3);
    assert_eq!(frames[0].packet_number, None);
    assert_eq!(frames[1].packet_number, None);
    let mut original = frames[0].frame.clone();
    original[1] |= 0x08;
    assert_eq!(frames[1].frame, original);
    assert_eq!(
        frames[0].frame, frames[2].frame,
        "renewal starts a fresh MPDU"
    );
}

/// Submit an attempt for every ticket, in every buffer of either class,
/// running `between` after each.
fn submit_all<'p, 'id: 'p>(
    phase: &mut ModelPhase<'p, 'id>,
    tickets: Vec<Slot<'id, ModelTickets>>,
    small: Vec<ModelBuffer<'id, ModelSmall>>,
    large: Vec<ModelBuffer<'id, ModelLarge>>,
    mut between: impl FnMut(&mut ModelPhase<'p, 'id>),
) {
    assert_eq!(small.len() + large.len(), MODEL_TICKETS);
    let mut tickets = tickets.into_iter();
    for buffer in small {
        submit(phase, tickets.next().unwrap(), buffer, None);
        between(phase);
    }
    for buffer in large {
        submit(phase, tickets.next().unwrap(), buffer, None);
        between(phase);
    }
}

/// What a body saw of its port's stream.
#[derive(Debug, Default, Eq, PartialEq)]
struct Stream {
    completions: usize,
    received: usize,
    lost: usize,
}

/// Take every queued event: completions and receptions, their tokens
/// recycled or dropped.
fn drain<'p, 'id: 'p>(phase: &mut ModelPhase<'p, 'id>, stream: &mut Stream) {
    loop {
        let event = {
            let event = core::pin::pin!(phase.next_event());
            let mut context = core::task::Context::from_waker(Waker::noop());
            match Future::poll(event, &mut context) {
                Poll::Ready(event) => event,
                Poll::Pending => return,
            }
        };
        match event {
            Ok(Ok(LowerMacEvent::TxCompleted(_))) => stream.completions += 1,
            Ok(Ok(LowerMacEvent::Received { frame, .. })) => {
                stream.received += 1;
                phase.recycle(frame);
            }
            Ok(Ok(_)) => {}
            Ok(Err(EventsLost)) => stream.lost += 1,
            Err(_) => return,
        }
    }
}

/// What a test's body does over the running phase and the install's
/// tokens.
trait Work {
    async fn work<'p, 'id: 'p>(
        self,
        phase: &mut ModelPhase<'p, 'id>,
        resources: ModelResources<'id>,
    );
}

/// The body of the tests: it enables, runs its work, then disables and
/// uninstalls.
struct Body<W>(W);

impl<W: Work> PortBody<ModelBackend> for Body<W> {
    async fn run<'id>(
        self,
        owner: Owner<'_, 'id, ModelBackend, Disabled>,
        resources: ModelResources<'id>,
    ) -> Ended<'id, ModelBackend> {
        let Ok(mut enabled) = owner.enable().await else {
            panic!("the model enables");
        };
        {
            let mut phase = enabled.phase();
            self.0.work(&mut phase, resources).await;
        }
        match enabled
            .disable(&mut |event| {
                if let LowerMacEvent::Received { mut frame, .. } = event {
                    frame.recycled = true;
                }
            })
            .await
        {
            Ok(disabled) => disabled.uninstall(()),
            Err(poisoned) => poisoned.fail_stop(),
        }
    }
}

/// Install a key and submit an attempt that names it, in one breath.
struct KeyThenAttempt;

impl Work for KeyThenAttempt {
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
        let mut stream = Stream::default();
        while stream.completions == 0 {
            drain(phase, &mut stream);
            yield_now().await;
        }
        // The key ends with the install.
        let _ = key;
    }
}

#[test]
fn an_attempt_naming_a_key_goes_after_the_keys_install() {
    let (backend, air) = backend();
    block_on(run_port(backend, Body(KeyThenAttempt)));
    let sent = air.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].key, Some((0, true)));
}

/// Install a key, submit an attempt that names it and remove the key, all
/// before the driver runs and without reading an event.
struct KeyAttemptRemoval;

impl Work for KeyAttemptRemoval {
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
        phase.station().remove_pairwise_key(key);
        let mut events = Vec::new();
        while events.len() < 2 {
            yield_now().await;
            let event = {
                let event = core::pin::pin!(phase.next_event());
                let mut context = core::task::Context::from_waker(Waker::noop());
                Future::poll(event, &mut context)
            };
            match event {
                Poll::Ready(Ok(Ok(LowerMacEvent::TxCompleted(_)))) => events.push("completed"),
                Poll::Ready(Ok(Ok(LowerMacEvent::KeyRemoved(_)))) => events.push("removed"),
                _ => {}
            }
        }
        assert_eq!(events, ["completed", "removed"]);
    }
}

#[test]
fn an_attempt_between_a_keys_install_and_removal_goes_under_the_key() {
    let (backend, air) = backend();
    block_on(run_port(backend, Body(KeyAttemptRemoval)));
    let sent = air.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].key, Some((0, true)));
    assert_eq!(air.commands(), 3);
}

/// Submit every ticket's attempt, then wait for `open` before reading.
struct WaitForever<'t> {
    open: &'t Cell<bool>,
    stream: &'t RefCell<Stream>,
}

impl Work for WaitForever<'_> {
    async fn work<'p, 'id: 'p>(
        self,
        phase: &mut ModelPhase<'p, 'id>,
        resources: ModelResources<'id>,
    ) {
        submit_all(
            phase,
            resources.tickets,
            resources.small,
            resources.large,
            |_| {},
        );
        // The body waits on something else entirely.
        until(self.open).await;
        drain(phase, &mut self.stream.borrow_mut());
    }
}

#[test]
fn a_body_that_waits_forever_never_stalls_the_driver() {
    let (backend, air) = backend();
    let open = Cell::new(false);
    let stream = RefCell::new(Stream::default());
    let mut port = core::pin::pin!(run_port(
        backend,
        Body(WaitForever {
            open: &open,
            stream: &stream,
        }),
    ));
    let mut context = core::task::Context::from_waker(Waker::noop());
    for _ in 0..10 {
        assert!(port.as_mut().poll(&mut context).is_pending());
    }
    // The world sends more frames than the stream holds while the body
    // waits; the driver keeps serving the hardware.
    let meta = ModelRxMeta::new(Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap(), -40, -95);
    for _ in 0..MODEL_RX_EVENTS + 3 {
        air.receive(&[0x80, 0, 0, 0], meta);
    }
    let passes = air.passes();
    for _ in 0..10 {
        assert!(port.as_mut().poll(&mut context).is_pending());
    }
    assert!(air.passes() > passes);
    // Nothing waits in the air: the driver took every frame.
    assert_eq!(air.pending_rx(), 0);
    open.set(true);
    while port.as_mut().poll(&mut context).is_pending() {}
    // Every completion, the frames that fit and one loss for the rest.
    assert_eq!(
        *stream.borrow(),
        Stream {
            completions: MODEL_TICKETS,
            received: MODEL_RX_EVENTS,
            lost: 1,
        }
    );
}

/// Submit every ticket's attempt, half of them unanswered, and read only
/// once the driver served it all.
struct MaxWork<'t> {
    air: &'t ModelAir,
    stream: &'t RefCell<Stream>,
    settings: bool,
}

impl Work for MaxWork<'_> {
    async fn work<'p, 'id: 'p>(
        self,
        phase: &mut ModelPhase<'p, 'id>,
        resources: ModelResources<'id>,
    ) {
        let ModelResources {
            tickets,
            small,
            large,
            station_pairwise,
            station_group,
            access_point_pairwise,
            access_point_group,
            gate,
            ..
        } = resources;
        for _ in 0..MODEL_TICKETS {
            self.air.answer(false);
            self.air.answer(false);
        }
        let settings = self.settings;
        submit_all(phase, tickets, small, large, |phase| {
            if settings {
                // Every setting changes between the submissions, unread.
                phase.station().configure(StationReceive::Off);
                phase.station().set_bss_color(BssColor::new(5).unwrap());
                phase
                    .access_point()
                    .configure(SupportedFilter::new(ReceiveFilter::BSS_MEMBER).unwrap());
            }
        });
        if settings {
            // Every key slot's install and the gate's closing as well: the
            // command queue at the contract's limit.
            let peer = IndividualAddress::new(PEER).unwrap();
            let group = GroupKeyId::new(1).unwrap();
            let mut keys = 0;
            for slot in station_pairwise {
                let _installed =
                    phase
                        .station()
                        .install_pairwise_key(slot, peer, Ccmp128Key([1; 16]));
                keys += 1;
            }
            for slot in station_group {
                let _installed =
                    phase
                        .station()
                        .install_group_key(slot, group, Ccmp128Key([2; 16]));
                keys += 1;
            }
            for slot in access_point_pairwise {
                let _installed =
                    phase
                        .access_point()
                        .install_pairwise_key(slot, peer, Ccmp128Key([3; 16]));
                keys += 1;
            }
            for slot in access_point_group {
                let _installed =
                    phase
                        .access_point()
                        .install_group_key(slot, group, Ccmp128Key([4; 16]));
                keys += 1;
            }
            assert_eq!(keys, MODEL_ALL_KEY_SLOTS);
            phase.close_gate(gate);
        } else {
            let _ = gate;
        }
        for _ in 0..10 {
            yield_now().await;
        }
        drain(phase, &mut self.stream.borrow_mut());
    }
}

#[test]
fn as_much_work_as_the_tickets_allow_loses_no_completion_unread() {
    let (backend, air) = backend();
    let stream = RefCell::new(Stream::default());
    block_on(run_port(
        backend,
        Body(MaxWork {
            air: &air,
            stream: &stream,
            settings: false,
        }),
    ));
    assert_eq!(stream.borrow().completions, MODEL_TICKETS);
}

#[test]
fn as_much_work_as_allowed_while_every_setting_changes_loses_no_command() {
    let (backend, air) = backend();
    let stream = RefCell::new(Stream::default());
    block_on(run_port(
        backend,
        Body(MaxWork {
            air: &air,
            stream: &stream,
            settings: true,
        }),
    ));
    // Every submission, install and the closing arrived; every attempt
    // ended; every kind of setting took its latest value before the next
    // command.
    assert_eq!(stream.borrow().completions, MODEL_TICKETS);
    assert_eq!(
        air.commands(),
        (MODEL_TICKETS + MODEL_ALL_KEY_SLOTS + 1) as u32
    );
    assert!(air.settings() >= 3);
}

/// What an attempt the closing gate aborted came back as.
#[derive(Debug, Default)]
struct Aborted {
    outcome: Option<TxOutcome>,
    fresh: bool,
    untouched: bool,
}

/// A protected attempt waits in a busy queue; the gate closes over it.
struct AbortHeld<'t> {
    air: &'t ModelAir,
    seen: &'t RefCell<Aborted>,
}

impl Work for AbortHeld<'_> {
    async fn work<'p, 'id: 'p>(
        self,
        phase: &mut ModelPhase<'p, 'id>,
        resources: ModelResources<'id>,
    ) {
        let ModelResources {
            mut tickets,
            mut large,
            station_pairwise: [slot, _],
            gate,
            ..
        } = resources;
        let key = phase.station().install_pairwise_key(
            slot,
            IndividualAddress::new(PEER).unwrap(),
            Ccmp128Key([7; 16]),
        );
        self.air.hold(WmmAccessCategory::BestEffort, true);
        submit(phase, tickets.remove(0), large.remove(0), Some(&key));
        for _ in 0..10 {
            yield_now().await;
        }
        phase.close_gate(gate);
        loop {
            yield_now().await;
            let event = {
                let event = core::pin::pin!(phase.next_event());
                let mut context = core::task::Context::from_waker(Waker::noop());
                Future::poll(event, &mut context)
            };
            if let Poll::Ready(Ok(Ok(LowerMacEvent::TxCompleted(TxCompleted::Single {
                buffer,
                completion,
                ..
            })))) = event
            {
                let mut seen = self.seen.borrow_mut();
                seen.outcome = Some(completion);
                if let Ok(mut fresh) = <ModelTypes as TxBufferPool<'id, ModelLarge>>::class(buffer)
                {
                    seen.fresh = true;
                    // The MPDU as it was written: its body, no CCMP header
                    // filled in.
                    let bytes = fresh.bytes_mut();
                    seen.untouched = bytes[24..32] == [0x5a; 8];
                }
                break;
            }
        }
        let _ = key;
    }
}

#[test]
fn an_attempt_the_closing_gate_aborts_comes_back_fresh_without_a_transmission() {
    let (backend, air) = backend();
    let seen = RefCell::new(Aborted::default());
    block_on(run_port(
        backend,
        Body(AbortHeld {
            air: &air,
            seen: &seen,
        }),
    ));
    let seen = seen.into_inner();
    // Contract (owner decision on #563): aborted with no transmission, its
    // buffer fresh, never numbered, its MPDU untouched.
    assert_eq!(
        seen.outcome,
        Some(TxOutcome::Withdrawn(WithdrawCause::GateClosed))
    );
    assert!(seen.fresh, "the buffer comes back fresh");
    assert!(seen.untouched, "the MPDU is as it was written");
    assert!(air.sent().is_empty());
}

/// Cancels `next_event` every way a body can: dropped after a pending
/// poll, dropped unpolled, and losing a `select` to a ready branch. Each
/// frame the air brought still arrives once, in order.
struct CancelTheEvent<'t> {
    air: &'t ModelAir,
    received: &'t RefCell<Vec<u8>>,
}

impl Work for CancelTheEvent<'_> {
    async fn work<'p, 'id: 'p>(self, phase: &mut ModelPhase<'p, 'id>, _: ModelResources<'id>) {
        let meta = ModelRxMeta::new(Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap(), -40, -95);
        let mut context = core::task::Context::from_waker(Waker::noop());
        // Dropped after a pending poll, before the frame arrives.
        {
            let event = core::pin::pin!(phase.next_event());
            assert!(Future::poll(event, &mut context).is_pending());
        }
        self.air.receive(&[0x80, 0, 1, 0], meta);
        for _ in 0..10 {
            yield_now().await;
        }
        // Dropped unpolled, with the frame queued.
        drop(phase.next_event());
        // Losing a select to a branch that is ready first.
        match embassy_futures::select::select(core::future::ready(()), phase.next_event()).await {
            embassy_futures::select::Either::First(()) => {}
            embassy_futures::select::Either::Second(_) => panic!("the ready branch wins"),
        }
        self.air.receive(&[0x80, 0, 2, 0], meta);
        for _ in 0..10 {
            yield_now().await;
        }
        // Each frame once, in order.
        for _ in 0..2 {
            match phase.next_event().await {
                Ok(Ok(LowerMacEvent::Received { frame, .. })) => {
                    self.received.borrow_mut().push(frame.bytes()[2]);
                    phase.recycle(frame);
                }
                _ => panic!("a received frame"),
            }
        }
        let event = core::pin::pin!(phase.next_event());
        assert!(
            Future::poll(event, &mut context).is_pending(),
            "nothing more"
        );
    }
}

#[test]
fn a_cancelled_next_event_loses_no_event() {
    let (backend, air) = backend();
    let received = RefCell::new(Vec::new());
    block_on(run_port(
        backend,
        Body(CancelTheEvent {
            air: &air,
            received: &received,
        }),
    ));
    assert_eq!(*received.borrow(), [1, 2]);
}

/// A protected data MPDU of `sequence` to the peer, 40 octets, in
/// `buffer`, and its frame.
fn subframe<'id, C>(
    mut buffer: ModelBuffer<'id, C>,
    sequence: u16,
) -> (ModelBuffer<'id, C>, SizedFrame<AckHeader, ModelCaps>) {
    let bytes = buffer.bytes_mut();
    bytes[..40].fill(0x5a);
    bytes[0..2].copy_from_slice(&0x4108_u16.to_le_bytes());
    bytes[22..24].copy_from_slice(&(sequence << 4).to_le_bytes());
    (buffer, subframe_frame(sequence))
}

/// The frame of a protected data MPDU of `sequence` to the peer, 40
/// octets.
fn subframe_frame(sequence: u16) -> SizedFrame<AckHeader, ModelCaps> {
    let header = HeaderSpec::new(
        FrameControl::new(0x4108).unwrap(),
        0,
        IndividualAddress::new(PEER).unwrap(),
        STATION,
        PEER,
        sequence << 4,
        None,
        None,
        None,
    )
    .and_then(AckHeader::new)
    .unwrap();
    SizedFrame::new(header, MpduLength::new(40).unwrap()).unwrap()
}

/// The next terminal: its ticket, buffers and Block Ack.
async fn aggregate_completion<'p, 'id: 'p>(
    phase: &mut ModelPhase<'p, 'id>,
) -> (
    Slot<'id, ModelTickets>,
    Vec<ModelAnyBuffer<'id>>,
    Vec<(u8, ModelBody)>,
    Option<oer_ieee80211_lower_mac::BlockAckReport>,
) {
    loop {
        yield_now().await;
        let event = {
            let event = core::pin::pin!(phase.next_event());
            let mut context = core::task::Context::from_waker(Waker::noop());
            Future::poll(event, &mut context)
        };
        if let Poll::Ready(Ok(Ok(LowerMacEvent::TxCompleted(TxCompleted::Aggregate {
            ticket,
            buffers,
            completion,
            bodies,
            ..
        })))) = event
        {
            return (
                ticket,
                buffers.into_iter().collect(),
                bodies,
                match completion {
                    TxOutcome::Ended { ack, .. } => ack.and_then(|ack| ack.block_ack),
                    other => panic!("aggregate did not end: {other:?}"),
                },
            );
        }
    }
}

/// An A-MPDU of three protected subframes whose Block Ack misses the
/// second; the second goes again from its numbered buffer beside a new
/// subframe (owner decision on #563, comment 6131631789).
struct AggregateRetransmission<'t> {
    air: &'t ModelAir,
    report: &'t Cell<Option<oer_ieee80211_lower_mac::BlockAckReport>>,
}

impl Work for AggregateRetransmission<'_> {
    async fn work<'p, 'id: 'p>(
        self,
        phase: &mut ModelPhase<'p, 'id>,
        resources: ModelResources<'id>,
    ) {
        use oer_ieee80211_lower_mac::{
            AggregateAttempt, AmpduPhase, MpduStartSpacing, PhaseAggregate, Tid,
        };
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
        let attempt = |ticket, aggregate| AggregateAttempt {
            context: (),
            ticket,
            aggregate,
            protection: Protection::None,
            key: Some(&key),
            power: SupportedPower::new(TxPower::Calibrated).unwrap(),
            coex: SupportedCoex::new(CoexPriority::Normal).unwrap(),
            min_mpdu_start_spacing: MpduStartSpacing::new(0).unwrap(),
        };
        // Three new subframes, sequence numbers 0 to 2; the Block Ack
        // misses the second.
        self.air.block_ack(0b101);
        let rate = phase.rate(PhyRate::Ht(ht)).unwrap();
        let mut station = phase.station();
        let rate = station.ampdu_rate(rate).ok().unwrap();
        let mut aggregate = station.aggregate(Tid::new(0).unwrap(), rate);
        let mut body_owners = Vec::new();
        for sequence in 0..3 {
            let (buffer, frame) = subframe(small.remove(0), sequence);
            let body = ModelBody(alloc::vec![sequence as u8; 4]);
            body_owners.push(body.0.as_ptr());
            aggregate
                .push(
                    prepare_subframe(buffer, frame, Some(body), &rate)
                        .ok()
                        .unwrap(),
                )
                .ok()
                .unwrap();
        }
        station
            .submit_ampdu(attempt(tickets.remove(0), aggregate))
            .unwrap();
        let (ticket, mut buffers, bodies, report) = aggregate_completion(phase).await;
        self.report.set(report);
        assert!(bodies.is_empty(), "numbered MPDUs retain their own bodies");
        for (index, (buffer, pointer)) in buffers.iter().zip(&body_owners).enumerate() {
            let ModelAnyBuffer::SmallSent(mpdu) = buffer else {
                panic!("numbered subframe")
            };
            let body = mpdu.body().unwrap();
            assert_eq!(body.0.as_ptr(), *pointer);
            assert_eq!(body.0, [index as u8; 4]);
        }
        // The second again, from its numbered buffer, beside a new one.
        let numbered = <ModelTypes as TxBufferPool<'id, ModelSmall>>::sent(buffers.remove(1))
            .map_err(|_| ())
            .expect("a protected subframe comes back numbered");
        let rate = phase.rate(PhyRate::Ht(ht)).unwrap();
        let mut station = phase.station();
        let rate = station.ampdu_rate(rate).ok().unwrap();
        let mut aggregate = station.aggregate(Tid::new(0).unwrap(), rate);
        aggregate.push(prepare_retry(numbered, &rate)).ok().unwrap();
        let (buffer, frame) = subframe(small.remove(0), 3);
        aggregate
            .push(prepare_subframe(buffer, frame, None, &rate).ok().unwrap())
            .ok()
            .unwrap();
        station.submit_ampdu(attempt(ticket, aggregate)).unwrap();
        let (_, buffers, bodies, _) = aggregate_completion(phase).await;
        assert!(bodies.is_empty());
        let ModelAnyBuffer::SmallSent(mpdu) = &buffers[0] else {
            panic!("numbered subframe")
        };
        assert_eq!(mpdu.body().unwrap().0.as_ptr(), body_owners[1]);
    }
}

#[test]
fn a_lost_subframe_goes_again_with_its_packet_number_beside_new_ones_numbered_anew() {
    let (backend, air) = backend();
    let report = Cell::new(None);
    block_on(run_port(
        backend,
        Body(AggregateRetransmission {
            air: &air,
            report: &report,
        }),
    ));
    let report = report.get().expect("the A-MPDU's Block Ack");
    assert_eq!(report.start_sequence.get(), 0);
    assert_eq!(report.bitmap, 0b101, "the second subframe missed");
    let sent = air.sent();
    let numbers: Vec<_> = sent.iter().map(|frame| frame.packet_number).collect();
    assert_eq!(numbers, [Some(1), Some(2), Some(3), Some(2), Some(4)]);
    // The retransmitted subframe carries the Retry bit; the others do not.
    let retry: Vec<_> = sent
        .iter()
        .map(|frame| frame.frame[1] & 0x08 != 0)
        .collect();
    assert_eq!(retry, [false, false, false, true, false]);
}

mod ownership;

mod preparation;

struct TerminalOutcomes<'a> {
    air: &'a ModelAir,
    outcomes: &'a RefCell<Vec<TxOutcome>>,
    collision: bool,
}

impl Work for TerminalOutcomes<'_> {
    async fn work<'p, 'id: 'p>(
        self,
        phase: &mut ModelPhase<'p, 'id>,
        resources: ModelResources<'id>,
    ) {
        let ModelResources {
            mut tickets,
            mut large,
            station_pairwise: [slot, _],
            ..
        } = resources;
        let key = phase.station().install_pairwise_key(
            slot,
            IndividualAddress::new(PEER).unwrap(),
            Ccmp128Key([7; 16]),
        );
        if self.collision {
            self.air.collision();
            self.air.collision();
        } else {
            self.air.timeout();
        }
        let expected_tickets = [tickets[0].index(), tickets[1].index()];
        submit(phase, tickets.remove(0), large.remove(0), Some(&key));
        submit(phase, tickets.remove(0), large.remove(0), Some(&key));
        for expected in expected_tickets {
            loop {
                yield_now().await;
                let next = core::pin::pin!(phase.next_event());
                let mut cx = core::task::Context::from_waker(Waker::noop());
                if let Poll::Ready(Ok(Ok(LowerMacEvent::TxCompleted(TxCompleted::Single {
                    ticket,
                    buffer,
                    completion,
                    body,
                    ..
                })))) = Future::poll(next, &mut cx)
                {
                    assert_eq!(
                        ticket.index(),
                        expected,
                        "head's terminal precedes its tail"
                    );

                    assert!(matches!(&buffer, ModelAnyBuffer::LargeSent(_)));
                    assert!(body.is_none());
                    self.outcomes.borrow_mut().push(completion);
                    break;
                }
            }
        }
    }
}

#[test]
fn a_timeout_is_final_and_does_not_stop_or_withdraw_the_queue_tail() {
    let (backend, air) = backend();
    let outcomes = RefCell::default();
    block_on(run_port(
        backend,
        Body(TerminalOutcomes {
            air: &air,
            outcomes: &outcomes,
            collision: false,
        }),
    ));
    let outcomes = outcomes.borrow();
    assert_eq!(
        outcomes[0],
        TxOutcome::Indeterminate(IndeterminateCause::HardwareTimeout)
    );
    assert!(matches!(
        outcomes[1],
        TxOutcome::Ended {
            status: TxEndStatus::Success,
            transmissions: 1,
            ..
        }
    ));
    let sent = air.sent();
    assert_eq!(
        sent.len(),
        2,
        "no timeout retry, despite the unused second plan entry"
    );
    assert_eq!(
        (sent[0].packet_number, sent[1].packet_number),
        (Some(1), Some(2))
    );
    assert!(sent.iter().all(|frame| frame.frame[1] & 0x08 == 0));
}

#[test]
fn collisions_retry_the_head_then_end_at_contention_limit_without_air_transmissions() {
    let (backend, air) = backend();
    let outcomes = RefCell::default();
    block_on(run_port(
        backend,
        Body(TerminalOutcomes {
            air: &air,
            outcomes: &outcomes,
            collision: true,
        }),
    ));
    assert!(matches!(
        outcomes.borrow()[0],
        TxOutcome::Ended {
            status: TxEndStatus::ContentionLimit,
            transmissions: 0,
            rate: None,
            ..
        }
    ));
    assert!(matches!(
        outcomes.borrow()[1],
        TxOutcome::Ended {
            status: TxEndStatus::Success,
            transmissions: 1,
            ..
        }
    ));
    let sent = air.sent();
    assert_eq!(sent.len(), 3);
    assert_eq!(
        sent[0].frame, sent[1].frame,
        "collision keeps SN, PN, payload and Retry bit"
    );
    assert_eq!(
        sent.iter()
            .map(|frame| frame.packet_number)
            .collect::<Vec<_>>(),
        [Some(1), Some(1), Some(2)]
    );
}

struct LongRetryPlan<'a>(&'a ModelAir);
impl Work for LongRetryPlan<'_> {
    async fn work<'p, 'id: 'p>(
        self,
        phase: &mut ModelPhase<'p, 'id>,
        resources: ModelResources<'id>,
    ) {
        let ModelResources {
            mut tickets,
            mut large,
            ..
        } = resources;
        for _ in 0..256 {
            self.0.answer(false);
        }
        self.0.answer(true);
        let rate = phase.rate(PhyRate::Legacy(LegacyRate::Ofdm6M)).unwrap();
        let plan = RetryPlan::new(RetryStage {
            rate,
            transmissions: core::num::NonZeroU8::new(255).unwrap(),
            protection: Protection::None,
        })
        .then(RetryStage {
            rate,
            transmissions: core::num::NonZeroU8::new(2).unwrap(),
            protection: Protection::None,
        })
        .unwrap();
        let TxFrame::Individual { frame, .. } = data_frame(phase, false) else {
            unreachable!()
        };
        submit_frame::<ModelLarge>(
            phase,
            tickets.remove(0),
            TxSource::Fresh {
                buffer: large.remove(0),
                frame: ClassFrame::new(TxFrame::Individual { frame, plan })
                    .ok()
                    .unwrap(),
                body: None,
            },
            None,
        );
        loop {
            let Ok(Ok(event)) = phase.next_event().await else {
                panic!("terminal")
            };
            if let LowerMacEvent::TxCompleted(TxCompleted::Single { completion, .. }) = event {
                assert!(matches!(
                    completion,
                    TxOutcome::Ended {
                        transmissions: 257,
                        status: TxEndStatus::Success,
                        ..
                    }
                ));
                break;
            }
        }
    }
}

#[test]
fn ended_preserves_exact_counts_above_u8_and_retry_changes_only_on_republication() {
    let (backend, air) = backend();
    block_on(run_port(backend, Body(LongRetryPlan(&air))));
    let sent = air.sent();
    assert_eq!(sent.len(), 257);
    assert_eq!(sent[0].frame[1] & 8, 0);
    assert!(sent[1..].iter().all(|frame| frame.frame[1] & 8 != 0));
    assert!(
        sent.iter()
            .all(|frame| frame.frame[22..24] == sent[0].frame[22..24])
    );
}
