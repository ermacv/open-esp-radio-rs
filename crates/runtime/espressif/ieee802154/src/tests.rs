//! Runtime behavior over the HAL register model: the lock, the portable
//! command path, owned events and the bounded queue. The engine's vendor
//! sequences are covered by the engine tests and the host stand.

use std::boxed::Box;

use embassy_futures::block_on;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use oer_esp32s31_hal::coex::{CoexEventId, CoexPti, CoexPtiTable};
use oer_esp32s31_hal::ieee802154::coex::{Ieee802154CoexConfig, resolve_priorities};
use oer_espressif_ieee802154_engine::engine::{
    Ieee802154Engine, Ieee802154EngineBuffers, Ieee802154Interfaces,
};
use oer_espressif_ieee802154_engine::pib::Ieee802154PibDefaults;
use oer_espressif_ieee802154_engine::{
    coex::Ieee802154Coexistence,
    ll::{Ieee802154LlCommand, model::Ieee802154LlModel},
    tx_power::Ieee802154TxPowerLevels,
    types::{Ieee802154Event, Ieee802154TxAbortReason, Ieee802154TxAbortReasonObservation},
};
use oer_ieee802154::{
    CancelError, Channel, CommandError, CslReceiver, EnhancedAckGeneration, EventsLost,
    FrameCounterUpdate, FrameView, Ieee802154RadioPort, Interface, LifecycleCommand,
    LifecycleError, LifecycleEvent, NotInstalled, RadioCapabilities, RadioCommand, RadioPort,
    RadioSetting, RadioState, RequestId, RestingState, SettingError, TxMode, TxRequest, TxStatus,
};
use oer_time::Duration;
use oer_time_virtual::VirtualClock;

use super::{
    IEEE802154_RADIO_CAPABILITIES, Ieee802154RadioEvent, Ieee802154Random, Ieee802154Runtime,
    Ieee802154RuntimeParts, Ieee802154TxRxStatistics,
};

static LEVELS: [i8; 1] = [0];

/// A random source that always draws 21.
struct FixedRandom;

impl Ieee802154Random for FixedRandom {
    fn random(&mut self) -> u32 {
        21
    }
}

/// 2006 data frame without an ACK request, as MAC bytes.
const MAC: [u8; 10] = [0x41, 0x98, 0x01, 0x34, 0x12, 0xff, 0xff, 0x78, 0x56, 0xaa];

/// `MAC` as the receive DMA writes it, RSSI -55 and LQI 180 after it.
fn received_image() -> [u8; 13] {
    let mut image = [0; 13];
    image[0] = 12;
    image[1..11].copy_from_slice(&MAC);
    image[11] = -55i8 as u8;
    image[12] = 180;
    image
}

fn channel(number: u8) -> Channel {
    Channel::new(number).unwrap()
}

type Runtime<const EVENTS: usize> =
    Ieee802154Runtime<'static, NoopRawMutex, Ieee802154LlModel, VirtualClock, FixedRandom, EVENTS>;

/// Take the next event; the runtime never poisons.
fn next<const EVENTS: usize>(
    runtime: &Runtime<EVENTS>,
) -> Result<Ieee802154RadioEvent, EventsLost> {
    match block_on(runtime.next_event()) {
        Ok(event) => event,
    }
}

/// The portable state; the runtime never poisons.
fn state<const EVENTS: usize>(runtime: &Runtime<EVENTS>) -> Result<RadioState, NotInstalled> {
    match runtime.state() {
        Ok(state) => state,
    }
}

/// Poll `future` once, as an executor does after a wake.
fn poll_once<F: core::future::Future>(
    future: core::pin::Pin<&mut F>,
) -> core::task::Poll<F::Output> {
    future.poll(&mut core::task::Context::from_waker(
        core::task::Waker::noop(),
    ))
}

fn parts() -> Ieee802154RuntimeParts<'static, Ieee802154LlModel> {
    let buffers = Box::leak(Box::new(Ieee802154EngineBuffers::new()));
    let levels = Ieee802154TxPowerLevels::new(&LEVELS).unwrap();
    Ieee802154RuntimeParts {
        engine: Ieee802154Engine::new(buffers, levels, Ieee802154PibDefaults::default()),
        hardware: Ieee802154LlModel::default(),
    }
}

fn enabled<const EVENTS: usize>() -> Runtime<EVENTS> {
    let runtime = Runtime::new(VirtualClock::new());
    assert!(
        runtime
            .install(parts(), FixedRandom, Ieee802154PibDefaults::default())
            .is_ok()
    );
    block_on(runtime.lifecycle(LifecycleCommand::Enable))
        .unwrap()
        .unwrap();
    assert_eq!(
        next(&runtime),
        Ok(Ieee802154RadioEvent::Lifecycle(LifecycleEvent::Enabled))
    );
    runtime
}

#[test]
fn install_admits_commands_only_after_enable() {
    let runtime = Runtime::<4>::new(VirtualClock::new());
    assert_eq!(
        runtime.submit(RadioCommand::Sleep {
            id: RequestId::new(1)
        }),
        Ok(Err(CommandError::NotInstalled))
    );
    assert!(
        runtime
            .install(parts(), FixedRandom, Ieee802154PibDefaults::default())
            .is_ok()
    );
    assert!(
        runtime
            .install(parts(), FixedRandom, Ieee802154PibDefaults::default())
            .is_err()
    );
    assert_eq!(state(&runtime), Ok(RadioState::Disabled));
    assert_eq!(
        runtime.submit(RadioCommand::Sleep {
            id: RequestId::new(2)
        }),
        Ok(Err(CommandError::Disabled))
    );
    block_on(runtime.lifecycle(LifecycleCommand::Enable))
        .unwrap()
        .unwrap();
    assert_eq!(
        next(&runtime),
        Ok(Ieee802154RadioEvent::Lifecycle(LifecycleEvent::Enabled))
    );
    assert_eq!(
        state(&runtime),
        Ok(RadioState::Resting(RestingState::Sleeping))
    );
}

#[test]
fn a_received_frame_arrives_as_an_owned_portable_event() {
    let runtime = enabled::<4>();
    runtime
        .submit(RadioCommand::Receive {
            id: RequestId::new(2),
            channel: channel(15),
        })
        .unwrap()
        .unwrap();
    runtime.model_interrupt(Some(&received_image()), &[Ieee802154Event::RxDone]);
    let Ok(Ieee802154RadioEvent::Received(frame)) = next(&runtime) else {
        panic!("a frame was received");
    };
    assert_eq!(frame.frame.as_bytes(), MAC);
    assert_eq!(frame.metadata.channel, channel(15));
    assert_eq!(frame.metadata.rssi_dbm, -55);
    assert_eq!(frame.metadata.link_quality, 180);
}

#[test]
fn a_transmission_completes_through_the_event_queue() {
    let runtime = enabled::<4>();
    runtime
        .submit(RadioCommand::Transmit(TxRequest {
            id: RequestId::new(5),
            frame: FrameView::new(&MAC).unwrap(),
            channel: channel(20),
            mode: TxMode::Direct,
            transmit_power_dbm: None,
            max_frame_retries: 0,
            security: Default::default(),
            interface: Interface::PRIMARY,
            time_sync: None,
        }))
        .unwrap()
        .unwrap();
    runtime.model_interrupt(None, &[Ieee802154Event::TxDone]);
    assert_eq!(
        next(&runtime),
        Ok(Ieee802154RadioEvent::TransmitDone {
            id: RequestId::new(5),
            status: TxStatus::Success,
            acknowledgement: None,
            security: Default::default(),
        })
    );
    assert_eq!(
        state(&runtime),
        Ok(RadioState::Resting(RestingState::Sleeping))
    );
}

/// Overflow reports the loss once, after the event queued before it.
#[test]
fn an_overflowing_queue_reports_the_loss_once() {
    // One of the three slots stays reserved for the fault of the enabled
    // period.
    let runtime = enabled::<3>();
    runtime
        .submit(RadioCommand::Receive {
            id: RequestId::new(2),
            channel: channel(11),
        })
        .unwrap()
        .unwrap();
    for _ in 0..4 {
        runtime.model_interrupt(Some(&received_image()), &[Ieee802154Event::RxDone]);
    }
    for _ in 0..2 {
        assert!(matches!(
            next(&runtime),
            Ok(Ieee802154RadioEvent::Received(_))
        ));
    }
    assert_eq!(next(&runtime), Err(EventsLost));
    // Events after the gap follow the marker.
    runtime.model_interrupt(Some(&received_image()), &[Ieee802154Event::RxDone]);
    assert!(matches!(
        next(&runtime),
        Ok(Ieee802154RadioEvent::Received(_))
    ));
}

#[test]
fn uninstall_returns_the_parts_and_keeps_queued_events() {
    let runtime = enabled::<4>();
    runtime
        .submit(RadioCommand::Receive {
            id: RequestId::new(2),
            channel: channel(11),
        })
        .unwrap()
        .unwrap();
    runtime.model_interrupt(Some(&received_image()), &[Ieee802154Event::RxDone]);
    assert!(runtime.uninstall().is_some());
    assert_eq!(state(&runtime), Err(NotInstalled));
    // The queued frame stays with the consumer, across a new install.
    assert!(
        runtime
            .install(parts(), FixedRandom, Ieee802154PibDefaults::default())
            .is_ok()
    );
    assert!(matches!(
        next(&runtime),
        Ok(Ieee802154RadioEvent::Received(_))
    ));
    assert!(runtime.events.take().is_none());
}

/// An uninstall ends the transmission in flight with its terminal event.
#[test]
fn uninstall_ends_the_operation_in_flight() {
    let runtime = enabled::<4>();
    runtime.submit(transmit(5)).unwrap().unwrap();
    assert!(runtime.uninstall().is_some());
    assert_eq!(next(&runtime), Ok(aborted(5)));
    assert!(runtime.events.take().is_none());
}

fn transmit(id: u32) -> RadioCommand<'static> {
    RadioCommand::Transmit(TxRequest {
        id: RequestId::new(id),
        frame: FrameView::new(&MAC).unwrap(),
        channel: channel(20),
        mode: TxMode::Direct,
        transmit_power_dbm: None,
        max_frame_retries: 0,
        security: Default::default(),
        interface: Interface::PRIMARY,
        time_sync: None,
    })
}

fn aborted(id: u32) -> Ieee802154RadioEvent {
    Ieee802154RadioEvent::TransmitDone {
        id: RequestId::new(id),
        status: TxStatus::Aborted,
        acknowledgement: None,
        security: None,
    }
}

/// The port's lifecycle ends each command with a terminal event.
#[test]
fn lifecycle_commands_end_with_terminal_events() {
    let runtime = Runtime::<4>::new(VirtualClock::new());
    assert!(
        runtime
            .install(parts(), FixedRandom, Ieee802154PibDefaults::default())
            .is_ok()
    );
    assert_eq!(
        block_on(runtime.lifecycle(LifecycleCommand::Quiesce)),
        Ok(Err(LifecycleError::InvalidState))
    );
    assert_eq!(
        block_on(runtime.lifecycle(LifecycleCommand::Enable)),
        Ok(Ok(()))
    );
    assert_eq!(
        next(&runtime),
        Ok(Ieee802154RadioEvent::Lifecycle(LifecycleEvent::Enabled))
    );
    assert_eq!(
        block_on(runtime.lifecycle(LifecycleCommand::Enable)),
        Ok(Err(LifecycleError::AlreadyInState))
    );
    assert_eq!(
        block_on(runtime.lifecycle(LifecycleCommand::Disable)),
        Ok(Ok(()))
    );
    assert_eq!(
        next(&runtime),
        Ok(Ieee802154RadioEvent::Lifecycle(LifecycleEvent::Disabled))
    );
    assert_eq!(state(&runtime), Ok(RadioState::Disabled));
    assert_eq!(
        block_on(runtime.lifecycle(LifecycleCommand::Disable)),
        Ok(Err(LifecycleError::AlreadyInState))
    );
    // The runtime's own identities lie in the backend-reserved range.
    assert!(oer_ieee802154::Correlation::is_backend_reserved(
        super::PAUSE_REQUEST
    ));
}

/// `Disable` with a transmission in flight ends it with its terminal
/// event, then reports `Disabled`.
#[test]
fn disable_ends_the_operation_in_flight() {
    let runtime = enabled::<4>();
    runtime.submit(transmit(5)).unwrap().unwrap();
    assert_eq!(
        block_on(runtime.lifecycle(LifecycleCommand::Disable)),
        Ok(Ok(()))
    );
    assert_eq!(next(&runtime), Ok(aborted(5)));
    assert_eq!(
        next(&runtime),
        Ok(Ieee802154RadioEvent::Lifecycle(LifecycleEvent::Disabled))
    );
    assert_eq!(state(&runtime), Ok(RadioState::Disabled));
}

/// `Quiesce` closes admission of operations, ends after the operation in
/// flight, and `Enable` opens admission again.
#[test]
fn quiesce_lets_the_operation_in_flight_end_and_enable_resumes() {
    let runtime = enabled::<4>();
    runtime.submit(transmit(5)).unwrap().unwrap();
    assert_eq!(
        block_on(runtime.lifecycle(LifecycleCommand::Quiesce)),
        Ok(Ok(()))
    );
    // Another lifecycle command waits for the quiesce to end.
    assert_eq!(
        block_on(runtime.lifecycle(LifecycleCommand::Enable)),
        Ok(Err(LifecycleError::Busy))
    );
    runtime.model_interrupt(None, &[Ieee802154Event::TxDone]);
    assert!(matches!(
        next(&runtime),
        Ok(Ieee802154RadioEvent::TransmitDone {
            status: TxStatus::Success,
            ..
        })
    ));
    assert_eq!(
        next(&runtime),
        Ok(Ieee802154RadioEvent::Lifecycle(LifecycleEvent::Quiesced))
    );
    assert_eq!(runtime.submit(transmit(6)), Ok(Err(CommandError::Quiesced)));
    assert_eq!(
        block_on(runtime.lifecycle(LifecycleCommand::Quiesce)),
        Ok(Err(LifecycleError::AlreadyInState))
    );
    assert_eq!(
        block_on(runtime.lifecycle(LifecycleCommand::Enable)),
        Ok(Ok(()))
    );
    assert_eq!(
        next(&runtime),
        Ok(Ieee802154RadioEvent::Lifecycle(LifecycleEvent::Enabled))
    );
    runtime.submit(transmit(6)).unwrap().unwrap();
}

/// A resting radio quiesces at once.
#[test]
fn a_resting_radio_quiesces_at_once() {
    let runtime = enabled::<4>();
    assert_eq!(
        block_on(runtime.lifecycle(LifecycleCommand::Quiesce)),
        Ok(Ok(()))
    );
    assert_eq!(
        next(&runtime),
        Ok(Ieee802154RadioEvent::Lifecycle(LifecycleEvent::Quiesced))
    );
}

/// An operation reserves its terminal event's slot: received frames never
/// take it, and an operation without a free slot is refused.
#[test]
fn an_operation_reserves_its_terminal_slot() {
    let runtime = enabled::<3>();
    runtime
        .submit(RadioCommand::Receive {
            id: RequestId::new(2),
            channel: channel(20),
        })
        .unwrap()
        .unwrap();
    // The fault slot and two free ones: two frames fill the queue, and a
    // third is lost.
    for _ in 0..3 {
        runtime.model_interrupt(Some(&received_image()), &[Ieee802154Event::RxDone]);
    }
    assert_eq!(
        runtime.submit(transmit(5)),
        Ok(Err(CommandError::EventQueueFull))
    );
    assert!(matches!(
        next(&runtime),
        Ok(Ieee802154RadioEvent::Received(_))
    ));
    runtime.submit(transmit(5)).unwrap().unwrap();
    // The transmission holds the freed slot; its completion follows the
    // loss.
    runtime.model_interrupt(None, &[Ieee802154Event::TxDone]);
    assert!(matches!(
        next(&runtime),
        Ok(Ieee802154RadioEvent::Received(_))
    ));
    assert_eq!(next(&runtime), Err(EventsLost));
    assert!(matches!(
        next(&runtime),
        Ok(Ieee802154RadioEvent::TransmitDone {
            id,
            status: TxStatus::Success,
            ..
        }) if id == RequestId::new(5)
    ));
}

/// The engine's software-coexistence PTIs are published while installed
/// and return to the disabled foundation image when the parts leave.
#[test]
fn uninstall_returns_the_coexistence_ptis_to_the_foundation_image() {
    let mut parts = parts();
    parts
        .engine
        .set_coexistence(Ieee802154Coexistence::Software(resolve_priorities(
            Ieee802154CoexConfig::VENDOR,
            &CoexPtiTable::VENDOR,
        )));
    let runtime = Runtime::<4>::new(VirtualClock::new());
    assert!(
        runtime
            .install(parts, FixedRandom, Ieee802154PibDefaults::default())
            .is_ok()
    );
    runtime.installed.lock(|installed| {
        let installed = installed.borrow();
        let hardware = &installed.as_ref().unwrap().hardware;
        assert_eq!((hardware.txrx_pti, hardware.ack_pti), (1, 8));
    });
    // A new table snapshot reaches the next scene switch.
    let mut table = CoexPtiTable::VENDOR;
    table.set(CoexEventId::new(43).unwrap(), CoexPti::new(6).unwrap());
    runtime
        .set_coexistence(Ieee802154Coexistence::Software(resolve_priorities(
            Ieee802154CoexConfig::VENDOR,
            &table,
        )))
        .unwrap();
    block_on(runtime.lifecycle(LifecycleCommand::Enable))
        .unwrap()
        .unwrap();
    assert_eq!(
        next(&runtime),
        Ok(Ieee802154RadioEvent::Lifecycle(LifecycleEvent::Enabled))
    );
    runtime
        .submit(RadioCommand::Receive {
            id: RequestId::new(2),
            channel: channel(11),
        })
        .unwrap()
        .unwrap();
    runtime.installed.lock(|installed| {
        let installed = installed.borrow();
        assert_eq!(installed.as_ref().unwrap().hardware.txrx_pti, 6);
    });
    let parts = runtime.uninstall().unwrap();
    assert_eq!((parts.hardware.txrx_pti, parts.hardware.ack_pti), (3, 3));
}

#[test]
fn the_pending_mode_reaches_the_pib_of_the_installed_radio() {
    let runtime = Runtime::<4>::new(VirtualClock::new());
    assert_eq!(
        runtime.set_pending_mode(oer_ieee802154::AutoPendingMode::Enable),
        Err(NotInstalled)
    );
    let runtime = enabled::<4>();
    runtime
        .set_pending_mode(oer_ieee802154::AutoPendingMode::Enhanced)
        .unwrap();
    let mode = runtime.installed.lock(|installed| {
        installed
            .borrow_mut()
            .as_mut()
            .unwrap()
            .radio
            .engine()
            .pib()
            .pending_mode(oer_espressif_ieee802154_engine::types::Ieee802154MultipanIndex::CONTEXT0)
    });
    assert_eq!(mode, oer_ieee802154::AutoPendingMode::Enhanced);
}

#[test]
fn pausing_leaves_receive_mode_and_resuming_enters_it_again() {
    let runtime = enabled::<4>();
    runtime
        .submit(RadioCommand::Receive {
            id: RequestId::new(2),
            channel: channel(15),
        })
        .unwrap()
        .unwrap();
    let mut paused = runtime.pause().unwrap();
    assert_eq!(paused.receiving(), Some(channel(15)));
    assert_eq!(
        paused.installed.radio.state(),
        RadioState::Resting(RestingState::Sleeping)
    );
    let _hardware: &mut Ieee802154LlModel = paused.hardware_mut();
    // Nothing reaches the paused radio.
    assert_eq!(state(&runtime), Err(NotInstalled));
    assert_eq!(
        runtime.submit(RadioCommand::Sleep {
            id: RequestId::new(3)
        }),
        Ok(Err(CommandError::NotInstalled))
    );
    runtime.on_interrupt();

    runtime
        .resume(paused)
        .unwrap_or_else(|_| panic!("the runtime is empty"));
    assert_eq!(
        state(&runtime),
        Ok(RadioState::Resting(RestingState::Receiving {
            channel: channel(15)
        }))
    );
    assert!(runtime.events.take().is_none(), "pausing emits no event");
}

#[test]
fn a_sleeping_radio_resumes_asleep() {
    let runtime = enabled::<4>();
    let paused = runtime.pause().unwrap();
    assert_eq!(paused.receiving(), None);
    runtime
        .resume(paused)
        .unwrap_or_else(|_| panic!("the runtime is empty"));
    assert_eq!(
        state(&runtime),
        Ok(RadioState::Resting(RestingState::Sleeping))
    );
}

#[test]
fn a_running_operation_or_a_missing_radio_refuses_the_pause() {
    use super::Ieee802154PauseError;
    assert_eq!(
        Runtime::<4>::new(VirtualClock::new()).pause().err(),
        Some(Ieee802154PauseError::NotInstalled)
    );
    let runtime = enabled::<4>();
    runtime
        .submit(RadioCommand::Transmit(TxRequest {
            id: RequestId::new(5),
            frame: FrameView::new(&MAC).unwrap(),
            channel: channel(20),
            mode: TxMode::Direct,
            transmit_power_dbm: None,
            max_frame_retries: 0,
            security: Default::default(),
            interface: Interface::PRIMARY,
            time_sync: None,
        }))
        .unwrap()
        .unwrap();
    assert_eq!(runtime.pause().err(), Some(Ieee802154PauseError::Busy));
    assert!(matches!(
        state(&runtime),
        Ok(RadioState::Transmitting { .. })
    ));
}

#[test]
fn resuming_over_an_installed_radio_returns_the_paused_one() {
    let first = enabled::<4>();
    let paused = first.pause().unwrap();
    let occupied = enabled::<4>();
    assert!(occupied.resume(paused).is_err());
}

/// An installed generator answers a 2015 frame with an enhanced ACK; the
/// frame reaches the queue once the ACK is sent.
#[test]
fn an_installed_enhanced_ack_generator_answers_2015_frames() {
    let generation = RadioSetting::EnhancedAck(Some(EnhancedAckGeneration { noise_floor_dbm: 0 }));
    let runtime = Runtime::<4>::new(VirtualClock::new());
    assert_eq!(
        runtime.apply(generation),
        Ok(Err(SettingError::NotInstalled))
    );
    let runtime = enabled::<4>();
    assert_eq!(runtime.apply(generation), Ok(Ok(())));
    runtime
        .submit(RadioCommand::Receive {
            id: RequestId::new(2),
            channel: channel(11),
        })
        .unwrap()
        .unwrap();
    // 2015 data frame requesting an ACK, short addresses, compressed PAN.
    let mac = [0x61, 0xa8, 0x05, 0x34, 0x12, 0x01, 0x00, 0x02, 0x00, 0xaa];
    let mut image = [0; 13];
    image[0] = 12;
    image[1..11].copy_from_slice(&mac);
    runtime.model_interrupt(Some(&image), &[Ieee802154Event::RxDone]);
    assert!(
        runtime.events.take().is_none(),
        "the frame waits for its ACK"
    );
    runtime.model_interrupt(None, &[Ieee802154Event::AckTxDone]);
    assert!(matches!(
        next(&runtime),
        Ok(Ieee802154RadioEvent::Received(frame)) if frame.frame.as_bytes() == mac
    ));
}

/// The runner runs the CSMA-CA backoff and then starts the CCA attempt; a
/// busy channel backs off again, and the outcome arrives as usual.
/// Awaiting events alone does not progress the backoff.
#[test]
fn the_runner_runs_csma_ca_backoffs() {
    let runtime = enabled::<4>();
    runtime
        .submit(RadioCommand::Transmit(TxRequest {
            id: RequestId::new(5),
            frame: FrameView::new(&MAC).unwrap(),
            channel: channel(20),
            mode: TxMode::CsmaCa { max_backoffs: 3 },
            transmit_power_dbm: None,
            max_frame_retries: 0,
            security: Default::default(),
            interface: Interface::PRIMARY,
            time_sync: None,
        }))
        .unwrap()
        .unwrap();
    let command = || {
        runtime
            .installed
            .lock(|installed| installed.borrow().as_ref().unwrap().hardware.command)
    };
    assert_ne!(command(), Some(Ieee802154LlCommand::CcaTxStart));
    // Taking events only dequeues them.
    assert!(poll_once(core::pin::pin!(runtime.next_event())).is_pending());
    assert_ne!(command(), Some(Ieee802154LlCommand::CcaTxStart));
    // The backoff (21 % 8 unit periods, 1.6 ms) ends only once its time has
    // passed.
    let mut run = core::pin::pin!(runtime.run());
    assert!(poll_once(run.as_mut()).is_pending());
    runtime.timer.advance(Duration::from_micros(1_599)).unwrap();
    assert!(poll_once(run.as_mut()).is_pending());
    assert_ne!(command(), Some(Ieee802154LlCommand::CcaTxStart));
    runtime.timer.advance(Duration::from_micros(1)).unwrap();
    assert!(poll_once(run.as_mut()).is_pending());
    assert_eq!(command(), Some(Ieee802154LlCommand::CcaTxStart));

    runtime.installed.lock(|installed| {
        installed.borrow_mut().as_mut().unwrap().hardware.tx_abort =
            Ieee802154TxAbortReasonObservation::Named(Ieee802154TxAbortReason::CcaBusy);
    });
    runtime.model_interrupt(None, &[Ieee802154Event::TxAbort]);
    assert_ne!(command(), Some(Ieee802154LlCommand::CcaTxStart));
    assert!(poll_once(run.as_mut()).is_pending());
    runtime.timer.advance(Duration::from_millis(50)).unwrap();
    assert!(poll_once(run.as_mut()).is_pending());
    assert_eq!(command(), Some(Ieee802154LlCommand::CcaTxStart));

    runtime.model_interrupt(None, &[Ieee802154Event::TxDone]);
    assert_eq!(
        next(&runtime),
        Ok(Ieee802154RadioEvent::TransmitDone {
            id: RequestId::new(5),
            status: TxStatus::Success,
            acknowledgement: None,
            security: Default::default(),
        })
    );
}

/// The runner runs the delay before a retry after a missing
/// acknowledgement, then the retry goes out.
#[test]
fn the_runner_runs_retry_delays() {
    let runtime = enabled::<4>();
    let mut acknowledged = MAC;
    acknowledged[0] |= 0x20;
    runtime
        .submit(RadioCommand::Transmit(TxRequest {
            id: RequestId::new(5),
            frame: FrameView::new(&acknowledged).unwrap(),
            channel: channel(20),
            mode: TxMode::Direct,
            transmit_power_dbm: None,
            max_frame_retries: 1,
            security: Default::default(),
            interface: Interface::PRIMARY,
            time_sync: None,
        }))
        .unwrap()
        .unwrap();
    // The last MAC command, cleared once read.
    let take_command = || {
        runtime.installed.lock(|installed| {
            installed
                .borrow_mut()
                .as_mut()
                .unwrap()
                .hardware
                .command
                .take()
        })
    };
    let no_ack = || {
        runtime.model_interrupt(None, &[Ieee802154Event::TxDone]);
        runtime.installed.lock(|installed| {
            installed.borrow_mut().as_mut().unwrap().hardware.tx_abort =
                Ieee802154TxAbortReasonObservation::Named(Ieee802154TxAbortReason::RxAckTimeout);
        });
        runtime.model_interrupt(None, &[Ieee802154Event::TxAbort]);
    };
    assert_eq!(take_command(), Some(Ieee802154LlCommand::TxStart));
    no_ack();
    assert_ne!(take_command(), Some(Ieee802154LlCommand::TxStart));
    let mut run = core::pin::pin!(runtime.run());
    assert!(poll_once(run.as_mut()).is_pending());
    runtime.timer.advance(Duration::from_millis(50)).unwrap();
    assert!(poll_once(run.as_mut()).is_pending());
    assert_eq!(take_command(), Some(Ieee802154LlCommand::TxStart));
    no_ack();
    assert_eq!(
        next(&runtime),
        Ok(Ieee802154RadioEvent::TransmitDone {
            id: RequestId::new(5),
            status: TxStatus::NoAcknowledgement,
            acknowledgement: None,
            security: Default::default(),
        })
    );
}

/// The runtime reads the radio clock and the CSL state of the installed
/// radio, and neither without one; the clock is the platform's monotonic
/// clock.
#[test]
fn the_clock_and_csl_state_belong_to_the_installed_radio() {
    let runtime = Runtime::<4>::new(VirtualClock::new());
    assert_eq!(
        block_on(runtime.now()).map(|now| now.map(oer_ieee802154::Ieee802154Instant::as_micros)),
        Ok(Ok(0)),
        "the clock runs before a radio is installed"
    );
    assert_eq!(
        runtime.clock_info().epoch,
        oer_ieee802154::RadioEpoch::Monotonic
    );
    let runtime = enabled::<4>();
    assert_eq!(
        block_on(runtime.now()).map(|now| now.map(oer_ieee802154::Ieee802154Instant::as_micros)),
        Ok(Ok(0)),
        "the radio clock is the runtime's virtual clock, at its epoch"
    );
    assert_eq!(
        runtime.apply(RadioSetting::Csl(CslReceiver {
            period: 100,
            sample_time: 5_000,
        })),
        Ok(Ok(()))
    );
    assert_eq!(
        runtime.with_radio(|radio, _, _| (radio.csl().period, radio.csl().sample_time)),
        Ok((100, 5_000))
    );
}

/// The runtime reads the hardware's live RSSI in any radio state, and only
/// while a radio is installed.
#[test]
fn recent_rssi_reads_the_hardware_live() {
    let runtime = Runtime::<4>::new(VirtualClock::new());
    assert_eq!(runtime.recent_rssi(), Ok(Err(NotInstalled)));
    let runtime = enabled::<4>();
    let set = |rssi: i8| {
        runtime.installed.lock(|installed| {
            installed
                .borrow_mut()
                .as_mut()
                .unwrap()
                .hardware
                .recent_rssi = rssi;
        });
    };
    set(-42);
    assert_eq!(runtime.recent_rssi(), Ok(Ok(-42)));
    set(-87);
    assert_eq!(runtime.recent_rssi(), Ok(Ok(-87)));
    runtime
        .submit(RadioCommand::Sleep {
            id: RequestId::new(2),
        })
        .unwrap()
        .unwrap();
    assert_eq!(runtime.recent_rssi(), Ok(Ok(-87)));
}

/// The runtime collects, lends and clears the engine's TX/RX statistics.
#[test]
fn txrx_statistics_are_collected_on_request() {
    let runtime = enabled::<4>();
    assert_eq!(runtime.txrx_statistics(), Ok(None));
    runtime.set_txrx_statistics(true).unwrap();
    runtime
        .submit(RadioCommand::Receive {
            id: RequestId::new(2),
            channel: channel(15),
        })
        .unwrap()
        .unwrap();
    runtime.model_interrupt(Some(&received_image()), &[Ieee802154Event::RxDone]);
    let statistics = runtime.txrx_statistics().unwrap().unwrap();
    assert_eq!(statistics.rx.done_nums, 1);
    runtime.clear_txrx_statistics().unwrap();
    assert_eq!(
        runtime.txrx_statistics(),
        Ok(Some(Ieee802154TxRxStatistics::default()))
    );
}

/// A runtime over a multi-PAN engine lends the keys of each interface it
/// has, and a single-interface runtime only the primary interface's.
#[test]
fn interface_keys_belong_to_the_interfaces_of_the_installed_radio() {
    let single = enabled::<4>();
    assert_eq!(single.capabilities().interfaces, 1);
    assert_eq!(
        single.apply(RadioSetting::RemoveMacKeys {
            interface: Interface::new(1)
        }),
        Ok(Err(SettingError::UnknownInterface {
            interface: Interface::new(1),
            interfaces: 1,
        }))
    );
    assert_eq!(single.frame_counter(Interface::new(1)), Ok(Ok(None)));

    let buffers = Box::leak(Box::new(Ieee802154EngineBuffers::new()));
    let levels = Ieee802154TxPowerLevels::new(&LEVELS).unwrap();
    let parts = Ieee802154RuntimeParts {
        engine: Ieee802154Engine::new_multipan(
            buffers,
            levels,
            Ieee802154PibDefaults::default(),
            Ieee802154Interfaces::new(2).unwrap(),
        ),
        hardware: Ieee802154LlModel::default(),
    };
    let runtime = Runtime::<4>::new(VirtualClock::new());
    assert!(
        runtime
            .install(parts, FixedRandom, Ieee802154PibDefaults::default())
            .is_ok()
    );
    assert_eq!(runtime.capabilities().interfaces, 2);
    assert!(
        runtime
            .capabilities()
            .operations
            .contains(RadioCapabilities::MULTI_PAN)
    );
    // Keys start from the zeroed ones and keep the counter; the counter
    // keeps the keys.
    let second = Interface::new(1);
    let keys = RadioSetting::MacKeys {
        interface: second,
        key_id: 1,
        previous: [1; 16],
        current: [2; 16],
        next: [3; 16],
    };
    assert_eq!(runtime.apply(keys), Ok(Ok(())));
    assert_eq!(runtime.frame_counter(second), Ok(Ok(Some(0))));
    for update in [
        FrameCounterUpdate::Set(9),
        FrameCounterUpdate::SetIfLarger(5),
    ] {
        assert_eq!(
            runtime.apply(RadioSetting::FrameCounter {
                interface: second,
                update,
            }),
            Ok(Ok(()))
        );
    }
    assert_eq!(runtime.frame_counter(second), Ok(Ok(Some(9))));
    assert_eq!(runtime.frame_counter(Interface::PRIMARY), Ok(Ok(None)));
    let installed = runtime.with_radio(|radio, _, _| {
        radio
            .interface_mac_keys(second)
            .and_then(|keys| *keys)
            .map(|mut keys| keys.transmit_security(false))
    });
    let security = installed.unwrap().unwrap();
    assert_eq!((security.key_id, security.key), (Some(1), [2; 16]));
    assert_eq!(
        runtime.apply(RadioSetting::RemoveMacKeys { interface: second }),
        Ok(Ok(()))
    );
    assert_eq!(runtime.frame_counter(second), Ok(Ok(None)));
}

/// Pausing and resuming record the lease with the receive channel, and a
/// received frame the full queue drops is recorded as dropped.
#[test]
fn the_lease_and_a_dropped_frame_are_traced() {
    use oer_ieee802154_trace::{Lease, PauseRefusal, RxDrop, RxOutcome, RxResult};
    let leases = || -> std::vec::Vec<Lease> {
        super::trace::take_recorded()
            .iter()
            .filter_map(|record| record.decode::<Lease>())
            .collect()
    };

    // The fault slot and two free ones.
    let runtime = enabled::<3>();
    runtime
        .submit(RadioCommand::Receive {
            id: RequestId::new(2),
            channel: channel(15),
        })
        .unwrap()
        .unwrap();
    leases();
    let paused = runtime.pause().unwrap();
    runtime
        .resume(paused)
        .unwrap_or_else(|_| panic!("the runtime is empty"));
    assert!(Runtime::<3>::new(VirtualClock::new()).pause().is_err());
    let paused = enabled::<3>().pause().unwrap();
    assert!(runtime.resume(paused).is_err());
    assert_eq!(
        leases(),
        [
            Lease::Paused {
                receiving: Some(15)
            },
            Lease::Resumed {
                receiving: Some(15)
            },
            Lease::PauseRefused(PauseRefusal::NotInstalled),
            Lease::Paused { receiving: None },
            Lease::ResumeRefused,
        ]
    );

    for _ in 0..3 {
        runtime.model_interrupt(Some(&received_image()), &[Ieee802154Event::RxDone]);
    }
    let drops: std::vec::Vec<RxOutcome> = super::trace::take_recorded()
        .iter()
        .filter_map(|record| record.decode::<RxOutcome>())
        .filter(|outcome| matches!(outcome.result, RxResult::Dropped(_)))
        .collect();
    assert_eq!(
        drops,
        [RxOutcome {
            length: 12,
            result: RxResult::Dropped(RxDrop::QueueFull),
        }]
    );
}

/// The port reports the role's capabilities before and after installation.
#[test]
fn the_port_reports_the_role_capabilities() {
    let runtime = Runtime::<4>::new(VirtualClock::new());
    assert_eq!(
        runtime.capabilities().operations,
        IEEE802154_RADIO_CAPABILITIES
    );
    assert_eq!(
        enabled::<4>().capabilities().operations,
        IEEE802154_RADIO_CAPABILITIES
    );
}

/// A cancelled transmission's aborted end arrives through the queue.
#[test]
fn a_cancellation_ends_the_transmission_through_the_queue() {
    let runtime = enabled::<4>();
    runtime
        .submit(RadioCommand::Transmit(TxRequest {
            id: RequestId::new(5),
            frame: FrameView::new(&MAC).unwrap(),
            channel: channel(20),
            mode: TxMode::Direct,
            transmit_power_dbm: None,
            max_frame_retries: 0,
            security: Default::default(),
            interface: Interface::PRIMARY,
            time_sync: None,
        }))
        .unwrap()
        .unwrap();
    assert_eq!(
        block_on(runtime.cancel(RequestId::new(6))),
        Ok(Err(CancelError::NotRunning))
    );
    assert_eq!(block_on(runtime.cancel(RequestId::new(5))), Ok(Ok(())));
    assert_eq!(
        next(&runtime),
        Ok(Ieee802154RadioEvent::TransmitDone {
            id: RequestId::new(5),
            status: TxStatus::Aborted,
            acknowledgement: None,
            security: None,
        })
    );
    assert_eq!(
        state(&runtime),
        Ok(RadioState::Resting(RestingState::Sleeping))
    );
}

/// Enhanced-ACK settings need a generator; header IEs are bounded.
#[test]
fn enhanced_ack_settings_need_the_generator() {
    let runtime = enabled::<4>();
    assert_eq!(
        runtime.apply(RadioSetting::EnhancedAckHeaderIes(&[1, 2])),
        Ok(Err(SettingError::EnhancedAckDisabled))
    );
    assert_eq!(
        runtime.apply(RadioSetting::EnhancedAckProbing(&[])),
        Ok(Err(SettingError::EnhancedAckDisabled))
    );
    runtime
        .apply(RadioSetting::EnhancedAck(Some(EnhancedAckGeneration {
            noise_floor_dbm: -100,
        })))
        .unwrap()
        .unwrap();
    assert_eq!(
        runtime.apply(RadioSetting::EnhancedAckHeaderIes(&[0; 17])),
        Ok(Err(SettingError::HeaderIesTooLong { capacity: 16 }))
    );
    assert_eq!(
        runtime.apply(RadioSetting::EnhancedAckHeaderIes(&[1, 2])),
        Ok(Ok(()))
    );
    let (ies, floor) = runtime
        .with_radio(|radio, _, _| {
            let generator = radio.enhanced_ack().as_mut().unwrap();
            (
                generator.header_ies().to_vec(),
                generator.probing().noise_floor(),
            )
        })
        .unwrap();
    assert_eq!((ies.as_slice(), floor), (&[1, 2][..], -100));
    runtime
        .apply(RadioSetting::EnhancedAck(None))
        .unwrap()
        .unwrap();
    assert_eq!(
        runtime.with_radio(|radio, _, _| radio.enhanced_ack().is_none()),
        Ok(true)
    );
}

/// Only the first fault of an enabled period holds a reserved slot; a
/// further one, as a disabled radio may still report, takes a free slot or
/// is lost instead of overrunning the queue.
#[test]
fn a_fault_beyond_the_reserved_one_never_overruns_the_queue() {
    use oer_ieee802154::RadioFault;
    let queue = super::EventQueue::<NoopRawMutex, 2>::new();
    queue.owed(|owed| owed.fault = true);
    let fault = || Ieee802154RadioEvent::Fault {
        id: None,
        fault: RadioFault::InvalidEventSequence,
    };
    assert!(queue.push(fault()));
    assert!(queue.push(fault()));
    // Both slots are taken: the third fault is lost, not a panic.
    assert!(!queue.push(fault()));
    assert_eq!(queue.take(), Some(Ok(fault())));
    assert_eq!(queue.take(), Some(Ok(fault())));
    assert_eq!(queue.take(), Some(Err(oer_ieee802154::EventsLost)));
    assert_eq!(queue.take(), None);
}
