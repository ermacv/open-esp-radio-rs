//! Finite ESP32-S31 IEEE 802.15.4 clock, reset and foundation sequence.
//!
//! This module stops before PHY/RF ownership, interrupt routing, DMA buffers or
//! an operational MAC state. The backend must be constructed from the existing
//! whole-radio owner; it is not a second peripheral singleton and exposes no
//! raw register or address operations.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), allow(dead_code))]

use core::marker::PhantomData;

use open_esp_radio_esp32s31_pac::{
    Ieee802154FoundationSnapshot, Ieee802154FrequencyCode, Ieee802154Pti,
};

/// PTI value used by the public vendor LL when coexistence is disabled.
pub(crate) const COEX_DISABLED_PTI: u8 = 3;

/// Lowest IEEE 802.15.4 channel supported by the 2.4 GHz PHY.
pub const IEEE802154_MIN_CHANNEL: u8 = 11;

/// Highest IEEE 802.15.4 channel supported by the 2.4 GHz PHY.
pub const IEEE802154_MAX_CHANNEL: u8 = 26;

/// One checked IEEE 802.15.4 2.4 GHz channel.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ieee802154Channel(u8);

/// An integer outside the IEEE 802.15.4 2.4 GHz channel range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154ChannelError {
    attempted: u8,
}

impl Ieee802154ChannelError {
    /// Return the rejected channel number.
    pub const fn attempted(self) -> u8 {
        self.attempted
    }
}

impl Ieee802154Channel {
    /// Check and construct one channel in the inclusive range 11 through 26.
    pub const fn new(channel: u8) -> Result<Self, Ieee802154ChannelError> {
        if channel >= IEEE802154_MIN_CHANNEL && channel <= IEEE802154_MAX_CHANNEL {
            Ok(Self(channel))
        } else {
            Err(Ieee802154ChannelError { attempted: channel })
        }
    }

    /// Return the standardized channel number.
    pub const fn number(self) -> u8 {
        self.0
    }

    /// Map a checked channel to the ESP32-S31 MAC frequency-code field.
    ///
    /// The pinned public vendor utility maps channels 11 through 26 to codes
    /// 3 through 78 with `(channel - 11) * 5 + 3`.
    pub const fn frequency_code(self) -> Ieee802154FrequencyCode {
        Ieee802154FrequencyCode::new((self.0 - IEEE802154_MIN_CHANNEL) * 5 + 3)
    }
}

impl TryFrom<u8> for Ieee802154Channel {
    type Error = Ieee802154ChannelError;

    fn try_from(channel: u8) -> Result<Self, Self::Error> {
        Self::new(channel)
    }
}

/// Closed semantic backend for the finite lifecycle sequence.
///
/// An implementation must retain exclusive access to the existing complete
/// radio owner for the lifetime of the returned typestate value. In
/// particular, implementations must not reconstruct the IEEE 802.15.4 block
/// from an address or independently claim a second peripheral singleton.
pub(crate) trait Ieee802154LifecycleBackend {
    fn configure_modem_clock_maps(&mut self);
    fn configure_modem_source_clock(&mut self);
    fn enable_wifi_bb_80x1_clock(&mut self);
    fn enable_etm_clock(&mut self);
    fn enable_bt_apb_clocks(&mut self);
    fn enable_bt_ieee802154_common_baseband_clock(&mut self);
    fn enable_ieee802154_mac_clocks(&mut self);
    fn set_ieee802154_mac_reset(&mut self, asserted: bool);
    fn set_ieee802154_apb_reset(&mut self, asserted: bool);
    fn ieee802154_reset_readback(&self) -> Ieee802154ResetReadback;
    /// Retain the route-owned MODEM_LPCON coexistence clock.
    fn enable_coexistence_clock(&mut self);

    /// Join platform and route-owned clock observations.
    fn ieee802154_clock_readback(&self) -> Ieee802154ClockReadback;

    /// Prevent every peripheral event from reaching the future MAC IRQ route.
    fn mask_all_events(&mut self);

    /// Keep every RX-abort source masked until the receive dataplane exists.
    fn mask_all_rx_aborts(&mut self);

    /// Keep every TX-abort source masked until the transmit dataplane exists.
    fn mask_all_tx_aborts(&mut self);

    fn select_average_ed_sampling(&mut self);
    fn set_txrx_pti(&mut self, pti: Ieee802154Pti);
    fn set_ack_pti(&mut self, pti: Ieee802154Pti);

    /// Order completed foundation writes before publishing the next typestate.
    fn order_device_accesses(&mut self);

    /// Sample the safe, non-operational foundation image.
    fn foundation_snapshot(&mut self) -> Ieee802154FoundationSnapshot;
}

/// Semantic readback of the complete IEEE 802.15.4 module dependency set.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Ieee802154ClockReadback {
    pub modem_clock_maps_configured: bool,
    pub pll_160m_clock_enabled: bool,
    pub modem_source_clock_configured: bool,
    pub coexistence_clock_enabled: bool,
    pub wifi_bb_80x1_clock_enabled: bool,
    pub etm_clock_enabled: bool,
    pub bt_apb_clock_enabled: bool,
    pub modem_security_apb_clock_enabled: bool,
    pub bt_ieee802154_common_baseband_clock_enabled: bool,
    pub ieee802154_apb_clock_enabled: bool,
    pub ieee802154_mac_clock_enabled: bool,
}

/// Semantic readback after the two private reset pulses.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Ieee802154ResetReadback {
    pub mac_reset_released: bool,
    pub apb_reset_released: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154ClockCheckpoint {
    ModemClockMaps,
    Pll160mClock,
    ModemSourceClock,
    CoexistenceClock,
    WifiBb80x1Clock,
    EtmClock,
    BtApbClock,
    ModemSecurityApbClock,
    BtIeee802154CommonBasebandClock,
    Ieee802154ApbClock,
    Ieee802154MacClock,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154ResetCheckpoint {
    MacResetReleased,
    ApbResetReleased,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154FoundationCheckpoint {
    EventsMasked,
    RxAbortsMasked,
    TxAbortsMasked,
    EdSampleAverage,
    TxrxPtiDisabled,
    AckPtiDisabled,
}

/// One failed semantic readback. Register images never escape the backend.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154ReadbackError<Checkpoint> {
    pub checkpoint: Checkpoint,
    pub expected: bool,
    pub observed: bool,
}

pub(crate) mod state {
    /// All module clock dependencies have passed semantic readback.
    ///
    /// Shared-clock release authority is not implied.
    #[derive(Debug)]
    pub(crate) struct Clocked;

    /// Both private MAC reset lines have been pulsed and released.
    #[derive(Debug)]
    pub(crate) struct Reset;

    /// Static MAC foundation is configured with every event still masked.
    #[derive(Debug)]
    pub(crate) struct FoundationConfigured;
}

/// Exclusive whole-radio backend at one proved IEEE 802.15.4 phase.
#[derive(Debug)]
pub(crate) struct Ieee802154Lifecycle<Backend, State> {
    backend: Backend,
    _state: PhantomData<State>,
}

impl<Backend, State> Ieee802154Lifecycle<Backend, State> {
    pub(crate) const fn backend(&self) -> &Backend {
        &self.backend
    }

    /// Mutably borrow the retained whole-owner backend inside a later closed
    /// semantic transition. This does not expose it outside the crate.
    pub(crate) fn backend_mut(&mut self) -> &mut Backend {
        &mut self.backend
    }

    pub(crate) fn into_backend(self) -> Backend {
        self.backend
    }
}

impl<Backend> Ieee802154Lifecycle<Backend, state::FoundationConfigured> {
    /// Forget a disproved foundation while retaining the last independent
    /// reset proof and the exact whole-radio backend.
    pub(crate) fn forget_foundation(self) -> Ieee802154Lifecycle<Backend, state::Reset> {
        Ieee802154Lifecycle {
            backend: self.backend,
            _state: PhantomData,
        }
    }
}

/// Failed clock transition which returns the complete backend unchanged.
#[derive(Debug)]
pub(crate) struct Ieee802154ClockFailure<Backend> {
    backend: Backend,
    error: Ieee802154ReadbackError<Ieee802154ClockCheckpoint>,
}

impl<Backend> Ieee802154ClockFailure<Backend> {
    pub(crate) const fn error(&self) -> Ieee802154ReadbackError<Ieee802154ClockCheckpoint> {
        self.error
    }

    pub(crate) fn into_backend(self) -> Backend {
        self.backend
    }
}

/// Failed reset transition which remains at the last proved `Clocked` phase.
#[derive(Debug)]
pub(crate) struct Ieee802154ResetFailure<Backend> {
    lifecycle: Ieee802154Lifecycle<Backend, state::Clocked>,
    error: Ieee802154ReadbackError<Ieee802154ResetCheckpoint>,
}

impl<Backend> Ieee802154ResetFailure<Backend> {
    pub(crate) const fn error(&self) -> Ieee802154ReadbackError<Ieee802154ResetCheckpoint> {
        self.error
    }

    pub(crate) fn into_lifecycle(self) -> Ieee802154Lifecycle<Backend, state::Clocked> {
        self.lifecycle
    }
}

/// Failed foundation transition which remains at the last proved `Reset` phase.
#[derive(Debug)]
pub(crate) struct Ieee802154FoundationFailure<Backend> {
    lifecycle: Ieee802154Lifecycle<Backend, state::Reset>,
    error: Ieee802154ReadbackError<Ieee802154FoundationCheckpoint>,
}

impl<Backend> Ieee802154FoundationFailure<Backend> {
    pub(crate) const fn error(&self) -> Ieee802154ReadbackError<Ieee802154FoundationCheckpoint> {
        self.error
    }

    pub(crate) fn into_lifecycle(self) -> Ieee802154Lifecycle<Backend, state::Reset> {
        self.lifecycle
    }
}

/// Establish the complete vendor module clock dependency set in its reviewed
/// low-bit-to-high-bit enable order.
pub(crate) fn establish_ieee802154_clocks<Backend>(
    mut backend: Backend,
) -> Result<Ieee802154Lifecycle<Backend, state::Clocked>, Ieee802154ClockFailure<Backend>>
where
    Backend: Ieee802154LifecycleBackend,
{
    backend.configure_modem_clock_maps();
    backend.configure_modem_source_clock();
    backend.enable_coexistence_clock();
    backend.enable_wifi_bb_80x1_clock();
    backend.enable_etm_clock();
    backend.enable_bt_apb_clocks();
    backend.enable_bt_ieee802154_common_baseband_clock();
    backend.enable_ieee802154_mac_clocks();

    if let Err(error) = verify_clock_readback(backend.ieee802154_clock_readback()) {
        return Err(Ieee802154ClockFailure { backend, error });
    }

    Ok(Ieee802154Lifecycle {
        backend,
        _state: PhantomData,
    })
}

impl<Backend> Ieee802154Lifecycle<Backend, state::Clocked>
where
    Backend: Ieee802154LifecycleBackend,
{
    /// Pulse ZBMAC first and ZBMAC APB second, preserving all unrelated resets.
    pub(crate) fn reset_mac(
        mut self,
    ) -> Result<Ieee802154Lifecycle<Backend, state::Reset>, Ieee802154ResetFailure<Backend>> {
        self.backend.set_ieee802154_mac_reset(true);
        self.backend.set_ieee802154_mac_reset(false);
        self.backend.set_ieee802154_apb_reset(true);
        self.backend.set_ieee802154_apb_reset(false);

        if let Err(error) = verify_reset_readback(self.backend.ieee802154_reset_readback()) {
            return Err(Ieee802154ResetFailure {
                lifecycle: self,
                error,
            });
        }

        Ok(Ieee802154Lifecycle {
            backend: self.backend,
            _state: PhantomData,
        })
    }
}

impl<Backend> Ieee802154Lifecycle<Backend, state::Reset>
where
    Backend: Ieee802154LifecycleBackend,
{
    /// Configure only the static, non-operational MAC foundation.
    ///
    /// Event and abort delivery remains masked throughout. `EVENT_STATUS` is
    /// deliberately untouched because foundation setup owns no pending-event
    /// acknowledgement; W1C acknowledgement belongs to the later polled or
    /// hard-IRQ owner. This transition does not claim PHY, RF, IRQ routing,
    /// buffers or an idle/ready hardware state.
    pub(crate) fn configure_foundation(
        mut self,
    ) -> Result<
        Ieee802154Lifecycle<Backend, state::FoundationConfigured>,
        Ieee802154FoundationFailure<Backend>,
    > {
        self.backend.mask_all_events();
        self.backend.mask_all_rx_aborts();
        self.backend.mask_all_tx_aborts();
        self.backend.select_average_ed_sampling();
        let disabled_pti = Ieee802154Pti::new(COEX_DISABLED_PTI)
            .expect("reviewed coexistence-disabled PTI fits five bits");
        self.backend.set_txrx_pti(disabled_pti);
        self.backend.set_ack_pti(disabled_pti);
        self.backend.order_device_accesses();

        if let Err(error) = verify_foundation_snapshot(self.backend.foundation_snapshot()) {
            return Err(Ieee802154FoundationFailure {
                lifecycle: self,
                error,
            });
        }

        Ok(Ieee802154Lifecycle {
            backend: self.backend,
            _state: PhantomData,
        })
    }
}

fn verify_clock_readback(
    readback: Ieee802154ClockReadback,
) -> Result<(), Ieee802154ReadbackError<Ieee802154ClockCheckpoint>> {
    verify(
        Ieee802154ClockCheckpoint::ModemClockMaps,
        readback.modem_clock_maps_configured,
    )?;
    verify(
        Ieee802154ClockCheckpoint::Pll160mClock,
        readback.pll_160m_clock_enabled,
    )?;
    verify(
        Ieee802154ClockCheckpoint::ModemSourceClock,
        readback.modem_source_clock_configured,
    )?;
    verify(
        Ieee802154ClockCheckpoint::CoexistenceClock,
        readback.coexistence_clock_enabled,
    )?;
    verify(
        Ieee802154ClockCheckpoint::WifiBb80x1Clock,
        readback.wifi_bb_80x1_clock_enabled,
    )?;
    verify(
        Ieee802154ClockCheckpoint::EtmClock,
        readback.etm_clock_enabled,
    )?;
    verify(
        Ieee802154ClockCheckpoint::BtApbClock,
        readback.bt_apb_clock_enabled,
    )?;
    verify(
        Ieee802154ClockCheckpoint::ModemSecurityApbClock,
        readback.modem_security_apb_clock_enabled,
    )?;
    verify(
        Ieee802154ClockCheckpoint::BtIeee802154CommonBasebandClock,
        readback.bt_ieee802154_common_baseband_clock_enabled,
    )?;
    verify(
        Ieee802154ClockCheckpoint::Ieee802154ApbClock,
        readback.ieee802154_apb_clock_enabled,
    )?;
    verify(
        Ieee802154ClockCheckpoint::Ieee802154MacClock,
        readback.ieee802154_mac_clock_enabled,
    )
}

fn verify_reset_readback(
    readback: Ieee802154ResetReadback,
) -> Result<(), Ieee802154ReadbackError<Ieee802154ResetCheckpoint>> {
    verify(
        Ieee802154ResetCheckpoint::MacResetReleased,
        readback.mac_reset_released,
    )?;
    verify(
        Ieee802154ResetCheckpoint::ApbResetReleased,
        readback.apb_reset_released,
    )
}

fn verify_foundation_snapshot(
    snapshot: Ieee802154FoundationSnapshot,
) -> Result<(), Ieee802154ReadbackError<Ieee802154FoundationCheckpoint>> {
    verify(
        Ieee802154FoundationCheckpoint::EventsMasked,
        snapshot.events_masked(),
    )?;
    verify(
        Ieee802154FoundationCheckpoint::RxAbortsMasked,
        snapshot.rx_aborts_masked(),
    )?;
    verify(
        Ieee802154FoundationCheckpoint::TxAbortsMasked,
        snapshot.tx_aborts_masked(),
    )?;
    verify(
        Ieee802154FoundationCheckpoint::EdSampleAverage,
        snapshot.ed_uses_average(),
    )?;
    verify(
        Ieee802154FoundationCheckpoint::TxrxPtiDisabled,
        snapshot.txrx_pti().value() == COEX_DISABLED_PTI,
    )?;
    verify(
        Ieee802154FoundationCheckpoint::AckPtiDisabled,
        snapshot.ack_pti().value() == COEX_DISABLED_PTI,
    )
}

fn verify<Checkpoint: Copy>(
    checkpoint: Checkpoint,
    observed: bool,
) -> Result<(), Ieee802154ReadbackError<Checkpoint>> {
    if observed {
        Ok(())
    } else {
        Err(Ieee802154ReadbackError {
            checkpoint,
            expected: true,
            observed,
        })
    }
}

#[cfg(test)]
mod tests;
