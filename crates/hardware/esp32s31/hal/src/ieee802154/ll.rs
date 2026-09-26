//! Hardware-side helpers of the public ESP-IDF IEEE 802.15.4 driver, ported
//! over a backend whose methods are the driver's `ieee802154_ll_*` accessors.
//!
//! The pinned driver (`components/ieee802154/driver`) mixes hardware steps
//! with its callbacks and state. This module owns only the hardware steps:
//! the modem ETM channel helpers of `esp_ieee802154_util.c`, the timer
//! helpers of `esp_ieee802154_timer.c`, `event_end_process` and the register
//! part of `ieee802154_mac_init` from `esp_ieee802154_dev.c`, and
//! `ieee802154_sec_clear`. Timer callbacks, operation state and interrupt
//! allocation stay with the driver engine.
//!
//! Every helper is generic over [`Ieee802154LowLevel`], so the engine and its
//! tests issue exactly the accessor sequence the vendor driver issues and a
//! recording backend can compare it with the vendor host stand.

use oer_esp32s31_pac::{
    Ieee802154Pti as PacPti, Ieee802154Timer0ThresholdWord as PacTimer0ThresholdWord,
    Ieee802154Timer1ThresholdWord as PacTimer1ThresholdWord,
    Ieee802154TxPowerCode as PacTxPowerCode,
};

pub use oer_esp32s31_pac::{
    Ieee802154EdSampleMode, Ieee802154EtmChannel, Ieee802154EtmRoute, Ieee802154RxAbortEnableSet,
    Ieee802154TxAbortEnableSet,
};

use crate::ieee802154::{
    lifecycle::{COEX_DISABLED_PTI, Ieee802154Channel},
    mac::{Ieee802154Event, Ieee802154TaskOwner},
    policy::Ieee802154CcaMode,
    tx_power::Ieee802154ResolvedTxPower,
};

/// One of the two MAC timers.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Ieee802154Timer {
    /// TIMER0: ACK watchdog and timed transmit.
    Timer0,
    /// TIMER1: timed receive.
    Timer1,
}

/// The `ieee802154_ll_*` accessors, the modem ETM register steps and the
/// timer commands used by the driver's hardware helpers.
///
/// Each method is one accessor of the pinned public LL; implementations must
/// not add, merge or reorder register transactions.
pub trait Ieee802154LowLevel {
    /// `ieee802154_ll_enable_events(IEEE802154_EVENT_MASK)`.
    fn enable_all_events(&mut self);
    /// `ieee802154_ll_enable_events` of one event.
    fn enable_event(&mut self, event: Ieee802154Event);
    /// `ieee802154_ll_disable_events` of one event.
    fn disable_event(&mut self, event: Ieee802154Event);
    /// `ieee802154_ll_enable_tx_abort_events`.
    fn enable_tx_aborts(&mut self, set: Ieee802154TxAbortEnableSet);
    /// `ieee802154_ll_enable_rx_abort_events`.
    fn enable_rx_aborts(&mut self, set: Ieee802154RxAbortEnableSet);
    /// `ieee802154_ll_set_ed_sample_mode`.
    fn set_ed_sample_mode(&mut self, mode: Ieee802154EdSampleMode);
    /// `ieee802154_ll_disable_coex`.
    fn disable_coex(&mut self);
    /// `ieee802154_ll_set_freq` of the channel's frequency code.
    fn set_channel(&mut self, channel: Ieee802154Channel);
    /// `ieee802154_ll_set_power` of the resolved power index.
    fn set_tx_power(&mut self, power: &Ieee802154ResolvedTxPower<'_>);
    /// `ieee802154_ll_set_cca_mode`.
    fn set_cca_mode(&mut self, mode: Ieee802154CcaMode);
    /// `ieee802154_ll_set_cca_threshold`.
    fn set_cca_threshold(&mut self, threshold_dbm: i8);
    /// `ieee802154_ll_set_tx_auto_ack`.
    fn set_tx_auto_ack(&mut self, enable: bool);
    /// `ieee802154_ll_set_rx_auto_ack`.
    fn set_rx_auto_ack(&mut self, enable: bool);
    /// `ieee802154_ll_set_tx_enhance_ack`.
    fn set_tx_enhanced_ack(&mut self, enable: bool);
    /// `ieee802154_ll_set_coordinator`.
    fn set_coordinator(&mut self, enable: bool);
    /// `ieee802154_ll_set_promiscuous`.
    fn set_promiscuous(&mut self, enable: bool);
    /// `ieee802154_ll_set_pending_mode`: the one-bit enhanced/Zigbee selector.
    fn set_pending_mode(&mut self, enhanced: bool);
    /// `ieee802154_ll_set_transmit_security`.
    fn set_transmit_security(&mut self, enable: bool);
    /// `ieee802154_ll_timer0_set_threshold` or `ieee802154_ll_timer1_set_threshold`
    /// in microseconds.
    fn set_timer_threshold(&mut self, timer: Ieee802154Timer, microseconds: u32);
    /// `ieee802154_ll_set_cmd` of the timer's start command.
    fn start_timer(&mut self, timer: Ieee802154Timer);
    /// `ieee802154_ll_set_cmd` of the timer's stop command.
    fn stop_timer(&mut self, timer: Ieee802154Timer);
    /// `REG_READ(ETM_CHEN_AD0_REG)` tested for the channel bit.
    fn etm_channel_enabled(&mut self, channel: Ieee802154EtmChannel) -> bool;
    /// `ETM_CHENCLR_AD0_REG` written back with the channel bit added.
    fn disable_etm_channel(&mut self, channel: Ieee802154EtmChannel);
    /// `ETM_CHENSET_AD0_REG` written back with the channel bit added.
    fn enable_etm_channel(&mut self, channel: Ieee802154EtmChannel);
    /// The channel's complete event word, then its complete task word.
    fn set_etm_route(&mut self, route: Ieee802154EtmRoute);
}

/// `ieee802154_etm_channel_clear`: disable `channel` only if it is enabled.
pub fn etm_channel_clear<Ll: Ieee802154LowLevel + ?Sized>(
    ll: &mut Ll,
    channel: Ieee802154EtmChannel,
) {
    if ll.etm_channel_enabled(channel) {
        ll.disable_etm_channel(channel);
    }
}

/// `ieee802154_etm_set_event_task`: clear the route's channel, program its
/// event and task words, then enable it.
pub fn etm_set_event_task<Ll: Ieee802154LowLevel + ?Sized>(ll: &mut Ll, route: Ieee802154EtmRoute) {
    let channel = route.channel();
    etm_channel_clear(ll, channel);
    ll.set_etm_route(route);
    ll.enable_etm_channel(channel);
}

/// `is_target_time_expired`: the wrapping difference `now - target` has a
/// clear sign bit, so a target equal to `now` has expired.
pub const fn target_time_expired(target: u32, now: u32) -> bool {
    now.wrapping_sub(target) & (1 << 31) == 0
}

/// Threshold `ieee802154_timer*_fire_at` programs for `fire_time` sampled at
/// `now`: zero once expired, otherwise the remaining microseconds.
pub const fn timer_threshold(fire_time: u32, now: u32) -> u32 {
    if target_time_expired(fire_time, now) {
        0
    } else {
        fire_time.wrapping_sub(now)
    }
}

/// `ieee802154_timer0_fire_at` / `ieee802154_timer1_fire_at`: program the
/// remaining interval, then start the timer. `now` is the caller's truncated
/// `esp_timer_get_time` sample.
pub fn timer_fire_at<Ll: Ieee802154LowLevel + ?Sized>(
    ll: &mut Ll,
    timer: Ieee802154Timer,
    fire_time: u32,
    now: u32,
) {
    ll.set_timer_threshold(timer, timer_threshold(fire_time, now));
    ll.start_timer(timer);
}

/// Register steps of `event_end_process`: clear both ETM channels, then stop
/// both timers. The driver engine also drops the timer callbacks.
pub fn event_end_process<Ll: Ieee802154LowLevel + ?Sized>(ll: &mut Ll) {
    etm_channel_clear(ll, Ieee802154EtmChannel::Channel0);
    etm_channel_clear(ll, Ieee802154EtmChannel::Channel1);
    ll.stop_timer(Ieee802154Timer::Timer0);
    ll.stop_timer(Ieee802154Timer::Timer1);
}

/// `ieee802154_sec_clear`.
pub fn sec_clear<Ll: Ieee802154LowLevel + ?Sized>(ll: &mut Ll) {
    ll.set_transmit_security(false);
}

/// Register steps of `ieee802154_mac_init` for a build without software
/// coexistence, between the MAC reset and `ieee802154_txon_delay_set`.
///
/// All events are enabled except TIMER0, which the ACK watchdog enables per
/// transmission; the runtime abort baselines are enabled; ED reports the
/// average sample; both coexistence PTIs are fixed. The caller owns the
/// preceding MAC reset and PIB initialization and the following TX-on delay,
/// interrupt allocation and modem initialization.
pub fn mac_init_registers<Ll: Ieee802154LowLevel + ?Sized>(ll: &mut Ll) {
    ll.enable_all_events();
    ll.disable_event(Ieee802154Event::Timer0Overflow);
    ll.enable_tx_aborts(Ieee802154TxAbortEnableSet::RuntimeBaseline);
    ll.enable_rx_aborts(Ieee802154RxAbortEnableSet::RuntimeBaseline);
    ll.set_ed_sample_mode(Ieee802154EdSampleMode::Average);
    ll.disable_coex();
}

impl Ieee802154LowLevel for Ieee802154TaskOwner {
    fn enable_all_events(&mut self) {
        self.lease().enable_all_events();
    }

    fn enable_event(&mut self, event: Ieee802154Event) {
        self.lease().enable_event(event);
    }

    fn disable_event(&mut self, event: Ieee802154Event) {
        self.lease().disable_event(event);
    }

    fn enable_tx_aborts(&mut self, set: Ieee802154TxAbortEnableSet) {
        self.lease().enable_tx_aborts(set);
    }

    fn enable_rx_aborts(&mut self, set: Ieee802154RxAbortEnableSet) {
        self.lease().enable_rx_aborts(set);
    }

    fn set_ed_sample_mode(&mut self, mode: Ieee802154EdSampleMode) {
        self.lease().set_ed_sample_mode(mode);
    }

    fn disable_coex(&mut self) {
        let pti = PacPti::new(COEX_DISABLED_PTI).expect("the disabled PTI fits five bits");
        let mut lease = self.lease();
        lease.set_txrx_pti(pti);
        lease.set_ack_pti(pti);
    }

    fn set_channel(&mut self, channel: Ieee802154Channel) {
        self.lease().set_frequency_code(channel.frequency_code());
    }

    fn set_tx_power(&mut self, power: &Ieee802154ResolvedTxPower<'_>) {
        let code = PacTxPowerCode::new(u32::from(power.field_code().value()))
            .expect("a provider index is at most 254");
        self.lease().set_tx_power_code(code);
    }

    fn set_cca_mode(&mut self, mode: Ieee802154CcaMode) {
        self.lease().set_cca_mode(mode.into_pac());
    }

    fn set_cca_threshold(&mut self, threshold_dbm: i8) {
        self.lease().set_cca_threshold_code(threshold_dbm);
    }

    fn set_tx_auto_ack(&mut self, enable: bool) {
        self.lease().set_tx_auto_ack(enable);
    }

    fn set_rx_auto_ack(&mut self, enable: bool) {
        self.lease().set_rx_auto_ack(enable);
    }

    fn set_tx_enhanced_ack(&mut self, enable: bool) {
        self.lease().set_enhanced_ack_tx(enable);
    }

    fn set_coordinator(&mut self, enable: bool) {
        self.lease().set_coordinator(enable);
    }

    fn set_promiscuous(&mut self, enable: bool) {
        self.lease().set_promiscuous(enable);
    }

    fn set_pending_mode(&mut self, enhanced: bool) {
        self.lease().set_pending_mode(enhanced);
    }

    fn set_transmit_security(&mut self, enable: bool) {
        self.lease().set_transmit_security(enable);
    }

    fn set_timer_threshold(&mut self, timer: Ieee802154Timer, microseconds: u32) {
        let mut lease = self.lease();
        let mut timers = lease.timer_lease();
        match timer {
            Ieee802154Timer::Timer0 => {
                timers.set_timer0_threshold(PacTimer0ThresholdWord::new(microseconds));
            }
            Ieee802154Timer::Timer1 => {
                timers.set_timer1_threshold(PacTimer1ThresholdWord::new(microseconds));
            }
        }
    }

    fn start_timer(&mut self, timer: Ieee802154Timer) {
        let mut lease = self.lease();
        let mut timers = lease.timer_lease();
        match timer {
            Ieee802154Timer::Timer0 => timers.start_timer0(),
            Ieee802154Timer::Timer1 => timers.start_timer1(),
        }
    }

    fn stop_timer(&mut self, timer: Ieee802154Timer) {
        let mut lease = self.lease();
        let mut timers = lease.timer_lease();
        match timer {
            Ieee802154Timer::Timer0 => timers.stop_timer0(),
            Ieee802154Timer::Timer1 => timers.stop_timer1(),
        }
    }

    fn etm_channel_enabled(&mut self, channel: Ieee802154EtmChannel) -> bool {
        self.lease().etm_channel_enabled(channel)
    }

    fn disable_etm_channel(&mut self, channel: Ieee802154EtmChannel) {
        self.lease().disable_etm_channel(channel);
    }

    fn enable_etm_channel(&mut self, channel: Ieee802154EtmChannel) {
        self.lease().enable_etm_channel(channel);
    }

    fn set_etm_route(&mut self, route: Ieee802154EtmRoute) {
        self.lease().set_etm_route(route);
    }
}

#[cfg(test)]
pub(crate) mod tests;
