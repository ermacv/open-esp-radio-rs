//! A register model of [`Ieee802154LowLevel`] for host tests.
//!
//! Getters return the value their setter last wrote; the test sets the
//! event image, abort reasons and measurement results the MAC would latch.
//! The model knows nothing else about the hardware.

use super::{
    Ieee802154EdSampleMode, Ieee802154EtmChannel, Ieee802154EtmRoute, Ieee802154EventObservation,
    Ieee802154LlCommand, Ieee802154LowLevel, Ieee802154MultipanEnableState,
    Ieee802154RxAbortEnableSet, Ieee802154RxStatus, Ieee802154Timer, Ieee802154TxAbortEnableSet,
};
use crate::ieee802154::{
    Ieee802154MultipanIndex,
    lifecycle::Ieee802154Channel,
    mac::{
        Ieee802154Event, Ieee802154EventMask, Ieee802154RxAbortReasonObservation,
        Ieee802154TxAbortReasonObservation,
    },
    policy::Ieee802154CcaMode,
    tx_power::Ieee802154ResolvedTxPower,
};

/// The modelled MAC state; every field is public for the test to set or
/// inspect.
#[derive(Clone, Debug)]
pub struct Ieee802154LlModel {
    /// Latched events; `clear_events` clears them.
    pub events: Ieee802154EventMask,
    /// Latched receive-abort reason.
    pub rx_abort: Ieee802154RxAbortReasonObservation,
    /// Latched transmit-abort reason.
    pub tx_abort: Ieee802154TxAbortReasonObservation,
    /// Whether a frame is past its SFD.
    pub current_rx_frame: bool,
    /// Energy-detection result.
    pub ed_rss: i8,
    /// CCA result.
    pub cca_busy: bool,
    /// The last operation command.
    pub command: Option<Ieee802154LlCommand>,
    /// The last published receive buffer.
    pub rx_address: Option<u32>,
    /// The last published transmit buffer.
    pub tx_address: Option<u32>,
    /// The published channel's frequency code.
    pub frequency_code: u8,
    /// Hardware ACK transmission.
    pub tx_auto_ack: bool,
    /// ACK reception after transmission.
    pub rx_auto_ack: bool,
    /// Enhanced-ACK transmission.
    pub tx_enhanced_ack: bool,
    /// Enhanced pending lookup.
    pub pending_mode: bool,
    /// The last pending bit.
    pub pending_bit: bool,
    /// Promiscuous receive.
    pub promiscuous: bool,
    /// Transmit security enable.
    pub transmit_security: bool,
    /// PAN ID of each context.
    pub panid: [u16; 4],
    /// Short address of each context.
    pub short_address: [u16; 4],
    /// Extended address of each context.
    pub extended_address: [[u8; 8]; 4],
    /// ACK timeout in 16-microsecond units.
    pub ack_timeout: u16,
    /// ETM channel enables.
    pub etm_enabled: [bool; 2],
    /// Multi-PAN context enables.
    pub multipan_enable: Ieee802154MultipanEnableState,
}

impl Default for Ieee802154LlModel {
    fn default() -> Self {
        Self {
            events: Ieee802154EventMask::NONE,
            rx_abort: Ieee802154RxAbortReasonObservation::Unclassified,
            tx_abort: Ieee802154TxAbortReasonObservation::Unclassified,
            current_rx_frame: false,
            ed_rss: 0,
            cca_busy: false,
            command: None,
            rx_address: None,
            tx_address: None,
            frequency_code: 0,
            tx_auto_ack: false,
            rx_auto_ack: false,
            tx_enhanced_ack: false,
            pending_mode: false,
            pending_bit: false,
            promiscuous: false,
            transmit_security: false,
            panid: [0; 4],
            short_address: [0; 4],
            extended_address: [[0; 8]; 4],
            ack_timeout: 0,
            etm_enabled: [false; 2],
            multipan_enable: Ieee802154MultipanEnableState::NONE,
        }
    }
}

impl Ieee802154LlModel {
    /// Latch `events` for the next interrupt.
    pub fn raise(&mut self, events: &[Ieee802154Event]) {
        self.events = events
            .iter()
            .fold(self.events, |mask, event| mask.union(event.mask()));
    }

    /// The published channel.
    pub fn channel(&self) -> Option<Ieee802154Channel> {
        Ieee802154Channel::from_frequency_code(self.frequency_code)
    }
}

const fn etm_index(channel: Ieee802154EtmChannel) -> usize {
    match channel {
        Ieee802154EtmChannel::Channel0 => 0,
        Ieee802154EtmChannel::Channel1 => 1,
    }
}

impl Ieee802154LowLevel for Ieee802154LlModel {
    fn set_command(&mut self, command: Ieee802154LlCommand) {
        self.command = Some(command);
    }
    fn events(&mut self) -> Ieee802154EventObservation {
        Ieee802154EventObservation::from_named(self.events)
    }
    fn clear_events(&mut self, mask: Ieee802154EventMask) {
        self.events = self.events.difference(mask);
    }
    fn rx_abort_reason(&mut self) -> Ieee802154RxAbortReasonObservation {
        self.rx_abort
    }
    fn tx_abort_reason(&mut self) -> Ieee802154TxAbortReasonObservation {
        self.tx_abort
    }
    fn rx_status(&mut self) -> Ieee802154RxStatus {
        Ieee802154RxStatus::new(
            0,
            self.rx_abort,
            super::Ieee802154RxStateCode::new(0).expect("zero is a state code"),
            false,
            false,
            false,
        )
    }
    fn is_current_rx_frame(&mut self) -> bool {
        self.current_rx_frame
    }
    fn set_tx_address(&mut self, address: u32) {
        self.tx_address = Some(address);
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
        self.tx_enhanced_ack
    }
    fn pending_mode(&mut self) -> bool {
        self.pending_mode
    }
    fn set_pending_bit(&mut self, pending: bool) {
        self.pending_bit = pending;
    }
    fn frequency_code(&mut self) -> u8 {
        self.frequency_code
    }
    fn ed_rss(&mut self) -> i8 {
        self.ed_rss
    }
    fn cca_busy(&mut self) -> bool {
        self.cca_busy
    }
    fn set_ed_duration(&mut self, _symbols: u16) {}
    fn notify_enhanced_ack_generated(&mut self) {}
    fn disable_rx_aborts(&mut self, _set: Ieee802154RxAbortEnableSet) {}
    fn set_multipan_panid(&mut self, index: Ieee802154MultipanIndex, panid: u16) {
        self.multipan_enable = self.multipan_enable.with(index);
        self.panid[usize::from(index.value())] = panid;
    }
    fn multipan_panid(&mut self, index: Ieee802154MultipanIndex) -> u16 {
        self.panid[usize::from(index.value())]
    }
    fn set_multipan_short_address(&mut self, index: Ieee802154MultipanIndex, address: u16) {
        self.multipan_enable = self.multipan_enable.with(index);
        self.short_address[usize::from(index.value())] = address;
    }
    fn multipan_short_address(&mut self, index: Ieee802154MultipanIndex) -> u16 {
        self.short_address[usize::from(index.value())]
    }
    fn set_multipan_extended_address(&mut self, index: Ieee802154MultipanIndex, address: [u8; 8]) {
        self.multipan_enable = self.multipan_enable.with(index);
        self.extended_address[usize::from(index.value())] = address;
    }
    fn multipan_extended_address(&mut self, index: Ieee802154MultipanIndex) -> [u8; 8] {
        self.extended_address[usize::from(index.value())]
    }
    fn set_multipan_enable(&mut self, state: Ieee802154MultipanEnableState) {
        self.multipan_enable = state;
    }
    fn multipan_enable(&mut self) -> Ieee802154MultipanEnableState {
        self.multipan_enable
    }
    fn set_ack_timeout(&mut self, units: u16) {
        self.ack_timeout = units;
    }
    fn ack_timeout(&mut self) -> u16 {
        self.ack_timeout
    }
    fn set_security_address(&mut self, _address: &[u8; 8]) {}
    fn set_security_key(&mut self, _key: &[u8; 16]) {}
    fn set_security_offset(&mut self, _offset: u8) {}
    fn enable_all_events(&mut self) {}
    fn enable_event(&mut self, _event: Ieee802154Event) {}
    fn disable_event(&mut self, _event: Ieee802154Event) {}
    fn enable_tx_aborts(&mut self, _set: Ieee802154TxAbortEnableSet) {}
    fn enable_rx_aborts(&mut self, _set: Ieee802154RxAbortEnableSet) {}
    fn set_ed_sample_mode(&mut self, _mode: Ieee802154EdSampleMode) {}
    fn disable_coex(&mut self) {}
    fn set_channel(&mut self, channel: Ieee802154Channel) {
        self.frequency_code = channel.frequency_code().value();
    }
    fn set_tx_power(&mut self, _power: &Ieee802154ResolvedTxPower<'_>) {}
    fn set_cca_mode(&mut self, _mode: Ieee802154CcaMode) {}
    fn set_cca_threshold(&mut self, _threshold_dbm: i8) {}
    fn set_tx_auto_ack(&mut self, enable: bool) {
        self.tx_auto_ack = enable;
    }
    fn set_rx_auto_ack(&mut self, enable: bool) {
        self.rx_auto_ack = enable;
    }
    fn set_tx_enhanced_ack(&mut self, enable: bool) {
        self.tx_enhanced_ack = enable;
    }
    fn set_coordinator(&mut self, _enable: bool) {}
    fn set_promiscuous(&mut self, enable: bool) {
        self.promiscuous = enable;
    }
    fn set_pending_mode(&mut self, enhanced: bool) {
        self.pending_mode = enhanced;
    }
    fn set_transmit_security(&mut self, enable: bool) {
        self.transmit_security = enable;
    }
    fn set_timer_threshold(&mut self, _timer: Ieee802154Timer, _microseconds: u32) {}
    fn start_timer(&mut self, _timer: Ieee802154Timer) {}
    fn stop_timer(&mut self, _timer: Ieee802154Timer) {}
    fn etm_channel_enabled(&mut self, channel: Ieee802154EtmChannel) -> bool {
        self.etm_enabled[etm_index(channel)]
    }
    fn disable_etm_channel(&mut self, channel: Ieee802154EtmChannel) {
        self.etm_enabled[etm_index(channel)] = false;
    }
    fn enable_etm_channel(&mut self, channel: Ieee802154EtmChannel) {
        self.etm_enabled[etm_index(channel)] = true;
    }
    fn set_etm_route(&mut self, _route: Ieee802154EtmRoute) {}
}
