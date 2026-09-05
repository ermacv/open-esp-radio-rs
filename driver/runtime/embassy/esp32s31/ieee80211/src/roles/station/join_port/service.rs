#![expect(
    clippy::manual_async_fn,
    reason = "join test services implement the same explicit borrowed Future contracts"
)]

use core::future::Future;

use open_esp_radio_esp32s31_wifi_sta::{
    association::esp32s31_sta_association_profile,
    join::{
        Esp32s31StaJoinObserver, Esp32s31StaJoinPortError, Esp32s31StaJoinReceive,
        Esp32s31StaJoinTransmit,
    },
};
use open_esp_radio_ieee80211::station::{AssociationRequest, OpenAuthenticationRequest};
use open_esp_radio_wifi_sta::join::{StaJoinBackend, StaJoinRxObserver};
use open_esp_radio_wifi_sta::join::{
    association::StaAssociationAttempt, authentication::StaAuthenticationAttempt,
};

use super::Esp32s31StaJoinPort;

impl<H, R, T, O> StaJoinBackend for Esp32s31StaJoinPort<'_, '_, '_, H, R, T, O>
where
    R: Esp32s31StaJoinReceive<H>,
    T: Esp32s31StaJoinTransmit<H>,
    O: Esp32s31StaJoinObserver,
{
    type Error = Esp32s31StaJoinPortError<R::Error, T::Error>;

    fn start_receive(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        async {
            self.radio
                .receive
                .start(self.radio.hardware)
                .await
                .map_err(Esp32s31StaJoinPortError::Receive)
        }
    }

    fn stop_receive(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        async {
            self.radio
                .receive
                .stop(self.radio.hardware)
                .map_err(Esp32s31StaJoinPortError::Receive)
        }
    }

    fn transmit_open_authentication(
        &mut self,
        attempt: StaAuthenticationAttempt,
    ) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        async move {
            let completion = self
                .radio
                .transmit
                .transmit_open_authentication(
                    self.radio.hardware,
                    OpenAuthenticationRequest {
                        source: self.station.station_address,
                        bssid: self.station.access_point.bssid,
                        sequence_number: attempt.sequence_number,
                    },
                )
                .await
                .map_err(Esp32s31StaJoinPortError::Transmit)?;
            self.storage.observer.authentication_transmitted(completion);
            Ok(())
        }
    }

    fn transmit_association(
        &mut self,
        attempt: StaAssociationAttempt,
    ) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        async move {
            let profile = esp32s31_sta_association_profile(
                &self.station.access_point,
                self.station.association_preference,
                self.radio.transmit.power_profile(),
            )
            .map_err(Esp32s31StaJoinPortError::AssociationProfile)?;
            self.storage.observer.association_profile_selected(profile);
            let completion = self
                .radio
                .transmit
                .transmit_association(
                    self.radio.hardware,
                    AssociationRequest {
                        source: self.station.station_address,
                        access_point: &self.station.access_point,
                        sequence_number: attempt.sequence_number,
                        listen_interval: self.station.listen_interval,
                        phy: profile.phy,
                        security: self.station.security,
                        power_capability: profile.power_capability,
                        he_ul_mu_power: profile.he_ul_mu_power,
                    },
                )
                .await
                .map_err(Esp32s31StaJoinPortError::Transmit)?;
            self.storage.observer.association_transmitted(completion);
            Ok(())
        }
    }

    fn service_receive<'a, V>(
        &'a mut self,
        observer: &'a mut V,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a
    where
        V: StaJoinRxObserver + 'a,
    {
        async move {
            self.radio
                .receive
                .service_management(self.radio.hardware, self.storage.frame, observer)
                .map_err(Esp32s31StaJoinPortError::Receive)
        }
    }
}
