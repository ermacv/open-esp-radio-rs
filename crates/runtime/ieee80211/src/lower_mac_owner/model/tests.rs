//! The model on the phase shape, under `run_port`: the conformance of its
//! order and the three tests of the owner decisions on #563
//! (https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6129037560).

use super::*;
use core::num::NonZeroU8;
use oer_ieee80211_lower_mac::{
    AckHeader, ChannelWidth, FrameControl, HeaderSpec, MpduLength, RetryPlan, RetryStage,
    SizedFrame, SupportedCoex, SupportedPower, TxPower,
};
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
    poll_fn(|_| if open.get() { Poll::Ready(()) } else { Poll::Pending })
}

/// A data frame to the peer, `len` octets, in `buffer`, submitted with
/// `ticket` and `key`.
fn submit<'p, 'id: 'p>(
    phase: &mut ModelPhase<'p, 'id>,
    ticket: Slot<'id, ModelTickets>,
    mut buffer: ModelBuffer<'id, ModelSmall>,
    key: Option<&ModelPairwiseKey<'id, Station>>,
) {
    let rate = phase
        .rate(PhyRate::Legacy(LegacyRate::Ofdm6M))
        .expect("the channel carries 6 Mbit/s");
    let header = HeaderSpec::new(
        FrameControl::new(0x0108).unwrap(),
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
    let len = 40;
    buffer.bytes_mut()[..len].fill(0x5a);
    let frame = TxFrame::Individual {
        frame: SizedFrame::new(header, MpduLength::new(len).unwrap()).unwrap(),
        plan: RetryPlan::new(RetryStage {
            rate,
            transmissions: NonZeroU8::new(2).unwrap(),
            protection: Protection::None,
        }),
    };
    let mut station = phase.station();
    let frame = station.tx_buffer::<ModelSmall>(buffer, ClassFrame::new(frame).ok().unwrap());
    station
        .submit(oer_ieee80211_lower_mac::Attempt {
            ticket,
            frame,
            body: None,
            access_category: WmmAccessCategory::BestEffort,
            protection: Protection::None,
            key: key.map(AttemptKey::Pairwise),
            power: SupportedPower::new(TxPower::Calibrated).unwrap(),
            coex: SupportedCoex::new(CoexPriority::Normal).unwrap(),
        })
        .unwrap();
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
async fn drain<'p, 'id: 'p>(phase: &mut ModelPhase<'p, 'id>, stream: &mut Stream) {
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
            Ok(Ok(LowerMacEvent::TxCompleted { .. })) => stream.completions += 1,
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
    async fn work<'p, 'id: 'p>(self, phase: &mut ModelPhase<'p, 'id>, resources: ModelResources<'id>);
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
    async fn work<'p, 'id: 'p>(self, phase: &mut ModelPhase<'p, 'id>, resources: ModelResources<'id>) {
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
            drain(phase, &mut stream).await;
            yield_now().await;
        }
        // The key ends with the install.
        core::mem::forget(key);
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

/// Submit every ticket's attempt, then wait for `open` before reading.
struct WaitForever<'t> {
    open: &'t Cell<bool>,
    stream: &'t RefCell<Stream>,
}

impl Work for WaitForever<'_> {
    async fn work<'p, 'id: 'p>(self, phase: &mut ModelPhase<'p, 'id>, resources: ModelResources<'id>) {
        let ModelResources { tickets, small, .. } = resources;
        for (ticket, buffer) in tickets.into_iter().zip(small) {
            submit(phase, ticket, buffer, None);
        }
        // The body waits on something else entirely.
        until(self.open).await;
        drain(phase, &mut self.stream.borrow_mut()).await;
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
    let meta = RxMeta::unavailable(Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap());
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
            completions: MODEL_SMALL_BUFFERS,
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
    async fn work<'p, 'id: 'p>(self, phase: &mut ModelPhase<'p, 'id>, resources: ModelResources<'id>) {
        let ModelResources { tickets, small, .. } = resources;
        for _ in 0..MODEL_SMALL_BUFFERS {
            self.air.answer(false);
            self.air.answer(false);
        }
        for (ticket, buffer) in tickets.into_iter().zip(small) {
            submit(phase, ticket, buffer, None);
            if self.settings {
                // Every setting changes between the submissions, unread.
                phase.station().configure(StationReceive::Off);
                phase.station().set_bss_color(BssColor::new(5).unwrap());
                phase
                    .access_point()
                    .configure(SupportedFilter::new(ReceiveFilter::BSS_MEMBER).unwrap());
            }
        }
        for _ in 0..10 {
            yield_now().await;
        }
        drain(phase, &mut self.stream.borrow_mut()).await;
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
    assert_eq!(stream.borrow().completions, MODEL_SMALL_BUFFERS);
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
    // Every submission arrived and ended; every kind of setting took its
    // latest value, before the next submission.
    assert_eq!(stream.borrow().completions, MODEL_SMALL_BUFFERS);
    assert_eq!(air.commands(), MODEL_SMALL_BUFFERS as u32);
    assert!(air.settings() >= 3);
}
