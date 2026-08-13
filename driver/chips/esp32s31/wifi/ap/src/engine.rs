//! One bounded AP MAC epoch above DMA/IRQ and below the Embassy actor.

use crate::{
    beacon::Esp32s31ApBeacon,
    security::{
        Esp32s31ApPairwiseKeyStorage, Esp32s31ApSecurity, Esp32s31ApSecurityError,
        Esp32s31ApSecurityStopReport,
    },
};
use open_esp_radio_esp32s31_pac::RadioRegisters;
use open_esp_radio_esp32s31_wifi_mac::{
    ap_policy::{ApRxPolicyHardware, configure_ap_receive_policy},
    crypto::{CcmpKeyHardware, CryptoKeyError},
};
use open_esp_radio_ieee80211::{
    ap::{
        ApAssociationResponseError, ApDataFrame, ApDataFrameError, ApManagementRequest,
        ApProtectedDataFrame, parse_ap_management_request, write_bg_association_response_frame,
        write_open_authentication_response,
    },
    beacon::{ApBeaconBuildError, WPA2_BEACON_CAPACITY, write_wpa2_erp_beacon},
    ssid::WifiSsid,
};
use open_esp_radio_wifi_ap::{
    AccessPointService, ApMlmeAction, ApPeerStatus, ApServiceError, ApWpa2Error, ApWpa2Progress,
};
use open_esp_radio_wpa2::{OwnedEapolFrame, frames::Wpa2TxFrame};

pub trait Esp32s31ApRuntimeHardware: CcmpKeyHardware + ApRxPolicyHardware {
    fn stop_ap_tsf(&mut self);
}

impl Esp32s31ApRuntimeHardware for RadioRegisters {
    fn stop_ap_tsf(&mut self) {
        self.stop_softap_tsf();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31ApEngineError {
    Beacon(ApBeaconBuildError),
    Crypto(CryptoKeyError),
    Security(Esp32s31ApSecurityError),
    Service(ApServiceError),
    Frame(ApAssociationResponseError),
    DataFrame(ApDataFrameError),
    Wpa2(ApWpa2Error),
}

impl From<ApAssociationResponseError> for Esp32s31ApEngineError {
    fn from(error: ApAssociationResponseError) -> Self {
        Self::Frame(error)
    }
}

impl From<ApDataFrameError> for Esp32s31ApEngineError {
    fn from(error: ApDataFrameError) -> Self {
        Self::DataFrame(error)
    }
}

impl From<ApServiceError> for Esp32s31ApEngineError {
    fn from(error: ApServiceError) -> Self {
        Self::Service(error)
    }
}

impl From<Esp32s31ApSecurityError> for Esp32s31ApEngineError {
    fn from(error: Esp32s31ApSecurityError) -> Self {
        Self::Security(error)
    }
}

impl From<ApWpa2Error> for Esp32s31ApEngineError {
    fn from(error: ApWpa2Error) -> Self {
        Self::Wpa2(error)
    }
}

pub struct Esp32s31ApEngineStartFailure<'storage> {
    pub service: AccessPointService<'storage>,
    pub beacon_storage: &'storage mut [u8; WPA2_BEACON_CAPACITY],
    pub pairwise_storage: &'storage mut Esp32s31ApPairwiseKeyStorage,
    pub error: Esp32s31ApEngineError,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31ApManagementOutcome {
    Ignored,
    Response { len: usize, begin_wpa2: bool },
    PeerRemoved { peer: [u8; 6] },
}

pub enum Esp32s31ApWpa2Outcome<const N: usize> {
    None,
    Transmit(Wpa2TxFrame<N>),
    PeerAuthorized { peer: [u8; 6] },
    DeauthenticatePeer { peer: [u8; 6] },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31ApProtectedFrame {
    pub length: usize,
    pub hardware_key_selector: u8,
}

pub struct Esp32s31ApEngineStop<'storage> {
    pub service: AccessPointService<'storage>,
    pub beacon_storage: &'storage mut [u8; WPA2_BEACON_CAPACITY],
    pub pairwise_storage: &'storage mut Esp32s31ApPairwiseKeyStorage,
    pub security: Esp32s31ApSecurityStopReport,
    pub report: Esp32s31ApEngineReport,
}

/// Value-only observations from one AP ownership epoch.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Esp32s31ApEngineReport {
    pub beacons_prepared: u32,
    pub authentication_responses_prepared: u32,
    pub association_responses_prepared: u32,
    /// Successful controlled-port openings during the epoch.
    pub authorized_peers: u32,
    /// Largest number of simultaneously admitted peers during the epoch.
    pub maximum_associated_peers: u8,
    /// Largest number of simultaneously open controlled ports during the epoch.
    pub maximum_authorized_peers: u8,
    pub peer_removals: u32,
}

/// Active AP policy and hardware-key owner.
///
/// Dropping this value loses the only route back to role-neutral Wi-Fi, so a
/// supervisor can only acknowledge stop after consuming [`stop`](Self::stop).
#[must_use = "an active AP engine must be consumed through stop before radio reuse"]
pub struct Esp32s31ApEngine<'storage> {
    service: AccessPointService<'storage>,
    beacon: Esp32s31ApBeacon<'storage>,
    security: Esp32s31ApSecurity<'storage>,
    report: Esp32s31ApEngineReport,
}

impl<'storage> Esp32s31ApEngine<'storage> {
    // Start failure must return the affine service and caller-owned beacon
    // storage. This no-alloc driver cannot box that rollback value merely to
    // shrink the Result discriminant.
    #[allow(clippy::result_large_err, clippy::too_many_arguments)]
    pub fn start<H: Esp32s31ApRuntimeHardware>(
        hardware: &mut H,
        service: AccessPointService<'storage>,
        beacon_storage: &'storage mut [u8; WPA2_BEACON_CAPACITY],
        pairwise_storage: &'storage mut Esp32s31ApPairwiseKeyStorage,
        ssid: &WifiSsid,
        primary_channel: u8,
        beacon_interval_tu: u16,
        dtim_period: u8,
    ) -> Result<Self, Esp32s31ApEngineStartFailure<'storage>> {
        let beacon_len = match write_wpa2_erp_beacon(
            beacon_storage,
            service.address(),
            ssid,
            primary_channel,
            beacon_interval_tu,
            dtim_period,
            0,
        ) {
            Ok(len) => len,
            Err(error) => {
                return Err(Esp32s31ApEngineStartFailure {
                    service,
                    beacon_storage,
                    pairwise_storage,
                    error: Esp32s31ApEngineError::Beacon(error),
                });
            }
        };
        let beacon =
            Esp32s31ApBeacon::from_initialized(beacon_storage, beacon_len, beacon_interval_tu);
        configure_ap_receive_policy(hardware, service.address());
        let security =
            match Esp32s31ApSecurity::install_group(hardware, service.gtk(), pairwise_storage) {
                Ok(security) => security,
                Err(failure) => {
                    return Err(Esp32s31ApEngineStartFailure {
                        service,
                        beacon_storage: beacon.into_storage(),
                        pairwise_storage: failure.storage,
                        error: Esp32s31ApEngineError::Security(failure.error),
                    });
                }
            };
        Ok(Self {
            service,
            beacon,
            security,
            report: Esp32s31ApEngineReport::default(),
        })
    }

    pub fn prepare_beacon(&mut self, executor_timestamp_micros: u64) -> Option<&mut [u8]> {
        let management_sequence = self.service.next_management_sequence();
        let beacon = self
            .beacon
            .prepare(executor_timestamp_micros, management_sequence, false, 0);
        if beacon.is_some() {
            self.report.beacons_prepared = self.report.beacons_prepared.saturating_add(1);
        }
        beacon
    }

    pub const fn next_beacon_delay(&self, now_micros: u32) -> Option<(u32, u32)> {
        self.beacon.next_delay(now_micros)
    }

    pub const fn beacon_publication_due(&self, now_micros: u32) -> bool {
        self.beacon.publication_due(now_micros)
    }

    pub const fn beacon_publication_lateness(&self, now_micros: u32) -> (u32, u32) {
        self.beacon.publication_lateness(now_micros)
    }

    /// Build handshake message one after the successful Association Response
    /// has reached TX completion.
    pub fn begin_wpa2<const N: usize>(
        &self,
        peer: [u8; 6],
    ) -> Result<Wpa2TxFrame<N>, Esp32s31ApEngineError> {
        Ok(self.service.begin_wpa2_frame(peer)?)
    }

    pub fn handle_management<H: Esp32s31ApRuntimeHardware>(
        &mut self,
        hardware: &mut H,
        frame: &[u8],
        authenticator_nonce: [u8; 32],
        initial_replay_counter: u64,
        output: &mut [u8],
    ) -> Result<Esp32s31ApManagementOutcome, Esp32s31ApEngineError> {
        let Some(request) = parse_ap_management_request(frame, self.service.address()) else {
            return Ok(Esp32s31ApManagementOutcome::Ignored);
        };
        match request {
            ApManagementRequest::OpenAuthentication { peer } => {
                // A peer may restart authentication without first sending a
                // deauthentication frame. End its old pairwise-key epoch
                // before the portable service resets the handshake state;
                // otherwise the stable AID would still own a stale hardware
                // entry and the next authorization could not install its PTK.
                if self.service.peer_status(peer).is_some() {
                    self.security.clear_peer(hardware, peer)?;
                }
                let ApMlmeAction::AuthenticationResponse { status, .. } =
                    self.service.authenticate_open(peer)
                else {
                    unreachable!("authenticate_open has one response action")
                };
                let sequence = self.service.next_management_sequence();
                let len = write_open_authentication_response(
                    output,
                    self.service.address(),
                    peer,
                    status,
                    sequence,
                )?;
                self.report.authentication_responses_prepared = self
                    .report
                    .authentication_responses_prepared
                    .saturating_add(1);
                Ok(Esp32s31ApManagementOutcome::Response {
                    len,
                    begin_wpa2: false,
                })
            }
            ApManagementRequest::Association { peer, rsn_ie } => {
                let action = self.service.associate_wpa2(
                    peer,
                    rsn_ie.unwrap_or(&[]),
                    authenticator_nonce,
                    initial_replay_counter,
                )?;
                let ApMlmeAction::AssociationResponse {
                    status,
                    association_id,
                    ..
                } = action
                else {
                    unreachable!("associate_wpa2 has one response action")
                };
                let sequence = self.service.next_management_sequence();
                let len = write_bg_association_response_frame(
                    output,
                    self.service.address(),
                    peer,
                    status,
                    association_id.unwrap_or(0),
                    sequence,
                )?;
                self.report.association_responses_prepared =
                    self.report.association_responses_prepared.saturating_add(1);
                if association_id.is_some() {
                    self.report.maximum_associated_peers = self
                        .report
                        .maximum_associated_peers
                        .max(self.service.associated_count());
                }
                Ok(Esp32s31ApManagementOutcome::Response {
                    len,
                    begin_wpa2: association_id.is_some(),
                })
            }
            ApManagementRequest::Disassociation { peer, .. }
            | ApManagementRequest::Deauthentication { peer, .. } => {
                if self.service.peer_status(peer).is_none() {
                    return Ok(Esp32s31ApManagementOutcome::Ignored);
                }
                self.security.clear_peer(hardware, peer)?;
                self.service.remove_peer(peer)?;
                self.report.peer_removals = self.report.peer_removals.saturating_add(1);
                Ok(Esp32s31ApManagementOutcome::PeerRemoved { peer })
            }
        }
    }

    /// Install the PTK only after message four has been MIC-verified, then
    /// atomically open the portable controlled port.
    pub fn authorize_peer<H: Esp32s31ApRuntimeHardware>(
        &mut self,
        hardware: &mut H,
        peer: [u8; 6],
    ) -> Result<(), Esp32s31ApEngineError> {
        if !self.service.wpa2_authorized(peer)? {
            return Err(Esp32s31ApEngineError::Service(
                ApServiceError::WrongPeerPhase,
            ));
        }
        let ptk = self.service.pending_ptk(peer)?;
        let association_id = self
            .service
            .peer_status(peer)
            .ok_or(ApServiceError::UnknownPeer)?
            .association_id;
        self.security
            .install_pairwise(hardware, peer, association_id, ptk)?;
        self.service.authorize(peer)?;
        self.report.authorized_peers = self.report.authorized_peers.saturating_add(1);
        self.report.maximum_authorized_peers = self
            .report
            .maximum_authorized_peers
            .max(self.service.authorized_count());
        Ok(())
    }

    /// Resolve one received EAPOL-Key frame and publish the pairwise hardware
    /// key before reporting that the controlled port may open.
    pub fn handle_eapol<H: Esp32s31ApRuntimeHardware, const N: usize>(
        &mut self,
        hardware: &mut H,
        peer: [u8; 6],
        frame: OwnedEapolFrame<N>,
    ) -> Result<Esp32s31ApWpa2Outcome<N>, Esp32s31ApEngineError> {
        match self.service.on_eapol(peer, frame)? {
            ApWpa2Progress::None => Ok(Esp32s31ApWpa2Outcome::None),
            ApWpa2Progress::Transmit(frame) => Ok(Esp32s31ApWpa2Outcome::Transmit(frame)),
            ApWpa2Progress::AuthorizePeer => {
                self.authorize_peer(hardware, peer)?;
                Ok(Esp32s31ApWpa2Outcome::PeerAuthorized { peer })
            }
            ApWpa2Progress::DeauthenticatePeer => {
                Ok(Esp32s31ApWpa2Outcome::DeauthenticatePeer { peer })
            }
        }
    }

    /// Encode one authenticator EAPOL action as an unprotected AP data MPDU.
    /// The sequence number is consumed only when the complete frame fits.
    pub fn encode_eapol<const N: usize>(
        &mut self,
        peer: [u8; 6],
        frame: &Wpa2TxFrame<N>,
        output: &mut [u8],
    ) -> Result<usize, Esp32s31ApEngineError> {
        let sequence_number = self.service.current_data_sequence();
        let len = ApDataFrame {
            access_point: self.service.address(),
            destination: peer,
            sequence_number,
            ether_type: 0x888e,
            payload: frame.as_bytes(),
        }
        .encode(output)?;
        // Consume protocol state in ordinary code. Keeping this call inside
        // `debug_assert_eq!` made release builds transmit every AP data MPDU
        // with sequence number zero because the assertion expression is
        // compiled out.
        let consumed_sequence_number = self.service.next_data_sequence();
        debug_assert_eq!(consumed_sequence_number, sequence_number);
        Ok(len)
    }

    /// Encode one network-owned Ethernet frame for an authorized unicast peer
    /// or for the AP group key. The returned selector is meaningful only to
    /// the S31 TX owner.
    pub fn encode_protected_ethernet(
        &mut self,
        destination: [u8; 6],
        ethernet: &[u8],
        output: &mut [u8],
    ) -> Result<Esp32s31ApProtectedFrame, Esp32s31ApEngineError> {
        let group = destination[0] & 1 != 0;
        let (hardware_key_selector, ccmp_header) = if group {
            if self.service.authorized_count() == 0 {
                return Err(ApServiceError::WrongPeerPhase.into());
            }
            (
                self.security.group_hardware_index(),
                self.security.next_group_tx_ccmp_header(),
            )
        } else {
            if self.service.peer_status(destination).is_none() {
                return Err(ApServiceError::UnknownPeer.into());
            }
            if !self.service.is_authorized(destination) {
                return Err(ApServiceError::WrongPeerPhase.into());
            }
            (
                self.security.pairwise_hardware_index(destination)?,
                self.security.next_pairwise_tx_ccmp_header(destination)?,
            )
        };
        let sequence_number = self.service.current_data_sequence();
        let length = ApProtectedDataFrame {
            access_point: self.service.address(),
            peer: destination,
            sequence_number,
            user_priority: 0,
            peer_qos: false,
            ccmp_header,
            ethernet,
        }
        .encode(output)?;
        // Advance only after the complete protected frame fits, but never as
        // an assertion side effect: release qualification must own the same
        // monotonic sequence space as debug tests.
        let consumed_sequence_number = self.service.next_data_sequence();
        debug_assert_eq!(consumed_sequence_number, sequence_number);
        Ok(Esp32s31ApProtectedFrame {
            length,
            hardware_key_selector,
        })
    }

    pub const fn report(&self) -> Esp32s31ApEngineReport {
        self.report
    }

    pub const fn service_address(&self) -> [u8; 6] {
        self.service.address()
    }

    pub fn peer_status(&self, peer: [u8; 6]) -> Option<ApPeerStatus> {
        self.service.peer_status(peer)
    }

    pub fn is_authorized_peer(&self, peer: [u8; 6]) -> bool {
        self.service.is_authorized(peer)
    }

    pub fn authorized_peer_count(&self) -> u8 {
        self.service.authorized_count()
    }

    pub fn associated_peer_count(&self) -> u8 {
        self.service.associated_count()
    }

    pub fn service_status(&self) -> open_esp_radio_wifi_ap::AccessPointServiceStatus {
        self.service.status()
    }

    pub const fn service_status_revision(&self) -> u32 {
        self.service.status_revision()
    }

    pub fn stop<H: Esp32s31ApRuntimeHardware>(
        self,
        hardware: &mut H,
    ) -> Esp32s31ApEngineStop<'storage> {
        let (security, pairwise_storage) = self.security.stop(hardware);
        hardware.stop_ap_tsf();
        Esp32s31ApEngineStop {
            service: self.service,
            beacon_storage: self.beacon.into_storage(),
            pairwise_storage,
            security,
            report: self.report,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use open_esp_radio_esp32s31_pac::MacKeyInstallOutcome;
    use open_esp_radio_wpa2::{
        OwnedEapolFrame, Pmk, PtkContext, Wpa2Interface,
        frames::{OwnedRsnIe, Wpa2Gtk, Wpa2TxFrame},
    };

    #[derive(Default)]
    struct Hardware {
        policy: Option<[u8; 6]>,
        installed: std::vec::Vec<u8>,
        cleared: std::vec::Vec<u8>,
        tsf_stopped: bool,
    }

    impl ApRxPolicyHardware for Hardware {
        fn apply_ap_link_policy(&mut self, address: [u8; 6]) {
            self.policy = Some(address);
        }
    }

    impl CcmpKeyHardware for Hardware {
        fn install_sta_ccmp_entry(&mut self, index: u8, _words: [u32; 6]) -> MacKeyInstallOutcome {
            self.installed.push(index);
            MacKeyInstallOutcome::Installed
        }

        fn clear_ccmp_entry(&mut self, index: u8) {
            self.cleared.push(index);
        }
    }

    impl Esp32s31ApRuntimeHardware for Hardware {
        fn stop_ap_tsf(&mut self) {
            self.tsf_stopped = true;
        }
    }

    fn service(
        ap: [u8; 6],
        storage: &mut open_esp_radio_wifi_ap::AccessPointPeerStorage,
    ) -> AccessPointService<'_> {
        AccessPointService::new(
            ap,
            Pmk::derive(b"password", b"ap").unwrap(),
            Wpa2Gtk::new(1, true, [0x55; 16]).unwrap(),
            open_esp_radio_wifi_ap::AccessPointClientLimit::new(2).unwrap(),
            storage,
        )
    }

    #[test]
    fn active_epoch_owns_policy_group_key_management_and_stop_frontier() {
        let ap = [2, 0, 0, 0, 0, 1];
        let peer = [2, 0, 0, 0, 0, 2];
        let mut beacon = [0; WPA2_BEACON_CAPACITY];
        let mut peers = open_esp_radio_wifi_ap::AccessPointPeerStorage::new();
        let mut pairwise = Esp32s31ApPairwiseKeyStorage::new();
        let ssid = WifiSsid::new(b"ap").unwrap();
        let mut hardware = Hardware::default();
        let mut engine = Esp32s31ApEngine::start(
            &mut hardware,
            service(ap, &mut peers),
            &mut beacon,
            &mut pairwise,
            &ssid,
            6,
            100,
            2,
        )
        .unwrap_or_else(|_| panic!("AP start"));

        let mut request = [0; 30];
        request[..2].copy_from_slice(&0x00b0_u16.to_le_bytes());
        request[4..10].copy_from_slice(&ap);
        request[10..16].copy_from_slice(&peer);
        request[16..22].copy_from_slice(&ap);
        request[26..28].copy_from_slice(&1_u16.to_le_bytes());
        let mut response = [0; 160];
        assert_eq!(
            engine
                .handle_management(&mut hardware, &request, [1; 32], 7, &mut response)
                .unwrap(),
            Esp32s31ApManagementOutcome::Response {
                len: 30,
                begin_wpa2: false
            }
        );
        assert!(engine.prepare_beacon(102_400).is_some());

        let stopped = engine.stop(&mut hardware);
        assert_eq!(hardware.policy, Some(ap));
        assert_eq!(hardware.installed, [2]);
        assert_eq!(hardware.cleared, [2]);
        assert!(hardware.tsf_stopped);
        assert_eq!(
            stopped.report,
            Esp32s31ApEngineReport {
                beacons_prepared: 1,
                authentication_responses_prepared: 1,
                ..Esp32s31ApEngineReport::default()
            }
        );
    }

    #[test]
    fn message_four_installs_pairwise_key_before_authorization_is_reported() {
        const RSN: [u8; 22] = [
            0x30, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0, 0,
        ];
        const ANONCE: [u8; 32] = [7; 32];
        const SNONCE: [u8; 32] = [8; 32];
        let ap = [2, 0, 0, 0, 0, 1];
        let peer = [2, 0, 0, 0, 0, 2];
        let mut beacon = [0; WPA2_BEACON_CAPACITY];
        let mut peers = open_esp_radio_wifi_ap::AccessPointPeerStorage::new();
        let mut pairwise = Esp32s31ApPairwiseKeyStorage::new();
        let ssid = WifiSsid::new(b"ap").unwrap();
        let mut hardware = Hardware::default();
        let mut engine = Esp32s31ApEngine::start(
            &mut hardware,
            service(ap, &mut peers),
            &mut beacon,
            &mut pairwise,
            &ssid,
            6,
            100,
            2,
        )
        .unwrap_or_else(|_| panic!("AP start"));

        let mut authentication = [0; 30];
        authentication[..2].copy_from_slice(&0x00b0_u16.to_le_bytes());
        authentication[4..10].copy_from_slice(&ap);
        authentication[10..16].copy_from_slice(&peer);
        authentication[16..22].copy_from_slice(&ap);
        authentication[26..28].copy_from_slice(&1_u16.to_le_bytes());
        let mut response = [0; 160];
        engine
            .handle_management(&mut hardware, &authentication, ANONCE, 9, &mut response)
            .unwrap();

        let mut association = [0; 50];
        association[4..10].copy_from_slice(&ap);
        association[10..16].copy_from_slice(&peer);
        association[16..22].copy_from_slice(&ap);
        association[28..].copy_from_slice(&RSN);
        assert!(matches!(
            engine
                .handle_management(&mut hardware, &association, ANONCE, 9, &mut response)
                .unwrap(),
            Esp32s31ApManagementOutcome::Response {
                begin_wpa2: true,
                ..
            }
        ));
        engine.begin_wpa2::<512>(peer).unwrap();

        let ptk = Pmk::derive(b"password", b"ap")
            .unwrap()
            .derive_ptk(PtkContext {
                authenticator_address: ap,
                supplicant_address: peer,
                authenticator_nonce: ANONCE,
                supplicant_nonce: SNONCE,
            });
        let rsn = OwnedRsnIe::<64>::try_copy(&RSN).unwrap();
        let message2 = Wpa2TxFrame::<512>::message2(ap, 9, SNONCE, &rsn)
            .unwrap()
            .authenticate(&ptk);
        let message2 =
            OwnedEapolFrame::<512>::try_copy(Wpa2Interface::AccessPoint, peer, message2.as_bytes())
                .unwrap();
        let Esp32s31ApWpa2Outcome::Transmit(message3) =
            engine.handle_eapol(&mut hardware, peer, message2).unwrap()
        else {
            panic!("message two must produce message three");
        };
        let mut message3_mpdu = [0; 768];
        let message3_len = engine
            .encode_eapol(peer, &message3, &mut message3_mpdu)
            .unwrap();
        assert!(message3_len > message3.as_bytes().len());
        assert_eq!(&message3_mpdu[4..10], &peer);
        assert_eq!(&message3_mpdu[10..16], &ap);
        assert_eq!(&message3_mpdu[22..24], &[0, 0]);
        assert_eq!(&message3_mpdu[30..32], &[0x88, 0x8e]);
        let message4 = Wpa2TxFrame::<512>::message4(ap, 10)
            .unwrap()
            .authenticate(&ptk);
        let message4 =
            OwnedEapolFrame::<512>::try_copy(Wpa2Interface::AccessPoint, peer, message4.as_bytes())
                .unwrap();
        assert!(matches!(
            engine.handle_eapol(&mut hardware, peer, message4).unwrap(),
            Esp32s31ApWpa2Outcome::PeerAuthorized { peer: authorized } if authorized == peer
        ));
        assert_eq!(engine.report().authorized_peers, 1);

        let mut ethernet = [0_u8; 18];
        ethernet[..6].copy_from_slice(&peer);
        ethernet[6..12].copy_from_slice(&ap);
        ethernet[12..14].copy_from_slice(&0x0800_u16.to_be_bytes());
        ethernet[14..].copy_from_slice(&[1, 2, 3, 4]);
        let mut protected = [0_u8; 96];
        let encoded = engine
            .encode_protected_ethernet(peer, &ethernet, &mut protected)
            .unwrap();
        assert_eq!(encoded.hardware_key_selector, 8);
        assert_eq!(&protected[..2], &0x4208_u16.to_le_bytes());
        assert_eq!(&protected[22..24], &0x0010_u16.to_le_bytes());
        assert_eq!(&protected[24..32], &[3, 0, 0, 0x20, 0, 0, 0, 0]);
        assert_eq!(&protected[40..44], &[1, 2, 3, 4]);
        assert_eq!(engine.service.current_data_sequence(), 2);

        ethernet[..6].fill(0xff);
        let encoded = engine
            .encode_protected_ethernet([0xff; 6], &ethernet, &mut protected)
            .unwrap();
        assert_eq!(encoded.hardware_key_selector, 2);
        assert_eq!(&protected[24..32], &[3, 0, 0, 0x60, 0, 0, 0, 0]);
        assert_eq!(engine.service.current_data_sequence(), 3);

        // Supplicants may restart authentication without a preceding
        // deauthentication. The old PTK must leave hardware before the same
        // AID begins a new handshake.
        engine
            .handle_management(&mut hardware, &authentication, ANONCE, 11, &mut response)
            .unwrap();
        assert_eq!(hardware.cleared, [8]);
        assert!(!engine.is_authorized_peer(peer));
        assert_eq!(engine.service.peer_status(peer).unwrap().association_id, 1);

        let _stopped = engine.stop(&mut hardware);
        assert_eq!(hardware.installed, [2, 8]);
        assert_eq!(hardware.cleared, [8, 2]);
    }
}
