//! Ownership boundary for scan-to-associated STA policy.

use crate::{
    edca::{EdcaParametersError, EdcaQueues},
    rate_control::{
        HeLowMetricReportFeatures, StaLinkMetric, StaRateControlAssociation,
        StaRateControlAssociationInput, StaRateControlPeerHighestRate, StaRateControlPhy,
    },
    tx::{HeEdcaTxopLimit, HeMcs, HtPeerAmpduParameters},
};
use open_esp_radio_esp32s31_registers::RadioRegisters;
use open_esp_radio_ieee80211::{
    he::{
        He20Capabilities, He20PeerState, HeDcmConstellation, HeElementError, HeMcsNssSupport,
        parse_he20_capabilities, parse_he20_operation, parse_he20_peer_state,
    },
    scan::ScanRecord,
    station::{AssociationResponse, StaAssociationPhy},
    wmm::{WmmAccessCategory, WmmParameterSet},
};

pub trait StaLinkRxPolicyHardware {
    fn apply_sta_link_policy(&mut self, bssid: [u8; 6]);
}

/// Read-only PHY observation used to complete associated-peer rate policy.
pub trait StaNoiseFloorHardware {
    fn read_noise_floor_dbm(&self) -> i8;
}

impl StaLinkRxPolicyHardware for RadioRegisters {
    fn apply_sta_link_policy(&mut self, bssid: [u8; 6]) {
        self.apply_sta_link_receive_policy(bssid);
    }
}

impl StaNoiseFloorHardware for RadioRegisters {
    fn read_noise_floor_dbm(&self) -> i8 {
        RadioRegisters::read_noise_floor_dbm(self)
    }
}

/// Switch RX queue zero from the cold/default scan policy to the associated
/// station-link policy.
///
/// This is the source-owned form of migration's
/// `scan::enable_sta_link_rx_policy`, completed with the preceding
/// `ic_set_bssid`/`hal_mac_set_bssid` transaction recovered from the same
/// vendor STA transition. Unlike migration's policy-five snapshot, the final
/// UBSSID edge follows `wifi_set_rx_policy(6)`, which is the branch observed
/// immediately before the first live vendor Authentication TX.
///
/// The production PAC operation also executes complete
/// `hal_sniffer_disable` first. The open scan bootstrap uses the vendor
/// queue-three sniffer leaf, whereas the normal vendor STA lifecycle does not
/// leave that leaf active after scan. Restoring the inverse leaf here returns
/// queue three to normal address filtering and clears its hardware
/// `AUTOACK_DISABLE` policy before Authentication. It must run after
/// off-channel scanning and before the station sends Authentication.
pub fn configure_sta_link_receive_policy<H: StaLinkRxPolicyHardware>(
    hardware: &mut H,
    bssid: [u8; 6],
) {
    hardware.apply_sta_link_policy(bssid);
}

/// Origin of the active WMM parameters for one station link.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaWmmSource {
    VendorDefaults,
    Scan,
    AssociationResponse,
}

/// Validated WMM/HE-TXOP policy retained across Association.
///
/// The standard WMM field is 16 bits, while complete
/// `libnet80211.a[ieee80211_sta.o]::ieee80211_parse_wmeparams`
/// retains only its low byte in the vendor per-AC record. A value outside that
/// representation is therefore bounded to the largest retained value instead
/// of being silently truncated.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaWmmPolicy {
    parameters: Option<WmmParameterSet>,
    source: StaWmmSource,
    best_effort_txop: HeEdcaTxopLimit,
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
                best_effort_txop: HeEdcaTxopLimit::DEFAULT,
            });
        };

        // Validate every AC as one transaction before the policy can escape.
        // Applying the retained value to live queues therefore cannot expose a
        // partially installed WMM set.
        let mut validation = EdcaQueues::vendor_defaults();
        validation.configure_from_wmm(parameters)?;
        let best_effort_txop = HeEdcaTxopLimit::from_units_32_us(
            parameters
                .access_category(WmmAccessCategory::BestEffort)
                .txop_limit_units_32_us,
        )
        .unwrap_or(HeEdcaTxopLimit::MAXIMUM_SUPPORTED);
        Ok(Self {
            parameters: Some(parameters),
            source,
            best_effort_txop,
        })
    }

    pub const fn parameters(self) -> Option<WmmParameterSet> {
        self.parameters
    }

    pub const fn source(self) -> StaWmmSource {
        self.source
    }

    pub const fn best_effort_txop(self) -> HeEdcaTxopLimit {
        self.best_effort_txop
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
    pub wmm: StaWmmPolicy,
    pub he_bss_color: u8,
    pub he_capabilities: Option<He20Capabilities>,
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
        let he_bss_color = parse_he20_operation(access_point.he_operation_ie_bytes())
            .map_or(0, |operation| operation.effective_bss_color());
        Ok(Self {
            ht_ampdu,
            wmm: StaWmmPolicy::from_scan(access_point)?,
            he_bss_color,
            he_capabilities: parse_he20_capabilities(access_point.he_capability_ie_bytes()).ok(),
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

        Ok(StaPeerAssociationPlan {
            association_phy,
            ht_ampdu: self.ht_ampdu,
            wmm,
            he_bss_color: self.he_bss_color,
            he_capabilities,
            he_peer_state,
            peer_qos: response.wmm,
            noise_floor_dbm,
            link_metric,
            rate_control,
        })
    }
}

/// Complete value plan consumed by the S31 hardware and connected STA owner.
#[derive(Debug, Eq, PartialEq)]
pub struct StaPeerAssociationPlan {
    pub association_phy: StaAssociationPhy,
    pub ht_ampdu: HtPeerAmpduParameters,
    pub wmm: StaWmmPolicy,
    pub he_bss_color: u8,
    pub he_capabilities: Option<He20Capabilities>,
    pub he_peer_state: Option<He20PeerState>,
    pub peer_qos: bool,
    pub noise_floor_dbm: i8,
    pub link_metric: StaLinkMetric,
    pub rate_control: StaRateControlAssociation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaPeerAssociationPlanError {
    AssociationRejected(u16),
    Edca(EdcaParametersError),
    HeCapabilities(HeElementError),
    HePeer(HeElementError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rate_schedule::RateScheduleKind;
    use open_esp_radio_ieee80211::wmm::parse_wmm_parameter_element;

    const HE20_MCS9_CAPABILITY: [u8; 24] = [
        255, 22, 35, 0x03, 0x18, 0x9c, 0xca, 0x10, 0x80, 0x00, 0x10, 0x8a, 0x1b, 0x0d, 0xc0, 0x1f,
        0x00, 0x02, 0x82, 0x01, 0xfd, 0xff, 0xfd, 0xff,
    ];
    const HE20_OPERATION: [u8; 9] = [255, 7, 36, 0, 0, 0, 5, 0xfd, 0xff];
    const STANDARD_WMM: [u8; 26] = [
        221, 24, 0x00, 0x50, 0xf2, 0x02, 1, 1, 0x85, 0, 0x03, 0xa4, 0, 0, 0x27, 0xa4, 0, 0, 0x42,
        0x43, 94, 0, 0x72, 0x32, 47, 0,
    ];

    fn he20_access_point() -> ScanRecord {
        let mut access_point = ScanRecord::EMPTY;
        access_point.rssi = -45;
        access_point.ht_capability_ie_present = true;
        access_point.ht_capability_ie[4] = 0x17;
        access_point.he_capability_ie[..HE20_MCS9_CAPABILITY.len()]
            .copy_from_slice(&HE20_MCS9_CAPABILITY);
        access_point.he_capability_ie_len = HE20_MCS9_CAPABILITY.len() as u8;
        access_point.he_operation_ie[..HE20_OPERATION.len()].copy_from_slice(&HE20_OPERATION);
        access_point.he_operation_ie_len = HE20_OPERATION.len() as u8;
        access_point
    }

    #[test]
    fn he20_plan_joins_one_peer_view_without_vendor_layout() {
        let access_point = he20_access_point();
        let scan = StaPeerScanPolicy::new(&access_point).unwrap();
        assert_eq!(scan.ht_ampdu.maximum_aggregate_bytes(), u16::MAX);
        assert_eq!(scan.he_bss_color, 5);

        let response = AssociationResponse {
            capability_info: 0,
            status_code: 0,
            association_id: 7,
            ht_capability: true,
            he_capability: true,
            he_operation: true,
            wmm: true,
            wmm_parameters: Some(parse_wmm_parameter_element(&STANDARD_WMM).unwrap()),
        };
        let plan = scan
            .complete(&access_point, &response, StaAssociationPhy::He20, -95)
            .unwrap();
        assert_eq!(plan.link_metric.value(), 50);
        assert_eq!(plan.wmm.source(), StaWmmSource::AssociationResponse);
        assert_eq!(plan.wmm.best_effort_txop(), HeEdcaTxopLimit::DEFAULT);
        assert_eq!(
            plan.he_capabilities.unwrap().dcm_receive_constellation(),
            HeDcmConstellation::Qam16
        );
        assert!(
            plan.he_peer_state
                .unwrap()
                .extended_range_single_user_permitted()
        );
        assert_eq!(
            plan.rate_control.current_schedule().kind,
            RateScheduleKind::Dot11Ax
        );
    }

    #[test]
    fn response_wmm_overrides_scan_and_bounds_vendor_txop_width() {
        let mut access_point = he20_access_point();
        access_point.wmm_ie[..STANDARD_WMM.len()].copy_from_slice(&STANDARD_WMM);
        access_point.wmm_ie_len = STANDARD_WMM.len() as u8;
        let scan = StaPeerScanPolicy::new(&access_point).unwrap();
        assert_eq!(scan.wmm.source(), StaWmmSource::Scan);

        let mut wider = STANDARD_WMM;
        wider[12..14].copy_from_slice(&300_u16.to_le_bytes());
        let response = AssociationResponse {
            capability_info: 0,
            status_code: 0,
            association_id: 7,
            ht_capability: true,
            he_capability: true,
            he_operation: true,
            wmm: true,
            wmm_parameters: Some(parse_wmm_parameter_element(&wider).unwrap()),
        };
        let plan = scan
            .complete(&access_point, &response, StaAssociationPhy::He20, -95)
            .unwrap();
        assert_eq!(plan.wmm.source(), StaWmmSource::AssociationResponse);
        assert_eq!(
            plan.wmm.best_effort_txop(),
            HeEdcaTxopLimit::MAXIMUM_SUPPORTED
        );
    }

    #[test]
    fn rejected_association_cannot_produce_a_peer_plan() {
        let access_point = he20_access_point();
        let response = AssociationResponse {
            capability_info: 0,
            status_code: 17,
            association_id: 0,
            ht_capability: false,
            he_capability: false,
            he_operation: false,
            wmm: false,
            wmm_parameters: None,
        };
        assert_eq!(
            StaPeerScanPolicy::new(&access_point)
                .unwrap()
                .complete(&access_point, &response, StaAssociationPhy::He20, -95)
                .unwrap_err(),
            StaPeerAssociationPlanError::AssociationRejected(17)
        );
    }
}
