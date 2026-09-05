//! Concrete ESP32-S31 transition from a selected scan record to one
//! programmed associated peer.
//!
//! Authentication and Association are finite protocol runners. This module
//! owns the driver transition around them: install scan-time TX policy before
//! the request, complete the peer plan from the successful response, program
//! the HE/rate-control hardware leaves, and return the exact connected-peer
//! state consumed by a runtime integration. Applications and HIL may observe
//! the returned report, but do not reproduce these policy or hardware
//! decisions.

use crate::peer_policy::{StaPeerAssociationPlanError, StaPeerScanPolicy, StaWmmSource};
use open_esp_radio_esp32s31_hal::types::MacHeBeamformingReportProfileError;
use open_esp_radio_esp32s31_wifi_mac::{
    edca::EdcaParametersError,
    he::{He20InstallError, He20PeerHardware, program_he20_peer_state},
    init::StaNoiseFloorHardware,
    rate_control::{BeamformingReportHardware, StaLinkMetric, StaRateControlAssociation},
    tx::HtPeerAmpduParameters,
    tx_protection::WifiTxProtectionPolicy,
};
use open_esp_radio_ieee80211::{
    extensions::wmm::WmmParameterSet,
    he::{He20Capabilities, He20PeerState, HeDcmConstellation},
    ht::HtPeerCapabilities,
    scan::ScanRecord,
    station::{AssociationResponse, StaAssociationPhy},
};

/// TX-policy capability consumed by the associated-peer transition.
///
/// Keeping this smaller than a complete TX backend makes the ordering testable
/// without a DMA slot and prevents the port from acquiring unrelated runtime
/// responsibilities.
pub trait Esp32s31StaPeerTransmit {
    fn install_ht_ampdu_policy(&mut self, parameters: HtPeerAmpduParameters);

    fn install_he_bss_color(&mut self, bss_color: u8);

    fn install_wmm_edca(&mut self, parameters: WmmParameterSet) -> Result<(), EdcaParametersError>;

    fn install_tx_protection_policy(&mut self, policy: WifiTxProtectionPolicy);
}

/// Opaque proof that scan-time policy was derived and installed for this
/// candidate before Association.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31PreparedStaPeer {
    policy: StaPeerScanPolicy,
    access_point: ScanRecord,
}

/// Named mutable capabilities used by final associated-peer programming.
pub struct Esp32s31StaPeerRadio<'a, H, T> {
    pub hardware: &'a mut H,
    pub transmit: &'a mut T,
}

impl<'a, H, T> Esp32s31StaPeerRadio<'a, H, T> {
    pub const fn new(hardware: &'a mut H, transmit: &'a mut T) -> Self {
        Self { hardware, transmit }
    }
}

/// Immutable station/candidate inputs for final peer programming.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31StaPeerStation {
    pub station_address: [u8; 6],
    pub association_phy: StaAssociationPhy,
}

impl Esp32s31StaPeerStation {
    pub const fn new(station_address: [u8; 6], association_phy: StaAssociationPhy) -> Self {
        Self {
            station_address,
            association_phy,
        }
    }
}

/// Stable connected-link facts derived from the selected candidate and its
/// successful Association Response.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31StaConnectedLink {
    pub station_address: [u8; 6],
    pub bssid: [u8; 6],
    pub association_id: u16,
    pub beacon_interval_tu: u16,
    pub peer_qos: bool,
    pub association_phy: StaAssociationPhy,
    /// The selected HT channel width is allowed to use a 400 ns guard
    /// interval according to the AP's retained HT Capabilities IE.
    pub peer_supports_ht_short_guard_interval: bool,
    /// The selected HT40 peer advertised the independent MCS32 receive bit.
    /// This fact is retained for diagnostics and future policy only: the S31
    /// hardware TX encoding is not yet oracle-qualified.
    pub peer_supports_ht_duplicate_mcs32: bool,
    pub peer_supports_one_ltf_800ns_gi: bool,
    pub peer_supports_ldpc: bool,
    pub peer_dcm_receive: HeDcmConstellation,
}

/// Driver-owned state retained for the connected epoch.
#[derive(Debug, Eq, PartialEq)]
pub struct Esp32s31ConnectedStaPeer {
    pub link: Esp32s31StaConnectedLink,
    pub rate_control: StaRateControlAssociation,
}

/// Value-only observations returned beside the connected owner.
///
/// These fields allow HIL and applications to report the selected policy
/// without inserting callbacks into the programming transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31StaPeerProgrammingReport {
    pub rssi_dbm: i8,
    pub noise_floor_dbm: i8,
    pub link_metric: StaLinkMetric,
    pub ht_capabilities: Option<HtPeerCapabilities>,
    pub he_capabilities: Option<He20Capabilities>,
    pub he_peer_state: Option<He20PeerState>,
}

/// Complete result of the finite peer-programming transition.
#[derive(Debug, Eq, PartialEq)]
pub struct Esp32s31ProgrammedStaPeer {
    pub peer: Esp32s31ConnectedStaPeer,
    pub report: Esp32s31StaPeerProgrammingReport,
}

/// Exact policy or hardware edge which failed in the peer port.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31StaPeerPortError {
    ScanPolicy(EdcaParametersError),
    ScanWmm(EdcaParametersError),
    AssociationPlan(StaPeerAssociationPlanError),
    AssociationWmmMissing,
    AssociationWmm(EdcaParametersError),
    He20(He20InstallError),
    RateControl(MacHeBeamformingReportProfileError),
}

/// Stateless namespace for the two finite peer-policy transactions.
pub struct Esp32s31StaPeerPort;

impl Esp32s31StaPeerPort {
    /// Derive and install every policy needed before Authentication and
    /// Association can use the selected candidate.
    pub fn prepare<T: Esp32s31StaPeerTransmit>(
        transmit: &mut T,
        access_point: &ScanRecord,
    ) -> Result<Esp32s31PreparedStaPeer, Esp32s31StaPeerPortError> {
        let policy =
            StaPeerScanPolicy::new(access_point).map_err(Esp32s31StaPeerPortError::ScanPolicy)?;
        transmit.install_ht_ampdu_policy(policy.ht_ampdu);
        transmit.install_he_bss_color(policy.he_bss_color);
        transmit.install_tx_protection_policy(policy.protection);
        if let Some(parameters) = policy.wmm.parameters() {
            transmit
                .install_wmm_edca(parameters)
                .map_err(Esp32s31StaPeerPortError::ScanWmm)?;
        }
        Ok(Esp32s31PreparedStaPeer {
            policy,
            access_point: *access_point,
        })
    }

    /// Complete and program one associated peer as a single driver-owned
    /// transition, then return the state needed by the connected runtime.
    pub fn program<H, T>(
        radio: Esp32s31StaPeerRadio<'_, H, T>,
        station: Esp32s31StaPeerStation,
        response: &AssociationResponse,
        prepared: Esp32s31PreparedStaPeer,
    ) -> Result<Esp32s31ProgrammedStaPeer, Esp32s31StaPeerPortError>
    where
        H: StaNoiseFloorHardware + He20PeerHardware + BeamformingReportHardware,
        T: Esp32s31StaPeerTransmit,
    {
        let noise_floor_dbm = radio.hardware.read_noise_floor_dbm();
        let plan = prepared
            .policy
            .complete(
                &prepared.access_point,
                response,
                station.association_phy,
                noise_floor_dbm,
            )
            .map_err(Esp32s31StaPeerPortError::AssociationPlan)?;

        radio.transmit.install_ht_ampdu_policy(plan.ht_ampdu);
        radio.transmit.install_he_bss_color(plan.he_bss_color);
        radio.transmit.install_tx_protection_policy(plan.protection);
        if plan.wmm.source() == StaWmmSource::AssociationResponse {
            let parameters = plan
                .wmm
                .parameters()
                .ok_or(Esp32s31StaPeerPortError::AssociationWmmMissing)?;
            radio
                .transmit
                .install_wmm_edca(parameters)
                .map_err(Esp32s31StaPeerPortError::AssociationWmm)?;
        }
        if let Some(state) = plan.he_peer_state {
            // The complete hardware threshold-table builder is not reviewed.
            // Keep HE RTS disabled in MMIO and retain the finite threshold in
            // the TX runtime, which admits only a proven below-threshold
            // aggregate and rejects every other publication before DMA.
            let mut hardware_state = state;
            hardware_state.rts_threshold = None;
            program_he20_peer_state(
                radio.hardware,
                hardware_state,
                response.association_id,
                0,
                0,
            )
            .map_err(Esp32s31StaPeerPortError::He20)?;
        }
        plan.rate_control
            .program_hardware(radio.hardware)
            .map_err(Esp32s31StaPeerPortError::RateControl)?;

        let ht_capabilities = plan.ht_capabilities;
        let he_capabilities = plan.he_capabilities;
        let link = Esp32s31StaConnectedLink {
            station_address: station.station_address,
            bssid: prepared.access_point.bssid,
            association_id: response.association_id,
            beacon_interval_tu: prepared.access_point.beacon_interval_tu,
            peer_qos: plan.peer_qos,
            association_phy: station.association_phy,
            peer_supports_ht_short_guard_interval: match station.association_phy {
                StaAssociationPhy::Ht20 => prepared
                    .access_point
                    .supports_ht_short_guard_interval_20mhz(),
                StaAssociationPhy::Ht40 => prepared
                    .access_point
                    .supports_ht_short_guard_interval_40mhz(),
                StaAssociationPhy::Legacy | StaAssociationPhy::He20 => false,
            },
            peer_supports_ht_duplicate_mcs32: station.association_phy == StaAssociationPhy::Ht40
                && ht_capabilities.is_some_and(HtPeerCapabilities::supports_ht_duplicate_mcs32),
            peer_supports_one_ltf_800ns_gi: he_capabilities
                .is_some_and(|capability| capability.supports_one_ltf_800ns_gi()),
            peer_supports_ldpc: he_capabilities
                .is_some_and(|capability| capability.supports_ldpc_coding_in_payload()),
            peer_dcm_receive: he_capabilities
                .map_or(HeDcmConstellation::NotSupported, |capability| {
                    capability.dcm_receive_constellation()
                }),
        };
        Ok(Esp32s31ProgrammedStaPeer {
            peer: Esp32s31ConnectedStaPeer {
                link,
                rate_control: plan.rate_control,
            },
            report: Esp32s31StaPeerProgrammingReport {
                rssi_dbm: prepared.access_point.rssi,
                noise_floor_dbm,
                link_metric: plan.link_metric,
                ht_capabilities,
                he_capabilities,
                he_peer_state: plan.he_peer_state,
            },
        })
    }
}

#[cfg(test)]
mod tests;
