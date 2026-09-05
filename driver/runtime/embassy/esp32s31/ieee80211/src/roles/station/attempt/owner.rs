use super::*;

/// Exact primitive error reported by the concrete target port.
#[derive(Debug, Eq, PartialEq)]
pub enum Esp32s31StaAttemptTargetError<J, W> {
    State(Esp32s31StaAttemptStateError),
    Candidate(Esp32s31StaPeerPortError),
    Channel(PhyTargetPortError),
    Authentication(StaJoinError<Esp32s31StaJoinPortError<Esp32s31RxFrontierError, J>>),
    Association(StaJoinError<Esp32s31StaJoinPortError<Esp32s31RxFrontierError, J>>),
    Peer(Esp32s31StaPeerPortError),
    Security(StaSecurityError),
    Wpa2Handshake(
        Wpa2HandshakeError<
            Esp32s31Wpa2HandshakePortError<Esp32s31RxFrontierError, W>,
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
    pub(super) hardware: &'hardware mut H,
    pub(super) channel: C,
    pub(super) receive: Option<Esp32s31RxFrontier<'storage, D, COUNT, DMA_BUFFER_SIZE>>,
    pub(super) rx_storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    pub(super) transmit: &'transmit mut T,
    pub(super) frame: &'scratch mut [u8],
    pub(super) station: Esp32s31StaAttemptStation,
    pub(super) listen_interval: open_esp_radio_wifi_sta::request::StationListenInterval,
    pub(super) security: Esp32s31StaAttemptSecurity<'security>,
    pub(super) prepared_peer: Option<Esp32s31PreparedStaPeer>,
    pub(super) association: Option<AssociationResponse>,
    pub(super) connected_peer: Option<Esp32s31ConnectedStaPeer>,
    pub(super) pending_keys: Option<Wpa2PendingKeyInstall>,
    pub(super) installed_security: Option<Esp32s31StaInstalledSecurity>,
    pub(super) report: Esp32s31StaAttemptReport,
    pub(super) _join_observer: PhantomData<fn() -> J>,
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
        listen_interval: open_esp_radio_wifi_sta::request::StationListenInterval,
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
            listen_interval,
            security,
            prepared_peer: None,
            association: None,
            connected_peer: None,
            pending_keys: None,
            installed_security: None,
            report: Esp32s31StaAttemptReport {
                security: None,
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

    pub fn take_installed_security(&mut self) -> Option<Esp32s31StaInstalledSecurity> {
        self.installed_security.take()
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
