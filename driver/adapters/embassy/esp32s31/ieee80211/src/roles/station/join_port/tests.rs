use core::future::{Future, ready};

use open_esp_radio_esp32s31_wifi_sta::join::{
    Esp32s31StaJoinObserver, Esp32s31StaJoinReceive, Esp32s31StaJoinTransmit,
};
use open_esp_radio_ieee80211::{
    scan::ScanRecord,
    station::{
        AssociationRequest, OpenAuthenticationRequest, StaAssociationPhy, StaAssociationPreference,
    },
};
use open_esp_radio_wifi_sta::join::{StaJoinBackend, StaJoinRxDirective, StaJoinRxObserver};
use open_esp_radio_wifi_sta::join::{
    association::StaAssociationAttempt, authentication::StaAuthenticationAttempt,
};

use std::vec::Vec;

use open_esp_radio_esp32s31_wifi::ordinary_tx::{WifiTxPowerPair, WifiTxPowerProfile};
use open_esp_radio_esp32s31_wifi_mac::tx::{TxCompletion, TxCookie};
use open_esp_radio_esp32s31_wifi_sta::association::Esp32s31StaAssociationProfile;

use super::*;

const LOCAL: [u8; 6] = [2, 0, 0, 0, 0, 1];
const BSSID: [u8; 6] = [2, 0, 0, 0, 0, 2];
const HE20_MCS9_CAPABILITY: [u8; 24] = [
    255, 22, 35, 0x03, 0x18, 0x9c, 0xca, 0x10, 0x80, 0x00, 0x10, 0x8a, 0x1b, 0x0d, 0xc0, 0x1f,
    0x00, 0x02, 0x82, 0x01, 0xfd, 0xff, 0xfd, 0xff,
];

const fn completion() -> TxCompletion {
    TxCompletion::new_model(TxCookie(1), 0, 0)
}

#[derive(Clone, Copy)]
struct Power;

impl WifiTxPowerProfile for Power {
    fn power_pair(&self, rate_code: u8) -> WifiTxPowerPair {
        let primary = [20, 20, 20, 19, 19, 18, 18, 16, 15, 20]
            [usize::from(rate_code.saturating_sub(16).min(9))];
        WifiTxPowerPair {
            primary,
            alternate: primary,
        }
    }
}

fn he_access_point() -> ScanRecord {
    let mut access_point = ScanRecord {
        bssid: BSSID,
        channel: 6,
        ..ScanRecord::EMPTY
    };
    access_point.he_capability_ie[..HE20_MCS9_CAPABILITY.len()]
        .copy_from_slice(&HE20_MCS9_CAPABILITY);
    access_point.he_capability_ie_len = HE20_MCS9_CAPABILITY.len() as u8;
    let operation = [255, 7, 36, 0, 0, 0, 1, 0xfd, 0xff];
    access_point.he_operation_ie[..operation.len()].copy_from_slice(&operation);
    access_point.he_operation_ie_len = operation.len() as u8;
    access_point
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Action {
    Start,
    Authentication(u16),
    Association(u16, u16, StaAssociationPhy),
    Service,
    Stop,
    AuthenticationObserved,
    AssociationProfileObserved,
    AssociationObserved,
}

#[derive(Default)]
struct Hardware {
    actions: Vec<Action>,
}

struct Receive;

impl Esp32s31StaJoinReceive<Hardware> for Receive {
    type Error = ();

    fn start<'a>(
        &'a mut self,
        hardware: &'a mut Hardware,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a {
        hardware.actions.push(Action::Start);
        ready(Ok(()))
    }

    fn stop(&mut self, hardware: &mut Hardware) -> Result<(), Self::Error> {
        hardware.actions.push(Action::Stop);
        Ok(())
    }

    fn service_management<O>(
        &mut self,
        hardware: &mut Hardware,
        _frame: &mut [u8],
        observer: &mut O,
    ) -> Result<(), Self::Error>
    where
        O: StaJoinRxObserver,
    {
        hardware.actions.push(Action::Service);
        let _ = observer.observe_completed(None);
        Ok(())
    }
}

struct Transmit;

impl Esp32s31StaJoinTransmit<Hardware> for Transmit {
    type Error = ();
    type PowerProfile = Power;

    fn power_profile(&self) -> &Self::PowerProfile {
        &Power
    }

    fn transmit_open_authentication<'a>(
        &'a mut self,
        hardware: &'a mut Hardware,
        request: OpenAuthenticationRequest,
    ) -> impl Future<Output = Result<TxCompletion, Self::Error>> + 'a {
        hardware
            .actions
            .push(Action::Authentication(request.sequence_number));
        ready(Ok(completion()))
    }

    fn transmit_association<'a>(
        &'a mut self,
        hardware: &'a mut Hardware,
        request: AssociationRequest<'a>,
    ) -> impl Future<Output = Result<TxCompletion, Self::Error>> + 'a {
        hardware.actions.push(Action::Association(
            request.sequence_number,
            request.listen_interval,
            request.phy,
        ));
        ready(Ok(completion()))
    }
}

struct Observer<'a>(&'a mut Hardware);

impl Esp32s31StaJoinObserver for Observer<'_> {
    fn authentication_transmitted(&mut self, _completion: TxCompletion) {
        self.0.actions.push(Action::AuthenticationObserved);
    }

    fn association_profile_selected(&mut self, _profile: Esp32s31StaAssociationProfile) {
        self.0.actions.push(Action::AssociationProfileObserved);
    }

    fn association_transmitted(&mut self, _completion: TxCompletion) {
        self.0.actions.push(Action::AssociationObserved);
    }
}

struct Ignore;

impl StaJoinRxObserver for Ignore {
    fn observe_completed(&mut self, _management_frame: Option<&[u8]>) -> StaJoinRxDirective {
        StaJoinRxDirective::Continue
    }
}

#[test]
fn port_orders_driver_edges_and_keeps_diagnostics_external() {
    let mut hardware = Hardware::default();
    let mut diagnostic_hardware = Hardware::default();
    let mut transmit = Transmit;
    let mut frame = [0; 128];
    let radio = Esp32s31StaJoinRadio::new(&mut hardware, Receive, &mut transmit);
    let storage = Esp32s31StaJoinStorage::new(&mut frame, Observer(&mut diagnostic_hardware));
    let station = Esp32s31StaJoinStation::new(
        LOCAL,
        he_access_point(),
        StaAssociationPreference::PreferHe20,
    );
    let mut port = Esp32s31StaJoinPort::new(radio, storage, station);

    embassy_futures::block_on(async {
        port.start_receive().await.unwrap();
        port.transmit_open_authentication(StaAuthenticationAttempt {
            ordinal: 1,
            sequence_number: 7,
            response_timeout_ms: 1_000,
        })
        .await
        .unwrap();
        port.transmit_association(StaAssociationAttempt {
            ordinal: 1,
            sequence_number: 8,
            elapsed_ms: 0,
        })
        .await
        .unwrap();
        port.service_receive(&mut Ignore).await.unwrap();
        port.stop_receive().await.unwrap();
    });

    assert_eq!(
        hardware.actions,
        [
            Action::Start,
            Action::Authentication(7),
            Action::Association(8, 3, StaAssociationPhy::He20),
            Action::Service,
            Action::Stop,
        ]
    );
    assert_eq!(
        diagnostic_hardware.actions,
        [
            Action::AuthenticationObserved,
            Action::AssociationProfileObserved,
            Action::AssociationObserved,
        ]
    );
}
