//! Concrete ESP32-S31 port for pre-connected STA Authentication/Association.
//!
//! [`StaJoinRunner`](crate::sta_join::StaJoinRunner) owns protocol retries and
//! deadlines. This module owns the chip-specific DMA/parser/TX composition:
//! the same RX frontier is retained across phases, completed descriptors are
//! reduced to management MPDUs, and Association is built from the selected
//! PHY plus the calibrated transmit-power profile. Board fixtures provide
//! only coherent owners, station policy and an optional diagnostic observer.

use core::future::Future;

use open_esp_radio_esp32s31_wifi_mac::{
    init::{StaLinkRxPolicyHardware, configure_sta_link_receive_policy},
    rx::{RxDma, RxIngressConfig, RxSegment, extract_management},
    tx::{TxCompletion, TxHardware},
};
use open_esp_radio_ieee80211::{
    scan::ScanRecord,
    station::{
        AssociationRequest, HeUlMuPowerCapability, HeUlMuPowerCapabilityError,
        OpenAuthenticationRequest, StaAssociationAttempt, StaAssociationPhy,
        StaAssociationPreference, StaAuthenticationAttempt, StaPowerCapability,
        StaPowerCapabilityError, select_sta_association,
    },
};

use crate::{
    control_tx::{ControlTxError, Esp32s31ControlTx},
    ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile, WifiTxTimer},
    preconnected_rx::{
        Esp32s31PreconnectedRx, Esp32s31PreconnectedRxDelay, Esp32s31PreconnectedRxDirective,
        Esp32s31PreconnectedRxError,
    },
    rx_backend::Esp32s31RxDmaStorage,
    sta_join::{StaJoinBackend, StaJoinRxDirective, StaJoinRxObserver},
};

/// Recovered minimum power advertised by the ESP32-S31 HE STA.
///
/// Complete vendor `hal_he_init` installs this value through
/// `hal_set_tx_min_pwr`; the maximum remains derived from calibrated rate 16.
pub const ESP32S31_STA_MINIMUM_TX_POWER_DBM: i8 = -11;

/// Listen interval used by the qualified ESP32-S31 infrastructure STA path.
pub const ESP32S31_STA_LISTEN_INTERVAL: u16 = 3;

/// RX capability consumed by the concrete join port.
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
        O: StaJoinRxObserver;
}

/// Control-TX capability consumed by the concrete join port.
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

/// Diagnostic observation points which do not participate in driver policy.
///
/// Callbacks run after the matching TX transaction has completed. Production
/// composition normally uses `()`; HIL may inspect registers or report the
/// already-selected power profile without wrapping the hardware operation.
pub trait Esp32s31StaJoinObserver {
    fn authentication_transmitted(&mut self, _completion: TxCompletion) {}

    fn association_profile_selected(&mut self, _profile: Esp32s31StaAssociationProfile) {}

    fn association_transmitted(&mut self, _completion: TxCompletion) {}
}

impl Esp32s31StaJoinObserver for () {}

/// Complete association inputs derived from scan policy and calibration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31StaAssociationProfile {
    pub phy: StaAssociationPhy,
    pub power_capability: Option<StaPowerCapability>,
    pub he_ul_mu_power: Option<HeUlMuPowerCapability>,
    pub rate_16_through_25: Option<[i8; 10]>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31StaAssociationProfileError {
    PowerCapability(StaPowerCapabilityError),
    HeUlMuPower(HeUlMuPowerCapabilityError),
}

/// Select an Association PHY and derive every HE power field from calibration.
pub fn esp32s31_sta_association_profile<P: WifiTxPowerProfile>(
    access_point: &ScanRecord,
    preference: StaAssociationPreference,
    power: &P,
) -> Result<Esp32s31StaAssociationProfile, Esp32s31StaAssociationProfileError> {
    let phy = select_sta_association(access_point, preference).phy;
    if phy != StaAssociationPhy::He20 {
        return Ok(Esp32s31StaAssociationProfile {
            phy,
            power_capability: None,
            he_ul_mu_power: None,
            rate_16_through_25: None,
        });
    }

    let rate_power = core::array::from_fn(|offset| power.power_pair(16 + offset as u8).primary);
    let power_capability =
        StaPowerCapability::new(ESP32S31_STA_MINIMUM_TX_POWER_DBM, rate_power[0])
            .map_err(Esp32s31StaAssociationProfileError::PowerCapability)?;
    let he_ul_mu_power = HeUlMuPowerCapability::from_rate_power_indices(rate_power)
        .map_err(Esp32s31StaAssociationProfileError::HeUlMuPower)?;
    Ok(Esp32s31StaAssociationProfile {
        phy,
        power_capability: Some(power_capability),
        he_ul_mu_power: Some(he_ul_mu_power),
        rate_16_through_25: Some(rate_power),
    })
}

/// Exact primitive edge which failed inside the concrete join port.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31StaJoinPortError<R, T> {
    Receive(R),
    AssociationProfile(Esp32s31StaAssociationProfileError),
    Transmit(T),
}

/// RX owner bound to the stable DMA storage used by every finite join phase.
pub struct Esp32s31StaJoinRx<
    'storage,
    D,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> {
    owner: Esp32s31PreconnectedRx<'storage, D, COUNT, DMA_BUFFER_SIZE>,
    storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
}

impl<'storage, D, const COUNT: usize, const DMA_BUFFER_SIZE: usize, const DMA_STORAGE_SIZE: usize>
    Esp32s31StaJoinRx<'storage, D, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
{
    pub const fn new(
        owner: Esp32s31PreconnectedRx<'storage, D, COUNT, DMA_BUFFER_SIZE>,
        storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    ) -> Self {
        Self { owner, storage }
    }

    pub fn into_owner(self) -> Esp32s31PreconnectedRx<'storage, D, COUNT, DMA_BUFFER_SIZE> {
        self.owner
    }
}

impl<
    'storage,
    D,
    H,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> Esp32s31StaJoinReceive<H>
    for Esp32s31StaJoinRx<'storage, D, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
where
    D: Esp32s31PreconnectedRxDelay,
    H: RxDma,
{
    type Error = Esp32s31PreconnectedRxError;

    fn start<'a>(
        &'a mut self,
        hardware: &'a mut H,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a {
        self.owner.start_with_storage(hardware, self.storage)
    }

    fn stop(&mut self, hardware: &mut H) -> Result<(), Self::Error> {
        self.owner.stop(hardware)
    }

    fn service_management<O>(
        &mut self,
        hardware: &mut H,
        frame: &mut [u8],
        observer: &mut O,
    ) -> Result<(), Self::Error>
    where
        O: StaJoinRxObserver,
    {
        self.owner
            .service_completed(hardware, self.storage, |segment: RxSegment<'_>| {
                let management = extract_management(
                    core::slice::from_ref(&segment),
                    RxIngressConfig {
                        ring_entry_limit: 1,
                        csi_config: 0,
                        flags: 0,
                    },
                    frame,
                )
                .ok();
                let management = management.map(|parsed| &frame[..parsed.length]);
                match observer.observe_completed(management) {
                    StaJoinRxDirective::Continue => Esp32s31PreconnectedRxDirective::Continue,
                    StaJoinRxDirective::Stop => Esp32s31PreconnectedRxDirective::Stop,
                }
            })
            .map(|_| ())
    }
}

impl<'slot, P, E, W, H, const BUFFER_SIZE: usize> Esp32s31StaJoinTransmit<H>
    for Esp32s31ControlTx<'slot, P, E, W, BUFFER_SIZE>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    W: WifiTxTimer,
    H: TxHardware,
{
    type Error = ControlTxError;
    type PowerProfile = P;

    fn power_profile(&self) -> &Self::PowerProfile {
        Self::power_profile(self)
    }

    fn transmit_open_authentication<'a>(
        &'a mut self,
        hardware: &'a mut H,
        request: OpenAuthenticationRequest,
    ) -> impl Future<Output = Result<TxCompletion, Self::Error>> + 'a {
        Self::transmit_open_authentication(self, hardware, request)
    }

    fn transmit_association<'a>(
        &'a mut self,
        hardware: &'a mut H,
        request: AssociationRequest<'a>,
    ) -> impl Future<Output = Result<TxCompletion, Self::Error>> + 'a {
        Self::transmit_association(self, hardware, request)
    }
}

/// Driver resources borrowed for one join runner lifetime.
pub struct Esp32s31StaJoinRadio<'hardware, 'transmit, H, R, T> {
    hardware: &'hardware mut H,
    receive: R,
    transmit: &'transmit mut T,
}

impl<'hardware, 'transmit, H, R, T> Esp32s31StaJoinRadio<'hardware, 'transmit, H, R, T> {
    pub const fn new(hardware: &'hardware mut H, receive: R, transmit: &'transmit mut T) -> Self {
        Self {
            hardware,
            receive,
            transmit,
        }
    }
}

/// Borrowed allocation-free parsing storage and diagnostic observer.
pub struct Esp32s31StaJoinStorage<'scratch, O> {
    frame: &'scratch mut [u8],
    observer: O,
}

impl<'scratch, O> Esp32s31StaJoinStorage<'scratch, O> {
    pub fn new(frame: &'scratch mut [u8], observer: O) -> Self {
        Self { frame, observer }
    }
}

/// Station and peer policy for one Authentication/Association epoch.
#[derive(Clone, Copy)]
pub struct Esp32s31StaJoinStation {
    station_address: [u8; 6],
    access_point: ScanRecord,
    association_preference: StaAssociationPreference,
    listen_interval: u16,
}

impl Esp32s31StaJoinStation {
    pub const fn new(
        station_address: [u8; 6],
        access_point: ScanRecord,
        association_preference: StaAssociationPreference,
    ) -> Self {
        Self {
            station_address,
            access_point,
            association_preference,
            listen_interval: ESP32S31_STA_LISTEN_INTERVAL,
        }
    }

    pub const fn with_listen_interval(mut self, listen_interval: u16) -> Self {
        self.listen_interval = listen_interval;
        self
    }

    pub const fn station_address(&self) -> [u8; 6] {
        self.station_address
    }

    pub const fn access_point(&self) -> &ScanRecord {
        &self.access_point
    }
}

/// Complete production ESP32-S31 STA join port.
pub struct Esp32s31StaJoinPort<'hardware, 'transmit, 'scratch, H, R, T, O> {
    radio: Esp32s31StaJoinRadio<'hardware, 'transmit, H, R, T>,
    storage: Esp32s31StaJoinStorage<'scratch, O>,
    station: Esp32s31StaJoinStation,
}

impl<'hardware, 'transmit, 'scratch, H, R, T, O>
    Esp32s31StaJoinPort<'hardware, 'transmit, 'scratch, H, R, T, O>
{
    pub const fn new(
        radio: Esp32s31StaJoinRadio<'hardware, 'transmit, H, R, T>,
        storage: Esp32s31StaJoinStorage<'scratch, O>,
        station: Esp32s31StaJoinStation,
    ) -> Self {
        Self {
            radio,
            storage,
            station,
        }
    }

    pub fn into_receive(self) -> R {
        self.radio.receive
    }

    /// Install the selected peer address into the pre-connected RX filter.
    ///
    /// The operation is explicit because callers may need to switch the PHY
    /// channel first, but its register sequence remains production driver
    /// logic rather than a board/HIL fixture responsibility.
    pub fn prepare_authentication(&mut self)
    where
        H: StaLinkRxPolicyHardware,
    {
        configure_sta_link_receive_policy(self.radio.hardware, self.station.access_point.bssid);
    }

    pub fn hardware_mut(&mut self) -> &mut H {
        self.radio.hardware
    }
}

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

#[cfg(test)]
mod tests {
    use core::future::{Future, ready};
    use std::vec::Vec;

    use crate::ordinary_tx::WifiTxPowerPair;
    use open_esp_radio_esp32s31_wifi_mac::tx::TxCookie;

    use super::*;

    const LOCAL: [u8; 6] = [2, 0, 0, 0, 0, 1];
    const BSSID: [u8; 6] = [2, 0, 0, 0, 0, 2];
    const HE20_MCS9_CAPABILITY: [u8; 24] = [
        255, 22, 35, 0x03, 0x18, 0x9c, 0xca, 0x10, 0x80, 0x00, 0x10, 0x8a, 0x1b, 0x0d, 0xc0, 0x1f,
        0x00, 0x02, 0x82, 0x01, 0xfd, 0xff, 0xfd, 0xff,
    ];

    const fn completion() -> TxCompletion {
        TxCompletion {
            cookie: TxCookie(1),
            status: 0,
            trigger_flow: false,
            used_alternate: false,
            auxiliary_a_word: 0,
            auxiliary_b_word: 0,
            auxiliary_c_word: 0,
            primary_word: 0,
            alternate_word: 0,
        }
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

    #[test]
    fn association_profile_owns_he_power_derivation() {
        let profile = esp32s31_sta_association_profile(
            &he_access_point(),
            StaAssociationPreference::PreferHe20,
            &Power,
        )
        .unwrap();
        assert_eq!(profile.phy, StaAssociationPhy::He20);
        assert_eq!(
            profile.rate_16_through_25,
            Some([20, 20, 20, 19, 19, 18, 18, 16, 15, 20])
        );
        assert_eq!(profile.power_capability.unwrap().minimum_dbm(), -11);
        assert_eq!(profile.power_capability.unwrap().maximum_dbm(), 20);
        assert_eq!(
            profile.he_ul_mu_power.unwrap().relative_to_rate_16(),
            [0, 0, 1, 1, 2, 2, 4, 5, 0]
        );
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
}
