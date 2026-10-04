//! The WPA2 four-way handshake and key installation over the lower-MAC port.

use oer_ieee80211_lower_mac::{
    Cipher, Ieee80211LowerMacPort, KeyHandle, KeyInstall, KeyScope, KeySelector, LowerMacSetting,
    MacAddress,
};
use oer_ieee80211_mac::{
    ccmp::{CcmpKeyId, CcmpTxPacketNumber},
    data::{DataInterfaceRole, plan_data_decapsulation},
    qos::WmmAccessCategory,
    sequence::SequenceNumber,
    station::{StaDataFrame, StaProtectedDataFrame, StaTxSequenceCounters},
};
use oer_ieee80211_rsn::{
    OwnedEapolFrame, RsnInterface,
    aes::{AsyncRsnKeyUnwrap, RsnUnwrappedKeyData},
    bip::BipReceiver,
    frames::RsnTxFrame,
    keys::{RsnKeyInstall, RsnKeyKind},
    runner::{
        RSN_HANDSHAKE_EAPOL_CAPACITY, RsnHandshakeBackend, RsnKeyInstallBackend, RsnRxProgress,
    },
    supplicant::RsnStaKeyInstallRequest,
};
use oer_ieee80211_sta::attempt::Wpa2Message4Protection;

use super::{
    link::{PortError, PortInput, PortLink, PortLinkError, PortStationEnv},
    wire,
};

/// The EtherType of EAPOL in transmission order, as the MPDU carries it.
pub(crate) const EAPOL_ETHER_TYPE: u16 = 0x888e;
/// Octets of the longest EAPOL MPDU the station sends.
const EAPOL_FRAME_CAPACITY: usize = RSN_HANDSHAKE_EAPOL_CAPACITY + 64;

/// The keys a completed handshake installed through the port.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PortKeys {
    /// The pairwise key with the access point.
    pub pairwise: KeyHandle,
    /// The group key.
    pub group: KeyHandle,
    pub group_key_id: u8,
    /// The group key's receive sequence counter from Message 3.
    pub group_receive_sequence: [u8; 8],
}

/// What a completed handshake installed: the port's keys and, for an
/// association that protects its management frames, the BIP receive state
/// of Message 3's IGTK, which the station keeps in software.
pub struct PortInstalledKeys {
    pub keys: PortKeys,
    pub bip: Option<BipReceiver>,
}

/// The EAPOL payload of an unprotected data MPDU `bssid` sent.
pub(crate) fn eapol_payload(frame: &[u8], bssid: MacAddress) -> Option<&[u8]> {
    if !wire::is_data(frame)
        || wire::is_protected(frame)
        || wire::is_null_data(frame)
        || wire::address2(frame)? != bssid
    {
        return None;
    }
    let header = wire::data_header_len(frame);
    let plan = plan_data_decapsulation(
        DataInterfaceRole::Station,
        frame,
        header,
        frame.len().checked_sub(header)?,
    )
    .ok()?;
    (plan.ether_type == EAPOL_ETHER_TYPE)
        .then(|| frame.get(plan.payload_offset..plan.payload_offset + plan.payload_length))
        .flatten()
}

/// The [`RsnHandshakeBackend`] of the handshake runner over the port.
///
/// Receive service reads the frames the station interface admits and
/// returns the first EAPOL-Key frame of its access point; every other frame
/// counts as completed. The interface keeps receiving throughout: there is
/// no descriptor ring to restart or stop, so those edges succeed without
/// action. Message 2 is an unprotected data MPDU sent through the planner at
/// the management rate.
pub struct PortHandshake<'a, 'p, X: PortStationEnv> {
    link: &'a mut PortLink<'p, X>,
    bssid: MacAddress,
}

impl<'a, 'p, X: PortStationEnv> PortHandshake<'a, 'p, X> {
    pub fn new(link: &'a mut PortLink<'p, X>, bssid: MacAddress) -> Self {
        Self { link, bssid }
    }
}

/// Send one EAPOL frame to `bssid` in an unprotected data MPDU.
async fn send_eapol<X: PortStationEnv>(
    link: &mut PortLink<'_, X>,
    bssid: MacAddress,
    sequence_number: SequenceNumber,
    eapol: &[u8],
) -> Result<(), PortLinkError<PortError<X>>> {
    let config = *link.config();
    let mut frame = [0_u8; EAPOL_FRAME_CAPACITY];
    let length = StaDataFrame {
        source: config.address,
        bssid,
        destination: bssid,
        sequence_number,
        ether_type: EAPOL_ETHER_TYPE,
        payload: eapol,
    }
    .encode(&mut frame)
    .map_err(PortLinkError::Frame)?;
    link.transmit(
        &frame[..length],
        KeySelector::Plaintext,
        WmmAccessCategory::Voice,
        config.management_rate,
    )
    .await
    .map(|_| ())
}

impl<X: PortStationEnv> RsnHandshakeBackend for PortHandshake<'_, '_, X> {
    type Error = PortLinkError<PortError<X>>;

    async fn service_receive(&mut self) -> Result<RsnRxProgress, Self::Error> {
        let mut completed = 0_u32;
        while let Some(input) = self.link.try_input().await {
            let frame = match input {
                PortInput::Frame(frame) => frame,
                PortInput::Poisoned => return Err(PortLinkError::Poisoned),
                PortInput::Tbtt(_) | PortInput::EventsLost => continue,
            };
            completed = completed.saturating_add(1);
            if let Some(eapol) = eapol_payload(frame.bytes(), self.bssid)
                && let Ok(eapol) = OwnedEapolFrame::<RSN_HANDSHAKE_EAPOL_CAPACITY>::try_copy(
                    RsnInterface::Station,
                    self.bssid,
                    eapol,
                )
            {
                return Ok(RsnRxProgress::eapol(completed, eapol));
            }
        }
        Ok(RsnRxProgress::drained(completed))
    }

    async fn restart_receive(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    async fn stop_receive(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    async fn transmit_message2<'b>(
        &'b mut self,
        frame: &'b RsnTxFrame<RSN_HANDSHAKE_EAPOL_CAPACITY>,
        sequence_number: SequenceNumber,
    ) -> Result<(), Self::Error> {
        send_eapol(self.link, self.bssid, sequence_number, frame.as_bytes()).await
    }
}

/// The [`RsnKeyInstallBackend`] of the key-install runner over the port.
///
/// The pairwise and group keys go to the port through
/// [`install_key`](oer_ieee80211_lower_mac::Ieee80211LowerMacPort::install_key)
/// as CCMP-128 keys; a failed group install removes the pairwise key again,
/// so a failed install leaves none. Message 4 is sent unprotected or under
/// the new pairwise key, with a CCMP header from the pairwise key's
/// packet-number allocator.
pub struct PortKeyInstall<'a, 'p, X: PortStationEnv> {
    link: &'a mut PortLink<'p, X>,
    bssid: MacAddress,
    sequences: &'a mut StaTxSequenceCounters,
    packet_number: &'a mut CcmpTxPacketNumber,
    protection: Wpa2Message4Protection,
}

impl<'a, 'p, X: PortStationEnv> PortKeyInstall<'a, 'p, X> {
    pub fn new(
        link: &'a mut PortLink<'p, X>,
        bssid: MacAddress,
        sequences: &'a mut StaTxSequenceCounters,
        packet_number: &'a mut CcmpTxPacketNumber,
        protection: Wpa2Message4Protection,
    ) -> Self {
        Self {
            link,
            bssid,
            sequences,
            packet_number,
            protection,
        }
    }

    fn install(
        &self,
        key: &RsnKeyInstall,
        scope: KeyScope,
    ) -> Result<KeyHandle, PortLinkError<PortError<X>>> {
        self.link
            .port()
            .install_key(KeyInstall {
                vif: self.link.config().vif,
                cipher: Cipher::Ccmp128,
                scope,
                key: key.key().as_bytes(),
            })
            .map_err(PortLinkError::Port)?
            .map_err(PortLinkError::Setting)
    }
}

impl<X: PortStationEnv> RsnKeyInstallBackend for PortKeyInstall<'_, '_, X> {
    type Error = PortLinkError<PortError<X>>;
    type InstalledKeys = PortInstalledKeys;

    fn install_keys(
        &mut self,
        request: &RsnStaKeyInstallRequest,
    ) -> Result<PortInstalledKeys, Self::Error> {
        let RsnKeyKind::Group { key_id, .. } = request.group().kind() else {
            return Err(PortLinkError::MissingState);
        };
        let pairwise = self.install(request.pairwise(), KeyScope::Pairwise { peer: self.bssid })?;
        let group = match self.install(request.group(), KeyScope::Group { key_id }) {
            Ok(group) => group,
            Err(error) => {
                self.link.apply(LowerMacSetting::RemoveKey(pairwise))?;
                return Err(error);
            }
        };
        Ok(PortInstalledKeys {
            keys: PortKeys {
                pairwise,
                group,
                group_key_id: key_id,
                group_receive_sequence: *request.group().receive_sequence(),
            },
            bip: request.igtk().map(BipReceiver::new),
        })
    }

    fn rollback_keys(&mut self, installed: PortInstalledKeys) -> Result<(), Self::Error> {
        let keys = installed.keys;
        let group = self.link.apply(LowerMacSetting::RemoveKey(keys.group));
        self.link.apply(LowerMacSetting::RemoveKey(keys.pairwise))?;
        group
    }

    async fn transmit_message4<'b>(
        &'b mut self,
        frame: &'b RsnTxFrame<RSN_HANDSHAKE_EAPOL_CAPACITY>,
        installed: &'b mut PortInstalledKeys,
    ) -> Result<(), Self::Error> {
        let keys = installed.keys;
        let sequence_number = self.sequences.take_non_qos();
        match self.protection {
            Wpa2Message4Protection::Unprotected => {
                send_eapol(self.link, self.bssid, sequence_number, frame.as_bytes()).await
            }
            Wpa2Message4Protection::PairwiseCcmp => {
                send_protected_eapol(
                    self.link,
                    self.bssid,
                    sequence_number,
                    self.packet_number,
                    keys.pairwise,
                    frame.as_bytes(),
                )
                .await
            }
        }
    }
}

/// Send one EAPOL frame to `bssid` in a data MPDU under the pairwise key,
/// with a CCMP header from its packet-number allocator.
pub(crate) async fn send_protected_eapol<X: PortStationEnv>(
    link: &mut PortLink<'_, X>,
    bssid: MacAddress,
    sequence_number: SequenceNumber,
    packet_number: &mut CcmpTxPacketNumber,
    pairwise: KeyHandle,
    eapol: &[u8],
) -> Result<(), PortLinkError<PortError<X>>> {
    let config = *link.config();
    let ccmp_header = packet_number
        .next_header(CcmpKeyId::new(0).expect("key identifier zero"))
        .map_err(PortLinkError::PacketNumber)?;
    let mut mpdu = [0_u8; EAPOL_FRAME_CAPACITY];
    let length = StaProtectedDataFrame {
        source: config.address,
        bssid,
        destination: bssid,
        sequence_number,
        user_priority: 7,
        peer_qos: false,
        ccmp_header,
        ether_type: EAPOL_ETHER_TYPE,
        payload: eapol,
    }
    .encode(&mut mpdu)
    .map_err(PortLinkError::Frame)?;
    link.transmit(
        &mpdu[..length],
        KeySelector::Key(pairwise),
        WmmAccessCategory::Voice,
        config.management_rate,
    )
    .await
    .map(|_| ())
}

/// An [`AsyncRsnKeyUnwrap`] borrowed for one handshake.
pub(crate) struct BorrowedUnwrap<'a, U>(pub(crate) &'a mut U);

impl<U: AsyncRsnKeyUnwrap> AsyncRsnKeyUnwrap for BorrowedUnwrap<'_, U> {
    type Error = U::Error;

    async fn unwrap_key_data<'a>(
        &'a mut self,
        kek: &'a [u8; 16],
        encrypted: &'a [u8],
    ) -> Result<RsnUnwrappedKeyData, Self::Error> {
        self.0.unwrap_key_data(kek, encrypted).await
    }
}
