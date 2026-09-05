//! ESP32-S31 semantic ports for pre-connected STA join.
//!
//! Portable Authentication/Association retry and deadline policy lives in
//! `open-esp-radio-wifi-sta::join`. These interfaces describe the narrower
//! chip operations needed to bind that policy to ESP32-S31. They deliberately
//! contain no DMA owner, executor timer, task primitive or board fixture.

use core::future::Future;

use open_esp_radio_esp32s31_wifi_mac::tx::TxCompletion;
use open_esp_radio_ieee80211::station::{AssociationRequest, OpenAuthenticationRequest};

use crate::association::{Esp32s31StaAssociationProfile, Esp32s31StaAssociationProfileError};
use open_esp_radio_esp32s31_wifi::tx::WifiTxPowerProfile;

/// RX capability consumed by a concrete ESP32-S31 join adapter.
pub trait Esp32s31StaJoinReceive<H> {
    type Error;

    fn start<'a>(
        &'a mut self,
        hardware: &'a mut H,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a;

    fn stop(&mut self, hardware: &mut H) -> Result<(), Self::Error>;

    fn service_management<O>(
        &mut self,
        hardware: &mut H,
        frame: &mut [u8],
        observer: &mut O,
    ) -> Result<(), Self::Error>
    where
        O: open_esp_radio_wifi_sta::join::StaJoinRxObserver;
}

/// Control-TX capability consumed by a concrete ESP32-S31 join adapter.
pub trait Esp32s31StaJoinTransmit<H> {
    type Error;
    type PowerProfile: WifiTxPowerProfile;

    fn power_profile(&self) -> &Self::PowerProfile;

    fn transmit_open_authentication<'a>(
        &'a mut self,
        hardware: &'a mut H,
        request: OpenAuthenticationRequest,
    ) -> impl Future<Output = Result<TxCompletion, Self::Error>> + 'a;

    fn transmit_association<'a>(
        &'a mut self,
        hardware: &'a mut H,
        request: AssociationRequest<'a>,
    ) -> impl Future<Output = Result<TxCompletion, Self::Error>> + 'a;
}

/// Value-only observation points which do not participate in driver policy.
///
/// Callbacks run after the matching TX transaction has completed. Production
/// composition normally uses `()`; applications and HIL may retain bounded
/// evidence without wrapping the hardware operation.
pub trait Esp32s31StaJoinObserver {
    fn authentication_transmitted(&mut self, _completion: TxCompletion) {}

    fn association_profile_selected(&mut self, _profile: Esp32s31StaAssociationProfile) {}

    fn association_transmitted(&mut self, _completion: TxCompletion) {}
}

impl Esp32s31StaJoinObserver for () {}

/// Exact primitive edge which failed inside a concrete join adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31StaJoinPortError<R, T> {
    Receive(R),
    AssociationProfile(Esp32s31StaAssociationProfileError),
    Transmit(T),
}
