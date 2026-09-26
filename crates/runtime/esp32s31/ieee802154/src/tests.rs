//! Runtime behavior over the HAL register model: the lock, the portable
//! command path, owned events and the bounded queue. The engine's vendor
//! sequences are covered by the engine tests and the host stand.

use std::boxed::Box;

use embassy_futures::block_on;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use oer_esp32s31_hal::ieee802154::{
    Ieee802154TxPowerLevels, ll::model::Ieee802154LlModel, mac::Ieee802154Event,
    pib::Ieee802154PibDefaults,
};
use oer_esp32s31_ieee802154::engine::{Ieee802154Engine, Ieee802154EngineBuffers};
use oer_ieee802154::{
    Channel, CommandError, FrameView, RadioCommand, RadioState, RequestId, RestingState, TxMode,
    TxRequest, TxStatus,
};

use super::{
    Ieee802154EventsLost, Ieee802154Platform, Ieee802154RadioEvent, Ieee802154Runtime,
    Ieee802154RuntimeError, Ieee802154RuntimeParts,
};

static LEVELS: [i8; 1] = [0];

const PLATFORM: Ieee802154Platform = Ieee802154Platform {
    now_micros: || 0,
    enhanced_ack: None,
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
        }))
        .unwrap();
    runtime.interrupt(None, &[Ieee802154Event::TxDone]);
    assert_eq!(
        block_on(runtime.next_event()),
        Ok(Ieee802154RadioEvent::TransmitDone {
            id: RequestId::new(5),
            status: TxStatus::Success,
            acknowledgement: None,
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
