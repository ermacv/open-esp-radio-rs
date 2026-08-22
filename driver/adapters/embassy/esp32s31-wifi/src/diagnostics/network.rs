//! Role-neutral network publication observations.

#[cfg(feature = "diagnostics")]
use open_esp_radio_embassy_net::RxEnqueueError;
#[cfg(feature = "diagnostics")]
use open_esp_radio_ieee80211::data::EthernetFrameParts;

/// Diagnostic observation of one exact network admission decision.
#[cfg(feature = "diagnostics")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxNetworkDeliveryEvent<'frame> {
    pub frame: EthernetFrameParts<'frame>,
    pub raw: Option<&'frame [u8]>,
}

#[cfg(feature = "diagnostics")]
pub trait RxNetworkDeliveryObserver: Sync {
    fn admitted(&self, event: RxNetworkDeliveryEvent<'_>);

    fn dropped(&self, event: RxNetworkDeliveryEvent<'_>, error: RxEnqueueError);
}
