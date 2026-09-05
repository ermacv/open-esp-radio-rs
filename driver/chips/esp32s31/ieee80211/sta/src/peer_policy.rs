//! Value-only scan-to-associated STA peer policy.

use open_esp_radio_esp32s31_wifi_mac::{
    edca::{EdcaParametersError, EdcaQueues},
    rate_control::{
        HeLowMetricReportFeatures, StaLinkMetric, StaRateControlAssociation,
        StaRateControlAssociationInput, StaRateControlPeerHighestRate, StaRateControlPhy,
    },
    tx::{HeMcs, HtPeerAmpduParameters},
    tx_protection::{
        ErpProtectionMode, HeTxopDurationRtsThreshold, HtProtectionMode, WifiTxProtectionPolicy,
    },
};
use open_esp_radio_ieee80211::{
    he::{
        He20Capabilities, He20PeerState, HeDcmConstellation, HeElementError, HeMcsNssSupport,
        parse_he20_capabilities, parse_he20_operation, parse_he20_peer_state,
    },
    ht::HtPeerCapabilities,
    scan::ScanRecord,
    station::{AssociationResponse, StaAssociationPhy},
    wmm::WmmParameterSet,
};

/// Origin of the active WMM parameters for one station link.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaWmmSource {
    VendorDefaults,
    Scan,
    AssociationResponse,
}

/// Validated WMM policy retained across Association.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaWmmPolicy {
    parameters: Option<WmmParameterSet>,
    source: StaWmmSource,
}

impl StaWmmPolicy {
    pub fn from_scan(access_point: &ScanRecord) -> Result<Self, EdcaParametersError> {
        Self::validated(access_point.wmm_parameters(), StaWmmSource::Scan)
    }

    fn with_association_response(
        self,
        response: &AssociationResponse,
    ) -> Result<Self, EdcaParametersError> {
        match response.wmm_parameters {
            Some(parameters) => {
                Self::validated(Some(parameters), StaWmmSource::AssociationResponse)
            }
            None => Ok(self),
        }
    }

    fn validated(
        parameters: Option<WmmParameterSet>,
        source: StaWmmSource,
    ) -> Result<Self, EdcaParametersError> {
        let Some(parameters) = parameters else {
            return Ok(Self {
                parameters: None,
                source: StaWmmSource::VendorDefaults,
            });
        };

        // Validate every AC as one transaction before the policy can escape.
        // Applying the retained value to live queues therefore cannot expose a
        // partially installed WMM set.
        let mut validation = EdcaQueues::vendor_defaults();
        validation.configure_from_wmm(parameters)?;
        Ok(Self {
            parameters: Some(parameters),
            source,
        })
    }

    pub const fn parameters(self) -> Option<WmmParameterSet> {
        self.parameters
    }

    pub const fn source(self) -> StaWmmSource {
        self.source
    }
}

/// Value-only policy derived from the peer's scan advertisement.
///
/// This state is prepared before Authentication/Association so management TX
/// observes the scan-time EDCA settings. The successful Association Response
/// may later replace WMM while every other advertised capability remains from
/// the complete Beacon/Probe Response record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaPeerScanPolicy {
    pub ht_ampdu: HtPeerAmpduParameters,
    pub ht_capabilities: Option<HtPeerCapabilities>,
    pub wmm: StaWmmPolicy,
    pub he_bss_color: u8,
    pub he_capabilities: Option<He20Capabilities>,
    pub protection: WifiTxProtectionPolicy,
}

impl StaPeerScanPolicy {
    pub fn new(access_point: &ScanRecord) -> Result<Self, EdcaParametersError> {
        // HT Capabilities is retained as a complete IE. Byte four is payload
        // byte two, the A-MPDU Parameters field consumed by complete
        // `libpp.a[trc.o]::rcUpdateAMPDUParam`.
        let ht_ampdu = HtPeerAmpduParameters::from_capability_byte(
            access_point
                .ht_capability_ie_bytes()
                .map_or(0, |capability| capability[4]),
        );
        let ht_capabilities = access_point.ht_peer_capabilities();
        let he_bss_color = parse_he20_operation(access_point.he_operation_ie_bytes())
            .map_or(0, |operation| operation.effective_bss_color());
        let protection = WifiTxProtectionPolicy::new(
            ErpProtectionMode::from_information(access_point.erp_information()),
            HtProtectionMode::from_operation_ie(access_point.ht_operation_ie_bytes()),
            None,
        );
        Ok(Self {
            ht_ampdu,
            ht_capabilities,
            wmm: StaWmmPolicy::from_scan(access_point)?,
            he_bss_color,
            he_capabilities: parse_he20_capabilities(access_point.he_capability_ie_bytes()).ok(),
            protection,
        })
    }

    /// Complete the associated-peer plan after a successful response.
    pub fn complete(
        self,
        access_point: &ScanRecord,
        response: &AssociationResponse,
        association_phy: StaAssociationPhy,
        noise_floor_dbm: i8,
    ) -> Result<StaPeerAssociationPlan, StaPeerAssociationPlanError> {
        if response.status_code != 0 {
            return Err(StaPeerAssociationPlanError::AssociationRejected(
                response.status_code,
            ));
        }
        let wmm = self
            .wmm
            .with_association_response(response)
            .map_err(StaPeerAssociationPlanError::Edca)?;

        let (he_capabilities, he_peer_state) = if association_phy == StaAssociationPhy::He20 {
            let capabilities = parse_he20_capabilities(access_point.he_capability_ie_bytes())
                .map_err(StaPeerAssociationPlanError::HeCapabilities)?;
            let state = parse_he20_peer_state(
                access_point.he_capability_ie_bytes(),
                access_point.he_operation_ie_bytes(),
            )
            .map_err(StaPeerAssociationPlanError::HePeer)?;
            (Some(capabilities), Some(state))
        } else {
            (self.he_capabilities, None)
        };

        // SOURCE: complete `libnet80211.a[wl_cnx.o]::ic_set_sta`
        // names `rssi` and `nf`, constructs the per-peer TRC input and calls
        // `ic_set_trc`. Complete `libpp.a[if_hwctrl.o]::ic_set_trc`
        // narrows `rssi - nf` to a signed byte before `rcUpdatePhyMode`.
        let link_metric =
            StaLinkMetric::from_rssi_and_noise_floor(access_point.rssi, noise_floor_dbm);
        let phy = match association_phy {
            StaAssociationPhy::Legacy => StaRateControlPhy::Dot11G,
            StaAssociationPhy::Ht20 | StaAssociationPhy::Ht40 => StaRateControlPhy::Ht,
            StaAssociationPhy::He20 => StaRateControlPhy::He,
        };

        // SOURCE: complete `ic_set_sta` supplies the negotiated maximum rate
        // to `ic_set_trc`; complete `libpp.a[trc.o]::
        // rc11AXRate2SchedIdx` maps HE20 1SS rates 172 and 229 to the MCS7 and
        // MCS9 frontiers. S31 caps a peer advertising MCS0..11 at MCS9.
        let peer_highest_rate = if association_phy == StaAssociationPhy::He20 {
            he_capabilities.and_then(|capability| {
                let maximum_mcs = match capability.receive_nss1 {
                    HeMcsNssSupport::Mcs0To7 => HeMcs::Mcs7,
                    HeMcsNssSupport::Mcs0To9 | HeMcsNssSupport::Mcs0To11 => HeMcs::Mcs9,
                    HeMcsNssSupport::NotSupported => return None,
                };
                Some(StaRateControlPeerHighestRate::he20_one_spatial_stream(
                    maximum_mcs,
                ))
            })
        } else {
            None
        };
        let rate_control = StaRateControlAssociation::new(StaRateControlAssociationInput {
            phy,
            link_metric,
            p2p: false,
            peer_highest_rate,
            // This is the vendor LR-rate count at node offset 0x84, not an HE
            // capability. A standard infrastructure association has no LR
            // rates until an owned LR negotiation path says otherwise.
            long_range_rates_present: false,
            he_low_metric_report: HeLowMetricReportFeatures {
                dcm_receive_supported: he_capabilities.is_some_and(|capability| {
                    capability.dcm_receive_constellation() != HeDcmConstellation::NotSupported
                }),
                extended_range_single_user_permitted: he_peer_state
                    .is_some_and(He20PeerState::extended_range_single_user_permitted),
            },
        });

        let protection = self.protection.with_he_txop_duration_rts_threshold(
            he_peer_state
                .and_then(|state| state.rts_threshold)
                .and_then(HeTxopDurationRtsThreshold::new),
        );

        Ok(StaPeerAssociationPlan {
            association_phy,
            ht_ampdu: self.ht_ampdu,
            ht_capabilities: self.ht_capabilities,
            wmm,
            he_bss_color: self.he_bss_color,
            he_capabilities,
            he_peer_state,
            peer_qos: response.wmm,
            noise_floor_dbm,
            link_metric,
            rate_control,
            protection,
        })
    }
}

/// Complete value plan consumed by the S31 hardware and connected STA owner.
#[derive(Debug, Eq, PartialEq)]
pub struct StaPeerAssociationPlan {
    pub association_phy: StaAssociationPhy,
    pub ht_ampdu: HtPeerAmpduParameters,
    pub ht_capabilities: Option<HtPeerCapabilities>,
    pub wmm: StaWmmPolicy,
    pub he_bss_color: u8,
    pub he_capabilities: Option<He20Capabilities>,
    pub he_peer_state: Option<He20PeerState>,
    pub peer_qos: bool,
    pub noise_floor_dbm: i8,
    pub link_metric: StaLinkMetric,
    pub rate_control: StaRateControlAssociation,
    pub protection: WifiTxProtectionPolicy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaPeerAssociationPlanError {
    AssociationRejected(u16),
    Edca(EdcaParametersError),
    HeCapabilities(HeElementError),
    HePeer(HeElementError),
}

#[cfg(test)]
mod tests;
