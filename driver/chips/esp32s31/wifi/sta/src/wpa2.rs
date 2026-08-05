//! Executor-independent ESP32-S31 ports for the WPA2 four-way handshake.
//!
//! The portable WPA2 crate owns replay protection, cryptography, deadlines and
//! rollback order. This module binds those transactions to ESP32-S31 frame TX
//! and hardware CCMP slots without choosing an executor, DMA allocation or
//! board fixture.

use core::future::Future;

use open_esp_radio_esp32s31_wifi_mac::{
    crypto::{
        CcmpKeyHardware, CryptoKeyError, StaGroupCcmpSlot, StaPairwiseCcmpSlot,
        install_sta_group_ccmp, install_sta_pairwise_ccmp,
    },
    tx::{LegacyRate, LegacyTxQueue, TxCompletion, TxPhyRate},
};
use open_esp_radio_ieee80211::station::{
    StaDataFrame, StaProtectedDataFrame, StaTxSequenceCounters,
};
use open_esp_radio_wpa2::{
    DEFAULT_EAPOL_FRAME_CAPACITY, OwnedEapolFrame, Wpa2Interface,
    frames::Wpa2TxFrame,
    keys::Wpa2KeyKind,
    runner::{Wpa2HandshakeBackend, Wpa2KeyInstallBackend, Wpa2RxProgress},
    supplicant::Wpa2StaKeyInstallRequest,
};

const LLC_SNAP_EAPOL: [u8; 8] = [0xaa, 0xaa, 0x03, 0, 0, 0, 0x88, 0x8e];

/// RX capability consumed by the concrete WPA2 handshake port.
pub trait Esp32s31Wpa2Receive<H> {
    type Error;

    fn service(
        &mut self,
        hardware: &mut H,
        frame: &mut [u8],
    ) -> Result<Wpa2RxProgress, Self::Error>;

    fn restart<'a>(
        &'a mut self,
        hardware: &'a mut H,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a;

    fn stop(&mut self, hardware: &mut H) -> Result<(), Self::Error>;
}

/// Ordinary EAPOL TX capability shared by handshake and key-install ports.
pub trait Esp32s31Wpa2Transmit<H> {
    type Error;

    fn transmit_unprotected<'a>(
        &'a mut self,
        hardware: &'a mut H,
        frame: StaDataFrame<'a>,
    ) -> impl Future<Output = Result<TxCompletion, Self::Error>> + 'a;

    fn transmit_protected<'a>(
        &'a mut self,
        hardware: &'a mut H,
        frame: StaProtectedDataFrame<'a>,
        queue: LegacyTxQueue,
        rate: TxPhyRate,
        hardware_key_selector: u8,
    ) -> impl Future<Output = Result<TxCompletion, Self::Error>> + 'a;
}

/// Stable local/peer identity shared by both WPA2 ports.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31Wpa2Station {
    station_address: [u8; 6],
    bssid: [u8; 6],
}

impl Esp32s31Wpa2Station {
    pub const fn new(station_address: [u8; 6], bssid: [u8; 6]) -> Self {
        Self {
            station_address,
            bssid,
        }
    }
}

/// Copy an EAPOL packet only when it belongs to the selected station link.
pub fn copy_station_eapol(
    frame: &[u8],
    mpdu_length: usize,
    payload_offset: usize,
    station: Esp32s31Wpa2Station,
) -> Option<OwnedEapolFrame<DEFAULT_EAPOL_FRAME_CAPACITY>> {
    if mpdu_length < 24
        || frame.get(4..10) != Some(&station.station_address)
        || frame.get(10..16) != Some(&station.bssid)
    {
        return None;
    }
    let eapol_offset = payload_offset.checked_add(LLC_SNAP_EAPOL.len())?;
    if frame.get(payload_offset..eapol_offset) != Some(&LLC_SNAP_EAPOL) {
        return None;
    }
    OwnedEapolFrame::try_copy(
        Wpa2Interface::Station,
        station.bssid,
        frame.get(eapol_offset..mpdu_length)?,
    )
    .ok()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31Wpa2HandshakePortError<R, T> {
    Receive(R),
    Transmit(T),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Esp32s31Wpa2HandshakeTelemetry {
    pub message2_transmissions: u16,
}

/// PAC/RX/TX owners borrowed for one handshake runner lifetime.
pub struct Esp32s31Wpa2HandshakeRadio<'hardware, 'transmit, H, R, T> {
    hardware: &'hardware mut H,
    receive: R,
    transmit: &'transmit mut T,
}

impl<'hardware, 'transmit, H, R, T> Esp32s31Wpa2HandshakeRadio<'hardware, 'transmit, H, R, T> {
    pub const fn new(hardware: &'hardware mut H, receive: R, transmit: &'transmit mut T) -> Self {
        Self {
            hardware,
            receive,
            transmit,
        }
    }
}

/// Borrowed allocation-free MPDU/EAPOL scratch storage.
pub struct Esp32s31Wpa2HandshakeStorage<'scratch> {
    frame: &'scratch mut [u8],
}

impl<'scratch> Esp32s31Wpa2HandshakeStorage<'scratch> {
    pub fn new(frame: &'scratch mut [u8]) -> Self {
        Self { frame }
    }
}

/// Complete production port for WPA2 Message 1/2/3 exchange.
pub struct Esp32s31Wpa2HandshakePort<'hardware, 'transmit, 'scratch, H, R, T> {
    radio: Esp32s31Wpa2HandshakeRadio<'hardware, 'transmit, H, R, T>,
    storage: Esp32s31Wpa2HandshakeStorage<'scratch>,
    station: Esp32s31Wpa2Station,
    telemetry: Esp32s31Wpa2HandshakeTelemetry,
}

impl<'hardware, 'transmit, 'scratch, H, R, T>
    Esp32s31Wpa2HandshakePort<'hardware, 'transmit, 'scratch, H, R, T>
{
    pub fn new(
        radio: Esp32s31Wpa2HandshakeRadio<'hardware, 'transmit, H, R, T>,
        storage: Esp32s31Wpa2HandshakeStorage<'scratch>,
        station: Esp32s31Wpa2Station,
    ) -> Self {
        Self {
            radio,
            storage,
            station,
            telemetry: Esp32s31Wpa2HandshakeTelemetry::default(),
        }
    }

    pub const fn telemetry(&self) -> Esp32s31Wpa2HandshakeTelemetry {
        self.telemetry
    }

    pub fn into_receive(self) -> R {
        self.radio.receive
    }
}

impl<H, R, T> Wpa2HandshakeBackend for Esp32s31Wpa2HandshakePort<'_, '_, '_, H, R, T>
where
    R: Esp32s31Wpa2Receive<H>,
    T: Esp32s31Wpa2Transmit<H>,
{
    type Error = Esp32s31Wpa2HandshakePortError<R::Error, T::Error>;

    fn service_receive(
        &mut self,
    ) -> impl Future<Output = Result<Wpa2RxProgress, Self::Error>> + '_ {
        async {
            self.radio
                .receive
                .service(self.radio.hardware, self.storage.frame)
                .map_err(Esp32s31Wpa2HandshakePortError::Receive)
        }
    }

    fn restart_receive(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        async {
            self.radio
                .receive
                .restart(self.radio.hardware)
                .await
                .map_err(Esp32s31Wpa2HandshakePortError::Receive)
        }
    }

    fn stop_receive(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        async {
            self.radio
                .receive
                .stop(self.radio.hardware)
                .map_err(Esp32s31Wpa2HandshakePortError::Receive)
        }
    }

    fn transmit_message2<'a>(
        &'a mut self,
        frame: &'a Wpa2TxFrame<DEFAULT_EAPOL_FRAME_CAPACITY>,
        sequence_number: u16,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a {
        async move {
            self.radio
                .transmit
                .transmit_unprotected(
                    self.radio.hardware,
                    StaDataFrame {
                        source: self.station.station_address,
                        bssid: self.station.bssid,
                        destination: self.station.bssid,
                        sequence_number,
                        ether_type: 0x888e,
                        payload: frame.as_bytes(),
                    },
                )
                .await
                .map_err(Esp32s31Wpa2HandshakePortError::Transmit)?;
            self.telemetry.message2_transmissions =
                self.telemetry.message2_transmissions.saturating_add(1);
            Ok(())
        }
    }
}

/// Whether Message 4 is sent before or after enabling pairwise protection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31Wpa2Message4Protection {
    Unprotected,
    PairwiseCcmp,
}

/// Installed hardware authorities returned to the connected data path.
pub struct Esp32s31InstalledWpa2Keys {
    pairwise: StaPairwiseCcmpSlot,
    group: StaGroupCcmpSlot,
}

impl Esp32s31InstalledWpa2Keys {
    pub fn into_parts(self) -> (StaPairwiseCcmpSlot, StaGroupCcmpSlot) {
        (self.pairwise, self.group)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31Wpa2KeyPortError<T> {
    InvalidGroupKind,
    Install(CryptoKeyError),
    Transmit(T),
    TxStatus(u8),
}

/// Owners recovered after a key-install runner has completed.
pub struct Esp32s31Wpa2KeyPortParts<'hardware, 'transmit, 'sequence, H, T> {
    pub hardware: &'hardware mut H,
    pub transmit: &'transmit mut T,
    pub sequences: &'sequence mut StaTxSequenceCounters,
    pub completion: Option<TxCompletion>,
}

/// Hardware and ordinary-TX owners borrowed by key publication.
pub struct Esp32s31Wpa2KeyRadio<'hardware, 'transmit, H, T> {
    hardware: &'hardware mut H,
    transmit: &'transmit mut T,
}

impl<'hardware, 'transmit, H, T> Esp32s31Wpa2KeyRadio<'hardware, 'transmit, H, T> {
    pub const fn new(hardware: &'hardware mut H, transmit: &'transmit mut T) -> Self {
        Self { hardware, transmit }
    }
}

/// Link policy and sequence ownership retained through Message 4.
pub struct Esp32s31Wpa2KeySession<'sequence> {
    station: Esp32s31Wpa2Station,
    peer_qos: bool,
    sequences: &'sequence mut StaTxSequenceCounters,
    message4_protection: Esp32s31Wpa2Message4Protection,
}

impl<'sequence> Esp32s31Wpa2KeySession<'sequence> {
    pub const fn new(
        station: Esp32s31Wpa2Station,
        peer_qos: bool,
        sequences: &'sequence mut StaTxSequenceCounters,
        message4_protection: Esp32s31Wpa2Message4Protection,
    ) -> Self {
        Self {
            station,
            peer_qos,
            sequences,
            message4_protection,
        }
    }
}

/// Complete production port for atomic PTK/GTK publication and Message 4.
pub struct Esp32s31Wpa2KeyPort<'hardware, 'transmit, 'sequence, H, T> {
    radio: Esp32s31Wpa2KeyRadio<'hardware, 'transmit, H, T>,
    session: Esp32s31Wpa2KeySession<'sequence>,
    completion: Option<TxCompletion>,
}

impl<'hardware, 'transmit, 'sequence, H, T>
    Esp32s31Wpa2KeyPort<'hardware, 'transmit, 'sequence, H, T>
{
    pub const fn new(
        radio: Esp32s31Wpa2KeyRadio<'hardware, 'transmit, H, T>,
        session: Esp32s31Wpa2KeySession<'sequence>,
    ) -> Self {
        Self {
            radio,
            session,
            completion: None,
        }
    }

    pub const fn completion(&self) -> Option<TxCompletion> {
        self.completion
    }

    pub fn into_parts(self) -> Esp32s31Wpa2KeyPortParts<'hardware, 'transmit, 'sequence, H, T> {
        Esp32s31Wpa2KeyPortParts {
            hardware: self.radio.hardware,
            transmit: self.radio.transmit,
            sequences: self.session.sequences,
            completion: self.completion,
        }
    }
}

impl<H, T> Wpa2KeyInstallBackend for Esp32s31Wpa2KeyPort<'_, '_, '_, H, T>
where
    H: CcmpKeyHardware,
    T: Esp32s31Wpa2Transmit<H>,
{
    type Error = Esp32s31Wpa2KeyPortError<T::Error>;
    type InstalledKeys = Esp32s31InstalledWpa2Keys;

    fn install_keys(
        &mut self,
        request: &Wpa2StaKeyInstallRequest,
    ) -> Result<Self::InstalledKeys, Self::Error> {
        let pairwise = request.pairwise();
        let group = request.group();
        let Wpa2KeyKind::Group { key_id, .. } = group.kind() else {
            return Err(Esp32s31Wpa2KeyPortError::InvalidGroupKind);
        };
        let pairwise = install_sta_pairwise_ccmp(
            self.radio.hardware,
            *pairwise.peer(),
            pairwise.key().as_bytes(),
        )
        .map_err(Esp32s31Wpa2KeyPortError::Install)?;
        let group =
            match install_sta_group_ccmp(self.radio.hardware, key_id, group.key().as_bytes()) {
                Ok(group) => group,
                Err(error) => {
                    pairwise.clear(self.radio.hardware);
                    return Err(Esp32s31Wpa2KeyPortError::Install(error));
                }
            };
        Ok(Esp32s31InstalledWpa2Keys { pairwise, group })
    }

    fn rollback_keys(&mut self, keys: Self::InstalledKeys) -> Result<(), Self::Error> {
        keys.group.clear(self.radio.hardware);
        keys.pairwise.clear(self.radio.hardware);
        Ok(())
    }

    fn transmit_message4<'a>(
        &'a mut self,
        frame: &'a Wpa2TxFrame<DEFAULT_EAPOL_FRAME_CAPACITY>,
        keys: &'a mut Self::InstalledKeys,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a {
        async move {
            let completion = match self.session.message4_protection {
                Esp32s31Wpa2Message4Protection::Unprotected => {
                    self.radio
                        .transmit
                        .transmit_unprotected(
                            self.radio.hardware,
                            StaDataFrame {
                                source: self.session.station.station_address,
                                bssid: self.session.station.bssid,
                                destination: self.session.station.bssid,
                                sequence_number: self.session.sequences.take_non_qos(),
                                ether_type: 0x888e,
                                payload: frame.as_bytes(),
                            },
                        )
                        .await
                }
                Esp32s31Wpa2Message4Protection::PairwiseCcmp => {
                    let ccmp_header = keys.pairwise.next_tx_ccmp_header();
                    self.radio
                        .transmit
                        .transmit_protected(
                            self.radio.hardware,
                            StaProtectedDataFrame {
                                source: self.session.station.station_address,
                                bssid: self.session.station.bssid,
                                destination: self.session.station.bssid,
                                sequence_number: self
                                    .session
                                    .sequences
                                    .take_data(self.session.peer_qos.then_some(0))
                                    .expect("selected EAPOL sequence-number owner exists"),
                                user_priority: 7,
                                peer_qos: self.session.peer_qos,
                                ccmp_header,
                                ether_type: 0x888e,
                                payload: frame.as_bytes(),
                            },
                            LegacyTxQueue::Voice,
                            TxPhyRate::Legacy(LegacyRate::Dsss1MLong),
                            keys.pairwise.hardware_index(),
                        )
                        .await
                }
            }
            .map_err(Esp32s31Wpa2KeyPortError::Transmit)?;
            self.completion = Some(completion);
            if completion.status == 0 {
                Ok(())
            } else {
                Err(Esp32s31Wpa2KeyPortError::TxStatus(completion.status))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use open_esp_radio_wpa2::{
        EAPOL_KEY_FIXED_LEN, EAPOL_KEY_PACKET_LEN, EAPOL_PACKET_TYPE_KEY, RSN_KEY_DESCRIPTOR_TYPE,
    };

    use super::*;

    const LOCAL: [u8; 6] = [2, 0, 0, 0, 0, 1];
    const BSSID: [u8; 6] = [2, 0, 0, 0, 0, 2];

    #[test]
    fn eapol_copy_accepts_only_the_selected_station_link() {
        let station = Esp32s31Wpa2Station::new(LOCAL, BSSID);
        let mut frame = [0_u8; 160];
        frame[4..10].copy_from_slice(&LOCAL);
        frame[10..16].copy_from_slice(&BSSID);
        frame[24..32].copy_from_slice(&LLC_SNAP_EAPOL);
        let eapol = &mut frame[32..32 + EAPOL_KEY_PACKET_LEN];
        eapol[0] = 2;
        eapol[1] = EAPOL_PACKET_TYPE_KEY;
        eapol[2..4].copy_from_slice(&(EAPOL_KEY_FIXED_LEN as u16).to_be_bytes());
        eapol[4] = RSN_KEY_DESCRIPTOR_TYPE;
        eapol[5..7].copy_from_slice(&(2_u16 | (1 << 3) | (1 << 7)).to_be_bytes());

        let mpdu_length = 32 + EAPOL_KEY_PACKET_LEN;
        let copied = copy_station_eapol(&frame, mpdu_length, 24, station).unwrap();
        assert_eq!(copied.interface(), Wpa2Interface::Station);
        assert_eq!(copied.peer(), &BSSID);
        assert_eq!(copied.as_bytes(), &frame[32..mpdu_length]);

        frame[10] ^= 1;
        assert!(copy_station_eapol(&frame, mpdu_length, 24, station).is_none());
    }
}
