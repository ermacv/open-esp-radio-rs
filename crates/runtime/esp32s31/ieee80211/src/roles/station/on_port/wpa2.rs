//! The station's WPA2 four-way handshake and key installation on the port:
//! the portable [`RsnHandshakeBackend`] and [`RsnKeyInstallBackend`] the
//! runners of `oer-ieee80211-rsn-service` drive.
//!
//! EAPOL packets of the station's link come from the phase's event
//! consumer; Messages 2 and 4 are data frames through the station's client
//! ([`send_encoded`]); the keys are installed in the station's key slots of
//! the install, and a Message 4 sent under the new pairwise key carries the
//! CCMP header of the station's own packet-number allocator, at the
//! Espressif step (`oer_espressif_ieee80211_policy::ccmp`).

use oer_esp32s31_ieee80211_mac::crypto::StaGroupCcmpKeyMaterial;
use oer_esp32s31_ieee80211_sta::{
    connected::management_protection::StationManagementProtection,
    connected_rx::StaCcmpRxReplayEpoch,
    connection_coex::{ConnectionFrame, ConnectionFrameCoex},
    wpa2::{Wpa2Message4Protection, Wpa2Station, copy_station_eapol},
};
use oer_espressif_ieee80211_policy::ccmp::TX_PACKET_NUMBER_STEP;
use oer_ieee80211_lower_mac::{
    Ccmp128Key, GroupKeyId, IndividualAddress, LowerMacClient, LowerMacEvent, PhyRate, TxSeats,
};
use oer_ieee80211_mac::{
    ccmp::{CcmpKeyId, CcmpTxPacketNumber},
    phy::LegacyRate,
    sequence::SequenceNumber,
    station::{StaDataFrame, StaProtectedDataFrame, StaTxSequenceCounters},
};
use oer_ieee80211_rsn::{
    DEFAULT_EAPOL_FRAME_CAPACITY,
    frames::RsnTxFrame,
    keys::RsnKeyKind,
    runner::{RsnHandshakeBackend, RsnKeyInstallBackend, RsnRxProgress},
    supplicant::RsnStaKeyInstallRequest,
};
use oer_radio_port::Slot;

use super::{OneHeldFrame, SendError, StationOwner, StationParts, send_encoded, try_next_event};

/// The rate of an EAPOL frame: the lowest OFDM rate.
const EAPOL_RATE: PhyRate = PhyRate::Legacy(LegacyRate::Ofdm6M);
/// The EAPOL EtherType.
const EAPOL_ETHER_TYPE: u16 = 0x888e;

/// The link the handshake runs on: the station's address and its access
/// point's BSSID.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Wpa2Link {
    pub station: [u8; 6],
    pub bssid: [u8; 6],
}

/// The length of a data frame's MAC header: QoS Control after a QoS data
/// header, HT Control when its Order bit is set.
fn data_header_len(frame: &[u8]) -> Option<usize> {
    let control = u16::from_le_bytes(frame.get(0..2)?.try_into().ok()?);
    let qos = control & 0x00fc == 0x0088;
    let order = control & 0x8000 != 0;
    Some(24 + if qos { 2 } else { 0 } + if qos && order { 4 } else { 0 })
}

/// The EAPOL packet `frame` carries when it belongs to `link`.
fn link_eapol(
    frame: &[u8],
    link: Wpa2Link,
) -> Option<oer_ieee80211_rsn::OwnedEapolFrame<DEFAULT_EAPOL_FRAME_CAPACITY>> {
    // Data frames only: type two.
    if frame.first()? & 0x0c != 0x08 {
        return None;
    }
    copy_station_eapol(
        frame,
        frame.len(),
        data_header_len(frame)?,
        Wpa2Station::new(link.station, link.bssid),
    )
}

/// Why a handshake or key step failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PortWpa2Error<F> {
    Send(SendError<F>),
    Encode,
    /// The group key is not a group key of a valid identifier.
    GroupKind,
    /// The receive sequences do not make a replay epoch.
    ReceiveSequence,
    /// The key material is refused.
    KeyMaterial,
    /// A Message 4 under the new key exhausted its packet numbers.
    PacketNumber,
    /// The station's peer address is a group address.
    Peer,
    /// The station holds no free key slot of a kind.
    NoKeySlot,
    /// Message 4 was not acknowledged.
    TxStatus,
}

/// The Message 1/2/3 exchange of a station on the port.
pub struct PortHandshake<'s, 'p, 'id, B: StationParts, C> {
    owner: &'s mut StationOwner<'p, 'id, B>,
    seats: &'s mut TxSeats<'id, B::QueueKind, B::BufferKind>,
    frame: &'s mut [u8],
    held: OneHeldFrame<'s>,
    coex: C,
    link: Wpa2Link,
    /// Message 2 transmissions, for telemetry.
    pub message2_transmissions: u16,
}

impl<'s, 'p, 'id, B: StationParts, C> PortHandshake<'s, 'p, 'id, B, C> {
    pub fn new(
        owner: &'s mut StationOwner<'p, 'id, B>,
        seats: &'s mut TxSeats<'id, B::QueueKind, B::BufferKind>,
        frame: &'s mut [u8],
        held: &'s mut [u8],
        coex: C,
        link: Wpa2Link,
    ) -> Self {
        Self {
            owner,
            seats,
            frame,
            held: OneHeldFrame::new(held),
            coex,
            link,
            message2_transmissions: 0,
        }
    }
}

impl<'s, 'p, 'id, B, C> RsnHandshakeBackend for PortHandshake<'s, 'p, 'id, B, C>
where
    B: StationParts,
    C: ConnectionFrameCoex,
{
    type Error = PortWpa2Error<B::Fault>;

    async fn service_receive(&mut self) -> Result<RsnRxProgress, Self::Error> {
        let link = self.link;
        let mut completed = 0u32;
        if let Some(frame) = self.held.take() {
            completed += 1;
            if let Some(eapol) = link_eapol(frame, link) {
                return Ok(RsnRxProgress::eapol(completed, eapol));
            }
        }
        let mut phase = self
            .owner
            .phase()
            .ok_or(PortWpa2Error::Send(SendError::NotRunning))?;
        while let Some(event) = try_next_event(&mut *phase.events) {
            let event = match event {
                Ok(Ok(event)) => event,
                Ok(Err(_lost)) => continue,
                Err(poisoned) => {
                    return Err(PortWpa2Error::Send(SendError::Poisoned(poisoned.cause)));
                }
            };
            if let LowerMacEvent::Received { frame, .. } = B::view(&event) {
                completed += 1;
                if let Some(eapol) = link_eapol(frame, link) {
                    return Ok(RsnRxProgress::eapol(completed, eapol));
                }
            }
        }
        Ok(RsnRxProgress::drained(completed))
    }

    async fn restart_receive(&mut self) -> Result<(), Self::Error> {
        // The station keeps receiving its BSS for the whole handshake.
        Ok(())
    }

    async fn stop_receive(&mut self) -> Result<(), Self::Error> {
        // Key installation has no hardware boundary to wait for on the port:
        // a key removal waits for the attempts that use it.
        Ok(())
    }

    async fn transmit_message2<'a>(
        &'a mut self,
        frame: &'a RsnTxFrame<DEFAULT_EAPOL_FRAME_CAPACITY>,
        sequence_number: SequenceNumber,
    ) -> Result<(), Self::Error> {
        let _ = self.coex.connection_frame(ConnectionFrame::Eapol).await;
        let len = StaDataFrame {
            source: self.link.station,
            bssid: self.link.bssid,
            destination: self.link.bssid,
            sequence_number,
            ether_type: EAPOL_ETHER_TYPE,
            payload: frame.as_bytes(),
        }
        .encode(self.frame)
        .map_err(|_| PortWpa2Error::Encode)?;
        send_encoded(
            self.owner,
            self.seats,
            &self.frame[..len],
            EAPOL_RATE,
            None,
            &mut self.held,
        )
        .await
        .map_err(PortWpa2Error::Send)?;
        self.message2_transmissions = self.message2_transmissions.saturating_add(1);
        Ok(())
    }
}

/// The keys a station installed on the port, with what its connected data
/// path needs of them.
pub struct PortWpa2Keys<'id, B: StationParts> {
    pub pairwise: B::PairwiseKey<'id>,
    pub group: B::GroupKey<'id>,
    /// The packet numbers of the station's protected transmissions.
    pub packet_number: CcmpTxPacketNumber,
    pub group_material: StaGroupCcmpKeyMaterial,
    pub replay: StaCcmpRxReplayEpoch,
    /// Present when the association protects its management frames.
    pub management: Option<StationManagementProtection>,
}

/// The key slots a station installs its keys in, which a removal gives
/// back.
pub struct StationKeySlots<'id, B: StationParts> {
    pub pairwise: Option<Slot<'id, B::PairwiseKeyKind>>,
    pub group: Option<Slot<'id, B::GroupKeyKind>>,
}

/// Atomic PTK/GTK installation and Message 4 of a station on the port.
pub struct PortKeyInstall<'s, 'p, 'id, B: StationParts, C> {
    owner: &'s mut StationOwner<'p, 'id, B>,
    seats: &'s mut TxSeats<'id, B::QueueKind, B::BufferKind>,
    frame: &'s mut [u8],
    held: OneHeldFrame<'s>,
    coex: C,
    link: Wpa2Link,
    peer_qos: bool,
    sequences: &'s mut StaTxSequenceCounters,
    message4_protection: Wpa2Message4Protection,
    slots: &'s mut StationKeySlots<'id, B>,
}

impl<'s, 'p, 'id, B: StationParts, C> PortKeyInstall<'s, 'p, 'id, B, C> {
    #[allow(
        clippy::too_many_arguments,
        reason = "the installation's owners and policy"
    )]
    pub fn new(
        owner: &'s mut StationOwner<'p, 'id, B>,
        seats: &'s mut TxSeats<'id, B::QueueKind, B::BufferKind>,
        frame: &'s mut [u8],
        held: &'s mut [u8],
        coex: C,
        link: Wpa2Link,
        peer_qos: bool,
        sequences: &'s mut StaTxSequenceCounters,
        message4_protection: Wpa2Message4Protection,
        slots: &'s mut StationKeySlots<'id, B>,
    ) -> Self {
        Self {
            owner,
            seats,
            frame,
            held: OneHeldFrame::new(held),
            coex,
            link,
            peer_qos,
            sequences,
            message4_protection,
            slots,
        }
    }
}

impl<'s, 'p, 'id, B, C> RsnKeyInstallBackend for PortKeyInstall<'s, 'p, 'id, B, C>
where
    B: StationParts,
    C: ConnectionFrameCoex,
{
    type Error = PortWpa2Error<B::Fault>;
    type InstalledKeys = PortWpa2Keys<'id, B>;

    fn install_keys(
        &mut self,
        request: &RsnStaKeyInstallRequest,
    ) -> Result<Self::InstalledKeys, Self::Error> {
        let pairwise = request.pairwise();
        let group = request.group();
        let RsnKeyKind::Group { key_id, .. } = group.kind() else {
            return Err(PortWpa2Error::GroupKind);
        };
        let replay = StaCcmpRxReplayEpoch::new(
            *pairwise.receive_sequence(),
            key_id,
            *group.receive_sequence(),
        )
        .map_err(|_| PortWpa2Error::ReceiveSequence)?;
        let group_material = StaGroupCcmpKeyMaterial::new(key_id, *group.key().as_bytes())
            .map_err(|_| PortWpa2Error::KeyMaterial)?;
        let peer = IndividualAddress::new(*pairwise.peer()).ok_or(PortWpa2Error::Peer)?;
        let group_key_id = GroupKeyId::new(key_id).ok_or(PortWpa2Error::GroupKind)?;
        let pairwise_slot = self.slots.pairwise.take().ok_or(PortWpa2Error::NoKeySlot)?;
        let Some(group_slot) = self.slots.group.take() else {
            self.slots.pairwise = Some(pairwise_slot);
            return Err(PortWpa2Error::NoKeySlot);
        };
        let phase = self
            .owner
            .phase()
            .ok_or(PortWpa2Error::Send(SendError::NotRunning))?;
        let station = B::station(&phase.clients);
        let pairwise_key = station.install_pairwise_key(
            pairwise_slot,
            peer,
            Ccmp128Key(*pairwise.key().as_bytes()),
        );
        let group_key = station.install_group_key(
            group_slot,
            group_key_id,
            Ccmp128Key(*group.key().as_bytes()),
        );
        // With management frame protection the station keeps its temporal
        // key to open protected management frames, as the vendor does.
        let management = request.igtk().map(|igtk| {
            StationManagementProtection::new(*request.pairwise().key().as_bytes(), igtk)
        });
        oer_trace::emit(&oer_ieee80211_trace::LinkControlTrace {
            event: oer_ieee80211_trace::LinkEvent::KeysInstalled {
                group_key_id: key_id,
            },
        });
        Ok(PortWpa2Keys {
            pairwise: pairwise_key,
            group: group_key,
            packet_number: CcmpTxPacketNumber::new(TX_PACKET_NUMBER_STEP),
            group_material,
            replay,
            management,
        })
    }

    fn rollback_keys(&mut self, keys: Self::InstalledKeys) -> Result<(), Self::Error> {
        let phase = self
            .owner
            .phase()
            .ok_or(PortWpa2Error::Send(SendError::NotRunning))?;
        let station = B::station(&phase.clients);
        // No attempt in flight uses a key that failed before its Message 4
        // completed, so each removal ends at its first poll.
        let pairwise = super::now_ready(station.remove_pairwise_key(keys.pairwise));
        let group = super::now_ready(station.remove_group_key(keys.group));
        self.slots.pairwise = Some(pairwise);
        self.slots.group = Some(group);
        Ok(())
    }

    async fn transmit_message4<'a>(
        &'a mut self,
        frame: &'a RsnTxFrame<DEFAULT_EAPOL_FRAME_CAPACITY>,
        keys: &'a mut Self::InstalledKeys,
    ) -> Result<(), Self::Error> {
        let _ = self.coex.connection_frame(ConnectionFrame::Eapol).await;
        let (len, key) = match self.message4_protection {
            Wpa2Message4Protection::Unprotected => (
                StaDataFrame {
                    source: self.link.station,
                    bssid: self.link.bssid,
                    destination: self.link.bssid,
                    sequence_number: self.sequences.take_non_qos(),
                    ether_type: EAPOL_ETHER_TYPE,
                    payload: frame.as_bytes(),
                }
                .encode(self.frame)
                .map_err(|_| PortWpa2Error::Encode)?,
                None,
            ),
            Wpa2Message4Protection::PairwiseCcmp => {
                let ccmp_header = keys
                    .packet_number
                    .next_header(CcmpKeyId::PAIRWISE)
                    .map_err(|_| PortWpa2Error::PacketNumber)?;
                let sequence_number = self
                    .sequences
                    .take_data(self.peer_qos.then_some(0))
                    .ok_or(PortWpa2Error::Encode)?;
                (
                    StaProtectedDataFrame {
                        source: self.link.station,
                        bssid: self.link.bssid,
                        destination: self.link.bssid,
                        sequence_number,
                        user_priority: 7,
                        peer_qos: self.peer_qos,
                        ccmp_header,
                        ether_type: EAPOL_ETHER_TYPE,
                        payload: frame.as_bytes(),
                    }
                    .encode(self.frame)
                    .map_err(|_| PortWpa2Error::Encode)?,
                    Some(&keys.pairwise),
                )
            }
        };
        let completion = send_encoded(
            self.owner,
            self.seats,
            &self.frame[..len],
            EAPOL_RATE,
            key,
            &mut self.held,
        )
        .await
        .map_err(PortWpa2Error::Send)?;
        if completion.status == oer_ieee80211_lower_mac::TxStatus::Success {
            Ok(())
        } else {
            Err(PortWpa2Error::TxStatus)
        }
    }
}
