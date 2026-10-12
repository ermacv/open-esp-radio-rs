use core::{
    cell::RefCell,
    future::Future,
    pin::pin,
    task::{Context, Poll, Waker},
};

use alloc::vec::Vec;

use oer_ieee80211_mac::{phy::LegacyRate, qos::WmmAccessCategory};
use oer_radio_port::{Disabled, Ended, Owner, PhaseParts, PortBody, run_port};

use super::*;
use oer_ieee80211_lower_mac::{
    Backoff, ChannelWidth, CoexPriority, PhyRate, Protection, QueueGrant, SupportedBackoff,
    SupportedCoex, SupportedPower, TxPower, TxResponse,
};

/// The value of a future the model drives to its end: the body and the
/// driver take turns, so it takes a few polls, never a wait.
fn now_ready<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    for _ in 0..64 {
        if let Poll::Ready(value) = future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            return value;
        }
    }
    panic!("the model answers within a few polls")
}

/// Let the driver run once: pending at the first poll.
fn yield_to_driver() -> impl Future<Output = ()> {
    let mut yielded = false;
    core::future::poll_fn(move |context| {
        if yielded {
            Poll::Ready(())
        } else {
            yielded = true;
            context.waker().wake_by_ref();
            Poll::Pending
        }
    })
}

/// Whether a future is still pending at its first poll.
fn pending<F: Future>(future: F) -> bool {
    pin!(future)
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
        .is_pending()
}

const SIX: SupportedChannel<ModelCaps> =
    SupportedChannel::from_const(match Channel::ghz2_4(6, ChannelWidth::Mhz20) {
        Ok(channel) => channel,
        Err(_) => panic!(),
    });

const ONE: SupportedChannel<ModelCaps> =
    SupportedChannel::from_const(match Channel::ghz2_4(1, ChannelWidth::Mhz20) {
        Ok(channel) => channel,
        Err(_) => panic!(),
    });

const ADDRESSES: [MacAddress; 2] = [[2, 0, 0, 0, 0, 1], [2, 0, 0, 0, 0, 2]];

/// An attempt at 6 Mb/s with no key.
fn attempt<'p, 'a, 'id, R: ModelRole>(
    client: &'p ModelClient<'a, 'id, R>,
    queue: QueuePermit<'p, 'id>,
    frame: ModelTxBuffer<'p, 'id>,
) -> PhaseAttempt<'p, 'id, ModelClient<'a, 'id, R>> {
    PhaseAttempt {
        queue,
        frame,
        body: None,
        response: TxResponse::None,
        rate: client
            .rate(PhyRate::Legacy(LegacyRate::Ofdm6M))
            .expect("channel 6 carries 6 Mb/s"),
        protection: Protection::None,
        key: None,
        power: SupportedPower::new(TxPower::Calibrated).unwrap(),
        backoff: SupportedBackoff::new(Backoff::Slots(0)).unwrap(),
        coex: SupportedCoex::new(CoexPriority::Normal).unwrap(),
    }
}

const ACCESS_CATEGORIES: [WmmAccessCategory; 4] = [
    WmmAccessCategory::BestEffort,
    WmmAccessCategory::Background,
    WmmAccessCategory::Video,
    WmmAccessCategory::Voice,
];

/// A body that transmits, drops a completion unread, drops a permit before
/// its submission and leaves an attempt for the disable; a second phase
/// then takes every queue and buffer at once.
struct OnePhase<'t> {
    world: &'t ModelHandle,
    leftovers: &'t RefCell<Vec<TxStatus>>,
}

impl PortBody<OwnedModel> for OnePhase<'_> {
    async fn run<'p, 'id>(
        self,
        mut owner: Owner<'p, 'id, OwnedModel, Disabled>,
        resources: ModelResources<'id>,
    ) -> Ended<'id, OwnedModel> {
        {
            let mut setup = owner.setup();
            setup.set_channel(SIX);
            setup.configure(VifRole::Station, [2, 0, 0, 0, 0, 1]);
        }
        let mut enabled = match owner.enable().await {
            Ok(enabled) => enabled,
            Err(failed) => return failed.fail_stop(),
        };
        assert_eq!(self.world.tuned(), SIX.channel());
        // The station's seats outlive the phases: the caller keeps them.
        let mut station_tx = resources.station_tx;
        let seats = &mut station_tx;
        {
            let PhaseParts {
                mut events,
                clients,
                ..
            } = enabled.split();
            let station = &clients.station;
            let queue = station
                .queue(seats.queue(WmmAccessCategory::BestEffort))
                .await;
            // The queue is occupied: a second taker waits, never refused.
            assert!(pending(
                station.queue(seats.queue(WmmAccessCategory::BestEffort))
            ));
            let frame = station
                .tx_buffer(
                    MpduLength::new(24).unwrap(),
                    seats.buffer(WmmAccessCategory::BestEffort),
                )
                .await;
            let work = station.submit(attempt(station, queue, frame)).unwrap();
            let id = work.id();
            // Dropping the handle neither loses nor frees the permits.
            drop(work);
            assert!(pending(
                station.queue(seats.queue(WmmAccessCategory::BestEffort))
            ));
            assert!(self.world.complete(id, TxStatus::Success));
            // The completion, dropped unread, gave its permits back.
            let event = events.next_event().await.unwrap().unwrap();
            drop(event);
            let queue = station
                .queue(seats.queue(WmmAccessCategory::BestEffort))
                .await;
            // A permit dropped before its submission goes back too.
            drop(queue);
            let queue = station
                .queue(seats.queue(WmmAccessCategory::BestEffort))
                .await;
            let frame = station
                .tx_buffer(
                    MpduLength::new(24).unwrap(),
                    seats.buffer(WmmAccessCategory::BestEffort),
                )
                .await;
            let _left = station.submit(attempt(station, queue, frame)).unwrap();
        }
        // The phase left an attempt in flight: the disable ends it and hands
        // its terminal to the sink, its permits already back.
        let leftovers = self.leftovers;
        let disabled = match enabled
            .disable(&mut |event| {
                if let Ok((completion, _)) = ModelEvents::into_completed(event) {
                    leftovers.borrow_mut().push(completion.status);
                }
            })
            .await
        {
            Ok(disabled) => disabled,
            Err(poisoned) => return poisoned.fail_stop(),
        };
        let Ok(mut enabled) = disabled.enable().await else {
            panic!("the model enables again");
        };
        {
            let PhaseParts { clients, .. } = enabled.split();
            let station = &clients.station;
            // Every queue and buffer is free: each take is served at once.
            let mut queues = Vec::new();
            for access_category in ACCESS_CATEGORIES {
                queues.push(station.queue(seats.queue(access_category)).await);
            }
            // Each seat's access category picked the queue its grant names.
            for (queue, access_category) in queues.iter().zip(ACCESS_CATEGORIES) {
                assert_eq!(queue.access_category(), access_category);
                assert_eq!(queue.queue(), ModelCaps::CAPS.tx_queue(access_category));
            }
            // An A-MPDU's buffers, taken one after another with one seat.
            let mut buffers = Vec::new();
            for _ in 0..OWNED_MODEL_TX_BUFFERS {
                buffers.push(
                    station
                        .tx_buffer(
                            MpduLength::new(24).unwrap(),
                            seats.buffer(WmmAccessCategory::BestEffort),
                        )
                        .await,
                );
            }
            assert!(pending(station.tx_buffer(
                MpduLength::new(24).unwrap(),
                seats.buffer(WmmAccessCategory::BestEffort)
            )));
        }
        match enabled.disable(&mut |_| {}).await {
            Ok(disabled) => disabled.uninstall(()),
            Err(poisoned) => poisoned.fail_stop(),
        }
    }
}

#[test]
fn a_phase_transmits_on_its_queue_and_the_backend_gives_every_permit_back_at_the_terminal() {
    let (backend, world) = OwnedModel::new(ONE, ADDRESSES);
    let leftovers = RefCell::default();
    let uninstalled = now_ready(run_port(
        backend,
        OnePhase {
            world: &world,
            leftovers: &leftovers,
        },
    ));
    assert_eq!(*leftovers.borrow(), [TxStatus::Aborted]);
    assert!(uninstalled.is_some());
    assert_eq!(
        world.lifecycle(),
        [
            "enable",
            "disable",
            "enable",
            "disable",
            "retire",
            "detach",
            "uninstall"
        ]
    );
}

/// A body that closes the gate while an attempt is published.
struct GateWaits<'t> {
    world: &'t ModelHandle,
}

impl PortBody<OwnedModel> for GateWaits<'_> {
    async fn run<'p, 'id>(
        self,
        mut owner: Owner<'p, 'id, OwnedModel, Disabled>,
        resources: ModelResources<'id>,
    ) -> Ended<'id, OwnedModel> {
        owner.setup().set_channel(SIX);
        let Ok(mut enabled) = owner.enable().await else {
            panic!("the model enables");
        };
        {
            let PhaseParts {
                mut control,
                clients,
                ..
            } = enabled.split();
            let station = &clients.station;
            let mut station_tx = resources.station_tx;
            let seats = &mut station_tx;
            let queue = station
                .queue(seats.queue(WmmAccessCategory::BestEffort))
                .await;
            let frame = station
                .tx_buffer(
                    MpduLength::new(24).unwrap(),
                    seats.buffer(WmmAccessCategory::BestEffort),
                )
                .await;
            let work = station.submit(attempt(station, queue, frame)).unwrap();
            {
                let mut closing = pin!(control.close_gate());
                // Admission stops at once; the guard waits for the air.
                assert!(
                    closing
                        .as_mut()
                        .poll(&mut Context::from_waker(Waker::noop()))
                        .is_pending()
                );
                assert!(!self.world.gate_open());
                assert!(self.world.complete(work.id(), TxStatus::Success));
                // The driver serves the end before the gate sees the air quiet.
                yield_to_driver().await;
                let Poll::Ready(gated) = closing
                    .as_mut()
                    .poll(&mut Context::from_waker(Waker::noop()))
                else {
                    panic!("no attempt is published");
                };
                drop(gated);
            }
            assert!(self.world.gate_open());
        }
        match enabled.disable(&mut |_| {}).await {
            Ok(disabled) => disabled.uninstall(()),
            Err(poisoned) => poisoned.fail_stop(),
        }
    }
}

#[test]
fn closing_the_gate_waits_for_the_published_attempt_and_its_guard_reopens_it() {
    let (backend, world) = OwnedModel::new(ONE, ADDRESSES);
    assert!(now_ready(run_port(backend, GateWaits { world: &world })).is_some());
}

/// A body that installs keys from the station's slots and frees one.
struct KeySlots<'t> {
    world: &'t ModelHandle,
}

impl PortBody<OwnedModel> for KeySlots<'_> {
    async fn run<'p, 'id>(
        self,
        mut owner: Owner<'p, 'id, OwnedModel, Disabled>,
        resources: ModelResources<'id>,
    ) -> Ended<'id, OwnedModel> {
        let [pairwise_slot, group_slot] = resources.station_keys;
        owner.setup().set_channel(SIX);
        let Ok(mut enabled) = owner.enable().await else {
            panic!("the model enables");
        };
        let pairwise = {
            let PhaseParts { clients, .. } = enabled.split();
            let station = &clients.station;
            let pairwise = station.install_key(
                pairwise_slot,
                KeyScope::Pairwise { peer: [2; 6] },
                Ccmp128Key([1; 16]),
            );
            let _group = station.install_key(
                group_slot,
                KeyScope::Group { key_id: 1 },
                Ccmp128Key([2; 16]),
            );
            pairwise
        };
        // A channel move ends the phase; the key outlives it.
        let Ok(disabled) = enabled.disable(&mut |_| {}).await else {
            panic!("the model disables");
        };
        let Ok(mut enabled) = disabled.enable().await else {
            panic!("the model enables again");
        };
        {
            let PhaseParts { clients, .. } = enabled.split();
            let station = &clients.station;
            assert_eq!(self.world.installed_keys().len(), 2);
            let slot = station.remove_key(pairwise);
            let _again = station.install_key(
                slot,
                KeyScope::Pairwise { peer: [3; 6] },
                Ccmp128Key([3; 16]),
            );
        }
        match enabled.disable(&mut |_| {}).await {
            Ok(disabled) => disabled.uninstall(()),
            Err(poisoned) => poisoned.fail_stop(),
        }
    }
}

#[test]
fn keys_from_the_installs_slots_outlive_a_channel_move_and_a_removal_returns_the_slot() {
    let (backend, world) = OwnedModel::new(ONE, ADDRESSES);
    assert!(now_ready(run_port(backend, KeySlots { world: &world })).is_some());
    assert_eq!(world.installed_keys().len(), 2);
}

#[test]
fn an_edca_record_outside_the_limits_has_no_supported_value() {
    use oer_ieee80211_mac::extensions::wmm::{WmmAcParameters, WmmParameterSet};
    let record = |aifsn, ecw_min, ecw_max| WmmAcParameters {
        admission_control_mandatory: false,
        aifsn,
        ecw_min,
        ecw_max,
        txop_limit_units_32_us: 0,
    };
    let set = |best_effort| {
        WmmParameterSet::new(
            0,
            false,
            [
                best_effort,
                record(7, 4, 10),
                record(2, 3, 4),
                record(2, 2, 3),
            ],
        )
    };
    assert!(
        oer_ieee80211_lower_mac::SupportedEdca::<ModelCaps>::new(set(record(3, 4, 10))).is_some()
    );
    assert!(
        oer_ieee80211_lower_mac::SupportedEdca::<ModelCaps>::new(set(record(16, 4, 10))).is_none()
    );
    assert!(
        oer_ieee80211_lower_mac::SupportedEdca::<ModelCaps>::new(set(record(3, 4, 11))).is_none()
    );
    assert!(
        oer_ieee80211_lower_mac::SupportedEdca::<ModelCaps>::new(set(record(3, 6, 5))).is_none()
    );
}

/// A body that enables without a setup and reads the phase's channel.
struct NoSetup<'t> {
    channel: &'t core::cell::Cell<Option<Channel>>,
}

impl PortBody<OwnedModel> for NoSetup<'_> {
    async fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, OwnedModel, Disabled>,
        _: ModelResources<'id>,
    ) -> Ended<'id, OwnedModel> {
        let Ok(mut enabled) = owner.enable().await else {
            panic!("the model enables");
        };
        {
            let PhaseParts { clients, .. } = enabled.split();
            self.channel.set(Some(clients.access_point.channel()));
            assert!(
                clients
                    .access_point
                    .rate(PhyRate::Legacy(LegacyRate::Ofdm6M))
                    .is_some()
            );
        }
        match enabled.disable(&mut |_| {}).await {
            Ok(disabled) => disabled.uninstall(()),
            Err(poisoned) => poisoned.fail_stop(),
        }
    }
}

#[test]
fn a_port_enabled_without_a_setup_runs_on_the_installs_channel_and_addresses() {
    let (backend, world) = OwnedModel::new(ONE, ADDRESSES);
    let channel = core::cell::Cell::new(None);
    assert!(now_ready(run_port(backend, NoSetup { channel: &channel })).is_some());
    assert_eq!(channel.get(), Some(ONE.channel()));
    assert_eq!(world.address(VifRole::AccessPoint), ADDRESSES[1]);
}

/// A body that drops a closing gate before it yields its guard.
struct DropsTheClosing<'t> {
    world: &'t ModelHandle,
}

impl PortBody<OwnedModel> for DropsTheClosing<'_> {
    async fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, OwnedModel, Disabled>,
        resources: ModelResources<'id>,
    ) -> Ended<'id, OwnedModel> {
        let Ok(mut enabled) = owner.enable().await else {
            panic!("the model enables");
        };
        {
            let PhaseParts {
                mut control,
                clients,
                ..
            } = enabled.split();
            let station = &clients.station;
            let mut station_tx = resources.station_tx;
            let seats = &mut station_tx;
            let queue = station
                .queue(seats.queue(WmmAccessCategory::BestEffort))
                .await;
            let frame = station
                .tx_buffer(
                    MpduLength::new(24).unwrap(),
                    seats.buffer(WmmAccessCategory::BestEffort),
                )
                .await;
            let _work = station.submit(attempt(station, queue, frame)).unwrap();
            {
                let closing = control.close_gate();
                assert!(pending(closing));
            }
            // The closing future is gone without a guard: the gate is open.
            assert!(self.world.gate_open());
        }
        match enabled.disable(&mut |_| {}).await {
            Ok(disabled) => disabled.uninstall(()),
            Err(poisoned) => poisoned.fail_stop(),
        }
    }
}

#[test]
fn a_closing_gate_dropped_before_its_guard_opens_the_gate_again() {
    let (backend, world) = OwnedModel::new(ONE, ADDRESSES);
    assert!(now_ready(run_port(backend, DropsTheClosing { world: &world })).is_some());
}

/// A waker that records whether it was woken.
struct Woken(std::sync::atomic::AtomicBool);

impl std::task::Wake for Woken {
    fn wake(self: std::sync::Arc<Self>) {
        self.0.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// A body that waits for an event the world produces later.
struct WaitsForTheWorld<'t> {
    world: &'t ModelHandle,
}

impl PortBody<OwnedModel> for WaitsForTheWorld<'_> {
    async fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, OwnedModel, Disabled>,
        resources: ModelResources<'id>,
    ) -> Ended<'id, OwnedModel> {
        let Ok(mut enabled) = owner.enable().await else {
            panic!("the model enables");
        };
        {
            let PhaseParts {
                mut events,
                clients,
                ..
            } = enabled.split();
            let station = &clients.station;
            let mut station_tx = resources.station_tx;
            let seats = &mut station_tx;
            let queue = station
                .queue(seats.queue(WmmAccessCategory::BestEffort))
                .await;
            let frame = station
                .tx_buffer(
                    MpduLength::new(24).unwrap(),
                    seats.buffer(WmmAccessCategory::BestEffort),
                )
                .await;
            let work = station.submit(attempt(station, queue, frame)).unwrap();
            let woken = std::sync::Arc::new(Woken(std::sync::atomic::AtomicBool::new(false)));
            let waker = Waker::from(woken.clone());
            let mut next = pin!(events.next_event());
            assert!(
                next.as_mut()
                    .poll(&mut Context::from_waker(&waker))
                    .is_pending()
            );
            // The world ends the attempt; the driver serves it and wakes the
            // waiting consumer.
            assert!(self.world.complete(work.id(), TxStatus::Success));
            yield_to_driver().await;
            assert!(woken.0.load(std::sync::atomic::Ordering::Relaxed));
            assert!(
                next.as_mut()
                    .poll(&mut Context::from_waker(&waker))
                    .is_ready()
            );
        }
        match enabled.disable(&mut |_| {}).await {
            Ok(disabled) => disabled.uninstall(()),
            Err(poisoned) => poisoned.fail_stop(),
        }
    }
}

#[test]
fn an_event_the_world_produces_wakes_the_waiting_consumer() {
    let (backend, world) = OwnedModel::new(ONE, ADDRESSES);
    assert!(now_ready(run_port(backend, WaitsForTheWorld { world: &world })).is_some());
}

/// A body that waits on a queue grant and never reads an event.
struct NeverReads<'t> {
    world: &'t ModelHandle,
}

impl PortBody<OwnedModel> for NeverReads<'_> {
    async fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, OwnedModel, Disabled>,
        resources: ModelResources<'id>,
    ) -> Ended<'id, OwnedModel> {
        let Ok(mut enabled) = owner.enable().await else {
            panic!("the model enables");
        };
        {
            let PhaseParts { clients, .. } = enabled.split();
            let station = &clients.station;
            let mut station_tx = resources.station_tx;
            let seats = &mut station_tx;
            let queue = station
                .queue(seats.queue(WmmAccessCategory::BestEffort))
                .await;
            let frame = station
                .tx_buffer(
                    MpduLength::new(24).unwrap(),
                    seats.buffer(WmmAccessCategory::BestEffort),
                )
                .await;
            let work = station.submit(attempt(station, queue, frame)).unwrap();
            assert!(self.world.complete(work.id(), TxStatus::Success));
            // No event is read: the driver alone gives the queue back.
            let _queue = station
                .queue(seats.queue(WmmAccessCategory::BestEffort))
                .await;
        }
        match enabled.disable(&mut |_| {}).await {
            Ok(disabled) => disabled.uninstall(()),
            Err(poisoned) => poisoned.fail_stop(),
        }
    }
}

#[test]
fn the_driver_gives_a_queue_back_to_a_body_that_reads_no_event() {
    let (backend, world) = OwnedModel::new(ONE, ADDRESSES);
    assert!(now_ready(run_port(backend, NeverReads { world: &world })).is_some());
    assert_eq!(
        world.lifecycle(),
        ["enable", "disable", "retire", "detach", "uninstall"]
    );
}
