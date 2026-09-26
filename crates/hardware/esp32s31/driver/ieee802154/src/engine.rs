//! Interrupt-driven IEEE 802.15.4 MAC engine ported from the public ESP-IDF
//! driver (`components/ieee802154/driver/esp_ieee802154_dev.c`).
//!
//! The engine keeps the vendor's private state machine, receive ring and
//! operation order. Every decision the vendor takes inside its interrupt
//! handler — ACK and pending-bit selection, the enhanced ACK, the ACK
//! watchdog and the next operation — is taken here in [`Ieee802154Engine::isr`].
//! Register accesses go through the HAL [`Ieee802154LowLevel`] backend and
//! upper-layer notifications through [`Ieee802154Environment`], so a
//! recording backend observes the vendor accessor sequence.
//!
//! The caller serializes every entry point in one critical section, as the
//! vendor does with `ieee802154_enter_critical`. The build matches the
//! vendor defaults: no software coexistence, no RF power gating, no
//! multi-PAN, no test mode and no statistics.

use oer_esp32s31_hal::ieee802154::{
    Ieee802154Channel, Ieee802154TxPowerLevels,
    ll::{
        self, Ieee802154EtmRoute, Ieee802154LlCommand, Ieee802154LowLevel,
        Ieee802154RxAbortEnableSet, Ieee802154RxStatus, Ieee802154Timer,
    },
    mac::{
        Ieee802154Event, Ieee802154EventMask, Ieee802154RxAbortReason,
        Ieee802154RxAbortReasonObservation, Ieee802154TxAbortReason,
        Ieee802154TxAbortReasonObservation,
    },
    pib::{Ieee802154MultipanIndex, Ieee802154Pib, Ieee802154PibDefaults},
};
use oer_ieee802154::{FrameVersion, PendingTable, PhrFrame, ack_pending};

mod buffers;

pub use buffers::{FRAME_SIZE, Ieee802154EngineBuffers, RX_BUFFER_COUNT};

use buffers::DmaFrame;

/// Pending-table entries per address kind (`CONFIG_IEEE802154_PENDING_TABLE_SIZE`).
pub const PENDING_TABLE_SIZE: usize = 20;

/// ESP32-S31 `IEEE802154_RSSI_COMPENSATION_VALUE`.
const RSSI_COMPENSATION: i8 = 0;
/// `CCA_DETECTION_TIME` in 16-microsecond symbols.
const CCA_DETECTION_TIME: u16 = 8;
/// ACK receive timeout started after an ACK-requesting transmission.
const ACK_TIMEOUT_MICROSECONDS: u32 = 200_000;
/// `IEEE802154_ED_TRIG_TX_RAMPUP_TIME_US`.
const ED_TRIG_TX_RAMPUP_MICROSECONDS: u32 = 256;
/// `IEEE802154_TX_RAMPUP_TIME_US`.
const TX_RAMPUP_MICROSECONDS: u32 = 98;
/// `IEEE802154_RX_RAMPUP_TIME_US`.
const RX_RAMPUP_MICROSECONDS: u32 = 146;
/// `IEEE802154_FRAME_MIN_LEN`.
const FRAME_MIN_LEN: u8 = 3;

/// The vendor private radio state (`ieee802154_state_t` without test mode).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Ieee802154State {
    /// Disabled.
    Disable,
    /// Idle.
    Idle,
    /// Sleeping.
    Sleep,
    /// Receiving.
    Rx,
    /// Transmitting a hardware ACK.
    TxAck,
    /// Transmitting a software enhanced ACK.
    TxEnhAck,
    /// Clear-channel assessment, then transmit.
    TxCca,
    /// Transmitting.
    Tx,
    /// Waiting for an ACK.
    RxAck,
    /// Energy detection.
    Ed,
    /// Standalone clear-channel assessment.
    Cca,
}

/// Transmit failure reported to the upper layer (`esp_ieee802154_tx_error_t`).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Ieee802154TxError {
    /// The channel is busy.
    CcaBusy,
    /// The transmission was aborted.
    Abort,
    /// No ACK arrived before the timeout.
    NoAck,
    /// The received ACK was invalid.
    InvalidAck,
    /// Coexistence rejected the transmission.
    Coexist,
    /// The security configuration is invalid.
    Security,
}

/// Receive metadata (`esp_ieee802154_frame_info_t` without multi-PAN).
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Ieee802154FrameInfo {
    /// The frame was acknowledged with the pending bit set.
    pub pending: bool,
    /// The upper layer holds the frame until `receive_handle_done`.
    pub process: bool,
    /// Receive channel, zero for a frame shorter than three bytes.
    pub channel: u8,
    /// RSSI in dBm.
    pub rssi: i8,
    /// Link quality indicator.
    pub lqi: u8,
    /// Microsecond timestamp of the SFD.
    pub timestamp: u64,
}

/// A receive-ring slot the upper layer holds until
/// [`Ieee802154Engine::receive_handle_done`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Ieee802154RxSlot(u8);

impl Ieee802154RxSlot {
    /// The ring index.
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// A received ACK reported with its transmit completion.
#[derive(Clone, Copy, Debug)]
pub struct Ieee802154ReceivedAck<'frame> {
    /// Ring slot holding the ACK.
    pub slot: Ieee802154RxSlot,
    /// The ACK image `[PHR, PSDU...]`.
    pub frame: &'frame [u8; FRAME_SIZE],
    /// ACK metadata.
    pub info: &'frame Ieee802154FrameInfo,
}

/// Upper-layer notifications and platform services, invoked inside the
/// engine's critical section (`esp_ieee802154_event.c` and `esp_timer`).
pub trait Ieee802154Environment {
    /// `esp_timer_get_time` in microseconds.
    fn now_micros(&mut self) -> u64;
    /// `esp_ieee802154_receive_done`.
    fn receive_done(
        &mut self,
        slot: Ieee802154RxSlot,
        frame: &[u8; FRAME_SIZE],
        info: &Ieee802154FrameInfo,
    );
    /// `esp_ieee802154_receive_sfd_done`.
    fn receive_sfd_done(&mut self);
    /// `esp_ieee802154_transmit_done`.
    fn transmit_done(&mut self, frame: &[u8; FRAME_SIZE], ack: Option<Ieee802154ReceivedAck<'_>>);
    /// `esp_ieee802154_transmit_failed`.
    fn transmit_failed(&mut self, frame: &[u8; FRAME_SIZE], error: Ieee802154TxError);
    /// `esp_ieee802154_transmit_sfd_done`.
    fn transmit_sfd_done(&mut self, frame: &[u8; FRAME_SIZE]);
    /// `esp_ieee802154_energy_detect_done` in dBm.
    fn energy_detect_done(&mut self, power: i8);
    /// `esp_ieee802154_cca_done`.
    fn cca_done(&mut self, busy: bool);
    /// `esp_ieee802154_ed_failed` with the receive status.
    fn ed_failed(&mut self, status: Ieee802154RxStatus);
    /// `esp_ieee802154_receive_at_done`.
    fn receive_at_done(&mut self);
    /// `esp_ieee802154_enh_ack_generator`: build the enhanced ACK for
    /// `frame` into `ack`, returning whether one was generated.
    fn generate_enhanced_ack(
        &mut self,
        frame: &[u8; FRAME_SIZE],
        info: &Ieee802154FrameInfo,
        ack: &mut [u8; FRAME_SIZE],
    ) -> bool;
}

/// A frame given to [`Ieee802154Engine::transmit`] is not a `[PHR, PSDU...]`
/// image that fits one DMA frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154FrameImageError;

/// A slot outside the receive ring (`ieee802154_receive_handle_done` failure).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154SlotError;

/// Which buffer `s_tx_frame` points to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TxSource {
    Frame,
    EnhancedAck,
}

/// A pending timer callback (`ieee802154_timer*_set_callback`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TimerAction {
    AckTimeout,
    StartReceiveAt { stop_time: u32 },
    FinishReceiveAt,
}

const STUB: u8 = RX_BUFFER_COUNT as u8;

fn event_mask(events: &[Ieee802154Event]) -> Ieee802154EventMask {
    events
        .iter()
        .fold(Ieee802154EventMask::NONE, |mask, event| {
            mask.union(event.mask())
        })
}

/// `IEEE802154_ASSERT`: a violated vendor invariant stops the radio.
macro_rules! vendor_assert {
    ($condition:expr) => {
        assert!(
            $condition,
            concat!("IEEE802154_ASSERT(", stringify!($condition), ")")
        )
    };
}

/// Hardware and environment borrowed for one engine entry.
struct Cx<'a, L: ?Sized, E: ?Sized> {
    ll: &'a mut L,
    env: &'a mut E,
}

/// The ported MAC engine.
pub struct Ieee802154Engine<'storage> {
    buffers: &'storage mut Ieee802154EngineBuffers,
    levels: Ieee802154TxPowerLevels<'storage>,
    pib: Ieee802154Pib,
    pending_table: PendingTable<PENDING_TABLE_SIZE>,
    state: Ieee802154State,
    tx: TxSource,
    rx_info: [Ieee802154FrameInfo; RX_BUFFER_COUNT + 1],
    rx_index: u8,
    recent_rx_info_index: u8,
    needs_next_operation: bool,
    pending_rx_stop: bool,
    timer0: Option<TimerAction>,
    timer1: Option<TimerAction>,
}

impl<'storage> Ieee802154Engine<'storage> {
    /// A disabled engine over exclusively borrowed buffers. `levels` is the
    /// transmit-power provider the vendor reads through
    /// `bt_bb_get_tx_pwr_table`.
    pub fn new(
        buffers: &'storage mut Ieee802154EngineBuffers,
        levels: Ieee802154TxPowerLevels<'storage>,
        defaults: Ieee802154PibDefaults,
    ) -> Self {
        Self {
            buffers,
            levels,
            pib: Ieee802154Pib::new(defaults, levels),
            pending_table: PendingTable::new(),
            state: Ieee802154State::Disable,
            tx: TxSource::Frame,
            rx_info: [Ieee802154FrameInfo::default(); RX_BUFFER_COUNT + 1],
            rx_index: 0,
            recent_rx_info_index: 0,
            needs_next_operation: false,
            pending_rx_stop: false,
            timer0: None,
            timer1: None,
        }
    }

    /// `ieee802154_get_state`.
    pub const fn state(&self) -> Ieee802154State {
        self.state
    }

    /// The PAN information base; changes are published before the next
    /// operation.
    pub fn pib(&mut self) -> &mut Ieee802154Pib {
        &mut self.pib
    }

    /// The frame-pending table of interface zero.
    pub fn pending_table(&mut self) -> &mut PendingTable<PENDING_TABLE_SIZE> {
        &mut self.pending_table
    }

    /// `ieee802154_get_recent_lqi`.
    pub fn recent_lqi(&self) -> u8 {
        self.rx_info[usize::from(self.recent_rx_info_index)].lqi
    }

    /// The image and metadata of a slot the upper layer holds.
    pub fn rx_frame(&self, slot: Ieee802154RxSlot) -> ([u8; FRAME_SIZE], Ieee802154FrameInfo) {
        (
            self.buffers.rx[slot.index()].read(),
            self.rx_info[slot.index()],
        )
    }

    /// Model a DMA write into the buffer published at `address`; host models
    /// have no MAC to write received frames.
    #[cfg(not(target_arch = "riscv32"))]
    pub fn model_dma_write(&mut self, address: u32, image: &[u8]) -> bool {
        match self.buffers.frame_at(address) {
            Some(frame) => {
                frame.write(image);
                true
            }
            None => false,
        }
    }

    /// The receive slot whose buffer is published at `address`, for host
    /// models that release the frame they delivered.
    #[cfg(not(target_arch = "riscv32"))]
    pub fn model_rx_slot(&self, address: u32) -> Option<Ieee802154RxSlot> {
        self.buffers.rx[..RX_BUFFER_COUNT]
            .iter()
            .position(|frame| frame.address() == address)
            .map(|index| Ieee802154RxSlot(index as u8))
    }

    /// The address the engine publishes for its transmit buffer, for host
    /// models that label DMA addresses.
    #[cfg(not(target_arch = "riscv32"))]
    pub fn model_transmit_address(&self) -> u32 {
        self.buffers.tx.address()
    }

    /// `ieee802154_enable` after the modem clock is enabled.
    pub fn enable(&mut self) {
        self.state = Ieee802154State::Idle;
    }

    /// `ieee802154_disable` after the modem clock is disabled.
    pub fn disable(&mut self) {
        self.state = Ieee802154State::Disable;
    }

    /// `ieee802154_mac_init` after the MAC reset: reinitialize the PIB,
    /// publish the register baseline and clear the receive ring. The caller
    /// then applies the TX-on delay and routes the interrupt.
    pub fn mac_init<L: Ieee802154LowLevel + ?Sized>(
        &mut self,
        ll: &mut L,
        defaults: Ieee802154PibDefaults,
    ) {
        self.pib = Ieee802154Pib::new(defaults, self.levels);
        ll::mac_init_registers(ll);
        self.rx_buffer_clear();
        self.state = Ieee802154State::Idle;
    }

    fn rx_buffer_clear(&mut self) {
        for frame in &self.buffers.rx {
            frame.write(&[]);
        }
        self.rx_info = [Ieee802154FrameInfo::default(); RX_BUFFER_COUNT + 1];
        self.rx_index = 0;
        self.recent_rx_info_index = 0;
        self.needs_next_operation = false;
        self.pending_rx_stop = false;
    }

    /// `ieee802154_receive_handle_done`: release a slot to the ring.
    pub fn receive_handle_done(
        &mut self,
        slot: Ieee802154RxSlot,
    ) -> Result<(), Ieee802154SlotError> {
        if slot.index() >= RX_BUFFER_COUNT {
            return Err(Ieee802154SlotError);
        }
        self.rx_info[slot.index()].process = false;
        Ok(())
    }

    /// `ieee802154_transmit`: `frame` is a `[PHR, PSDU...]` image. A frame
    /// arriving while a received frame or its ACK is in flight fails at
    /// once.
    pub fn transmit<L, E>(
        &mut self,
        ll: &mut L,
        env: &mut E,
        frame: &[u8],
        cca: bool,
    ) -> Result<(), Ieee802154FrameImageError>
    where
        L: Ieee802154LowLevel + ?Sized,
        E: Ieee802154Environment + ?Sized,
    {
        if frame.is_empty() || frame.len() > FRAME_SIZE {
            return Err(Ieee802154FrameImageError);
        }
        let mut cx = Cx { ll, env };
        if (self.state == Ieee802154State::Rx && cx.ll.is_current_rx_frame())
            || self.state == Ieee802154State::TxAck
            || self.state == Ieee802154State::TxEnhAck
        {
            let error = if cca {
                Ieee802154TxError::CcaBusy
            } else {
                Ieee802154TxError::Abort
            };
            let mut image = [0; FRAME_SIZE];
            image[..frame.len()].copy_from_slice(frame);
            cx.env.transmit_failed(&image, error);
            ll::sec_clear(cx.ll);
            return Ok(());
        }
        self.tx_init(&mut cx, frame);
        if cca {
            cx.ll.set_ed_duration(CCA_DETECTION_TIME);
            cx.ll.set_command(Ieee802154LlCommand::CcaTxStart);
            self.state = Ieee802154State::TxCca;
        } else {
            cx.ll.set_command(Ieee802154LlCommand::TxStart);
            self.state = Ieee802154State::Tx;
        }
        Ok(())
    }

    /// `ieee802154_transmit_at`: start the transmission when TIMER0 reaches
    /// `time` minus the ramp-up, through ETM channel zero.
    pub fn transmit_at<L, E>(
        &mut self,
        ll: &mut L,
        env: &mut E,
        frame: &[u8],
        cca: bool,
        time: u32,
    ) -> Result<(), Ieee802154FrameImageError>
    where
        L: Ieee802154LowLevel + ?Sized,
        E: Ieee802154Environment + ?Sized,
    {
        if frame.is_empty() || frame.len() > FRAME_SIZE {
            return Err(Ieee802154FrameImageError);
        }
        let mut cx = Cx { ll, env };
        self.tx_init(&mut cx, frame);
        if cca {
            cx.ll.set_ed_duration(CCA_DETECTION_TIME);
        }
        self.state = if cca {
            Ieee802154State::TxCca
        } else {
            Ieee802154State::Tx
        };
        let (route, rampup) = if cca {
            (
                Ieee802154EtmRoute::Timer0ToCcaTx,
                ED_TRIG_TX_RAMPUP_MICROSECONDS,
            )
        } else {
            (Ieee802154EtmRoute::Timer0ToTxStart, TX_RAMPUP_MICROSECONDS)
        };
        ll::etm_set_event_task(cx.ll, route);
        self.timer_fire_at(&mut cx, Ieee802154Timer::Timer0, time.wrapping_sub(rampup));
        Ok(())
    }

    /// `ieee802154_receive`: keep an ongoing reception unless the PIB changed.
    pub fn receive<L, E>(&mut self, ll: &mut L, env: &mut E)
    where
        L: Ieee802154LowLevel + ?Sized,
        E: Ieee802154Environment + ?Sized,
    {
        if matches!(self.state, Ieee802154State::Rx | Ieee802154State::TxAck)
            && !self.pib.is_pending()
        {
            return;
        }
        let mut cx = Cx { ll, env };
        self.rx_init(&mut cx);
        self.enable_rx(&mut cx);
    }

    /// `ieee802154_receive_at`: receive from `time`, for `duration`
    /// microseconds when nonzero, starting through ETM channel one.
    pub fn receive_at<L, E>(&mut self, ll: &mut L, env: &mut E, time: u32, duration: u32)
    where
        L: Ieee802154LowLevel + ?Sized,
        E: Ieee802154Environment + ?Sized,
    {
        let mut cx = Cx { ll, env };
        if duration != 0 {
            let now = cx.env.now_micros() as u32;
            if ll::target_time_expired(time.wrapping_add(duration), now) {
                return;
            }
        }
        self.rx_init(&mut cx);
        self.set_next_rx_buffer(&mut cx);
        self.state = Ieee802154State::Rx;
        ll::etm_set_event_task(cx.ll, Ieee802154EtmRoute::Timer1ToRxStart);
        if duration != 0 {
            self.timer1 = Some(TimerAction::StartReceiveAt {
                stop_time: time.wrapping_add(duration),
            });
        }
        self.timer_fire_at(
            &mut cx,
            Ieee802154Timer::Timer1,
            time.wrapping_sub(RX_RAMPUP_MICROSECONDS),
        );
    }

    /// `ieee802154_sleep`.
    pub fn sleep<L, E>(&mut self, ll: &mut L, env: &mut E)
    where
        L: Ieee802154LowLevel + ?Sized,
        E: Ieee802154Environment + ?Sized,
    {
        let mut cx = Cx { ll, env };
        self.enter_sleep(&mut cx);
    }

    /// `ieee802154_energy_detect` for `duration` 16-microsecond symbols.
    pub fn energy_detect<L, E>(&mut self, ll: &mut L, env: &mut E, duration: u16)
    where
        L: Ieee802154LowLevel + ?Sized,
        E: Ieee802154Environment + ?Sized,
    {
        let mut cx = Cx { ll, env };
        self.stop_current_operation(&mut cx);
        self.pib.update(cx.ll, self.levels);
        Self::start_ed(&mut cx, duration);
        self.state = Ieee802154State::Ed;
    }

    /// `ieee802154_cca`.
    pub fn cca<L, E>(&mut self, ll: &mut L, env: &mut E)
    where
        L: Ieee802154LowLevel + ?Sized,
        E: Ieee802154Environment + ?Sized,
    {
        let mut cx = Cx { ll, env };
        self.stop_current_operation(&mut cx);
        self.pib.update(cx.ll, self.levels);
        Self::start_ed(&mut cx, CCA_DETECTION_TIME);
        self.state = Ieee802154State::Cca;
    }

    /// `ieee802154_isr`: sample and clear the events, handle them in the
    /// vendor order and start the next operation.
    pub fn isr<L, E>(&mut self, ll: &mut L, env: &mut E)
    where
        L: Ieee802154LowLevel + ?Sized,
        E: Ieee802154Environment + ?Sized,
    {
        let mut cx = Cx { ll, env };
        let observation = cx.ll.events();
        let rx_abort_reason = cx.ll.rx_abort_reason();
        let tx_abort_reason = cx.ll.tx_abort_reason();
        let Ok(mut events) = observation.classification() else {
            panic!("IEEE802154_ASSERT(events == 0): unclassified MAC event");
        };
        cx.ll.clear_events(events);

        if events.contains(Ieee802154Event::RxAbort) {
            self.isr_rx_phase_rx_abort(&mut cx, rx_abort_reason);
        }
        if events.contains(Ieee802154Event::RxSfdDone) {
            vendor_assert!(matches!(
                self.state,
                Ieee802154State::Rx
                    | Ieee802154State::RxAck
                    | Ieee802154State::Tx
                    | Ieee802154State::TxCca
                    | Ieee802154State::TxEnhAck
            ));
            self.rx_info[usize::from(self.rx_index)].timestamp = cx.env.now_micros();
            cx.env.receive_sfd_done();
            events = events.difference(Ieee802154Event::RxSfdDone.mask());
        }
        if events.contains(Ieee802154Event::TxSfdDone) {
            vendor_assert!(matches!(
                self.state,
                Ieee802154State::Tx
                    | Ieee802154State::TxCca
                    | Ieee802154State::TxEnhAck
                    | Ieee802154State::TxAck
            ));
            let frame = self.tx_frame().read();
            cx.env.transmit_sfd_done(&frame);
            events = events.difference(Ieee802154Event::TxSfdDone.mask());
        }
        if events.contains(Ieee802154Event::TxDone) {
            vendor_assert!(matches!(
                self.state,
                Ieee802154State::Tx | Ieee802154State::TxCca
            ));
            self.isr_tx_done(&mut cx);
            events = events.difference(Ieee802154Event::TxDone.mask());
        }
        if events.contains(Ieee802154Event::RxDone) {
            vendor_assert!(self.state == Ieee802154State::Rx);
            self.isr_rx_done(&mut cx);
            events = events.difference(Ieee802154Event::RxDone.mask());
        }
        if events.contains(Ieee802154Event::AckTxDone) {
            vendor_assert!(matches!(
                self.state,
                Ieee802154State::TxAck | Ieee802154State::Rx | Ieee802154State::TxEnhAck
            ));
            ll::sec_clear(cx.ll);
            self.receive_done(&mut cx);
            self.needs_next_operation = true;
            events = events.difference(Ieee802154Event::AckTxDone.mask());
        }
        if events.contains(Ieee802154Event::AckRxDone) {
            vendor_assert!(matches!(
                self.state,
                Ieee802154State::RxAck
                    | Ieee802154State::Tx
                    | Ieee802154State::TxCca
                    | Ieee802154State::TxEnhAck
            ));
            self.timer_stop(&mut cx, Ieee802154Timer::Timer0);
            cx.ll.disable_event(Ieee802154Event::Timer0Overflow);
            self.rx_frame_info_update(&mut cx);
            self.transmit_done(&mut cx, true);
            self.needs_next_operation = true;
            events = events.difference(Ieee802154Event::AckRxDone.mask());
        }
        if events.contains(Ieee802154Event::RxAbort) {
            self.isr_tx_ack_phase_rx_abort(&mut cx, rx_abort_reason);
            events = events.difference(Ieee802154Event::RxAbort.mask());
        }
        if events.contains(Ieee802154Event::TxAbort) {
            self.isr_tx_abort(&mut cx, tx_abort_reason);
            events = events.difference(Ieee802154Event::TxAbort.mask());
        }
        if events.contains(Ieee802154Event::EdDone) {
            vendor_assert!(matches!(
                self.state,
                Ieee802154State::Ed | Ieee802154State::Cca
            ));
            if self.state == Ieee802154State::Cca {
                let busy = cx.ll.cca_busy();
                cx.env.cca_done(busy);
            } else if self.state == Ieee802154State::Ed {
                let power = cx.ll.ed_rss().wrapping_add(RSSI_COMPENSATION);
                cx.env.energy_detect_done(power);
            }
            self.needs_next_operation = true;
            events = events.difference(Ieee802154Event::EdDone.mask());
        }
        if events.contains(Ieee802154Event::Timer0Overflow) {
            vendor_assert!(self.state == Ieee802154State::RxAck);
            if let Some(action) = self.timer0.take() {
                self.run_timer_action(&mut cx, action);
            }
            events = events.difference(Ieee802154Event::Timer0Overflow.mask());
        }
        if events.contains(Ieee802154Event::Timer1Overflow) {
            if let Some(action) = self.timer1.take() {
                self.run_timer_action(&mut cx, action);
            }
            events = events.difference(Ieee802154Event::Timer1Overflow.mask());
        }
        if self.needs_next_operation {
            self.next_operation(&mut cx);
            self.needs_next_operation = false;
        }
        vendor_assert!(events.is_empty());
    }

    // ---- buffers and notifications -------------------------------------

    fn tx_frame(&self) -> &DmaFrame {
        match self.tx {
            TxSource::Frame => &self.buffers.tx,
            TxSource::EnhancedAck => &self.buffers.enhanced_ack,
        }
    }

    fn current_rx(&self) -> &DmaFrame {
        &self.buffers.rx[usize::from(self.rx_index)]
    }

    /// `ieee802154_receive_done` of the current slot: the stub buffer drops
    /// the frame.
    fn receive_done<L, E>(&mut self, cx: &mut Cx<'_, L, E>)
    where
        L: Ieee802154LowLevel + ?Sized,
        E: Ieee802154Environment + ?Sized,
    {
        if self.rx_index == STUB {
            return;
        }
        let index = usize::from(self.rx_index);
        self.buffers.rx[index].mask_length();
        self.rx_info[index].process = true;
        let frame = self.buffers.rx[index].read();
        cx.env.receive_done(
            Ieee802154RxSlot(self.rx_index),
            &frame,
            &self.rx_info[index],
        );
    }

    /// `ieee802154_transmit_done`: an ACK landing in the stub buffer turns
    /// the completion into a missing ACK.
    fn transmit_done<L, E>(&mut self, cx: &mut Cx<'_, L, E>, with_ack: bool)
    where
        L: Ieee802154LowLevel + ?Sized,
        E: Ieee802154Environment + ?Sized,
    {
        let frame = self.tx_frame().read();
        if !with_ack {
            cx.env.transmit_done(&frame, None);
            return;
        }
        if self.rx_index == STUB {
            cx.env.transmit_failed(&frame, Ieee802154TxError::NoAck);
            return;
        }
        let index = usize::from(self.rx_index);
        self.rx_info[index].process = true;
        let ack = self.buffers.rx[index].read();
        cx.env.transmit_done(
            &frame,
            Some(Ieee802154ReceivedAck {
                slot: Ieee802154RxSlot(self.rx_index),
                frame: &ack,
                info: &self.rx_info[index],
            }),
        );
    }

    fn transmit_failed<L, E>(&mut self, cx: &mut Cx<'_, L, E>, error: Ieee802154TxError)
    where
        L: Ieee802154LowLevel + ?Sized,
        E: Ieee802154Environment + ?Sized,
    {
        let frame = self.tx_frame().read();
        cx.env.transmit_failed(&frame, error);
    }

    /// `ieee802154_rx_frame_info_update`.
    fn rx_frame_info_update<L, E>(&mut self, cx: &mut Cx<'_, L, E>)
    where
        L: Ieee802154LowLevel + ?Sized,
        E: Ieee802154Environment + ?Sized,
    {
        let index = usize::from(self.rx_index);
        let image = self.buffers.rx[index].read();
        let length = image[0] & 0x7f;
        let info = &mut self.rx_info[index];
        if length < FRAME_MIN_LEN {
            info.channel = 0;
            info.rssi = 0;
            info.lqi = 0;
            return;
        }
        let rssi = image[usize::from(length) - 1] as i8;
        let lqi = image[usize::from(length)];
        let channel = Ieee802154Channel::from_frequency_code(cx.ll.frequency_code());
        vendor_assert!(channel.is_some());
        info.channel = channel.map_or(0, Ieee802154Channel::number);
        info.rssi = rssi.wrapping_add(RSSI_COMPENSATION);
        info.lqi = lqi;
        self.recent_rx_info_index = self.rx_index;
    }

    /// `set_next_rx_buffer`: reuse the current slot when free, otherwise the
    /// first free slot after it, otherwise the stub buffer.
    fn set_next_rx_buffer<L, E>(&mut self, cx: &mut Cx<'_, L, E>)
    where
        L: Ieee802154LowLevel + ?Sized,
        E: Ieee802154Environment + ?Sized,
    {
        let current_free =
            self.rx_index != STUB && !self.rx_info[usize::from(self.rx_index)].process;
        if !current_free {
            let next = (1..=RX_BUFFER_COUNT)
                .map(|step| (step + usize::from(self.rx_index)) % RX_BUFFER_COUNT)
                .find(|&index| !self.rx_info[index].process);
            self.rx_index = next.map_or(STUB, |index| index as u8);
        }
        let address = self.current_rx().address();
        cx.ll.set_rx_address(address);
    }

    // ---- timers --------------------------------------------------------

    fn timer_slot(&mut self, timer: Ieee802154Timer) -> &mut Option<TimerAction> {
        match timer {
            Ieee802154Timer::Timer0 => &mut self.timer0,
            Ieee802154Timer::Timer1 => &mut self.timer1,
        }
    }

    /// `ieee802154_timer*_stop`: drop the callback, then stop the timer.
    fn timer_stop<L, E>(&mut self, cx: &mut Cx<'_, L, E>, timer: Ieee802154Timer)
    where
        L: Ieee802154LowLevel + ?Sized,
        E: Ieee802154Environment + ?Sized,
    {
        *self.timer_slot(timer) = None;
        cx.ll.stop_timer(timer);
    }

    /// `ieee802154_timer*_fire_at` against a fresh `esp_timer` sample.
    fn timer_fire_at<L, E>(&mut self, cx: &mut Cx<'_, L, E>, timer: Ieee802154Timer, time: u32)
    where
        L: Ieee802154LowLevel + ?Sized,
        E: Ieee802154Environment + ?Sized,
    {
        let now = cx.env.now_micros() as u32;
        ll::timer_fire_at(cx.ll, timer, time, now);
    }

    /// `event_end_process`, including the timer callbacks it drops.
    fn event_end_process<L, E>(&mut self, cx: &mut Cx<'_, L, E>)
    where
        L: Ieee802154LowLevel + ?Sized,
        E: Ieee802154Environment + ?Sized,
    {
        self.timer0 = None;
        self.timer1 = None;
        ll::event_end_process(cx.ll);
    }

    fn run_timer_action<L, E>(&mut self, cx: &mut Cx<'_, L, E>, action: TimerAction)
    where
        L: Ieee802154LowLevel + ?Sized,
        E: Ieee802154Environment + ?Sized,
    {
        match action {
            // `ieee802154_rx_ack_timeout_callback`.
            TimerAction::AckTimeout => {
                vendor_assert!(self.state == Ieee802154State::RxAck);
                self.transmit_failed(cx, Ieee802154TxError::NoAck);
                self.needs_next_operation = true;
            }
            // `ieee802154_start_receive_at`.
            TimerAction::StartReceiveAt { stop_time } => {
                self.timer1 = Some(TimerAction::FinishReceiveAt);
                self.timer_fire_at(cx, Ieee802154Timer::Timer1, stop_time);
            }
            // `ieee802154_finish_receive_at`.
            TimerAction::FinishReceiveAt => {
                if self.state == Ieee802154State::Rx && cx.ll.is_current_rx_frame() {
                    cx.ll.enable_rx_aborts(Ieee802154RxAbortEnableSet::All);
                    self.pending_rx_stop = true;
                } else {
                    self.stop_current_operation(cx);
                    cx.env.receive_at_done();
                }
            }
        }
    }

    // ---- operation starts ------------------------------------------------

    fn tx_init<L, E>(&mut self, cx: &mut Cx<'_, L, E>, frame: &[u8])
    where
        L: Ieee802154LowLevel + ?Sized,
        E: Ieee802154Environment + ?Sized,
    {
        // The vendor points `s_tx_frame` at the new frame before stopping the
        // current operation, so a stopped transmission reports the new frame.
        self.buffers.tx.write(frame);
        self.tx = TxSource::Frame;
        self.stop_current_operation(cx);
        self.pib.update(cx.ll, self.levels);
        cx.ll.set_tx_address(self.buffers.tx.address());
        if PhrFrame::new(frame).ack_required() {
            self.set_next_rx_buffer(cx);
        }
    }

    fn rx_init<L, E>(&mut self, cx: &mut Cx<'_, L, E>)
    where
        L: Ieee802154LowLevel + ?Sized,
        E: Ieee802154Environment + ?Sized,
    {
        self.stop_current_operation(cx);
        self.pib.update(cx.ll, self.levels);
    }

    fn enable_rx<L, E>(&mut self, cx: &mut Cx<'_, L, E>)
    where
        L: Ieee802154LowLevel + ?Sized,
        E: Ieee802154Environment + ?Sized,
    {
        self.set_next_rx_buffer(cx);
        cx.ll.set_command(Ieee802154LlCommand::RxStart);
        self.state = Ieee802154State::Rx;
    }

    fn start_ed<L, E>(cx: &mut Cx<'_, L, E>, duration: u16)
    where
        L: Ieee802154LowLevel + ?Sized,
        E: Ieee802154Environment + ?Sized,
    {
        cx.ll.enable_event(Ieee802154Event::EdDone);
        cx.ll.set_ed_duration(duration);
        cx.ll.set_command(Ieee802154LlCommand::EdStart);
    }

    fn enter_sleep<L, E>(&mut self, cx: &mut Cx<'_, L, E>)
    where
        L: Ieee802154LowLevel + ?Sized,
        E: Ieee802154Environment + ?Sized,
    {
        if self.state != Ieee802154State::Sleep {
            self.stop_current_operation(cx);
            self.state = Ieee802154State::Sleep;
        }
    }

    fn next_operation<L, E>(&mut self, cx: &mut Cx<'_, L, E>)
    where
        L: Ieee802154LowLevel + ?Sized,
        E: Ieee802154Environment + ?Sized,
    {
        if self.pending_rx_stop {
            cx.ll.disable_rx_aborts(Ieee802154RxAbortEnableSet::All);
            cx.ll
                .enable_rx_aborts(Ieee802154RxAbortEnableSet::RuntimeBaseline);
            cx.env.receive_at_done();
            self.pending_rx_stop = false;
        }
        if self.pib.rx_when_idle() {
            self.enable_rx(cx);
        } else {
            self.state = Ieee802154State::Idle;
            self.enter_sleep(cx);
        }
    }

    // ---- stops -----------------------------------------------------------

    fn stop_current_operation<L, E>(&mut self, cx: &mut Cx<'_, L, E>)
    where
        L: Ieee802154LowLevel + ?Sized,
        E: Ieee802154Environment + ?Sized,
    {
        self.event_end_process(cx);
        match self.state {
            Ieee802154State::Disable | Ieee802154State::Sleep => {}
            Ieee802154State::Idle => cx.ll.set_command(Ieee802154LlCommand::Stop),
            Ieee802154State::Rx => self.stop_rx(cx),
            Ieee802154State::TxAck => self.stop_tx_ack(cx),
            Ieee802154State::TxCca => {
                self.stop_tx(cx);
                cx.ll.clear_events(Ieee802154Event::TxAbort.mask());
            }
            Ieee802154State::Cca | Ieee802154State::Ed => {
                cx.ll.set_command(Ieee802154LlCommand::Stop);
                cx.ll.clear_events(event_mask(&[
                    Ieee802154Event::EdDone,
                    Ieee802154Event::RxAbort,
                ]));
            }
            Ieee802154State::Tx | Ieee802154State::TxEnhAck => self.stop_tx(cx),
            Ieee802154State::RxAck => self.stop_rx_ack(cx),
        }
    }

    fn stop_rx<L, E>(&mut self, cx: &mut Cx<'_, L, E>)
    where
        L: Ieee802154LowLevel + ?Sized,
        E: Ieee802154Environment + ?Sized,
    {
        cx.ll.set_command(Ieee802154LlCommand::Stop);
        if cx.ll.events().contains(Ieee802154Event::RxDone) {
            self.receive_done(cx);
        }
        cx.ll.clear_events(event_mask(&[
            Ieee802154Event::RxDone,
            Ieee802154Event::RxAbort,
            Ieee802154Event::RxSfdDone,
        ]));
    }

    fn stop_tx_ack<L, E>(&mut self, cx: &mut Cx<'_, L, E>)
    where
        L: Ieee802154LowLevel + ?Sized,
        E: Ieee802154Environment + ?Sized,
    {
        cx.ll.set_command(Ieee802154LlCommand::Stop);
        ll::sec_clear(cx.ll);
        self.receive_done(cx);
        cx.ll.clear_events(event_mask(&[
            Ieee802154Event::AckTxDone,
            Ieee802154Event::RxAbort,
            Ieee802154Event::TxSfdDone,
        ]));
    }

    fn stop_tx<L, E>(&mut self, cx: &mut Cx<'_, L, E>)
    where
        L: Ieee802154LowLevel + ?Sized,
        E: Ieee802154Environment + ?Sized,
    {
        cx.ll.set_command(Ieee802154LlCommand::Stop);
        ll::sec_clear(cx.ll);
        let events = cx.ll.events();
        if self.state == Ieee802154State::TxEnhAck {
            self.receive_done(cx);
            cx.ll.clear_events(Ieee802154Event::AckTxDone.mask());
        } else if events.contains(Ieee802154Event::TxDone)
            && (!PhrFrame::new(&self.tx_frame().read()).ack_required() || !cx.ll.rx_auto_ack())
        {
            self.transmit_done(cx, false);
        } else {
            self.transmit_failed(cx, Ieee802154TxError::Abort);
        }
        cx.ll.clear_events(event_mask(&[
            Ieee802154Event::TxDone,
            Ieee802154Event::TxAbort,
            Ieee802154Event::TxSfdDone,
        ]));
    }

    fn stop_rx_ack<L, E>(&mut self, cx: &mut Cx<'_, L, E>)
    where
        L: Ieee802154LowLevel + ?Sized,
        E: Ieee802154Environment + ?Sized,
    {
        cx.ll.set_command(Ieee802154LlCommand::Stop);
        let events = cx.ll.events();
        self.timer_stop(cx, Ieee802154Timer::Timer0);
        cx.ll.disable_event(Ieee802154Event::Timer0Overflow);
        if events.contains(Ieee802154Event::AckRxDone) {
            self.transmit_done(cx, true);
        } else {
            self.transmit_failed(cx, Ieee802154TxError::NoAck);
        }
        cx.ll.clear_events(event_mask(&[
            Ieee802154Event::AckRxDone,
            Ieee802154Event::RxSfdDone,
            Ieee802154Event::TxAbort,
        ]));
    }

    // ---- interrupt handlers ---------------------------------------------

    fn isr_tx_done<L, E>(&mut self, cx: &mut Cx<'_, L, E>)
    where
        L: Ieee802154LowLevel + ?Sized,
        E: Ieee802154Environment + ?Sized,
    {
        ll::sec_clear(cx.ll);
        self.event_end_process(cx);
        let frame = self.tx_frame().read();
        let frame = PhrFrame::new(&frame);
        if !frame.frame_type().is_supported() {
            cx.ll.set_command(Ieee802154LlCommand::Stop);
            self.transmit_done(cx, false);
            self.needs_next_operation = true;
            return;
        }
        if matches!(self.state, Ieee802154State::Tx | Ieee802154State::TxCca) {
            if frame.ack_required() && cx.ll.rx_auto_ack() {
                self.state = Ieee802154State::RxAck;
                // `receive_ack_timeout_timer_start`.
                cx.ll.enable_event(Ieee802154Event::Timer0Overflow);
                let now = cx.env.now_micros() as u32;
                self.timer0 = Some(TimerAction::AckTimeout);
                self.timer_fire_at(
                    cx,
                    Ieee802154Timer::Timer0,
                    now.wrapping_add(ACK_TIMEOUT_MICROSECONDS),
                );
                self.needs_next_operation = false;
            } else {
                self.transmit_done(cx, false);
                self.needs_next_operation = true;
            }
        }
    }

    fn isr_rx_done<L, E>(&mut self, cx: &mut Cx<'_, L, E>)
    where
        L: Ieee802154LowLevel + ?Sized,
        E: Ieee802154Environment + ?Sized,
    {
        self.event_end_process(cx);
        self.rx_frame_info_update(cx);
        let image = self.current_rx().read();
        let frame = PhrFrame::new(&image);
        if !frame.frame_type().is_supported() {
            cx.ll.set_command(Ieee802154LlCommand::Stop);
            self.receive_done(cx);
            self.needs_next_operation = true;
            return;
        }
        if self.state != Ieee802154State::Rx {
            return;
        }
        let index = usize::from(self.rx_index);
        if frame.ack_required()
            && matches!(frame.version(), FrameVersion::V2003 | FrameVersion::V2006)
            && cx.ll.tx_auto_ack()
        {
            self.rx_info[index].pending = self.ack_config_pending_bit(cx, frame);
            self.state = Ieee802154State::TxAck;
            self.needs_next_operation = false;
        } else if frame.ack_required()
            && frame.version() == FrameVersion::V2015
            && cx.ll.tx_enhanced_ack()
        {
            self.rx_info[index].pending = self.ack_config_pending_bit(cx, frame);
            let mut ack = [0; FRAME_SIZE];
            if cx
                .env
                .generate_enhanced_ack(&image, &self.rx_info[index], &mut ack)
            {
                self.buffers.enhanced_ack.write(&ack);
                cx.ll.set_tx_address(self.buffers.enhanced_ack.address());
                self.tx = TxSource::EnhancedAck;
                cx.ll.notify_enhanced_ack_generated();
                self.state = Ieee802154State::TxEnhAck;
                self.needs_next_operation = false;
            } else {
                cx.ll.set_command(Ieee802154LlCommand::Stop);
                self.receive_done(cx);
                self.needs_next_operation = true;
            }
        } else {
            self.receive_done(cx);
            self.needs_next_operation = true;
        }
    }

    /// `ieee802154_ack_config_pending_bit`.
    fn ack_config_pending_bit<L, E>(&mut self, cx: &mut Cx<'_, L, E>, frame: PhrFrame<'_>) -> bool
    where
        L: Ieee802154LowLevel + ?Sized,
        E: Ieee802154Environment + ?Sized,
    {
        let enhanced_lookup = cx.ll.pending_mode();
        let mode = self.pib.pending_mode(Ieee802154MultipanIndex::CONTEXT0);
        let decision = ack_pending(frame, enhanced_lookup, mode, &self.pending_table);
        if decision.set_to_hardware {
            cx.ll.set_pending_bit(decision.pending);
        }
        decision.pending
    }

    fn isr_rx_phase_rx_abort<L, E>(
        &mut self,
        cx: &mut Cx<'_, L, E>,
        reason: Ieee802154RxAbortReasonObservation,
    ) where
        L: Ieee802154LowLevel + ?Sized,
        E: Ieee802154Environment + ?Sized,
    {
        use Ieee802154RxAbortReason as Reason;
        self.event_end_process(cx);
        let status = cx.ll.rx_status();
        let Ieee802154RxAbortReasonObservation::Named(reason) = reason else {
            panic!("IEEE802154_ASSERT(false): unclassified RX abort reason");
        };
        match reason {
            Reason::RxStop | Reason::TxAckStop | Reason::EdStop => return,
            Reason::SfdTimeout
            | Reason::CrcError
            | Reason::InvalidLength
            | Reason::FilterFail
            | Reason::NoRss
            | Reason::UnexpectedAck
            | Reason::RxRestart
            | Reason::CoexistenceBreak => {
                vendor_assert!(self.state == Ieee802154State::Rx);
            }
            Reason::EdAbort | Reason::EdCoexistenceReject => {
                vendor_assert!(matches!(
                    self.state,
                    Ieee802154State::Ed | Ieee802154State::Cca
                ));
                cx.env.ed_failed(status);
            }
            Reason::TxAckTimeout
            | Reason::TxAckCoexistenceBreak
            | Reason::EnhancedAckSecurityError => {
                return;
            }
        }
        self.needs_next_operation = true;
    }

    fn isr_tx_ack_phase_rx_abort<L, E>(
        &mut self,
        cx: &mut Cx<'_, L, E>,
        reason: Ieee802154RxAbortReasonObservation,
    ) where
        L: Ieee802154LowLevel + ?Sized,
        E: Ieee802154Environment + ?Sized,
    {
        use Ieee802154RxAbortReason as Reason;
        ll::sec_clear(cx.ll);
        self.event_end_process(cx);
        let Ieee802154RxAbortReasonObservation::Named(reason) = reason else {
            panic!("IEEE802154_ASSERT(false): unclassified RX abort reason");
        };
        match reason {
            Reason::TxAckStop
            | Reason::RxStop
            | Reason::EdStop
            | Reason::SfdTimeout
            | Reason::CrcError
            | Reason::InvalidLength
            | Reason::FilterFail
            | Reason::NoRss
            | Reason::UnexpectedAck
            | Reason::RxRestart
            | Reason::CoexistenceBreak
            | Reason::EdAbort
            | Reason::EdCoexistenceReject => return,
            Reason::TxAckTimeout
            | Reason::TxAckCoexistenceBreak
            | Reason::EnhancedAckSecurityError => self.receive_done(cx),
        }
        self.needs_next_operation = true;
    }

    fn isr_tx_abort<L, E>(
        &mut self,
        cx: &mut Cx<'_, L, E>,
        reason: Ieee802154TxAbortReasonObservation,
    ) where
        L: Ieee802154LowLevel + ?Sized,
        E: Ieee802154Environment + ?Sized,
    {
        use Ieee802154TxAbortReason as Reason;
        ll::sec_clear(cx.ll);
        self.event_end_process(cx);
        let Ieee802154TxAbortReasonObservation::Named(reason) = reason else {
            panic!("IEEE802154_ASSERT(false): unclassified TX abort reason");
        };
        match reason {
            Reason::RxAckStop | Reason::TxStop => {}
            Reason::RxAckSfdTimeout
            | Reason::RxAckCrcError
            | Reason::RxAckInvalidLength
            | Reason::RxAckFilterFail
            | Reason::RxAckNoRss
            | Reason::RxAckCoexistenceBreak
            | Reason::RxAckTypeNotAck
            | Reason::RxAckRestart => {
                vendor_assert!(self.state == Ieee802154State::RxAck);
                self.transmit_failed(cx, Ieee802154TxError::InvalidAck);
                self.needs_next_operation = false;
            }
            Reason::RxAckTimeout => {
                vendor_assert!(self.state == Ieee802154State::RxAck);
                cx.ll.disable_event(Ieee802154Event::Timer0Overflow);
                self.transmit_failed(cx, Ieee802154TxError::NoAck);
                self.needs_next_operation = true;
            }
            Reason::TxCoexistenceBreak | Reason::TxSecurityError => {
                vendor_assert!(matches!(
                    self.state,
                    Ieee802154State::Tx | Ieee802154State::TxCca
                ));
                let error = if reason == Reason::TxCoexistenceBreak {
                    Ieee802154TxError::Coexist
                } else {
                    Ieee802154TxError::Security
                };
                self.transmit_failed(cx, error);
                self.needs_next_operation = true;
            }
            Reason::CcaFailed | Reason::CcaBusy => {
                vendor_assert!(self.state == Ieee802154State::TxCca);
                let error = if reason == Reason::CcaFailed {
                    Ieee802154TxError::Abort
                } else {
                    Ieee802154TxError::CcaBusy
                };
                self.transmit_failed(cx, error);
                self.needs_next_operation = true;
            }
        }
    }
}

#[cfg(test)]
mod tests;
