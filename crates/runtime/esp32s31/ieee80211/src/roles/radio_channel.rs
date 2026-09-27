//! Wi-Fi channel transactions leased from the shared radio.
//!
//! Every retune takes the arbiter lease for exactly one transaction, as the
//! vendor `phy_lock` scope does, so another radio client can use the shared
//! domain between the channels of one scan or hop sequence. A completed
//! retune records the channel for coexistence, as the vendor
//! `ieee80211_update_channel` calls `coex_wifi_channel_set`.

use oer_esp32s31_coex::CoexStatusType;
use oer_esp32s31_hal::{
    ieee80211::arena::RadioAccess, owner::RadioRuntimeOwner, shared_radio::PlatformClockProvider,
};
use oer_esp32s31_ieee80211::coex::{WifiCoexActivity, WifiCoexChannel, shared_scan_dwell_millis};
use oer_esp32s31_phy::{ConcurrentWifiChannelError, PhyAsyncDelay, PhyTargetObserver};
use oer_esp32s31_radio_runtime::{CoexWifiChannel, RadioGuard, RadioSystem};

use oer_esp32s31_ieee80211_sta::hardware::channel::ScanPhy;

/// A Wi-Fi role's channel authority on the shared radio.
pub struct RadioChannel<'radio, P, C, O, D> {
    radio: &'radio RadioSystem<P, C>,
    phy: ScanPhy<O, D>,
}

impl<'radio, P, C, O, D> RadioChannel<'radio, P, C, O, D>
where
    C: PlatformClockProvider,
    O: PhyTargetObserver,
    D: PhyAsyncDelay,
{
    pub const fn new(radio: &'radio RadioSystem<P, C>, observer: O) -> Self {
        Self {
            radio,
            phy: ScanPhy::new(observer),
        }
    }

    /// Retune while the caller still owns a cold, stopped MAC.
    pub async fn select_channel(
        &mut self,
        channel_or_frequency: u16,
        cbw: u8,
        owner: &mut RadioRuntimeOwner,
    ) -> Result<(), ConcurrentWifiChannelError> {
        let mut guard = self.radio.lock().await;
        let (lease, platform, _) = guard.parts();
        self.phy
            .select_channel(lease, platform, channel_or_frequency, cbw, owner)
            .await?;
        record_coex_channel(&mut guard, channel_or_frequency, cbw);
        Ok(())
    }

    /// Stop the MAC, retune and restore the qualified REGDMA link.
    pub async fn switch_channel(
        &mut self,
        channel_or_frequency: u16,
        cbw: u8,
        owner: &mut RadioRuntimeOwner,
    ) -> Result<(), ConcurrentWifiChannelError> {
        let mut guard = self.radio.lock().await;
        let (lease, platform, _) = guard.parts();
        self.phy
            .switch_channel(lease, platform, channel_or_frequency, cbw, owner)
            .await?;
        record_coex_channel(&mut guard, channel_or_frequency, cbw);
        Ok(())
    }

    /// Switch through the arena's serialized channel-only capability.
    pub async fn switch_published_channel(
        &mut self,
        channel_or_frequency: u16,
        cbw: u8,
        access: RadioAccess<'_>,
    ) -> Result<(), ConcurrentWifiChannelError> {
        let mut guard = self.radio.lock().await;
        let (lease, platform, _) = guard.parts();
        self.phy
            .switch_published_channel(lease, platform, channel_or_frequency, cbw, access)
            .await?;
        record_coex_channel(&mut guard, channel_or_frequency, cbw);
        Ok(())
    }

    /// Publish Wi-Fi's activity to the coexistence schedule, as the vendor
    /// `pm_on_coex_schm_status_config` does, and restart the phases after it
    /// while another radio shares the schedule, as its callers do.
    pub async fn publish_coex_activity(&self, activity: WifiCoexActivity) {
        publish_coex_activity(&mut self.radio.lock().await, activity);
    }

    /// Enter one scan channel for coexistence, as the vendor
    /// `scan_next_channel` does before it dwells: publish the scanning
    /// activity and, while another radio shares the schedule, replace the
    /// requested dwell with the schedule's scan dwell.
    pub async fn enter_scan_channel(&self, requested_dwell_millis: u16) -> u16 {
        let mut guard = self.radio.lock().await;
        publish_coex_activity(&mut guard, WifiCoexActivity::Scanning);
        if !guard.coex_active_for(CoexStatusType::Wifi) {
            return requested_dwell_millis;
        }
        let dwell = shared_scan_dwell_millis(
            u32::from(requested_dwell_millis),
            guard.coex_schedule().current_period(),
        );
        u16::try_from(dwell).unwrap_or(u16::MAX)
    }

    /// Return the observer once the role no longer retunes.
    pub fn into_observer(self) -> O {
        self.phy.into_observer()
    }
}

/// Record a completed retune for coexistence. Only a channel the PHY
/// accepted reaches this point, so an unrepresentable request is a caller
/// defect and records nothing.
fn record_coex_channel<P, C: PlatformClockProvider>(
    guard: &mut RadioGuard<'_, P, C>,
    channel_or_frequency: u16,
    cbw: u8,
) {
    if let Some(channel) = WifiCoexChannel::from_phy_request(channel_or_frequency, cbw) {
        guard.set_coex_wifi_channel(CoexWifiChannel {
            primary: channel.primary,
            secondary: channel.secondary,
        });
    }
}

/// Publish one Wi-Fi activity under an already-held arbiter lease, and
/// restart the phases when its caller in the vendor library does.
pub(crate) fn publish_coex_activity<P, C: PlatformClockProvider>(
    guard: &mut RadioGuard<'_, P, C>,
    activity: WifiCoexActivity,
) {
    let restart = apply_coex_status(guard, activity);
    if restart && guard.coex_active_for(CoexStatusType::Wifi) {
        guard.restart_coex_phases();
    }
}

/// Replace Wi-Fi's status and set the schedule interval, as the vendor
/// `pm_on_coex_schm_status_config` does, and report whether its callers
/// restart the phases after it.
pub(crate) fn apply_coex_status<P, C: PlatformClockProvider>(
    guard: &mut RadioGuard<'_, P, C>,
    activity: WifiCoexActivity,
) -> bool {
    let update = activity.status_update();
    guard.clear_coex_status_bits(CoexStatusType::Wifi, u16::MAX);
    guard.set_coex_status_bits(CoexStatusType::Wifi, update.status);
    guard.set_coex_interval(update.interval);
    update.restart_when_shared
}
