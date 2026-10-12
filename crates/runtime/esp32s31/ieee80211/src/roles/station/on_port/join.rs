//! The station's Authentication and Association on the port: the portable
//! [`StaJoinBackend`] the `StaJoinRunner` of `oer-ieee80211-sta-service`
//! drives.
//!
//! The station receives its access point's frames (the BSS's receive rules
//! with the access point's BSSID) from the phase's event consumer, and each
//! request is an individual management frame through the station's client
//! ([`send_encoded`]), its coexistence request made to the radio system
//! first as before.

use oer_esp32s31_ieee80211::ordinary_tx::WifiTxPowerProfile;
use oer_esp32s31_ieee80211_sta::{
    association::StaAssociationProfile,
    association::esp32s31_sta_association_profile,
    connection_coex::{ConnectionFrame, ConnectionFrameCoex},
};
use oer_ieee80211_lower_mac::{
    IndividualAddress, LowerMacClient, LowerMacEvent, PhyRate, ReceiveFilter, StationClient,
    StationReceive, SupportedFilter, TxCompletion, TxSeats,
};
use oer_ieee80211_mac::{
    phy::LegacyRate,
    scan::ScanRecord,
    sequence::SequenceNumber,
    station::{
        AssociationRequest, OpenAuthenticationRequest, SaeAuthenticationFrame, SelectedRsn,
        association::Preference,
    },
};
use oer_ieee80211_sta::join::{
    StaJoinBackend, StaJoinRxDirective, StaJoinRxObserver, association::StaAssociationAttempt,
    authentication::StaAuthenticationAttempt, sae::StaSaeTransmission,
};

use super::{OneHeldFrame, SendError, StationOwner, StationParts, send_encoded, try_next_event};

/// The rate of a pre-connected management frame: the lowest OFDM rate.
const JOIN_RATE: PhyRate = PhyRate::Legacy(LegacyRate::Ofdm6M);

/// Value-only observation points of a join on the port, which do not
/// participate in its policy: applications and HIL may keep bounded
/// evidence.
pub trait PortJoinObserver {
    fn authentication_transmitted(&mut self, _completion: TxCompletion) {}

    fn association_profile_selected(&mut self, _profile: StaAssociationProfile) {}

    fn association_transmitted(&mut self, _completion: TxCompletion) {}
}

impl PortJoinObserver for () {}

/// The station and its peer for one Authentication and Association.
#[derive(Clone, Copy)]
pub struct JoinStation {
    pub access_point: ScanRecord,
    pub association_preference: Preference,
    pub listen_interval: u16,
    pub rsn: SelectedRsn,
}

/// Why a join step failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PortJoinError<F> {
    /// A frame could not be sent.
    Send(SendError<F>),
    /// The access point's BSSID is a group address.
    Bssid,
    /// A request does not encode.
    Encode,
    /// The association profile could not be selected.
    AssociationProfile,
}

/// The Authentication and Association of a station on the port.
pub struct PortJoin<'s, 'p, 'id, B: StationParts, P, C, O> {
    owner: &'s mut StationOwner<'p, 'id, B>,
    seats: &'s mut TxSeats<'id, B::QueueKind, B::BufferKind>,
    /// The encoding scratch of one request.
    frame: &'s mut [u8],
    held: OneHeldFrame<'s>,
    power: &'s P,
    coex: C,
    observer: O,
    station: JoinStation,
}

impl<'s, 'p, 'id, B: StationParts, P, C, O> PortJoin<'s, 'p, 'id, B, P, C, O> {
    #[allow(clippy::too_many_arguments, reason = "the join's owners and policy")]
    pub fn new(
        owner: &'s mut StationOwner<'p, 'id, B>,
        seats: &'s mut TxSeats<'id, B::QueueKind, B::BufferKind>,
        frame: &'s mut [u8],
        held: &'s mut [u8],
        power: &'s P,
        coex: C,
        observer: O,
        station: JoinStation,
    ) -> Self {
        Self {
            owner,
            seats,
            frame,
            held: OneHeldFrame::new(held),
            power,
            coex,
            observer,
            station,
        }
    }

    /// The station's address, which its client knows.
    fn address(&mut self) -> Result<[u8; 6], PortJoinError<B::Fault>> {
        let phase = self
            .owner
            .phase()
            .ok_or(PortJoinError::Send(SendError::NotRunning))?;
        Ok(B::station(&phase.clients).address().get())
    }
}

impl<'s, 'p, 'id, B, P, C, O> StaJoinBackend for PortJoin<'s, 'p, 'id, B, P, C, O>
where
    B: StationParts,
    P: WifiTxPowerProfile,
    C: ConnectionFrameCoex,
    O: PortJoinObserver,
{
    type Error = PortJoinError<B::Fault>;

    async fn start_receive(&mut self) -> Result<(), Self::Error> {
        let bssid =
            IndividualAddress::new(self.station.access_point.bssid).ok_or(PortJoinError::Bssid)?;
        let filter = SupportedFilter::new(ReceiveFilter::BSS_MEMBER)
            .ok_or(PortJoinError::Send(SendError::Frame))?;
        let phase = self
            .owner
            .phase()
            .ok_or(PortJoinError::Send(SendError::NotRunning))?;
        B::station(&phase.clients)
            .configure(StationReceive::Bss { bssid, filter })
            .await;
        Ok(())
    }

    async fn stop_receive(&mut self) -> Result<(), Self::Error> {
        // Association success leaves reception running for the handshake;
        // a failed join turns it off.
        let phase = self
            .owner
            .phase()
            .ok_or(PortJoinError::Send(SendError::NotRunning))?;
        B::station(&phase.clients)
            .configure(StationReceive::Off)
            .await;
        Ok(())
    }

    async fn transmit_open_authentication(
        &mut self,
        attempt: StaAuthenticationAttempt,
    ) -> Result<(), Self::Error> {
        let _ = self
            .coex
            .connection_frame(ConnectionFrame::Authentication)
            .await;
        let source = self.address()?;
        let len = OpenAuthenticationRequest {
            source,
            bssid: self.station.access_point.bssid,
            sequence_number: attempt.sequence_number,
        }
        .encode(self.frame)
        .map_err(|_| PortJoinError::Encode)?;
        let completion = send_encoded(
            self.owner,
            self.seats,
            &self.frame[..len],
            JOIN_RATE,
            &mut self.held,
        )
        .await
        .map_err(PortJoinError::Send)?;
        self.observer.authentication_transmitted(completion);
        Ok(())
    }

    async fn transmit_association(
        &mut self,
        attempt: StaAssociationAttempt,
    ) -> Result<(), Self::Error> {
        let profile = esp32s31_sta_association_profile(
            &self.station.access_point,
            self.station.association_preference,
            self.power,
        )
        .map_err(|_| PortJoinError::AssociationProfile)?;
        self.observer.association_profile_selected(profile);
        let _ = self
            .coex
            .connection_frame(ConnectionFrame::Association)
            .await;
        let source = self.address()?;
        let len = AssociationRequest {
            source,
            access_point: &self.station.access_point,
            sequence_number: attempt.sequence_number,
            listen_interval: self.station.listen_interval,
            phy: profile.phy,
            security: &self.station.rsn,
            power_capability: profile.power_capability,
            he_ul_mu_power: profile.he_ul_mu_power,
        }
        .encode(
            self.frame,
            &oer_esp32s31_ieee80211_sta::profile::ASSOCIATION_CAPABILITIES,
        )
        .map_err(|_| PortJoinError::Encode)?;
        let completion = send_encoded(
            self.owner,
            self.seats,
            &self.frame[..len],
            JOIN_RATE,
            &mut self.held,
        )
        .await
        .map_err(PortJoinError::Send)?;
        self.observer.association_transmitted(completion);
        Ok(())
    }

    async fn transmit_sae_authentication<'a>(
        &'a mut self,
        sequence_number: SequenceNumber,
        transmission: &'a StaSaeTransmission,
    ) -> Result<(), Self::Error> {
        let _ = self
            .coex
            .connection_frame(ConnectionFrame::Authentication)
            .await;
        let source = self.address()?;
        let len = SaeAuthenticationFrame {
            source,
            bssid: self.station.access_point.bssid,
            sequence_number,
            transaction: transmission.transaction,
            status_code: transmission.status_code,
            body: transmission.body(),
        }
        .encode(self.frame)
        .map_err(|_| PortJoinError::Encode)?;
        let completion = send_encoded(
            self.owner,
            self.seats,
            &self.frame[..len],
            JOIN_RATE,
            &mut self.held,
        )
        .await
        .map_err(PortJoinError::Send)?;
        self.observer.authentication_transmitted(completion);
        Ok(())
    }

    async fn service_receive<'a, V>(&'a mut self, observer: &'a mut V) -> Result<(), Self::Error>
    where
        V: StaJoinRxObserver + 'a,
    {
        // A frame that arrived while a request waited for its completion
        // comes first.
        if let Some(frame) = self.held.take()
            && observer.observe_completed(Some(frame)) == StaJoinRxDirective::Stop
        {
            return Ok(());
        }
        let mut phase = self
            .owner
            .phase()
            .ok_or(PortJoinError::Send(SendError::NotRunning))?;
        while let Some(event) = try_next_event(&mut *phase.events) {
            let event = match event {
                Ok(Ok(event)) => event,
                Ok(Err(_lost)) => continue,
                Err(poisoned) => {
                    return Err(PortJoinError::Send(SendError::Poisoned(poisoned.cause)));
                }
            };
            let management = match B::view(&event) {
                // Management frames: type zero.
                LowerMacEvent::Received { frame, .. }
                    if frame.first().is_some_and(|fc| fc & 0x0c == 0) =>
                {
                    Some(frame)
                }
                LowerMacEvent::Received { .. } => None,
                _ => continue,
            };
            if observer.observe_completed(management) == StaJoinRxDirective::Stop {
                return Ok(());
            }
        }
        Ok(())
    }
}
