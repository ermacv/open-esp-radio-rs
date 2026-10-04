//! Authentication and Association over the lower-MAC port.

use oer_ieee80211_lower_mac::{KeySelector, MacAddress, ReceiveFilter};
use oer_ieee80211_mac::{
    qos::WmmAccessCategory,
    scan::ScanRecord,
    sequence::SequenceNumber,
    station::{
        AssociationCapabilities, AssociationRequest, OpenAuthenticationRequest,
        SaeAuthenticationFrame, SelectedRsn, association::PhyMode,
    },
};
use oer_ieee80211_sta::join::{
    StaJoinBackend, StaJoinRxDirective, StaJoinRxObserver, association::StaAssociationAttempt,
    authentication::StaAuthenticationAttempt, sae::StaSaeTransmission,
};

use super::{
    link::{PortConnectionFrame, PortError, PortInput, PortLink, PortLinkError, PortStationEnv},
    wire,
};

/// What an Association Request carries besides its sequence number.
#[derive(Clone, Copy, Debug)]
pub struct PortAssociation<'a> {
    pub access_point: &'a ScanRecord,
    /// The security elements the station selected for the access point.
    pub security: &'a SelectedRsn,
    pub phy: PhyMode,
    /// The station's HT, HE and WMM elements.
    pub capabilities: &'a AssociationCapabilities,
    pub listen_interval: u16,
    /// The HE power elements an HE association carries.
    pub he_power: Option<PortHePower>,
}

/// The station's HE power elements, from its calibrated transmit power:
/// an HE Association Request needs both.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PortHePower {
    pub power_capability: oer_ieee80211_mac::station::association::StaPowerCapability,
    pub ul_mu: oer_ieee80211_mac::station::association::HeUlMuPowerCapability,
}

/// Octets of the longest management frame the station sends while joining.
const JOIN_FRAME_CAPACITY: usize = 512;

/// The [`StaJoinBackend`] of the join runner over the lower-MAC port.
///
/// Authentication and Association frames are sent through the transmit
/// planner at the management rate; the runner's receive service reads the
/// frames the station interface admits with its access point's BSSID
/// (`ReceiveFilter::BSS_MEMBER`) and hands each received MPDU to the
/// runner's observer, a management frame as `Some`. Receiving stops by
/// narrowing the filter to nothing; a frame that arrived before a stop
/// stays for the next phase, so an Association success hands the EAPOL
/// Message 1 behind it to the handshake.
pub struct PortJoin<'a, 'p, X: PortStationEnv> {
    link: &'a mut PortLink<'p, X>,
    bssid: MacAddress,
    association: Option<PortAssociation<'a>>,
}

impl<'a, 'p, X: PortStationEnv> PortJoin<'a, 'p, X> {
    /// Authentication with `bssid`.
    pub fn new(link: &'a mut PortLink<'p, X>, bssid: MacAddress) -> Self {
        Self {
            link,
            bssid,
            association: None,
        }
    }

    /// Association with the access point `association` names.
    pub fn with_association(mut self, association: PortAssociation<'a>) -> Self {
        self.bssid = association.access_point.bssid;
        self.association = Some(association);
        self
    }

    async fn send(&mut self, frame: &[u8]) -> Result<(), PortLinkError<PortError<X>>> {
        let rate = self.link.config().management_rate;
        // The join runner times the response; an unacknowledged request is
        // one the access point did not answer.
        let connection = if frame.first() == Some(&0xb0) {
            PortConnectionFrame::Authentication
        } else {
            PortConnectionFrame::Association
        };
        self.link
            .transmit_connection_frame(
                connection,
                frame,
                KeySelector::Plaintext,
                WmmAccessCategory::Voice,
                rate,
            )
            .await
            .map(|_| ())
    }
}

impl<X: PortStationEnv> StaJoinBackend for PortJoin<'_, '_, X> {
    type Error = PortLinkError<PortError<X>>;

    async fn start_receive(&mut self) -> Result<(), Self::Error> {
        self.link
            .configure(Some(self.bssid), ReceiveFilter::BSS_MEMBER)
    }

    async fn stop_receive(&mut self) -> Result<(), Self::Error> {
        self.link.configure(Some(self.bssid), ReceiveFilter::NONE)
    }

    async fn transmit_open_authentication(
        &mut self,
        attempt: StaAuthenticationAttempt,
    ) -> Result<(), Self::Error> {
        let mut frame = [0_u8; JOIN_FRAME_CAPACITY];
        let length = OpenAuthenticationRequest {
            source: self.link.config().address,
            bssid: self.bssid,
            sequence_number: attempt.sequence_number,
        }
        .encode(&mut frame)
        .map_err(PortLinkError::Frame)?;
        self.send(&frame[..length]).await
    }

    async fn transmit_association(
        &mut self,
        attempt: StaAssociationAttempt,
    ) -> Result<(), Self::Error> {
        let association = self.association.ok_or(PortLinkError::MissingState)?;
        let mut frame = [0_u8; JOIN_FRAME_CAPACITY];
        let length = AssociationRequest {
            source: self.link.config().address,
            access_point: association.access_point,
            sequence_number: attempt.sequence_number,
            listen_interval: association.listen_interval,
            phy: association.phy,
            security: association.security,
            // Only an HE association carries them; the encoder refuses an HE
            // request without them.
            power_capability: association
                .he_power
                .filter(|_| association.phy == PhyMode::He20)
                .map(|power| power.power_capability),
            he_ul_mu_power: association
                .he_power
                .filter(|_| association.phy == PhyMode::He20)
                .map(|power| power.ul_mu),
        }
        .encode(&mut frame, association.capabilities)
        .map_err(PortLinkError::Association)?;
        self.send(&frame[..length]).await
    }

    async fn transmit_sae_authentication<'b>(
        &'b mut self,
        sequence_number: SequenceNumber,
        transmission: &'b StaSaeTransmission,
    ) -> Result<(), Self::Error> {
        let mut frame = [0_u8; JOIN_FRAME_CAPACITY];
        let length = SaeAuthenticationFrame {
            source: self.link.config().address,
            bssid: self.bssid,
            sequence_number,
            transaction: transmission.transaction,
            status_code: transmission.status_code,
            body: transmission.body(),
        }
        .encode(&mut frame)
        .map_err(PortLinkError::Frame)?;
        self.send(&frame[..length]).await
    }

    async fn service_receive<'b, O>(&'b mut self, observer: &'b mut O) -> Result<(), Self::Error>
    where
        O: StaJoinRxObserver + 'b,
    {
        while let Some(input) = self.link.try_input().await {
            let frame = match input {
                PortInput::Frame(frame) => frame,
                PortInput::Poisoned => return Err(PortLinkError::Poisoned),
                PortInput::Tbtt(_) | PortInput::EventsLost => continue,
            };
            let bytes = frame.bytes();
            let management = wire::is_management(bytes).then_some(bytes);
            if observer.observe_completed(management) == StaJoinRxDirective::Stop {
                break;
            }
        }
        Ok(())
    }
}
