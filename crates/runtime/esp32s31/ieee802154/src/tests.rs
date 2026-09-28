//! Runtime behavior over the HAL register model: the lock, the portable
//! command path, owned events and the bounded queue. The engine's vendor
//! sequences are covered by the engine tests and the host stand.

use std::boxed::Box;

use embassy_futures::{
    block_on,
    select::{Either, select},
};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_time::{Duration, Timer};
use oer_esp32s31_hal::coex::{CoexEventId, CoexPti, CoexPtiTable};
use oer_esp32s31_hal::ieee802154::coex::{Ieee802154CoexConfig, resolve_priorities};
use oer_ieee802154::{
    Channel, CommandError, FrameView, Interface, MacKeys, RadioCommand, RadioState, RequestId,
    RestingState, TxMode, TxRequest, TxStatus,
};
use oer_ieee802154_engine::engine::{
    Ieee802154Engine, Ieee802154EngineBuffers, Ieee802154Interfaces,
};
use oer_ieee802154_engine::pib::Ieee802154PibDefaults;
use oer_ieee802154_engine::{
    coex::Ieee802154Coexistence,
    ll::{Ieee802154LlCommand, model::Ieee802154LlModel},
    tx_power::Ieee802154TxPowerLevels,
    types::{Ieee802154Event, Ieee802154TxAbortReason, Ieee802154TxAbortReasonObservation},
};

use super::{
    Ieee802154Csl, Ieee802154EnhancedAckGenerator, Ieee802154EventsLost, Ieee802154Platform,
    Ieee802154RadioEvent, Ieee802154Runtime, Ieee802154RuntimeError, Ieee802154RuntimeParts,
    Ieee802154TxRxStatistics,
};

static LEVELS: [i8; 1] = [0];

const PLATFORM: Ieee802154Platform = Ieee802154Platform {
    now_micros: || 0,
    random: || 21,
};

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
    Ieee802154Runtime<'static, NoopRawMutex, Ieee802154LlModel, EVENTS>;

fn parts() -> Ieee802154RuntimeParts<'static, Ieee802154LlModel> {
    let buffers = Box::leak(Box::new(Ieee802154EngineBuffers::new()));
    let levels = Ieee802154TxPowerLevels::new(&LEVELS).unwrap();
    Ieee802154RuntimeParts {
        engine: Ieee802154Engine::new(buffers, levels, Ieee802154PibDefaults::default()),
        hardware: Ieee802154LlModel::default(),
    }
}

fn enabled<const EVENTS: usize>() -> Runtime<EVENTS> {
    let runtime = Runtime::new();
    assert!(
        runtime
            .install(parts(), PLATFORM, Ieee802154PibDefaults::default())
            .is_ok()
    );
    runtime
        .submit(RadioCommand::Enable {
            id: RequestId::new(1),
        })
        .unwrap();
    runtime
}

#[test]
fn install_admits_commands_only_after_enable() {
    let runtime = Runtime::<4>::new();
    assert_eq!(
        runtime.submit(RadioCommand::Sleep {
            id: RequestId::new(1)
        }),
        Err(Ieee802154RuntimeError::NotInstalled)
    );
    assert!(
        runtime
            .install(parts(), PLATFORM, Ieee802154PibDefaults::default())
            .is_ok()
    );
    assert!(
        runtime
            .install(parts(), PLATFORM, Ieee802154PibDefaults::default())
            .is_err()
    );
    assert_eq!(runtime.state(), Ok(RadioState::Disabled));
    assert_eq!(
        runtime.submit(RadioCommand::Sleep {
            id: RequestId::new(2)
        }),
        Err(Ieee802154RuntimeError::Rejected(CommandError::Disabled))
    );
    runtime
        .submit(RadioCommand::Enable {
            id: RequestId::new(3),
        })
        .unwrap();
    assert_eq!(
        runtime.state(),
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
        .unwrap();
    runtime.model_interrupt(Some(&received_image()), &[Ieee802154Event::RxDone]);
    let Ok(Ieee802154RadioEvent::Received(frame)) = block_on(runtime.next_event()) else {
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
        .unwrap();
    runtime.model_interrupt(None, &[Ieee802154Event::TxDone]);
    assert_eq!(
        block_on(runtime.next_event()),
        Ok(Ieee802154RadioEvent::TransmitDone {
            id: RequestId::new(5),
            status: TxStatus::Success,
            acknowledgement: None,
            security: Default::default(),
        })
    );
    assert_eq!(
        runtime.state(),
        Ok(RadioState::Resting(RestingState::Sleeping))
    );
}

/// Overflow reports the loss once and keeps the queued event.
#[test]
fn an_overflowing_queue_reports_the_loss_once() {
    let runtime = enabled::<1>();
    runtime
        .submit(RadioCommand::Receive {
            id: RequestId::new(2),
            channel: channel(11),
        })
        .unwrap();
    for _ in 0..3 {
        runtime.model_interrupt(Some(&received_image()), &[Ieee802154Event::RxDone]);
    }
    assert_eq!(block_on(runtime.next_event()), Err(Ieee802154EventsLost));
    assert!(matches!(
        block_on(runtime.next_event()),
        Ok(Ieee802154RadioEvent::Received(_))
    ));
}

#[test]
fn uninstall_returns_the_parts_and_discards_events() {
    let runtime = enabled::<4>();
    runtime
        .submit(RadioCommand::Receive {
            id: RequestId::new(2),
            channel: channel(11),
        })
        .unwrap();
    runtime.model_interrupt(Some(&received_image()), &[Ieee802154Event::RxDone]);
    assert!(runtime.uninstall().is_some());
    assert!(runtime.events.try_receive().is_err());
    assert_eq!(runtime.state(), Err(Ieee802154RuntimeError::NotInstalled));
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
    let runtime = Runtime::<4>::new();
    assert!(
        runtime
            .install(parts, PLATFORM, Ieee802154PibDefaults::default())
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
    runtime
        .submit(RadioCommand::Enable {
            id: RequestId::new(1),
        })
        .unwrap();
    runtime
        .submit(RadioCommand::Receive {
            id: RequestId::new(2),
            channel: channel(11),
        })
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
    let runtime = Runtime::<4>::new();
    assert_eq!(
        runtime.set_pending_mode(oer_ieee802154::AutoPendingMode::Enable),
        Err(Ieee802154RuntimeError::NotInstalled)
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
            .pending_mode(oer_ieee802154_engine::types::Ieee802154MultipanIndex::CONTEXT0)
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
        .unwrap();
    let mut paused = runtime.pause().unwrap();
    assert_eq!(paused.receiving(), Some(channel(15)));
    assert_eq!(
        paused.installed.radio.state(),
        RadioState::Resting(RestingState::Sleeping)
    );
    let _hardware: &mut Ieee802154LlModel = paused.hardware_mut();
    // Nothing reaches the paused radio.
    assert_eq!(runtime.state(), Err(Ieee802154RuntimeError::NotInstalled));
    assert_eq!(
        runtime.submit(RadioCommand::Sleep {
            id: RequestId::new(3)
        }),
        Err(Ieee802154RuntimeError::NotInstalled)
    );
    runtime.on_interrupt();

    runtime
        .resume(paused)
        .unwrap_or_else(|_| panic!("the runtime is empty"));
    assert_eq!(
        runtime.state(),
        Ok(RadioState::Resting(RestingState::Receiving {
            channel: channel(15)
        }))
    );
    assert!(
        runtime.events.try_receive().is_err(),
        "pausing emits no event"
    );
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
        runtime.state(),
        Ok(RadioState::Resting(RestingState::Sleeping))
    );
}

#[test]
fn a_running_operation_or_a_missing_radio_refuses_the_pause() {
    use super::Ieee802154PauseError;
    assert_eq!(
        Runtime::<4>::new().pause().err(),
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
        .unwrap();
    assert_eq!(runtime.pause().err(), Some(Ieee802154PauseError::Busy));
    assert!(matches!(
        runtime.state(),
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
    let runtime = Runtime::<4>::new();
    assert_eq!(
        runtime.with_enhanced_ack(|_| ()),
        Err(Ieee802154RuntimeError::NotInstalled)
    );
    let runtime = enabled::<4>();
    runtime
        .with_enhanced_ack(|generator| *generator = Some(Ieee802154EnhancedAckGenerator::new()))
        .unwrap();
    runtime
        .submit(RadioCommand::Receive {
            id: RequestId::new(2),
            channel: channel(11),
        })
        .unwrap();
    // 2015 data frame requesting an ACK, short addresses, compressed PAN.
    let mac = [0x61, 0xa8, 0x05, 0x34, 0x12, 0x01, 0x00, 0x02, 0x00, 0xaa];
    let mut image = [0; 13];
    image[0] = 12;
    image[1..11].copy_from_slice(&mac);
    runtime.model_interrupt(Some(&image), &[Ieee802154Event::RxDone]);
    assert!(
        runtime.events.try_receive().is_err(),
        "the frame waits for its ACK"
    );
    runtime.model_interrupt(None, &[Ieee802154Event::AckTxDone]);
    assert!(matches!(
        block_on(runtime.next_event()),
        Ok(Ieee802154RadioEvent::Received(frame)) if frame.frame.as_bytes() == mac
    ));
}

/// `next_event` runs the CSMA-CA backoff and then starts the CCA attempt; a
/// busy channel backs off again, and the outcome arrives as usual.
#[test]
fn next_event_runs_csma_ca_backoffs() {
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
        .unwrap();
    let command = || {
        runtime
            .installed
            .lock(|installed| installed.borrow().as_ref().unwrap().hardware.command)
    };
    assert_ne!(command(), Some(Ieee802154LlCommand::CcaTxStart));
    // The backoff (21 % 8 unit periods, 1.6 ms) ends within the wait.
    let wait = || {
        block_on(select(
            runtime.next_event(),
            Timer::after(Duration::from_millis(50)),
        ))
    };
    assert!(matches!(wait(), Either::Second(())));
    assert_eq!(command(), Some(Ieee802154LlCommand::CcaTxStart));

    runtime.installed.lock(|installed| {
        installed.borrow_mut().as_mut().unwrap().hardware.tx_abort =
            Ieee802154TxAbortReasonObservation::Named(Ieee802154TxAbortReason::CcaBusy);
    });
    runtime.model_interrupt(None, &[Ieee802154Event::TxAbort]);
    assert_ne!(command(), Some(Ieee802154LlCommand::CcaTxStart));
    assert!(matches!(wait(), Either::Second(())));
    assert_eq!(command(), Some(Ieee802154LlCommand::CcaTxStart));

    runtime.model_interrupt(None, &[Ieee802154Event::TxDone]);
    assert_eq!(
        block_on(runtime.next_event()),
        Ok(Ieee802154RadioEvent::TransmitDone {
            id: RequestId::new(5),
            status: TxStatus::Success,
            acknowledgement: None,
            security: Default::default(),
        })
    );
}

/// `next_event` runs the delay before a retry after a missing
/// acknowledgement, then the retry goes out.
#[test]
fn next_event_runs_retry_delays() {
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
    let waited = block_on(select(
        runtime.next_event(),
        Timer::after(Duration::from_millis(50)),
    ));
    assert!(matches!(waited, Either::Second(())));
    assert_eq!(take_command(), Some(Ieee802154LlCommand::TxStart));
    no_ack();
    assert_eq!(
        block_on(runtime.next_event()),
        Ok(Ieee802154RadioEvent::TransmitDone {
            id: RequestId::new(5),
            status: TxStatus::NoAcknowledgement,
            acknowledgement: None,
            security: Default::default(),
        })
    );
}

/// The runtime lends the radio clock and the CSL state of the installed
/// radio, and neither without one.
#[test]
fn the_clock_and_csl_state_belong_to_the_installed_radio() {
    let runtime = Runtime::<4>::new();
    assert_eq!(runtime.clock(), Err(Ieee802154RuntimeError::NotInstalled));
    let runtime = enabled::<4>();
    let clock = runtime.clock().unwrap();
    assert_eq!(clock(), (PLATFORM.now_micros)());
    runtime
        .with_csl(|csl| {
            *csl = Ieee802154Csl {
                period: 100,
                sample_time: 5_000,
            }
        })
        .unwrap();
    assert_eq!(runtime.with_csl(|csl| csl.period), Ok(100));
}

/// The runtime reads the hardware's live RSSI in any radio state, and only
/// while a radio is installed.
#[test]
fn recent_rssi_reads_the_hardware_live() {
    let runtime = Runtime::<4>::new();
    assert_eq!(
        runtime.recent_rssi(),
        Err(Ieee802154RuntimeError::NotInstalled)
    );
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
    assert_eq!(runtime.recent_rssi(), Ok(-42));
    set(-87);
    assert_eq!(runtime.recent_rssi(), Ok(-87));
    runtime
        .submit(RadioCommand::Sleep {
            id: RequestId::new(2),
        })
        .unwrap();
    assert_eq!(runtime.recent_rssi(), Ok(-87));
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
    assert_eq!(single.interfaces(), Ok(1));
    assert_eq!(
        single.with_interface_mac_keys(Interface::new(1), |_| ()),
        Ok(None)
    );

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
    let runtime = Runtime::<4>::new();
    assert!(
        runtime
            .install(parts, PLATFORM, Ieee802154PibDefaults::default())
            .is_ok()
    );
    assert_eq!(runtime.interfaces(), Ok(2));
    let keys = MacKeys::new(1, [1; 16], [2; 16], [3; 16], 9);
    assert_eq!(
        runtime.with_interface_mac_keys(Interface::new(1), |slot| *slot = Some(keys)),
        Ok(Some(()))
    );
    assert_eq!(runtime.with_mac_keys(|slot| slot.is_none()), Ok(true));
    assert_eq!(
        runtime.with_interface_mac_keys(Interface::new(1), |slot| *slot),
        Ok(Some(Some(keys)))
    );
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

    let runtime = enabled::<1>();
    runtime
        .submit(RadioCommand::Receive {
            id: RequestId::new(2),
            channel: channel(15),
        })
        .unwrap();
    leases();
    let paused = runtime.pause().unwrap();
    runtime
        .resume(paused)
        .unwrap_or_else(|_| panic!("the runtime is empty"));
    assert!(Runtime::<1>::new().pause().is_err());
    let paused = enabled::<1>().pause().unwrap();
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

    for _ in 0..2 {
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
