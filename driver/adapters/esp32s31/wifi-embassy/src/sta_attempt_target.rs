//! Concrete ESP32-S31 owner used by the shared station-attempt transaction.
//!
//! This target binding composes the already-qualified channel, join, peer and
//! WPA2 ports. Board code supplies coherent resource groups and a value-only
//! observer type; it does not sequence protocol or hardware phases.

use core::{future::Future, marker::PhantomData};

use open_esp_radio_esp32s31_hal::{
    RadioRegisters, phy_i2c::PhyI2cMasterControl, phy_temperature::PhyTemperatureSystemControl,
    wifi_bb::PhyWifiBbControl,
};
use open_esp_radio_esp32s31_phy::{PhyAsyncDelay, PhyTargetObserver, PhyTargetPortError};
use open_esp_radio_esp32s31_wifi_lmac::{
    crypto::{CcmpKeyHardware, StaGroupCcmpSlot, StaPairwiseCcmpSlot},
    he::He20PeerHardware,
    init::{StaLinkRxPolicyHardware, StaNoiseFloorHardware},
    rate_control::BeamformingReportHardware,
    rx::RxDma,
    tx::TxHardware,
};
use open_esp_radio_esp32s31_wifi_sta::{
    attempt::{
        Esp32s31StaAttemptConnected, Esp32s31StaAttemptPort, Esp32s31StaAttemptReport,
        Esp32s31StaAttemptSecurity, Esp32s31StaAttemptStateError, Esp32s31StaAttemptStation,
        Esp32s31StaAttemptStepError, Esp32s31StaConnectedEntryFailure,
    },
    channel::Esp32s31ScanPhy,
    join::{Esp32s31StaJoinObserver, Esp32s31StaJoinPortError, Esp32s31StaJoinTransmit},
    peer::{
        Esp32s31ConnectedStaPeer, Esp32s31PreparedStaPeer, Esp32s31ProgrammedStaPeer,
        Esp32s31StaPeerPort, Esp32s31StaPeerPortError, Esp32s31StaPeerRadio,
        Esp32s31StaPeerStation, Esp32s31StaPeerTransmit,
    },
    wpa2::{
        Esp32s31InstalledWpa2Keys, Esp32s31Wpa2HandshakePort, Esp32s31Wpa2HandshakePortError,
        Esp32s31Wpa2HandshakeRadio, Esp32s31Wpa2HandshakeStorage, Esp32s31Wpa2KeyPort,
        Esp32s31Wpa2KeyPortError, Esp32s31Wpa2KeyRadio, Esp32s31Wpa2KeySession,
        Esp32s31Wpa2Station, Esp32s31Wpa2Transmit,
    },
};
use open_esp_radio_ieee80211::station::{
    AssociationResponse, StaSecurityError, select_sta_association, select_wpa2_psk_rsn,
};
use open_esp_radio_wifi_sta::{
    join::{StaJoinError, StaJoinRunner},
    station::StaFailureDisposition,
};
use open_esp_radio_wpa2::{
    aes::{SoftwareAesKeyUnwrapError, Wpa2SoftwareAes},
    runner::{
        Wpa2Established, Wpa2HandshakeConfig, Wpa2HandshakeError, Wpa2HandshakeRunner,
        Wpa2KeyInstallError, Wpa2KeyInstallRunner, Wpa2PendingKeyInstall,
    },
};

use crate::{
    cooperative_tx::CooperativeTxHardware,
    join_time::EmbassyStaJoinTimer,
    preconnected_rx::{
        Esp32s31PreconnectedRx, Esp32s31PreconnectedRxDelay, Esp32s31PreconnectedRxError,
    },
    rx_backend::Esp32s31RxDmaStorage,
    sta_join_port::{
        Esp32s31StaJoinPort, Esp32s31StaJoinRadio, Esp32s31StaJoinRx, Esp32s31StaJoinStation,
        Esp32s31StaJoinStorage,
    },
    wpa2_port::Esp32s31Wpa2Rx,
    wpa2_time::EmbassyWpa2HandshakeTimer,
};

/// Channel-switch capability accepted by the concrete attempt owner.
pub trait Esp32s31StaAttemptChannel<H> {
    fn switch_channel<'a>(
        &'a mut self,
        hardware: &'a mut H,
        channel_or_frequency: u16,
        cbw: u8,
    ) -> impl Future<Output = Result<(), PhyTargetPortError>> + 'a;
}

impl<P, O, D> Esp32s31StaAttemptChannel<RadioRegisters> for Esp32s31ScanPhy<'_, P, O, D>
where
    P: PhyWifiBbControl + PhyTemperatureSystemControl + PhyI2cMasterControl,
    O: PhyTargetObserver,
    D: PhyAsyncDelay,
{
    fn switch_channel<'a>(
        &'a mut self,
        hardware: &'a mut RadioRegisters,
        channel_or_frequency: u16,
        cbw: u8,
    ) -> impl Future<Output = Result<(), PhyTargetPortError>> + 'a {
        Esp32s31ScanPhy::switch_channel(self, channel_or_frequency, cbw, hardware)
    }
}

impl<'cell, 'registers, P, O, D> Esp32s31StaAttemptChannel<CooperativeTxHardware<'cell, 'registers>>
    for Esp32s31ScanPhy<'_, P, O, D>
where
    P: PhyWifiBbControl + PhyTemperatureSystemControl + PhyI2cMasterControl,
    O: PhyTargetObserver,
    D: PhyAsyncDelay,
{
    fn switch_channel<'a>(
        &'a mut self,
        hardware: &'a mut CooperativeTxHardware<'cell, 'registers>,
        channel_or_frequency: u16,
        cbw: u8,
    ) -> impl Future<Output = Result<(), PhyTargetPortError>> + 'a {
        async move {
            let mut registers = hardware.register_cell().borrow_mut();
            Esp32s31ScanPhy::switch_channel(self, channel_or_frequency, cbw, &mut registers).await
        }
    }
}

/// Coherent mutable radio resources used by one finite attempt.
pub struct Esp32s31StaAttemptRadio<
    'hardware,
    'transmit,
    'storage,
    H,
    C,
    D,
    T,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> {
    pub hardware: &'hardware mut H,
    pub channel: C,
    pub receive: Esp32s31PreconnectedRx<'storage, D, COUNT, DMA_BUFFER_SIZE>,
    pub rx_storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    pub transmit: &'transmit mut T,
}

impl<
    'hardware,
    'transmit,
    'storage,
    H,
    C,
    D,
    T,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
>
    Esp32s31StaAttemptRadio<
        'hardware,
        'transmit,
        'storage,
        H,
        C,
        D,
        T,
        COUNT,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >
{
    pub const fn new(
        hardware: &'hardware mut H,
        channel: C,
        receive: Esp32s31PreconnectedRx<'storage, D, COUNT, DMA_BUFFER_SIZE>,
        rx_storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        transmit: &'transmit mut T,
    ) -> Self {
        Self {
            hardware,
            channel,
            receive,
            rx_storage,
            transmit,
        }
    }
}

/// Allocation-free frame scratch used by management and EAPOL parsing.
pub struct Esp32s31StaAttemptStorage<'scratch> {
    pub frame: &'scratch mut [u8],
}

impl<'scratch> Esp32s31StaAttemptStorage<'scratch> {
    pub const fn new(frame: &'scratch mut [u8]) -> Self {
        Self { frame }
    }
}

/// Exact primitive error reported by the concrete target port.
#[derive(Debug, Eq, PartialEq)]
pub enum Esp32s31StaAttemptTargetError<J, W> {
    State(Esp32s31StaAttemptStateError),
    Candidate(Esp32s31StaPeerPortError),
    Channel(PhyTargetPortError),
    Authentication(StaJoinError<Esp32s31StaJoinPortError<Esp32s31PreconnectedRxError, J>>),
    Association(StaJoinError<Esp32s31StaJoinPortError<Esp32s31PreconnectedRxError, J>>),
    Peer(Esp32s31StaPeerPortError),
    Security(StaSecurityError),
    Wpa2Handshake(
        Wpa2HandshakeError<
            Esp32s31Wpa2HandshakePortError<Esp32s31PreconnectedRxError, W>,
            SoftwareAesKeyUnwrapError,
        >,
    ),
    Wpa2KeyInstall(Wpa2KeyInstallError<Esp32s31Wpa2KeyPortError<W>>),
}

/// Coherent owner consumed by
/// [`Esp32s31StaAttempt`](open_esp_radio_esp32s31_wifi_sta::attempt::Esp32s31StaAttempt).
pub struct Esp32s31StaAttemptTargetOwner<
    'hardware,
    'transmit,
    'storage,
    'scratch,
    'security,
    H,
    C,
    D,
    T,
    J,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> {
    hardware: &'hardware mut H,
    channel: C,
    receive: Option<Esp32s31PreconnectedRx<'storage, D, COUNT, DMA_BUFFER_SIZE>>,
    rx_storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    transmit: &'transmit mut T,
    frame: &'scratch mut [u8],
    station: Esp32s31StaAttemptStation,
    security: Esp32s31StaAttemptSecurity<'security>,
    prepared_peer: Option<Esp32s31PreparedStaPeer>,
    association: Option<AssociationResponse>,
    connected_peer: Option<Esp32s31ConnectedStaPeer>,
    pending_keys: Option<Wpa2PendingKeyInstall>,
    installed_keys: Option<(StaPairwiseCcmpSlot, StaGroupCcmpSlot)>,
    report: Esp32s31StaAttemptReport,
    _join_observer: PhantomData<fn() -> J>,
}

impl<
    'hardware,
    'transmit,
    'storage,
    'scratch,
    'security,
    H,
    C,
    D,
    T,
    J,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
>
    Esp32s31StaAttemptTargetOwner<
        'hardware,
        'transmit,
        'storage,
        'scratch,
        'security,
        H,
        C,
        D,
        T,
        J,
        COUNT,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >
{
    pub fn new(
        radio: Esp32s31StaAttemptRadio<
            'hardware,
            'transmit,
            'storage,
            H,
            C,
            D,
            T,
            COUNT,
            DMA_BUFFER_SIZE,
            DMA_STORAGE_SIZE,
        >,
        storage: Esp32s31StaAttemptStorage<'scratch>,
        station: Esp32s31StaAttemptStation,
        security: Esp32s31StaAttemptSecurity<'security>,
    ) -> Self {
        Self {
            hardware: radio.hardware,
            channel: radio.channel,
            receive: Some(radio.receive),
            rx_storage: radio.rx_storage,
            transmit: radio.transmit,
            frame: storage.frame,
            station,
            security,
            prepared_peer: None,
            association: None,
            connected_peer: None,
            pending_keys: None,
            installed_keys: None,
            report: Esp32s31StaAttemptReport {
                authentication: None,
                association: None,
                peer: None,
                wpa2_handshake: None,
                wpa2: None,
                message4: None,
            },
            _join_observer: PhantomData,
        }
    }

    pub const fn report(&self) -> Esp32s31StaAttemptReport {
        self.report
    }

    pub fn take_connected_peer(&mut self) -> Option<Esp32s31ConnectedStaPeer> {
        self.connected_peer.take()
    }

    pub fn take_installed_keys(&mut self) -> Option<(StaPairwiseCcmpSlot, StaGroupCcmpSlot)> {
        self.installed_keys.take()
    }

    pub fn into_parts(
        self,
    ) -> (
        Esp32s31StaAttemptRadio<
            'hardware,
            'transmit,
            'storage,
            H,
            C,
            D,
            T,
            COUNT,
            DMA_BUFFER_SIZE,
            DMA_STORAGE_SIZE,
        >,
        Esp32s31StaAttemptStorage<'scratch>,
        Esp32s31StaAttemptStation,
        Esp32s31StaAttemptSecurity<'security>,
    ) {
        (
            Esp32s31StaAttemptRadio::new(
                self.hardware,
                self.channel,
                self.receive.expect("attempt owner retains RX"),
                self.rx_storage,
                self.transmit,
            ),
            Esp32s31StaAttemptStorage::new(self.frame),
            self.station,
            self.security,
        )
    }
}

/// Stateless target port. All unique resources live in `Owner`; the phantom
/// type only selects the concrete owner graph for the trait implementation.
pub struct Esp32s31StaAttemptTargetPort<O> {
    _owner: PhantomData<fn() -> O>,
}

impl<O> Esp32s31StaAttemptTargetPort<O> {
    pub const fn new() -> Self {
        Self {
            _owner: PhantomData,
        }
    }
}

impl<O> Clone for Esp32s31StaAttemptTargetPort<O> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<O> Copy for Esp32s31StaAttemptTargetPort<O> {}

impl<O> Default for Esp32s31StaAttemptTargetPort<O> {
    fn default() -> Self {
        Self::new()
    }
}

impl<
    'hardware,
    'transmit,
    'storage,
    'scratch,
    'security,
    H,
    C,
    D,
    T,
    J,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> Esp32s31StaAttemptPort
    for Esp32s31StaAttemptTargetPort<
        Esp32s31StaAttemptTargetOwner<
            'hardware,
            'transmit,
            'storage,
            'scratch,
            'security,
            H,
            C,
            D,
            T,
            J,
            COUNT,
            DMA_BUFFER_SIZE,
            DMA_STORAGE_SIZE,
        >,
    >
where
    H: RxDma
        + TxHardware
        + StaLinkRxPolicyHardware
        + StaNoiseFloorHardware
        + He20PeerHardware
        + BeamformingReportHardware
        + CcmpKeyHardware
        + 'hardware,
    C: Esp32s31StaAttemptChannel<H>,
    D: Esp32s31PreconnectedRxDelay,
    T: Esp32s31StaJoinTransmit<H> + Esp32s31Wpa2Transmit<H> + Esp32s31StaPeerTransmit + 'transmit,
    J: Esp32s31StaJoinObserver + Default,
{
    type Owner = Esp32s31StaAttemptTargetOwner<
        'hardware,
        'transmit,
        'storage,
        'scratch,
        'security,
        H,
        C,
        D,
        T,
        J,
        COUNT,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >;
    type Connected = Esp32s31StaAttemptConnected<Self::Owner>;
    type Error = Esp32s31StaAttemptTargetError<
        <T as Esp32s31StaJoinTransmit<H>>::Error,
        <T as Esp32s31Wpa2Transmit<H>>::Error,
    >;

    fn prepare_candidate<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> impl Future<Output = Result<(), Esp32s31StaAttemptStepError<Self::Error>>> + 'a {
        async move {
            owner.prepared_peer = None;
            owner.association = None;
            owner.connected_peer = None;
            owner.pending_keys = None;
            owner.installed_keys = None;
            owner.report = Esp32s31StaAttemptReport::default();
            owner.prepared_peer = Some(
                Esp32s31StaPeerPort::prepare(owner.transmit, &owner.station.access_point).map_err(
                    |error| {
                        Esp32s31StaAttemptStepError::terminal(
                            Esp32s31StaAttemptTargetError::Candidate(error),
                        )
                    },
                )?,
            );
            Ok(())
        }
    }

    fn select_channel<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> impl Future<Output = Result<(), Esp32s31StaAttemptStepError<Self::Error>>> + 'a {
        async move {
            let selection = select_sta_association(
                &owner.station.access_point,
                owner.station.association_preference,
            );
            owner
                .channel
                .switch_channel(
                    owner.hardware,
                    selection.channel_or_frequency,
                    selection.cbw,
                )
                .await
                .map_err(|error| {
                    Esp32s31StaAttemptStepError::retry_current(
                        Esp32s31StaAttemptTargetError::Channel(error),
                    )
                })
        }
    }

    fn authenticate<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> impl Future<Output = Result<(), Esp32s31StaAttemptStepError<Self::Error>>> + 'a {
        async move {
            let receive = owner.receive.take().ok_or_else(|| {
                Esp32s31StaAttemptStepError::terminal(Esp32s31StaAttemptTargetError::State(
                    Esp32s31StaAttemptStateError::MissingReceive,
                ))
            })?;
            let mut port = Esp32s31StaJoinPort::new(
                Esp32s31StaJoinRadio::new(
                    &mut *owner.hardware,
                    Esp32s31StaJoinRx::new(receive, owner.rx_storage),
                    &mut *owner.transmit,
                ),
                Esp32s31StaJoinStorage::new(owner.frame, J::default()),
                Esp32s31StaJoinStation::new(
                    owner.station.station_address,
                    owner.station.access_point,
                    owner.station.association_preference,
                ),
            );
            port.prepare_authentication();
            let mut runner = StaJoinRunner::new(port, EmbassyStaJoinTimer);
            let result = runner
                .authenticate(
                    owner.station.station_address,
                    owner.station.access_point.bssid,
                    owner.security.sequences.non_qos_mut(),
                )
                .await;
            let (port, _) = runner.into_parts();
            owner.receive = Some(port.into_receive().into_owner());
            match result {
                Ok(success) => {
                    owner.report.authentication = Some(success);
                    Ok(())
                }
                Err(error) => Err(Esp32s31StaAttemptStepError::retry_current(
                    Esp32s31StaAttemptTargetError::Authentication(error),
                )),
            }
        }
    }

    fn associate<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> impl Future<Output = Result<(), Esp32s31StaAttemptStepError<Self::Error>>> + 'a {
        async move {
            let receive = owner.receive.take().ok_or_else(|| {
                Esp32s31StaAttemptStepError::terminal(Esp32s31StaAttemptTargetError::State(
                    Esp32s31StaAttemptStateError::MissingReceive,
                ))
            })?;
            let port = Esp32s31StaJoinPort::new(
                Esp32s31StaJoinRadio::new(
                    &mut *owner.hardware,
                    Esp32s31StaJoinRx::new(receive, owner.rx_storage),
                    &mut *owner.transmit,
                ),
                Esp32s31StaJoinStorage::new(owner.frame, J::default()),
                Esp32s31StaJoinStation::new(
                    owner.station.station_address,
                    owner.station.access_point,
                    owner.station.association_preference,
                ),
            );
            let mut runner = StaJoinRunner::new(port, EmbassyStaJoinTimer);
            let result = runner
                .associate(
                    owner.station.station_address,
                    owner.station.access_point.bssid,
                    owner.security.sequences.non_qos_mut(),
                )
                .await;
            let (port, _) = runner.into_parts();
            owner.receive = Some(port.into_receive().into_owner());
            match result {
                Ok(success) => {
                    owner.association = Some(success.response);
                    owner.report.association = Some(success);
                    Ok(())
                }
                Err(error) => Err(Esp32s31StaAttemptStepError::retry_current(
                    Esp32s31StaAttemptTargetError::Association(error),
                )),
            }
        }
    }

    fn program_peer<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> impl Future<Output = Result<(), Esp32s31StaAttemptStepError<Self::Error>>> + 'a {
        async move {
            let prepared = owner.prepared_peer.take().ok_or_else(|| {
                Esp32s31StaAttemptStepError::terminal(Esp32s31StaAttemptTargetError::State(
                    Esp32s31StaAttemptStateError::MissingPreparedPeer,
                ))
            })?;
            let response = owner.association.ok_or_else(|| {
                Esp32s31StaAttemptStepError::terminal(Esp32s31StaAttemptTargetError::State(
                    Esp32s31StaAttemptStateError::MissingAssociation,
                ))
            })?;
            let association_phy = select_sta_association(
                &owner.station.access_point,
                owner.station.association_preference,
            )
            .phy;
            let Esp32s31ProgrammedStaPeer { peer, report } = Esp32s31StaPeerPort::program(
                Esp32s31StaPeerRadio::new(&mut *owner.hardware, &mut *owner.transmit),
                Esp32s31StaPeerStation::new(owner.station.station_address, association_phy),
                &response,
                prepared,
            )
            .map_err(|error| {
                Esp32s31StaAttemptStepError::terminal(Esp32s31StaAttemptTargetError::Peer(error))
            })?;
            owner.connected_peer = Some(peer);
            owner.report.peer = Some(report);
            Ok(())
        }
    }

    fn run_wpa2_handshake<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> impl Future<Output = Result<(), Esp32s31StaAttemptStepError<Self::Error>>> + 'a {
        async move {
            if owner.connected_peer.is_none() {
                return Err(Esp32s31StaAttemptStepError::terminal(
                    Esp32s31StaAttemptTargetError::State(
                        Esp32s31StaAttemptStateError::MissingConnectedPeer,
                    ),
                ));
            }
            let selected_rsn =
                select_wpa2_psk_rsn(&owner.station.access_point).map_err(|error| {
                    Esp32s31StaAttemptStepError::terminal(Esp32s31StaAttemptTargetError::Security(
                        error,
                    ))
                })?;
            let receive = owner.receive.take().ok_or_else(|| {
                Esp32s31StaAttemptStepError::terminal(Esp32s31StaAttemptTargetError::State(
                    Esp32s31StaAttemptStateError::MissingReceive,
                ))
            })?;
            let station = Esp32s31Wpa2Station::new(
                owner.station.station_address,
                owner.station.access_point.bssid,
            );
            let port = Esp32s31Wpa2HandshakePort::new(
                Esp32s31Wpa2HandshakeRadio::new(
                    &mut *owner.hardware,
                    Esp32s31Wpa2Rx::new(receive, owner.rx_storage, station),
                    &mut *owner.transmit,
                ),
                Esp32s31Wpa2HandshakeStorage::new(owner.frame),
                station,
            );
            let mut runner =
                Wpa2HandshakeRunner::new(port, EmbassyWpa2HandshakeTimer, Wpa2SoftwareAes::new());
            let mut next_sequence = || owner.security.sequences.take_non_qos();
            let result = runner
                .run(
                    Wpa2HandshakeConfig {
                        local: owner.station.station_address,
                        authenticator: owner.station.access_point.bssid,
                        supplicant_nonce: owner.security.supplicant_nonce,
                        association_security_ies: selected_rsn.as_bytes(),
                        authenticator_rsn_ie: owner.station.access_point.rsn_ie_bytes(),
                        authenticator_rsnxe: owner.station.access_point.rsnxe_bytes(),
                        pmk: owner.security.pmk,
                    },
                    &mut next_sequence,
                )
                .await;
            let telemetry = runner.backend().telemetry();
            let (port, _, _) = runner.into_parts();
            owner.receive = Some(port.into_receive().into_owner());
            owner.report.wpa2_handshake = Some(telemetry);
            match result {
                Ok(pending) => {
                    owner.pending_keys = Some(pending);
                    Ok(())
                }
                Err(error) => Err(Esp32s31StaAttemptStepError::retry_current(
                    Esp32s31StaAttemptTargetError::Wpa2Handshake(error),
                )),
            }
        }
    }

    fn install_wpa2_keys<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> impl Future<Output = Result<(), Esp32s31StaAttemptStepError<Self::Error>>> + 'a {
        async move {
            let pending = owner.pending_keys.take().ok_or_else(|| {
                Esp32s31StaAttemptStepError::terminal(Esp32s31StaAttemptTargetError::State(
                    Esp32s31StaAttemptStateError::MissingHandshake,
                ))
            })?;
            let link = owner
                .connected_peer
                .as_ref()
                .ok_or_else(|| {
                    Esp32s31StaAttemptStepError::terminal(Esp32s31StaAttemptTargetError::State(
                        Esp32s31StaAttemptStateError::MissingConnectedPeer,
                    ))
                })?
                .link;
            let port = Esp32s31Wpa2KeyPort::new(
                Esp32s31Wpa2KeyRadio::new(&mut *owner.hardware, &mut *owner.transmit),
                Esp32s31Wpa2KeySession::new(
                    Esp32s31Wpa2Station::new(link.station_address, link.bssid),
                    link.peer_qos,
                    &mut *owner.security.sequences,
                    owner.security.message4_protection,
                ),
            );
            let mut runner = Wpa2KeyInstallRunner::new(port);
            let result: Result<Wpa2Established<Esp32s31InstalledWpa2Keys>, _> =
                runner.run(pending).await;
            let port = runner.into_backend();
            let completion = port.completion();
            let _parts = port.into_parts();
            owner.report.message4 = completion;
            match result {
                Ok(established) => {
                    owner.report.wpa2 = Some(established.metadata());
                    owner.installed_keys = Some(established.into_keys().into_parts());
                    Ok(())
                }
                Err(error) => Err(Esp32s31StaAttemptStepError::retry_current(
                    Esp32s31StaAttemptTargetError::Wpa2KeyInstall(error),
                )),
            }
        }
    }

    fn enter_connected(
        &mut self,
        owner: Self::Owner,
    ) -> impl Future<
        Output = Result<
            Self::Connected,
            Esp32s31StaConnectedEntryFailure<Self::Owner, Self::Error>,
        >,
    > + '_ {
        async move {
            let missing = if owner.connected_peer.is_none() {
                Some(Esp32s31StaAttemptStateError::MissingConnectedPeer)
            } else if owner.installed_keys.is_none() {
                Some(Esp32s31StaAttemptStateError::MissingKeys)
            } else {
                None
            };
            match missing {
                Some(error) => Err(Esp32s31StaConnectedEntryFailure::new(
                    owner,
                    StaFailureDisposition::Terminal,
                    Esp32s31StaAttemptTargetError::State(error),
                )),
                None => Ok(Esp32s31StaAttemptConnected::new(owner)),
            }
        }
    }
}
