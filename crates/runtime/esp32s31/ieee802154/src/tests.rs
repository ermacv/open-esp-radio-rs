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
use oer_esp32s31_hal::ieee802154::{
    Ieee802154TxPowerLevels,
    coex::{Ieee802154CoexConfig, Ieee802154CoexPriorities, Ieee802154Coexistence},
    ll::{Ieee802154LlCommand, model::Ieee802154LlModel},
    mac::{Ieee802154Event, Ieee802154TxAbortReason, Ieee802154TxAbortReasonObservation},
};
use oer_esp32s31_ieee802154::engine::{Ieee802154Engine, Ieee802154EngineBuffers};
use oer_esp32s31_ieee802154::pib::Ieee802154PibDefaults;
use oer_ieee802154::{
    Channel, CommandError, FrameView, RadioCommand, RadioState, RequestId, RestingState, TxMode,
    TxRequest, TxStatus,
};

use super::{
    Ieee802154Csl, Ieee802154EnhancedAckGenerator, Ieee802154EventsLost, Ieee802154Platform,
    Ieee802154RadioEvent, Ieee802154RfCloseError, Ieee802154Runtime, Ieee802154RuntimeError,
    Ieee802154RuntimeParts, Ieee802154TxRxStatistics,
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

impl<const EVENTS: usize> Runtime<EVENTS> {
    /// Model the MAC DMA writing `frame` and raising `events`, then run the
    /// interrupt handler.
    fn interrupt(&self, frame: Option<&[u8]>, events: &[Ieee802154Event]) {
        self.installed.lock(|installed| {
            let mut installed = installed.borrow_mut();
            let installed = installed.as_mut().unwrap();
            if let Some(frame) = frame {
                let address = installed.hardware.rx_address.unwrap();
                assert!(installed.radio.engine().model_dma_write(address, frame));
            }
            installed.hardware.raise(events);
        });
        self.on_interrupt();
    }
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
    runtime.interrupt(Some(&received_image()), &[Ieee802154Event::RxDone]);
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
        }))
        .unwrap();
    runtime.interrupt(None, &[Ieee802154Event::TxDone]);
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
        runtime.interrupt(Some(&received_image()), &[Ieee802154Event::RxDone]);
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
    runtime.interrupt(Some(&received_image()), &[Ieee802154Event::RxDone]);
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
        .set_coexistence(Ieee802154Coexistence::Software(
            Ieee802154CoexPriorities::resolve(Ieee802154CoexConfig::VENDOR, &CoexPtiTable::VENDOR),
        ));
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
        .set_coexistence(Ieee802154Coexistence::Software(
            Ieee802154CoexPriorities::resolve(Ieee802154CoexConfig::VENDOR, &table),
        ))
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
            .pending_mode(oer_esp32s31_hal::ieee802154::Ieee802154MultipanIndex::CONTEXT0)
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
    runtime.interrupt(Some(&image), &[Ieee802154Event::RxDone]);
    assert!(
        runtime.events.try_receive().is_err(),
        "the frame waits for its ACK"
    );
    runtime.interrupt(None, &[Ieee802154Event::AckTxDone]);
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
    runtime.interrupt(None, &[Ieee802154Event::TxAbort]);
    assert_ne!(command(), Some(Ieee802154LlCommand::CcaTxStart));
    assert!(matches!(wait(), Either::Second(())));
    assert_eq!(command(), Some(Ieee802154LlCommand::CcaTxStart));

    runtime.interrupt(None, &[Ieee802154Event::TxDone]);
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
        runtime.interrupt(None, &[Ieee802154Event::TxDone]);
        runtime.installed.lock(|installed| {
            installed.borrow_mut().as_mut().unwrap().hardware.tx_abort =
                Ieee802154TxAbortReasonObservation::Named(Ieee802154TxAbortReason::RxAckTimeout);
        });
        runtime.interrupt(None, &[Ieee802154Event::TxAbort]);
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

/// RF admission closes only while the radio sleeps; while closed, commands
/// that need RF are refused and the others still run.
#[test]
fn closed_rf_admission_refuses_commands_that_need_rf() {
    let runtime = enabled::<4>();
    runtime
        .submit(RadioCommand::Receive {
            id: RequestId::new(2),
            channel: channel(11),
        })
        .unwrap();
    assert_eq!(
        runtime.close_rf_admission(),
        Err(Ieee802154RfCloseError::Awake)
    );
    runtime
        .submit(RadioCommand::Sleep {
            id: RequestId::new(3),
        })
        .unwrap();
    runtime.close_rf_admission().unwrap();
    assert_eq!(runtime.rf_closed(), Ok(true));
    for command in [
        RadioCommand::Receive {
            id: RequestId::new(4),
            channel: channel(11),
        },
        RadioCommand::Transmit(TxRequest {
            id: RequestId::new(5),
            frame: FrameView::new(&MAC).unwrap(),
            channel: channel(11),
            mode: TxMode::Direct,
            transmit_power_dbm: None,
            max_frame_retries: 0,
            security: Default::default(),
        }),
        RadioCommand::ClearChannelAssessment {
            id: RequestId::new(6),
            channel: channel(11),
        },
    ] {
        assert_eq!(
            runtime.submit(command),
            Err(Ieee802154RuntimeError::RfClosed)
        );
    }
    assert_eq!(
        runtime.state(),
        Ok(RadioState::Resting(RestingState::Sleeping))
    );
    runtime
        .submit(RadioCommand::Configure {
            id: RequestId::new(7),
            configuration: oer_ieee802154::Configuration::PanId(0x1234),
        })
        .unwrap();
    runtime.open_rf_admission();
    assert_eq!(runtime.rf_closed(), Ok(false));
    runtime
        .submit(RadioCommand::Receive {
            id: RequestId::new(8),
            channel: channel(11),
        })
        .unwrap();
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
    runtime.interrupt(Some(&received_image()), &[Ieee802154Event::RxDone]);
    let statistics = runtime.txrx_statistics().unwrap().unwrap();
    assert_eq!(statistics.rx.done_nums, 1);
    runtime.clear_txrx_statistics().unwrap();
    assert_eq!(
        runtime.txrx_statistics(),
        Ok(Some(Ieee802154TxRxStatistics::default()))
    );
}
