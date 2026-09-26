//! Engine behavior against a register model. Every expectation is read from
//! the pinned `esp_ieee802154_dev.c`, not from this engine's output.

use std::{boxed::Box, vec, vec::Vec};

use oer_esp32s31_hal::ieee802154::{
    Ieee802154CcaMode, Ieee802154Channel, Ieee802154ResolvedTxPower, Ieee802154TxPowerLevels,
    ll::{
        Ieee802154EdSampleMode, Ieee802154EtmChannel, Ieee802154EtmRoute,
        Ieee802154EventObservation, Ieee802154LlCommand, Ieee802154LowLevel,
        Ieee802154RxAbortEnableSet, Ieee802154RxStateCode, Ieee802154RxStatus, Ieee802154Timer,
        Ieee802154TxAbortEnableSet,
    },
    mac::{
        Ieee802154Event, Ieee802154EventMask, Ieee802154RxAbortReason,
        Ieee802154RxAbortReasonObservation, Ieee802154TxAbortReasonObservation,
    },
    pib::{Ieee802154MultipanIndex, Ieee802154PibDefaults},
};

use super::{
    FRAME_SIZE, Ieee802154Engine, Ieee802154EngineBuffers, Ieee802154Environment,
    Ieee802154FrameInfo, Ieee802154ReceivedAck, Ieee802154RxSlot, Ieee802154SlotError,
    Ieee802154State, Ieee802154TxError, RX_BUFFER_COUNT,
};

static LEVELS: [i8; 4] = [-9, -3, 4, 10];

/// Recorded register accessor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Call {
    Command(Ieee802154LlCommand),
    Events,
    ClearEvents(Ieee802154EventMask),
    RxAbortReason,
    TxAbortReason,
    RxStatus,
    IsCurrentRxFrame,
    TxAddress(u32),
    RxAddress(u32),
    TxAutoAck,
    RxAutoAck,
    TxEnhancedAck,
    PendingMode,
    SetPendingBit(bool),
    FrequencyCode,
    EdRss,
    CcaBusy,
    EdDuration(u16),
    EnhancedAckNotify,
    DisableRxAborts(Ieee802154RxAbortEnableSet),
    EnableAllEvents,
    EnableEvent(Ieee802154Event),
    DisableEvent(Ieee802154Event),
    EnableTxAborts(Ieee802154TxAbortEnableSet),
    EnableRxAborts(Ieee802154RxAbortEnableSet),
    EdSampleMode(Ieee802154EdSampleMode),
    DisableCoex,
    SetChannel(u8),
    SetTxPower(u8),
    SetCcaMode(Ieee802154CcaMode),
    SetCcaThreshold(i8),
    SetTxAutoAck(bool),
    SetRxAutoAck(bool),
    SetTxEnhancedAck(bool),
    SetCoordinator(bool),
    SetPromiscuous(bool),
    SetPendingMode(bool),
    SetTransmitSecurity(bool),
    Threshold(Ieee802154Timer, u32),
    StartTimer(Ieee802154Timer),
    StopTimer(Ieee802154Timer),
    EtmEnabled(Ieee802154EtmChannel),
    DisableEtm(Ieee802154EtmChannel),
    EnableEtm(Ieee802154EtmChannel),
    EtmRoute(Ieee802154EtmRoute),
    SetPanId(u16),
    SetShortAddress(u16),
    SetExtendedAddress([u8; 8]),
    SetAckTimeout(u16),
    SecurityAddress([u8; 8]),
    SecurityKey([u8; 16]),
    SecurityOffset(u8),
}

/// Register model: getters return the last value set.
struct Hw {
    calls: Vec<Call>,
    events: Ieee802154EventMask,
    rx_abort: Ieee802154RxAbortReasonObservation,
    tx_abort: Ieee802154TxAbortReasonObservation,
    tx_auto_ack: bool,
    rx_auto_ack: bool,
    tx_enhanced_ack: bool,
    pending_mode: bool,
    frequency_code: u8,
    etm_enabled: [bool; 2],
    current_rx_frame: bool,
    ed_rss: i8,
    cca_busy: bool,
    rx_address: Option<u32>,
    panid: u16,
    short_address: u16,
    extended_address: [u8; 8],
    ack_timeout: u16,
}

impl Default for Hw {
    fn default() -> Self {
        Self {
            calls: Vec::new(),
            events: Ieee802154EventMask::NONE,
            rx_abort: Ieee802154RxAbortReasonObservation::Unclassified,
            tx_abort: Ieee802154TxAbortReasonObservation::Unclassified,
            tx_auto_ack: false,
            rx_auto_ack: false,
            tx_enhanced_ack: false,
            pending_mode: false,
            frequency_code: 0,
            etm_enabled: [false; 2],
            current_rx_frame: false,
            ed_rss: 0,
            cca_busy: false,
            rx_address: None,
            panid: 0,
            short_address: 0,
            extended_address: [0; 8],
            ack_timeout: 0,
        }
    }
}

fn etm_index(channel: Ieee802154EtmChannel) -> usize {
    match channel {
        Ieee802154EtmChannel::Channel0 => 0,
        Ieee802154EtmChannel::Channel1 => 1,
    }
}

impl Ieee802154LowLevel for Hw {
    fn set_command(&mut self, command: Ieee802154LlCommand) {
        self.calls.push(Call::Command(command));
    }
    fn events(&mut self) -> Ieee802154EventObservation {
        self.calls.push(Call::Events);
        Ieee802154EventObservation::from_named(self.events)
    }
    fn clear_events(&mut self, mask: Ieee802154EventMask) {
        self.calls.push(Call::ClearEvents(mask));
        self.events = self.events.difference(mask);
    }
    fn rx_abort_reason(&mut self) -> Ieee802154RxAbortReasonObservation {
        self.calls.push(Call::RxAbortReason);
        self.rx_abort
    }
    fn tx_abort_reason(&mut self) -> Ieee802154TxAbortReasonObservation {
        self.calls.push(Call::TxAbortReason);
        self.tx_abort
    }
    fn rx_status(&mut self) -> Ieee802154RxStatus {
        self.calls.push(Call::RxStatus);
        Ieee802154RxStatus::new(
            0,
            self.rx_abort,
            Ieee802154RxStateCode::new(0).unwrap(),
            false,
            false,
            false,
        )
    }
    fn is_current_rx_frame(&mut self) -> bool {
        self.calls.push(Call::IsCurrentRxFrame);
        self.current_rx_frame
    }
    fn set_tx_address(&mut self, address: u32) {
        self.calls.push(Call::TxAddress(address));
    }
    fn set_rx_address(&mut self, address: u32) {
        self.calls.push(Call::RxAddress(address));
        self.rx_address = Some(address);
    }
    fn tx_auto_ack(&mut self) -> bool {
        self.calls.push(Call::TxAutoAck);
        self.tx_auto_ack
    }
    fn rx_auto_ack(&mut self) -> bool {
        self.calls.push(Call::RxAutoAck);
        self.rx_auto_ack
    }
    fn tx_enhanced_ack(&mut self) -> bool {
        self.calls.push(Call::TxEnhancedAck);
        self.tx_enhanced_ack
    }
    fn pending_mode(&mut self) -> bool {
        self.calls.push(Call::PendingMode);
        self.pending_mode
    }
    fn set_pending_bit(&mut self, pending: bool) {
        self.calls.push(Call::SetPendingBit(pending));
    }
    fn frequency_code(&mut self) -> u8 {
        self.calls.push(Call::FrequencyCode);
        self.frequency_code
    }
    fn ed_rss(&mut self) -> i8 {
        self.calls.push(Call::EdRss);
        self.ed_rss
    }
    fn cca_busy(&mut self) -> bool {
        self.calls.push(Call::CcaBusy);
        self.cca_busy
    }
    fn set_ed_duration(&mut self, symbols: u16) {
        self.calls.push(Call::EdDuration(symbols));
    }
    fn notify_enhanced_ack_generated(&mut self) {
        self.calls.push(Call::EnhancedAckNotify);
    }
    fn disable_rx_aborts(&mut self, set: Ieee802154RxAbortEnableSet) {
        self.calls.push(Call::DisableRxAborts(set));
    }
    fn enable_all_events(&mut self) {
        self.calls.push(Call::EnableAllEvents);
    }
    fn enable_event(&mut self, event: Ieee802154Event) {
        self.calls.push(Call::EnableEvent(event));
    }
    fn disable_event(&mut self, event: Ieee802154Event) {
        self.calls.push(Call::DisableEvent(event));
    }
    fn enable_tx_aborts(&mut self, set: Ieee802154TxAbortEnableSet) {
        self.calls.push(Call::EnableTxAborts(set));
    }
    fn enable_rx_aborts(&mut self, set: Ieee802154RxAbortEnableSet) {
        self.calls.push(Call::EnableRxAborts(set));
    }
    fn set_ed_sample_mode(&mut self, mode: Ieee802154EdSampleMode) {
        self.calls.push(Call::EdSampleMode(mode));
    }
    fn disable_coex(&mut self) {
        self.calls.push(Call::DisableCoex);
    }
    fn set_channel(&mut self, channel: Ieee802154Channel) {
        self.calls.push(Call::SetChannel(channel.number()));
        self.frequency_code = channel.frequency_code().value();
    }
    fn set_tx_power(&mut self, power: &Ieee802154ResolvedTxPower<'_>) {
        self.calls
            .push(Call::SetTxPower(power.selected_provider_index()));
    }
    fn set_cca_mode(&mut self, mode: Ieee802154CcaMode) {
        self.calls.push(Call::SetCcaMode(mode));
    }
    fn set_cca_threshold(&mut self, threshold_dbm: i8) {
        self.calls.push(Call::SetCcaThreshold(threshold_dbm));
    }
    fn set_tx_auto_ack(&mut self, enable: bool) {
        self.calls.push(Call::SetTxAutoAck(enable));
        self.tx_auto_ack = enable;
    }
    fn set_rx_auto_ack(&mut self, enable: bool) {
        self.calls.push(Call::SetRxAutoAck(enable));
        self.rx_auto_ack = enable;
    }
    fn set_tx_enhanced_ack(&mut self, enable: bool) {
        self.calls.push(Call::SetTxEnhancedAck(enable));
        self.tx_enhanced_ack = enable;
    }
    fn set_coordinator(&mut self, enable: bool) {
        self.calls.push(Call::SetCoordinator(enable));
    }
    fn set_promiscuous(&mut self, enable: bool) {
        self.calls.push(Call::SetPromiscuous(enable));
    }
    fn set_pending_mode(&mut self, enhanced: bool) {
        self.calls.push(Call::SetPendingMode(enhanced));
        self.pending_mode = enhanced;
    }
    fn set_transmit_security(&mut self, enable: bool) {
        self.calls.push(Call::SetTransmitSecurity(enable));
    }
    fn set_timer_threshold(&mut self, timer: Ieee802154Timer, microseconds: u32) {
        self.calls.push(Call::Threshold(timer, microseconds));
    }
    fn start_timer(&mut self, timer: Ieee802154Timer) {
        self.calls.push(Call::StartTimer(timer));
    }
    fn stop_timer(&mut self, timer: Ieee802154Timer) {
        self.calls.push(Call::StopTimer(timer));
    }
    fn etm_channel_enabled(&mut self, channel: Ieee802154EtmChannel) -> bool {
        self.calls.push(Call::EtmEnabled(channel));
        self.etm_enabled[etm_index(channel)]
    }
    fn disable_etm_channel(&mut self, channel: Ieee802154EtmChannel) {
        self.calls.push(Call::DisableEtm(channel));
        self.etm_enabled[etm_index(channel)] = false;
    }
    fn enable_etm_channel(&mut self, channel: Ieee802154EtmChannel) {
        self.calls.push(Call::EnableEtm(channel));
        self.etm_enabled[etm_index(channel)] = true;
    }
    fn set_etm_route(&mut self, route: Ieee802154EtmRoute) {
        self.calls.push(Call::EtmRoute(route));
    }
    fn set_multipan_panid(&mut self, index: Ieee802154MultipanIndex, panid: u16) {
        assert_eq!(index, Ieee802154MultipanIndex::CONTEXT0);
        self.calls.push(Call::SetPanId(panid));
        self.panid = panid;
    }
    fn multipan_panid(&mut self, _: Ieee802154MultipanIndex) -> u16 {
        self.panid
    }
    fn set_multipan_short_address(&mut self, index: Ieee802154MultipanIndex, address: u16) {
        assert_eq!(index, Ieee802154MultipanIndex::CONTEXT0);
        self.calls.push(Call::SetShortAddress(address));
        self.short_address = address;
    }
    fn multipan_short_address(&mut self, _: Ieee802154MultipanIndex) -> u16 {
        self.short_address
    }
    fn set_multipan_extended_address(&mut self, index: Ieee802154MultipanIndex, address: [u8; 8]) {
        assert_eq!(index, Ieee802154MultipanIndex::CONTEXT0);
        self.calls.push(Call::SetExtendedAddress(address));
        self.extended_address = address;
    }
    fn multipan_extended_address(&mut self, _: Ieee802154MultipanIndex) -> [u8; 8] {
        self.extended_address
    }
    fn set_ack_timeout(&mut self, units: u16) {
        self.calls.push(Call::SetAckTimeout(units));
        self.ack_timeout = units;
    }
    fn ack_timeout(&mut self) -> u16 {
        self.ack_timeout
    }
    fn set_security_address(&mut self, address: &[u8; 8]) {
        self.calls.push(Call::SecurityAddress(*address));
    }
    fn set_security_key(&mut self, key: &[u8; 16]) {
        self.calls.push(Call::SecurityKey(*key));
    }
    fn set_security_offset(&mut self, offset: u8) {
        self.calls.push(Call::SecurityOffset(offset));
    }
}

/// Recorded upper-layer notification; frames are recorded as their PSDU.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Note {
    ReceiveDone {
        slot: usize,
        psdu: Vec<u8>,
        info: Ieee802154FrameInfo,
    },
    ReceiveSfd,
    TransmitDone {
        psdu: Vec<u8>,
        ack: Option<(usize, Vec<u8>, Ieee802154FrameInfo)>,
    },
    TransmitFailed(Vec<u8>, Ieee802154TxError),
    TransmitSfd,
    EnergyDetect(i8),
    Cca(bool),
    EdFailed,
    ReceiveAtDone,
}

fn psdu(frame: &[u8; FRAME_SIZE]) -> Vec<u8> {
    frame[1..=usize::from(frame[0] & 0x7f)].to_vec()
}

#[derive(Default)]
struct Env {
    now: u64,
    notes: Vec<Note>,
    enhanced_ack: Option<Vec<u8>>,
}

impl Ieee802154Environment for Env {
    fn now_micros(&mut self) -> u64 {
        self.now
    }
    fn receive_done(
        &mut self,
        slot: Ieee802154RxSlot,
        frame: &[u8; FRAME_SIZE],
        info: &Ieee802154FrameInfo,
    ) {
        self.notes.push(Note::ReceiveDone {
            slot: slot.index(),
            psdu: psdu(frame),
            info: *info,
        });
    }
    fn receive_sfd_done(&mut self) {
        self.notes.push(Note::ReceiveSfd);
    }
    fn transmit_done(&mut self, frame: &[u8; FRAME_SIZE], ack: Option<Ieee802154ReceivedAck<'_>>) {
        self.notes.push(Note::TransmitDone {
            psdu: psdu(frame),
            ack: ack.map(|ack| (ack.slot.index(), psdu(ack.frame), *ack.info)),
        });
    }
    fn transmit_failed(&mut self, frame: &[u8; FRAME_SIZE], error: Ieee802154TxError) {
        self.notes.push(Note::TransmitFailed(psdu(frame), error));
    }
    fn transmit_sfd_done(&mut self, _frame: &[u8; FRAME_SIZE]) {
        self.notes.push(Note::TransmitSfd);
    }
    fn energy_detect_done(&mut self, power: i8) {
        self.notes.push(Note::EnergyDetect(power));
    }
    fn cca_done(&mut self, busy: bool) {
        self.notes.push(Note::Cca(busy));
    }
    fn ed_failed(&mut self, _status: Ieee802154RxStatus) {
        self.notes.push(Note::EdFailed);
    }
    fn receive_at_done(&mut self) {
        self.notes.push(Note::ReceiveAtDone);
    }
    fn generate_enhanced_ack(
        &mut self,
        _frame: &[u8; FRAME_SIZE],
        _info: &Ieee802154FrameInfo,
        ack: &mut [u8; FRAME_SIZE],
    ) -> bool {
        match &self.enhanced_ack {
            Some(image) => {
                ack[..image.len()].copy_from_slice(image);
                true
            }
            None => false,
        }
    }
}

/// 2006 data frame requesting an ACK, short addresses.
const DATA_WITH_ACK: [u8; 13] = [
    0x0c, 0x61, 0x98, 0x01, 0x34, 0x12, 0xff, 0xff, 0x78, 0x56, 0xaa, 0x00, 0x00,
];
/// 2006 data frame without an ACK request.
const DATA_NO_ACK: [u8; 13] = [
    0x0c, 0x41, 0x98, 0x02, 0x34, 0x12, 0xff, 0xff, 0x78, 0x56, 0xbb, 0x00, 0x00,
];

fn mask(events: &[Ieee802154Event]) -> Ieee802154EventMask {
    super::event_mask(events)
}

struct Bench {
    engine: Ieee802154Engine<'static>,
    hw: Hw,
    env: Env,
}

impl Bench {
    /// `esp_ieee802154_enable` up to the end of `ieee802154_mac_init`.
    fn enabled() -> Self {
        let buffers = Box::leak(Box::new(Ieee802154EngineBuffers::new()));
        let levels = Ieee802154TxPowerLevels::new(&LEVELS).unwrap();
        let mut engine = Ieee802154Engine::new(buffers, levels, Ieee802154PibDefaults::default());
        let mut hw = Hw::default();
        engine.enable();
        engine.mac_init(&mut hw, Ieee802154PibDefaults::default());
        hw.calls.clear();
        Self {
            engine,
            hw,
            env: Env::default(),
        }
    }

    fn rx_address(&self, slot: usize) -> u32 {
        self.engine.buffers.rx[slot].address()
    }

    fn tx_address(&self) -> u32 {
        self.engine.buffers.tx.address()
    }

    fn receive(&mut self) {
        self.engine.receive(&mut self.hw, &mut self.env);
    }

    fn transmit(&mut self, frame: &[u8], cca: bool) {
        self.engine
            .transmit(&mut self.hw, &mut self.env, frame, cca)
            .unwrap();
    }

    fn deliver(&mut self, image: &[u8]) {
        let address = self.hw.rx_address.expect("an RX buffer is published");
        assert!(self.engine.model_dma_write(address, image));
    }

    fn interrupt(&mut self, events: &[Ieee802154Event]) {
        self.hw.events = mask(events);
        self.hw.calls.clear();
        self.engine.isr(&mut self.hw, &mut self.env);
    }

    fn take_notes(&mut self) -> Vec<Note> {
        core::mem::take(&mut self.env.notes)
    }
}

/// The records in `expected` appear in `calls` in this order.
fn assert_subsequence(calls: &[Call], expected: &[Call]) {
    let mut remaining = calls.iter();
    for call in expected {
        assert!(
            remaining.any(|recorded| recorded == call),
            "missing or out of order: {call:?}\ncalls: {calls:#?}"
        );
    }
}

/// `ieee802154_mac_init` (esp_ieee802154_dev.c L909-L930).
#[test]
fn mac_init_publishes_the_register_baseline_and_idles() {
    let buffers = Box::leak(Box::new(Ieee802154EngineBuffers::new()));
    let levels = Ieee802154TxPowerLevels::new(&LEVELS).unwrap();
    let mut engine = Ieee802154Engine::new(buffers, levels, Ieee802154PibDefaults::default());
    assert_eq!(engine.state(), Ieee802154State::Disable);
    let mut hw = Hw::default();
    engine.mac_init(&mut hw, Ieee802154PibDefaults::default());
    assert_eq!(
        hw.calls,
        [
            Call::EnableAllEvents,
            Call::DisableEvent(Ieee802154Event::Timer0Overflow),
            Call::EnableTxAborts(Ieee802154TxAbortEnableSet::RuntimeBaseline),
            Call::EnableRxAborts(Ieee802154RxAbortEnableSet::RuntimeBaseline),
            Call::EdSampleMode(Ieee802154EdSampleMode::Average),
            Call::DisableCoex,
        ]
    );
    assert_eq!(engine.state(), Ieee802154State::Idle);
}

/// `tx_init` stops the idle MAC, publishes the PIB, the frame and the ACK
/// buffer; `TX_DONE` arms the 200 ms ACK timer and `ACK_RX_DONE` reports the
/// ACK and returns to sleep (L992-L1026, L507-L536, L595-L602, L604-L640).
#[test]
fn transmit_with_ack_reports_the_received_ack() {
    let mut bench = Bench::enabled();
    bench.env.now = 1_000;
    bench.transmit(&DATA_WITH_ACK, false);
    let (tx, rx0) = (bench.tx_address(), bench.rx_address(0));
    assert_subsequence(
        &bench.hw.calls,
        &[
            Call::EtmEnabled(Ieee802154EtmChannel::Channel0),
            Call::EtmEnabled(Ieee802154EtmChannel::Channel1),
            Call::StopTimer(Ieee802154Timer::Timer0),
            Call::StopTimer(Ieee802154Timer::Timer1),
            Call::Command(Ieee802154LlCommand::Stop),
            Call::SetChannel(11),
            Call::SetTxPower(3),
            Call::SetPendingMode(false),
            Call::TxAddress(tx),
            Call::RxAddress(rx0),
            Call::Command(Ieee802154LlCommand::TxStart),
        ],
    );
    assert_eq!(bench.engine.state(), Ieee802154State::Tx);

    bench.interrupt(&[Ieee802154Event::TxDone]);
    assert_subsequence(
        &bench.hw.calls,
        &[
            Call::Events,
            Call::RxAbortReason,
            Call::TxAbortReason,
            Call::ClearEvents(mask(&[Ieee802154Event::TxDone])),
            Call::SetTransmitSecurity(false),
            Call::StopTimer(Ieee802154Timer::Timer1),
            Call::RxAutoAck,
            Call::EnableEvent(Ieee802154Event::Timer0Overflow),
            Call::Threshold(Ieee802154Timer::Timer0, 200_000),
            Call::StartTimer(Ieee802154Timer::Timer0),
        ],
    );
    assert_eq!(bench.engine.state(), Ieee802154State::RxAck);
    assert_eq!(bench.take_notes(), []);

    bench.deliver(&[0x05, 0x02, 0x00, 0x01, 0xc4, 0xc8]);
    bench.interrupt(&[Ieee802154Event::AckRxDone]);
    assert_subsequence(
        &bench.hw.calls,
        &[
            Call::StopTimer(Ieee802154Timer::Timer0),
            Call::DisableEvent(Ieee802154Event::Timer0Overflow),
            Call::FrequencyCode,
            Call::Command(Ieee802154LlCommand::Stop),
        ],
    );
    assert_eq!(
        bench.take_notes(),
        [Note::TransmitDone {
            psdu: DATA_WITH_ACK[1..].to_vec(),
            ack: Some((
                0,
                vec![0x02, 0x00, 0x01, 0xc4, 0xc8],
                Ieee802154FrameInfo {
                    pending: false,
                    process: true,
                    channel: 11,
                    rssi: -60,
                    lqi: 200,
                    timestamp: 0,
                }
            )),
        }]
    );
    assert_eq!(bench.engine.state(), Ieee802154State::Sleep);
}

/// A timer-zero overflow in `RX_ACK` reports a missing ACK
/// (`ieee802154_rx_ack_timeout_callback`, L150-L156).
#[test]
fn the_ack_timer_reports_a_missing_ack() {
    let mut bench = Bench::enabled();
    bench.transmit(&DATA_WITH_ACK, false);
    bench.interrupt(&[Ieee802154Event::TxDone]);
    bench.interrupt(&[Ieee802154Event::Timer0Overflow]);
    assert_eq!(
        bench.take_notes(),
        [Note::TransmitFailed(
            DATA_WITH_ACK[1..].to_vec(),
            Ieee802154TxError::NoAck
        )]
    );
    assert_eq!(bench.engine.state(), Ieee802154State::Sleep);
}

/// A CRC abort is not reported and `next_operation` re-arms receive into the
/// same buffer (L604-L640, L488-L505).
#[test]
fn a_crc_abort_restarts_receive_into_the_same_buffer() {
    let mut bench = Bench::enabled();
    bench.engine.pib().set_rx_when_idle(true);
    bench.receive();
    bench.hw.rx_abort =
        Ieee802154RxAbortReasonObservation::Named(Ieee802154RxAbortReason::CrcError);
    bench.interrupt(&[Ieee802154Event::RxAbort]);
    let rx0 = bench.rx_address(0);
    assert_subsequence(
        &bench.hw.calls,
        &[
            Call::ClearEvents(mask(&[Ieee802154Event::RxAbort])),
            Call::RxStatus,
            Call::SetTransmitSecurity(false),
            Call::RxAddress(rx0),
            Call::Command(Ieee802154LlCommand::RxStart),
        ],
    );
    assert_eq!(bench.take_notes(), []);
    assert_eq!(bench.engine.state(), Ieee802154State::Rx);
}

/// `RX_DONE` of an ACK-requesting 2006 frame selects the pending bit and
/// enters `TX_ACK`; `ACK_TX_DONE` delivers it and receive moves on to the
/// next free buffer (L538-L593).
#[test]
fn an_auto_acked_frame_is_delivered_after_its_ack() {
    let mut bench = Bench::enabled();
    bench.engine.pib().set_rx_when_idle(true);
    bench.receive();
    let mut image = DATA_WITH_ACK;
    image[11] = 0xc9;
    image[12] = 0xb4;
    bench.deliver(&image);
    bench.interrupt(&[Ieee802154Event::RxDone]);
    assert_subsequence(
        &bench.hw.calls,
        &[
            Call::FrequencyCode,
            Call::TxAutoAck,
            Call::PendingMode,
            Call::SetPendingBit(true),
        ],
    );
    assert_eq!(bench.engine.state(), Ieee802154State::TxAck);
    assert_eq!(bench.take_notes(), []);

    bench.interrupt(&[Ieee802154Event::AckTxDone]);
    let rx1 = bench.rx_address(1);
    assert_subsequence(
        &bench.hw.calls,
        &[
            Call::SetTransmitSecurity(false),
            Call::RxAddress(rx1),
            Call::Command(Ieee802154LlCommand::RxStart),
        ],
    );
    assert_eq!(
        bench.take_notes(),
        [Note::ReceiveDone {
            slot: 0,
            psdu: image[1..].to_vec(),
            info: Ieee802154FrameInfo {
                pending: true,
                process: true,
                channel: 11,
                rssi: -55,
                lqi: 0xb4,
                timestamp: 0,
            },
        }]
    );
}

/// A 2015 frame gets a software enhanced ACK published as the TX frame
/// (L556-L573).
#[test]
fn a_2015_frame_is_answered_with_the_generated_enhanced_ack() {
    let mut bench = Bench::enabled();
    bench.engine.pib().set_rx_when_idle(true);
    bench.env.enhanced_ack = Some(vec![0x05, 0x02, 0x20, 0x01, 0, 0]);
    bench.receive();
    let mut image = DATA_WITH_ACK;
    image[2] = 0xa8;
    bench.deliver(&image);
    bench.interrupt(&[Ieee802154Event::RxDone]);
    let enhanced_ack = bench.engine.buffers.enhanced_ack.address();
    assert_subsequence(
        &bench.hw.calls,
        &[
            Call::TxEnhancedAck,
            Call::PendingMode,
            Call::TxAddress(enhanced_ack),
            Call::EnhancedAckNotify,
        ],
    );
    assert!(!bench.hw.calls.contains(&Call::SetPendingBit(true)));
    assert_eq!(bench.engine.state(), Ieee802154State::TxEnhAck);

    bench.env.enhanced_ack = None;
    bench.interrupt(&[Ieee802154Event::AckTxDone]);
    assert!(matches!(
        bench.take_notes().as_slice(),
        [Note::ReceiveDone { slot: 0, .. }]
    ));
}

/// Energy detection and standalone CCA share `ED_START`; `ED_DONE` reports
/// the RSS plus the S31 compensation 0, or the busy bit (L985-L990,
/// L761-L770, L1221-L1252).
#[test]
fn energy_detection_and_cca_report_their_sample() {
    let mut bench = Bench::enabled();
    bench.engine.energy_detect(&mut bench.hw, &mut bench.env, 8);
    assert_subsequence(
        &bench.hw.calls,
        &[
            Call::EnableEvent(Ieee802154Event::EdDone),
            Call::EdDuration(8),
            Call::Command(Ieee802154LlCommand::EdStart),
        ],
    );
    assert_eq!(bench.engine.state(), Ieee802154State::Ed);
    bench.hw.ed_rss = -70;
    bench.interrupt(&[Ieee802154Event::EdDone]);
    assert_eq!(bench.take_notes(), [Note::EnergyDetect(-70)]);

    bench.hw.calls.clear();
    bench.engine.cca(&mut bench.hw, &mut bench.env);
    assert_subsequence(
        &bench.hw.calls,
        &[
            Call::EdDuration(8),
            Call::Command(Ieee802154LlCommand::EdStart),
        ],
    );
    bench.hw.cca_busy = true;
    bench.interrupt(&[Ieee802154Event::EdDone]);
    assert_eq!(bench.take_notes(), [Note::Cca(true)]);
    assert_eq!(bench.engine.state(), Ieee802154State::Sleep);
}

/// An ED abort reports `esp_ieee802154_ed_failed` (L624-L628).
#[test]
fn an_ed_abort_reports_the_failure() {
    let mut bench = Bench::enabled();
    bench.engine.energy_detect(&mut bench.hw, &mut bench.env, 8);
    bench.hw.rx_abort = Ieee802154RxAbortReasonObservation::Named(Ieee802154RxAbortReason::EdAbort);
    bench.interrupt(&[Ieee802154Event::RxAbort]);
    assert_eq!(bench.take_notes(), [Note::EdFailed]);
    assert_eq!(bench.engine.state(), Ieee802154State::Sleep);
}

/// A transmission requested while an ACK is being sent fails at once
/// without touching the operation (`ieee802154_transmit`, L1029-L1046).
#[test]
fn a_transmission_during_an_ack_fails_immediately() {
    let mut bench = Bench::enabled();
    bench.receive();
    bench.deliver(&DATA_WITH_ACK);
    bench.interrupt(&[Ieee802154Event::RxDone]);
    assert_eq!(bench.engine.state(), Ieee802154State::TxAck);
    bench.hw.calls.clear();
    bench.transmit(&DATA_NO_ACK, true);
    assert_eq!(bench.hw.calls, [Call::SetTransmitSecurity(false)]);
    assert_eq!(
        bench.take_notes(),
        [Note::TransmitFailed(
            DATA_NO_ACK[1..].to_vec(),
            Ieee802154TxError::CcaBusy
        )]
    );
    assert_eq!(bench.engine.state(), Ieee802154State::TxAck);
}

/// A transmission that finds the previous one still running stops it and
/// reports the new frame, since `tx_init` repoints `s_tx_frame` before
/// `stop_current_operation` (L992-L997, L334-L357).
#[test]
fn restarting_a_transmission_reports_the_new_frame_as_aborted() {
    let mut bench = Bench::enabled();
    bench.transmit(&DATA_WITH_ACK, false);
    bench.transmit(&DATA_NO_ACK, false);
    assert_eq!(
        bench.take_notes(),
        [Note::TransmitFailed(
            DATA_NO_ACK[1..].to_vec(),
            Ieee802154TxError::Abort
        )]
    );
    assert_eq!(bench.engine.state(), Ieee802154State::Tx);
}

/// With every buffer held, reception lands in the stub buffer and is
/// dropped; a released slot is found again (L85-L96, L257-L287).
#[test]
fn a_full_ring_drops_frames_until_a_slot_is_released() {
    let mut bench = Bench::enabled();
    bench.engine.pib().set_rx_when_idle(true);
    bench.receive();
    for slot in 0..RX_BUFFER_COUNT {
        assert_eq!(bench.hw.rx_address, Some(bench.rx_address(slot)));
        bench.deliver(&DATA_NO_ACK);
        bench.interrupt(&[Ieee802154Event::RxDone]);
    }
    assert_eq!(bench.take_notes().len(), RX_BUFFER_COUNT);
    let stub = bench.rx_address(RX_BUFFER_COUNT);
    assert_eq!(bench.hw.rx_address, Some(stub));

    bench.deliver(&DATA_NO_ACK);
    bench.interrupt(&[Ieee802154Event::RxDone]);
    assert_eq!(bench.take_notes(), []);

    let slot = Ieee802154RxSlot(5);
    bench.engine.receive_handle_done(slot).unwrap();
    assert_eq!(
        bench
            .engine
            .receive_handle_done(Ieee802154RxSlot(RX_BUFFER_COUNT as u8)),
        Err(Ieee802154SlotError)
    );
    bench.deliver(&DATA_NO_ACK);
    bench.interrupt(&[Ieee802154Event::RxDone]);
    assert_eq!(bench.hw.rx_address, Some(bench.rx_address(5)));
}

/// `ieee802154_transmit_at` routes TIMER0 through ETM channel zero and
/// fires it the ramp-up time early (L1049-L1065).
#[test]
fn a_timed_transmission_starts_through_etm_channel_zero() {
    for (cca, route, rampup) in [
        (false, Ieee802154EtmRoute::Timer0ToTxStart, 98),
        (true, Ieee802154EtmRoute::Timer0ToCcaTx, 256),
    ] {
        let mut bench = Bench::enabled();
        bench.env.now = 1_000;
        bench
            .engine
            .transmit_at(&mut bench.hw, &mut bench.env, &DATA_NO_ACK, cca, 5_000)
            .unwrap();
        assert_subsequence(
            &bench.hw.calls,
            &[
                Call::EtmEnabled(Ieee802154EtmChannel::Channel0),
                Call::EtmRoute(route),
                Call::EnableEtm(Ieee802154EtmChannel::Channel0),
                Call::Threshold(Ieee802154Timer::Timer0, 4_000 - rampup),
                Call::StartTimer(Ieee802154Timer::Timer0),
            ],
        );
        assert!(
            !bench
                .hw
                .calls
                .contains(&Call::Command(Ieee802154LlCommand::TxStart))
        );
    }
}

/// `ieee802154_receive_at` with a duration: TIMER1 starts receive through
/// ETM channel one, then reprograms itself for the window end, which stops
/// receive when no frame is in flight (L1086-L1140).
#[test]
fn a_timed_receive_window_closes_without_a_frame() {
    let mut bench = Bench::enabled();
    bench.env.now = 1_000;
    bench
        .engine
        .receive_at(&mut bench.hw, &mut bench.env, 5_000, 2_000);
    let rx0 = bench.rx_address(0);
    assert_subsequence(
        &bench.hw.calls,
        &[
            Call::RxAddress(rx0),
            Call::EtmRoute(Ieee802154EtmRoute::Timer1ToRxStart),
            Call::EnableEtm(Ieee802154EtmChannel::Channel1),
            Call::Threshold(Ieee802154Timer::Timer1, 5_000 - 146 - 1_000),
            Call::StartTimer(Ieee802154Timer::Timer1),
        ],
    );
    assert_eq!(bench.engine.state(), Ieee802154State::Rx);

    bench.env.now = 4_854;
    bench.interrupt(&[Ieee802154Event::Timer1Overflow]);
    assert_eq!(
        bench.hw.calls[4..],
        [
            Call::Threshold(Ieee802154Timer::Timer1, 7_000 - 4_854),
            Call::StartTimer(Ieee802154Timer::Timer1),
        ]
    );

    bench.interrupt(&[Ieee802154Event::Timer1Overflow]);
    assert_subsequence(
        &bench.hw.calls,
        &[
            Call::IsCurrentRxFrame,
            Call::Command(Ieee802154LlCommand::Stop),
        ],
    );
    assert_eq!(bench.take_notes(), [Note::ReceiveAtDone]);
}

/// A window closing during a frame keeps receiving with every abort
/// enabled; the next operation restores the abort baseline and reports the
/// window end (L1068-L1080, L488-L495).
#[test]
fn a_timed_receive_window_closing_mid_frame_finishes_the_frame() {
    let mut bench = Bench::enabled();
    bench
        .engine
        .receive_at(&mut bench.hw, &mut bench.env, 5_000, 2_000);
    bench.interrupt(&[Ieee802154Event::Timer1Overflow]);
    bench.hw.current_rx_frame = true;
    bench.interrupt(&[Ieee802154Event::Timer1Overflow]);
    assert!(
        bench
            .hw
            .calls
            .contains(&Call::EnableRxAborts(Ieee802154RxAbortEnableSet::All))
    );
    assert_eq!(bench.take_notes(), []);

    bench.deliver(&DATA_NO_ACK);
    bench.interrupt(&[Ieee802154Event::RxDone]);
    assert_subsequence(
        &bench.hw.calls,
        &[
            Call::DisableRxAborts(Ieee802154RxAbortEnableSet::All),
            Call::EnableRxAborts(Ieee802154RxAbortEnableSet::RuntimeBaseline),
        ],
    );
    let notes = bench.take_notes();
    assert!(matches!(
        notes.as_slice(),
        [Note::ReceiveDone { .. }, Note::ReceiveAtDone]
    ));
}

/// An elapsed window is skipped without touching the MAC (L1094-L1101).
#[test]
fn an_elapsed_receive_window_is_skipped() {
    let mut bench = Bench::enabled();
    bench.env.now = 1_000;
    bench
        .engine
        .receive_at(&mut bench.hw, &mut bench.env, 100, 50);
    assert_eq!(bench.hw.calls, []);
    assert_eq!(bench.engine.state(), Ieee802154State::Idle);
}

/// Identity setters write interface zero; the ACK timeout rounds up to the
/// 16-microsecond unit (`esp_ieee802154.c` L173-L223).
#[test]
fn identity_and_ack_timeout_address_interface_zero() {
    let mut bench = Bench::enabled();
    let (engine, hw) = (&mut bench.engine, &mut bench.hw);
    engine.set_panid(hw, 0x1234);
    engine.set_short_address(hw, 0x5678);
    engine.set_extended_address(hw, [1, 2, 3, 4, 5, 6, 7, 8]);
    engine.set_ack_timeout(hw, 200);
    assert_eq!(
        hw.calls,
        [
            Call::SetPanId(0x1234),
            Call::SetShortAddress(0x5678),
            Call::SetExtendedAddress([1, 2, 3, 4, 5, 6, 7, 8]),
            Call::SetAckTimeout(13),
        ]
    );
    assert_eq!(engine.panid(hw), 0x1234);
    assert_eq!(engine.short_address(hw), 0x5678);
    assert_eq!(engine.extended_address(hw), [1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(engine.ack_timeout(hw), 208);
}

/// `ieee802154_transmit_security_config` programs address, key and the
/// secured payload offset, then enables transmit security; `TX_DONE` clears
/// it (esp_ieee802154_sec.c L12-L25).
#[test]
fn transmit_security_is_armed_for_one_transmission() {
    let mut bench = Bench::enabled();
    let mut secured = DATA_NO_ACK;
    secured[1] |= 0x08;
    secured[10] = 0x25;
    let key = [7; 16];
    let address = [9; 8];
    bench
        .engine
        .set_transmit_security(&mut bench.hw, &secured, &key, &address);
    assert_eq!(
        bench.hw.calls,
        [
            Call::SecurityAddress(address),
            Call::SecurityKey(key),
            Call::SecurityOffset(10),
            Call::SetTransmitSecurity(true),
        ]
    );
    bench.transmit(&secured, false);
    bench.interrupt(&[Ieee802154Event::TxDone]);
    assert!(bench.hw.calls.contains(&Call::SetTransmitSecurity(false)));
}
