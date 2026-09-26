//! Runtime behavior over a register model: the engine's vendor sequences
//! are covered by the engine tests and the host stand; these tests cover the
//! lock, the event queue and slot ownership.

use std::{boxed::Box, vec::Vec};

use embassy_futures::block_on;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use oer_esp32s31_hal::ieee802154::{
    Ieee802154CcaMode, Ieee802154Channel, Ieee802154ResolvedTxPower, Ieee802154TxPowerLevels,
    ll::{
        Ieee802154EdSampleMode, Ieee802154EtmChannel, Ieee802154EtmRoute,
        Ieee802154EventObservation, Ieee802154LlCommand, Ieee802154LowLevel,
        Ieee802154RxAbortEnableSet, Ieee802154RxStateCode, Ieee802154RxStatus, Ieee802154Timer,
        Ieee802154TxAbortEnableSet,
    },
    mac::{
        Ieee802154Event, Ieee802154EventMask, Ieee802154RxAbortReasonObservation,
        Ieee802154TxAbortReasonObservation,
    },
    pib::{Ieee802154MultipanIndex, Ieee802154PibDefaults},
};
use oer_esp32s31_ieee802154::engine::{Ieee802154Engine, Ieee802154EngineBuffers, Ieee802154State};

use super::{
    Ieee802154EventsLost, Ieee802154Hardware, Ieee802154Platform, Ieee802154RadioEvent,
    Ieee802154Runtime, Ieee802154RuntimeError, Ieee802154RuntimeParts,
};

/// Register model: the scenario sets the event image; getters of the
/// automatic-ACK policy return the last value written.
#[derive(Default)]
struct Model {
    events: Ieee802154EventMask,
    rx_address: Option<u32>,
    rx_auto_ack: bool,
    tx_auto_ack: bool,
    frequency_code: u8,
    commands: Vec<Ieee802154LlCommand>,
}

impl Ieee802154Hardware for Model {
    type Port<'a> = &'a mut Model;

    fn port(&mut self) -> Self::Port<'_> {
        self
    }
}

macro_rules! ignored {
    ($($name:ident($($arg:ty),*);)*) => {
        $(fn $name(&mut self, $(_: $arg),*) {})*
    };
}

impl Ieee802154LowLevel for &mut Model {
    ignored! {
        enable_event(Ieee802154Event);
        disable_event(Ieee802154Event);
        enable_all_events();
        enable_tx_aborts(Ieee802154TxAbortEnableSet);
        enable_rx_aborts(Ieee802154RxAbortEnableSet);
        disable_rx_aborts(Ieee802154RxAbortEnableSet);
        set_ed_sample_mode(Ieee802154EdSampleMode);
        disable_coex();
        set_tx_power(&Ieee802154ResolvedTxPower<'_>);
        set_cca_mode(Ieee802154CcaMode);
        set_cca_threshold(i8);
        set_tx_enhanced_ack(bool);
        set_coordinator(bool);
        set_promiscuous(bool);
        set_pending_mode(bool);
        set_pending_bit(bool);
        set_transmit_security(bool);
        set_timer_threshold(Ieee802154Timer, u32);
        start_timer(Ieee802154Timer);
        stop_timer(Ieee802154Timer);
        disable_etm_channel(Ieee802154EtmChannel);
        enable_etm_channel(Ieee802154EtmChannel);
        set_etm_route(Ieee802154EtmRoute);
        set_tx_address(u32);
        set_ed_duration(u16);
        notify_enhanced_ack_generated();
        set_multipan_panid(Ieee802154MultipanIndex, u16);
        set_multipan_short_address(Ieee802154MultipanIndex, u16);
        set_multipan_extended_address(Ieee802154MultipanIndex, [u8; 8]);
        set_ack_timeout(u16);
        set_security_address(&[u8; 8]);
        set_security_key(&[u8; 16]);
        set_security_offset(u8);
    }

    fn set_command(&mut self, command: Ieee802154LlCommand) {
        self.commands.push(command);
    }
    fn events(&mut self) -> Ieee802154EventObservation {
        Ieee802154EventObservation::from_named(self.events)
    }
    fn clear_events(&mut self, mask: Ieee802154EventMask) {
        self.events = self.events.difference(mask);
    }
    fn rx_abort_reason(&mut self) -> Ieee802154RxAbortReasonObservation {
        Ieee802154RxAbortReasonObservation::Unclassified
    }
    fn tx_abort_reason(&mut self) -> Ieee802154TxAbortReasonObservation {
        Ieee802154TxAbortReasonObservation::Unclassified
    }
    fn rx_status(&mut self) -> Ieee802154RxStatus {
        Ieee802154RxStatus::new(
            0,
            Ieee802154RxAbortReasonObservation::Unclassified,
            Ieee802154RxStateCode::new(0).unwrap(),
            false,
            false,
            false,
        )
    }
    fn is_current_rx_frame(&mut self) -> bool {
        false
    }
    fn set_rx_address(&mut self, address: u32) {
        self.rx_address = Some(address);
    }
    fn tx_auto_ack(&mut self) -> bool {
        self.tx_auto_ack
    }
    fn rx_auto_ack(&mut self) -> bool {
        self.rx_auto_ack
    }
    fn tx_enhanced_ack(&mut self) -> bool {
        false
    }
    fn pending_mode(&mut self) -> bool {
        false
    }
    fn frequency_code(&mut self) -> u8 {
        self.frequency_code
    }
    fn ed_rss(&mut self) -> i8 {
        -70
    }
    fn cca_busy(&mut self) -> bool {
        false
    }
    fn set_channel(&mut self, channel: Ieee802154Channel) {
        self.frequency_code = channel.frequency_code().value();
    }
    fn set_tx_auto_ack(&mut self, enable: bool) {
        self.tx_auto_ack = enable;
    }
    fn set_rx_auto_ack(&mut self, enable: bool) {
        self.rx_auto_ack = enable;
    }
    fn etm_channel_enabled(&mut self, _: Ieee802154EtmChannel) -> bool {
        false
    }
    fn multipan_panid(&mut self, _: Ieee802154MultipanIndex) -> u16 {
        0
    }
    fn multipan_short_address(&mut self, _: Ieee802154MultipanIndex) -> u16 {
        0
    }
    fn multipan_extended_address(&mut self, _: Ieee802154MultipanIndex) -> [u8; 8] {
        [0; 8]
    }
    fn ack_timeout(&mut self) -> u16 {
        0
    }
}

static LEVELS: [i8; 1] = [0];

const PLATFORM: Ieee802154Platform = Ieee802154Platform {
    now_micros: || 0,
    enhanced_ack: None,
};

/// 2006 data frame without an ACK request.
const DATA: [u8; 13] = [
    0x0c, 0x41, 0x98, 0x01, 0x34, 0x12, 0xff, 0xff, 0x78, 0x56, 0xaa, 0x00, 0x00,
];

type Runtime<const EVENTS: usize> = Ieee802154Runtime<'static, NoopRawMutex, Model, EVENTS>;

fn installed<const EVENTS: usize>() -> Runtime<EVENTS> {
    let runtime = Runtime::new();
    let buffers = Box::leak(Box::new(Ieee802154EngineBuffers::new()));
    let levels = Ieee802154TxPowerLevels::new(&LEVELS).unwrap();
    let parts = Ieee802154RuntimeParts {
        engine: Ieee802154Engine::new(buffers, levels, Ieee802154PibDefaults::default()),
        hardware: Model::default(),
    };
    assert!(
        runtime
            .install(parts, PLATFORM, Ieee802154PibDefaults::default())
            .is_ok()
    );
    runtime
}

impl<const EVENTS: usize> Runtime<EVENTS> {
    /// Model the MAC DMA writing `frame` and raising `events`, then run the
    /// interrupt handler.
    fn interrupt(&self, frame: Option<&[u8]>, events: &[Ieee802154Event]) {
        self.installed.lock(|installed| {
            let mut installed = installed.borrow_mut();
            let parts = &mut installed.as_mut().unwrap().parts;
            if let Some(frame) = frame {
                let address = parts.hardware.rx_address.unwrap();
                assert!(parts.engine.model_dma_write(address, frame));
            }
            parts.hardware.events = events
                .iter()
                .fold(Ieee802154EventMask::NONE, |mask, event| {
                    mask.union(event.mask())
                });
        });
        self.on_interrupt();
    }
}

#[test]
fn install_initializes_the_mac_once() {
    let runtime = installed::<4>();
    assert_eq!(runtime.state(), Ok(Ieee802154State::Idle));

    let buffers = Box::leak(Box::new(Ieee802154EngineBuffers::new()));
    let levels = Ieee802154TxPowerLevels::new(&LEVELS).unwrap();
    let second = Ieee802154RuntimeParts {
        engine: Ieee802154Engine::new(buffers, levels, Ieee802154PibDefaults::default()),
        hardware: Model::default(),
    };
    assert!(
        runtime
            .install(second, PLATFORM, Ieee802154PibDefaults::default())
            .is_err()
    );
}

#[test]
fn operations_without_an_engine_are_refused() {
    let runtime = Runtime::<4>::new();
    assert_eq!(runtime.receive(), Err(Ieee802154RuntimeError::NotInstalled));
    runtime.on_interrupt();
    assert!(runtime.uninstall().is_none());
}

#[test]
fn a_transmission_completes_through_the_event_queue() {
    let runtime = installed::<4>();
    runtime.transmit(&DATA, false).unwrap();
    assert_eq!(
        runtime.transmit(&[0; 129], false),
        Err(Ieee802154RuntimeError::FrameImage)
    );
    runtime.interrupt(None, &[Ieee802154Event::TxDone]);
    assert_eq!(
        block_on(runtime.next_event()),
        Ok(Ieee802154RadioEvent::Transmitted { ack: None })
    );
    assert_eq!(runtime.state(), Ok(Ieee802154State::Sleep));
}

#[test]
fn a_received_frame_stays_in_its_slot_until_taken() {
    let runtime = installed::<4>();
    runtime.with_pib(|pib| pib.set_rx_when_idle(true)).unwrap();
    runtime.receive().unwrap();
    runtime.interrupt(Some(&DATA), &[Ieee802154Event::RxDone]);
    let Ok(Ieee802154RadioEvent::Received { slot, info }) = block_on(runtime.next_event()) else {
        panic!("a frame was received");
    };
    assert!(info.process);
    assert_eq!(slot.index(), 0);
    let (frame, _) = runtime.take_frame(slot).unwrap();
    assert_eq!(frame[..DATA.len()], DATA);
    assert_eq!(runtime.state(), Ok(Ieee802154State::Rx));
}

/// Overflow reports the loss once, keeps the queued event and returns
/// every dropped frame's slot to the ring, so reception never falls back to
/// the stub buffer while one frame is held.
#[test]
fn an_overflowing_queue_reports_the_loss_and_frees_the_slots() {
    let runtime = installed::<1>();
    runtime.with_pib(|pib| pib.set_rx_when_idle(true)).unwrap();
    runtime.receive().unwrap();
    for _ in 0..2 * oer_esp32s31_ieee802154::engine::RX_BUFFER_COUNT {
        runtime.interrupt(Some(&DATA), &[Ieee802154Event::RxDone]);
    }
    let receiving_into_the_ring = runtime.installed.lock(|installed| {
        let installed = installed.borrow();
        let parts = &installed.as_ref().unwrap().parts;
        parts
            .engine
            .model_rx_slot(parts.hardware.rx_address.unwrap())
            .is_some()
    });
    assert!(receiving_into_the_ring);

    assert_eq!(block_on(runtime.next_event()), Err(Ieee802154EventsLost));
    let Ok(Ieee802154RadioEvent::Received { slot, .. }) = block_on(runtime.next_event()) else {
        panic!("the first frame stays queued");
    };
    assert_eq!(slot.index(), 0);
}

#[test]
fn uninstall_returns_the_parts_and_discards_events() {
    let runtime = installed::<4>();
    runtime.transmit(&DATA, false).unwrap();
    runtime.interrupt(None, &[Ieee802154Event::TxDone]);
    let parts = runtime.uninstall().expect("an engine was installed");
    assert_eq!(parts.engine.state(), Ieee802154State::Disable);
    assert!(runtime.events.try_receive().is_err());
    assert_eq!(runtime.cca(), Err(Ieee802154RuntimeError::NotInstalled));
}
