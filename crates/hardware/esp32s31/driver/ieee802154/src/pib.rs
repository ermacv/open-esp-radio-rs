//! Port of the public ESP-IDF IEEE 802.15.4 PAN information base
//! (`esp_ieee802154_pib.c`).
//!
//! The PIB holds the radio configuration the driver publishes lazily: every
//! setter that changes a value marks the PIB pending, and
//! [`Ieee802154Pib::update`] writes the complete set in the vendor order
//! before the next operation. Addresses, PAN identifiers and the ACK timeout
//! are not PIB values; the vendor driver writes them directly.

pub use oer_esp32s31_hal::ieee802154::Ieee802154MultipanIndex;
pub use oer_ieee802154::AutoPendingMode;

use oer_esp32s31_hal::ieee802154::{
    IEEE802154_MIN_CHANNEL, Ieee802154CcaMode, Ieee802154Channel, Ieee802154TxPowerLevels,
    ll::Ieee802154LowLevel,
};

const CHANNEL_COUNT: usize = 16;
const INTERFACE_COUNT: usize = Ieee802154MultipanIndex::COUNT as usize;

/// Build-time PIB defaults (`CONFIG_IEEE802154_CCA_MODE` and
/// `CONFIG_IEEE802154_CCA_THRESHOLD`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154PibDefaults {
    /// Initial clear-channel-assessment mode.
    pub cca_mode: Ieee802154CcaMode,
    /// Initial CCA energy threshold in dBm.
    pub cca_threshold_dbm: i8,
}

impl Default for Ieee802154PibDefaults {
    /// The Kconfig defaults: energy detection above -75 dBm.
    fn default() -> Self {
        Self {
            cca_mode: Ieee802154CcaMode::EnergyDetection,
            cca_threshold_dbm: -75,
        }
    }
}

/// The driver's PAN information base.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Ieee802154Pib {
    auto_ack_tx: bool,
    auto_ack_rx: bool,
    enhanced_ack_tx: bool,
    promiscuous: bool,
    coordinator: bool,
    rx_when_idle: bool,
    power_dbm: [i8; CHANNEL_COUNT],
    channel: Ieee802154Channel,
    pending_mode: [AutoPendingMode; INTERFACE_COUNT],
    cca_threshold_dbm: i8,
    cca_mode: Ieee802154CcaMode,
    pending: bool,
}

const fn channel_slot(channel: Ieee802154Channel) -> usize {
    (channel.number() - IEEE802154_MIN_CHANNEL) as usize
}

impl Ieee802154Pib {
    /// `ieee802154_pib_init`: auto-ACK in both directions with enhanced ACK
    /// timing, promiscuous, not a coordinator, receiver off when idle,
    /// channel 11, every channel at the provider's highest power, and a
    /// pending update.
    pub fn new(defaults: Ieee802154PibDefaults, levels: Ieee802154TxPowerLevels<'_>) -> Self {
        Self {
            auto_ack_tx: true,
            auto_ack_rx: true,
            enhanced_ack_tx: true,
            promiscuous: true,
            coordinator: false,
            rx_when_idle: false,
            power_dbm: [levels.highest_dbm(); CHANNEL_COUNT],
            channel: match Ieee802154Channel::new(11) {
                Ok(channel) => channel,
                Err(_) => unreachable!(),
            },
            pending_mode: [AutoPendingMode::Disable; INTERFACE_COUNT],
            cca_threshold_dbm: defaults.cca_threshold_dbm,
            cca_mode: defaults.cca_mode,
            pending: true,
        }
    }

    /// `ieee802154_pib_is_pending`.
    pub const fn is_pending(&self) -> bool {
        self.pending
    }

    /// `ieee802154_pib_update`: when pending, publish the channel, the
    /// current channel's power, CCA, ACK, role and pending-mode settings in
    /// the vendor order, then clear the pending mark.
    pub fn update<Ll: Ieee802154LowLevel + ?Sized>(
        &mut self,
        ll: &mut Ll,
        levels: Ieee802154TxPowerLevels<'_>,
    ) {
        if !self.pending {
            return;
        }
        ll.set_channel(self.channel);
        ll.set_tx_power(&levels.resolve(self.channel, self.power()));
        ll.set_cca_mode(self.cca_mode);
        ll.set_cca_threshold(self.cca_threshold_dbm);
        ll.set_tx_auto_ack(self.auto_ack_tx);
        ll.set_rx_auto_ack(self.auto_ack_rx);
        ll.set_tx_enhanced_ack(self.enhanced_ack_tx);
        ll.set_coordinator(self.coordinator);
        ll.set_promiscuous(self.promiscuous);
        ll.set_pending_mode(
            self.pending_mode
                .iter()
                .any(|mode| mode.selects_enhanced_lookup()),
        );
        self.pending = false;
    }

    fn replace<T: PartialEq>(pending: &mut bool, slot: &mut T, value: T) {
        if *slot != value {
            *slot = value;
            *pending = true;
        }
    }

    /// `ieee802154_pib_get_channel`.
    pub const fn channel(&self) -> Ieee802154Channel {
        self.channel
    }

    /// `ieee802154_pib_set_channel`.
    pub fn set_channel(&mut self, channel: Ieee802154Channel) {
        Self::replace(&mut self.pending, &mut self.channel, channel);
    }

    /// `ieee802154_pib_get_power`: the requested power of the current channel.
    pub const fn power(&self) -> i8 {
        self.power_for_channel(self.channel)
    }

    /// `ieee802154_pib_set_power`: set the current channel's requested power.
    pub fn set_power(&mut self, power_dbm: i8) {
        self.set_power_for_channel(self.channel, power_dbm);
    }

    /// `ieee802154_pib_get_power_with_channel`.
    pub const fn power_for_channel(&self, channel: Ieee802154Channel) -> i8 {
        self.power_dbm[channel_slot(channel)]
    }

    /// `ieee802154_pib_set_power_with_channel`.
    pub fn set_power_for_channel(&mut self, channel: Ieee802154Channel, power_dbm: i8) {
        Self::replace(
            &mut self.pending,
            &mut self.power_dbm[channel_slot(channel)],
            power_dbm,
        );
    }

    /// `ieee802154_pib_get_power_table`: requested power of channels 11
    /// through 26.
    pub const fn power_table(&self) -> [i8; CHANNEL_COUNT] {
        self.power_dbm
    }

    /// `ieee802154_pib_set_power_table`.
    pub fn set_power_table(&mut self, power_dbm: [i8; CHANNEL_COUNT]) {
        Self::replace(&mut self.pending, &mut self.power_dbm, power_dbm);
    }

    /// `ieee802154_pib_get_promiscuous`.
    pub const fn promiscuous(&self) -> bool {
        self.promiscuous
    }

    /// `ieee802154_pib_set_promiscuous`.
    pub fn set_promiscuous(&mut self, enable: bool) {
        Self::replace(&mut self.pending, &mut self.promiscuous, enable);
    }

    /// `ieee802154_pib_get_cca_threshold`.
    pub const fn cca_threshold(&self) -> i8 {
        self.cca_threshold_dbm
    }

    /// `ieee802154_pib_set_cca_threshold`.
    pub fn set_cca_threshold(&mut self, threshold_dbm: i8) {
        Self::replace(
            &mut self.pending,
            &mut self.cca_threshold_dbm,
            threshold_dbm,
        );
    }

    /// `ieee802154_pib_get_cca_mode`.
    pub const fn cca_mode(&self) -> Ieee802154CcaMode {
        self.cca_mode
    }

    /// `ieee802154_pib_set_cca_mode`.
    pub fn set_cca_mode(&mut self, mode: Ieee802154CcaMode) {
        Self::replace(&mut self.pending, &mut self.cca_mode, mode);
    }

    /// `ieee802154_pib_get_auto_ack_tx`.
    pub const fn auto_ack_tx(&self) -> bool {
        self.auto_ack_tx
    }

    /// `ieee802154_pib_set_auto_ack_tx`.
    pub fn set_auto_ack_tx(&mut self, enable: bool) {
        Self::replace(&mut self.pending, &mut self.auto_ack_tx, enable);
    }

    /// `ieee802154_pib_get_auto_ack_rx`.
    pub const fn auto_ack_rx(&self) -> bool {
        self.auto_ack_rx
    }

    /// `ieee802154_pib_set_auto_ack_rx`.
    pub fn set_auto_ack_rx(&mut self, enable: bool) {
        Self::replace(&mut self.pending, &mut self.auto_ack_rx, enable);
    }

    /// `ieee802154_pib_get_enhance_ack_tx`.
    pub const fn enhanced_ack_tx(&self) -> bool {
        self.enhanced_ack_tx
    }

    /// `ieee802154_pib_set_enhance_ack_tx`.
    pub fn set_enhanced_ack_tx(&mut self, enable: bool) {
        Self::replace(&mut self.pending, &mut self.enhanced_ack_tx, enable);
    }

    /// `ieee802154_pib_get_coordinator`.
    pub const fn coordinator(&self) -> bool {
        self.coordinator
    }

    /// `ieee802154_pib_set_coordinator`.
    pub fn set_coordinator(&mut self, enable: bool) {
        Self::replace(&mut self.pending, &mut self.coordinator, enable);
    }

    /// `ieee802154_pib_get_pending_mode`.
    pub const fn pending_mode(&self, interface: Ieee802154MultipanIndex) -> AutoPendingMode {
        self.pending_mode[interface.value() as usize]
    }

    /// `ieee802154_pib_set_pending_mode`.
    pub fn set_pending_mode(&mut self, interface: Ieee802154MultipanIndex, mode: AutoPendingMode) {
        Self::replace(
            &mut self.pending,
            &mut self.pending_mode[interface.value() as usize],
            mode,
        );
    }

    /// `ieee802154_pib_get_rx_when_idle`.
    pub const fn rx_when_idle(&self) -> bool {
        self.rx_when_idle
    }

    /// `ieee802154_pib_set_rx_when_idle`: a driver-only value, so it never
    /// marks the PIB pending.
    pub fn set_rx_when_idle(&mut self, enable: bool) {
        self.rx_when_idle = enable;
    }
}

#[cfg(test)]
mod tests;
