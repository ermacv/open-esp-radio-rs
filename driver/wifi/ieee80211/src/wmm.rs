//! Bounded WMM Parameter Element parsing.

const VENDOR_ELEMENT_ID: u8 = 221;
const WMM_PARAMETER_BODY_LEN: usize = 24;
const WMM_OUI_AND_TYPE: [u8; 4] = [0x00, 0x50, 0xf2, 0x02];
const WMM_PARAMETER_SUBTYPE: u8 = 1;
const WMM_VERSION: u8 = 1;

/// Standard WMM access-category identifier carried in ACI bits 5..6.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum WmmAccessCategory {
    #[default]
    BestEffort = 0,
    Background = 1,
    Video = 2,
    Voice = 3,
}

impl WmmAccessCategory {
    pub const fn from_aci(aci: u8) -> Option<Self> {
        match aci {
            0 => Some(Self::BestEffort),
            1 => Some(Self::Background),
            2 => Some(Self::Video),
            3 => Some(Self::Voice),
            _ => None,
        }
    }

    const fn index(self) -> usize {
        self as usize
    }

    /// Next lower access category used when the requested AC requires an
    /// admission which the station does not own.
    pub const fn downgrade(self) -> Option<Self> {
        match self {
            Self::Voice => Some(Self::Video),
            Self::Video => Some(Self::BestEffort),
            Self::BestEffort => Some(Self::Background),
            Self::Background => None,
        }
    }

    /// Canonical user priority used when ACM policy downgrades into this AC.
    pub const fn canonical_user_priority(self) -> WmmUserPriority {
        match self {
            Self::Voice => WmmUserPriority::UP7,
            // The recovered station classifier enters VI at UP5.
            Self::Video => WmmUserPriority::UP5,
            Self::BestEffort => WmmUserPriority::UP0,
            Self::Background => WmmUserPriority::UP1,
        }
    }
}

/// Valid IEEE 802.1D / IEEE 802.11 user priority and QoS TID.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WmmUserPriority(u8);

impl WmmUserPriority {
    pub const UP0: Self = Self(0);
    pub const UP1: Self = Self(1);
    pub const UP2: Self = Self(2);
    pub const UP3: Self = Self(3);
    pub const UP4: Self = Self(4);
    pub const UP5: Self = Self(5);
    pub const UP6: Self = Self(6);
    pub const UP7: Self = Self(7);

    pub const fn new(value: u8) -> Option<Self> {
        if value <= 7 { Some(Self(value)) } else { None }
    }

    pub const fn value(self) -> u8 {
        self.0
    }

    /// Standard UP-to-AC mapping used by WMM and the existing data encoder.
    pub const fn access_category(self) -> WmmAccessCategory {
        match self.0 {
            0 | 3 => WmmAccessCategory::BestEffort,
            1 | 2 => WmmAccessCategory::Background,
            4 | 5 => WmmAccessCategory::Video,
            6 | 7 => WmmAccessCategory::Voice,
            _ => unreachable!(),
        }
    }
}

/// Valid six-bit IPv4/IPv6 Differentiated Services Code Point.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Dscp(u8);

impl Dscp {
    pub const fn new(value: u8) -> Option<Self> {
        if value <= 0x3f {
            Some(Self(value))
        } else {
            None
        }
    }

    pub const fn value(self) -> u8 {
        self.0
    }

    /// Widely deployed default mapping based on the three DSCP MSBs.
    pub const fn default_user_priority(self) -> WmmUserPriority {
        WmmUserPriority(self.0 >> 3)
    }

    /// RFC 8325 station-safe DSCP mapping.
    ///
    /// Only the standardized service-class codepoints receive elevated
    /// priority. Unknown codepoints and endpoint-sourced CS6/CS7 are bleached
    /// to UP0, preventing an application marking from acquiring the reserved
    /// network-control priority. RFC 8622's Lower Effort codepoint joins CS1
    /// in the background category.
    pub const fn station_user_priority(self) -> WmmUserPriority {
        match self.0 {
            // Lower Effort and CS1.
            1 | 8 => WmmUserPriority::UP1,
            // AF2x low-latency data.
            18 | 20 | 22 => WmmUserPriority::UP3,
            // AF3x, CS3, AF4x and CS4 video classes.
            24 | 26 | 28 | 30 | 32 | 34 | 36 | 38 => WmmUserPriority::UP4,
            // CS5 signaling.
            40 => WmmUserPriority::UP5,
            // VOICE-ADMIT and EF telephony.
            44 | 46 => WmmUserPriority::UP6,
            // DF, OAM, AF1x, CS6/CS7 and unassigned values fail closed to BE.
            _ => WmmUserPriority::UP0,
        }
    }
}

/// Header field which selected one network frame's WMM class.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WmmClassificationSource {
    /// Untagged non-IP traffic or a truncated network header.
    #[default]
    Unmarked,
    /// The outer IEEE 802.1Q/802.1ad priority-code-point field.
    Ieee8021d(WmmUserPriority),
    /// IPv4 or IPv6 DSCP mapped with the station-safe policy.
    Dscp(Dscp),
}

/// Complete portable WMM classification for one network frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WmmTrafficClass {
    pub user_priority: WmmUserPriority,
    pub access_category: WmmAccessCategory,
    pub source: WmmClassificationSource,
}

impl WmmTrafficClass {
    pub const BEST_EFFORT: Self = Self {
        user_priority: WmmUserPriority::UP0,
        access_category: WmmAccessCategory::BestEffort,
        source: WmmClassificationSource::Unmarked,
    };

    pub const fn from_user_priority(
        user_priority: WmmUserPriority,
        source: WmmClassificationSource,
    ) -> Self {
        Self {
            user_priority,
            access_category: user_priority.access_category(),
            source,
        }
    }
}

const ETHERNET_HEADER_LEN: usize = 14;
const ETHER_TYPE_IPV4: u16 = 0x0800;
const ETHER_TYPE_VLAN: u16 = 0x8100;
const ETHER_TYPE_PROVIDER_VLAN: u16 = 0x88a8;
const ETHER_TYPE_IPV6: u16 = 0x86dd;

/// Classify an Ethernet frame from an outer 802.1D PCP or an IP DSCP.
///
/// An explicit VLAN PCP takes precedence over an encapsulated DSCP. Untagged
/// non-IP and truncated headers remain best effort. The function never
/// invents priority from transport ports or payload bytes.
pub fn classify_ethernet_wmm(ethernet: &[u8]) -> WmmTrafficClass {
    if ethernet.len() < ETHERNET_HEADER_LEN {
        return WmmTrafficClass::BEST_EFFORT;
    }

    let ether_type = u16::from_be_bytes([ethernet[12], ethernet[13]]);
    let payload_offset = ETHERNET_HEADER_LEN;
    if matches!(ether_type, ETHER_TYPE_VLAN | ETHER_TYPE_PROVIDER_VLAN) {
        let Some(tag) = ethernet.get(payload_offset..payload_offset + 4) else {
            return WmmTrafficClass::BEST_EFFORT;
        };
        let tci = u16::from_be_bytes([tag[0], tag[1]]);
        let user_priority = WmmUserPriority((tci >> 13) as u8);
        // The explicit outer PCP is authoritative even when it is zero.
        return WmmTrafficClass::from_user_priority(
            user_priority,
            WmmClassificationSource::Ieee8021d(user_priority),
        );
    }

    let dscp = match ether_type {
        ETHER_TYPE_IPV4 => {
            let Some(header) = ethernet.get(payload_offset..payload_offset + 2) else {
                return WmmTrafficClass::BEST_EFFORT;
            };
            if header[0] >> 4 != 4 {
                return WmmTrafficClass::BEST_EFFORT;
            }
            Dscp(header[1] >> 2)
        }
        ETHER_TYPE_IPV6 => {
            let Some(header) = ethernet.get(payload_offset..payload_offset + 2) else {
                return WmmTrafficClass::BEST_EFFORT;
            };
            if header[0] >> 4 != 6 {
                return WmmTrafficClass::BEST_EFFORT;
            }
            Dscp(((header[0] & 0x0f) << 2) | (header[1] >> 6))
        }
        _ => return WmmTrafficClass::BEST_EFFORT,
    };
    let user_priority = dscp.station_user_priority();
    WmmTrafficClass::from_user_priority(user_priority, WmmClassificationSource::Dscp(dscp))
}

/// One four-byte AC Parameter Record from a WMM Parameter Element.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WmmAcParameters {
    pub admission_control_mandatory: bool,
    pub aifsn: u8,
    pub ecw_min: u8,
    pub ecw_max: u8,
    /// TXOP limit in the element's native 32-us units.
    pub txop_limit_units_32_us: u16,
}

/// Complete WMM Parameter Set indexed by the standard ACI value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WmmParameterSet {
    pub parameter_set_count: u8,
    pub uapsd: bool,
    access_categories: [WmmAcParameters; 4],
}

impl WmmParameterSet {
    pub const fn access_category(self, category: WmmAccessCategory) -> WmmAcParameters {
        self.access_categories[category.index()]
    }
}

/// Parse one complete vendor-specific WMM Parameter Element.
///
/// SOURCE: complete
/// `libnet80211.a[ieee80211_sta.o]::ieee80211_parse_wmeparams`
/// (size `0xae`) and
/// `ieee80211_wme_standard_ac_to_esp_ac` (size `0x48`). The blob accepts a
/// 24-byte-or-larger body, reads QoS Info byte eight, then consumes four
/// four-byte records from byte ten. It maps the standard record order
/// BE/BK/VI/VO to its internal queue order 2/3/1/0 and extracts ACM, AIFSN,
/// ECWmin, ECWmax, and the low TXOP byte.
///
/// This hardware-independent parser keys records by their explicit ACI field
/// and retains the complete standard little-endian 16-bit TXOP value. The
/// blob's discarded high byte is an implementation limitation, not a
/// different on-air field.
pub fn parse_wmm_parameter_element(element: &[u8]) -> Option<WmmParameterSet> {
    if element.len() < WMM_PARAMETER_BODY_LEN + 2
        || element[0] != VENDOR_ELEMENT_ID
        || usize::from(element[1]) < WMM_PARAMETER_BODY_LEN
        || element.len() < usize::from(element[1]) + 2
        || element[2..6] != WMM_OUI_AND_TYPE
        || element[6] != WMM_PARAMETER_SUBTYPE
        || element[7] != WMM_VERSION
    {
        return None;
    }

    let qos_info = element[8];
    let mut access_categories = [WmmAcParameters::default(); 4];
    let mut seen = 0_u8;
    for record in element[10..26].chunks_exact(4) {
        let aci = (record[0] >> 5) & 0x03;
        let category = WmmAccessCategory::from_aci(aci)?;
        let bit = 1_u8 << aci;
        if seen & bit != 0 {
            return None;
        }
        seen |= bit;
        access_categories[category.index()] = WmmAcParameters {
            admission_control_mandatory: record[0] & 0x10 != 0,
            aifsn: record[0] & 0x0f,
            ecw_min: record[1] & 0x0f,
            ecw_max: record[1] >> 4,
            txop_limit_units_32_us: u16::from_le_bytes([record[2], record[3]]),
        };
    }
    if seen != 0x0f {
        return None;
    }

    Some(WmmParameterSet {
        parameter_set_count: qos_info & 0x0f,
        uapsd: qos_info & 0x80 != 0,
        access_categories,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        Dscp, WMM_OUI_AND_TYPE, WMM_PARAMETER_BODY_LEN, WmmAccessCategory, WmmClassificationSource,
        WmmUserPriority, classify_ethernet_wmm, parse_wmm_parameter_element,
    };

    const STANDARD_PARAMETER_ELEMENT: [u8; 26] = [
        221, 24, 0x00, 0x50, 0xf2, 0x02, 1, 1, 0x85, 0, 0x03, 0xa4, 0, 0, 0x27, 0xa4, 0, 0, 0x42,
        0x43, 94, 0, 0x72, 0x32, 47, 0,
    ];

    #[test]
    fn parses_all_four_access_categories_and_complete_txop() {
        let parameters = parse_wmm_parameter_element(&STANDARD_PARAMETER_ELEMENT).unwrap();
        assert_eq!(parameters.parameter_set_count, 5);
        assert!(parameters.uapsd);

        let best_effort = parameters.access_category(WmmAccessCategory::BestEffort);
        assert_eq!(best_effort.aifsn, 3);
        assert_eq!(best_effort.ecw_min, 4);
        assert_eq!(best_effort.ecw_max, 10);
        assert_eq!(best_effort.txop_limit_units_32_us, 0);

        let video = parameters.access_category(WmmAccessCategory::Video);
        assert_eq!(video.aifsn, 2);
        assert_eq!(video.ecw_min, 3);
        assert_eq!(video.ecw_max, 4);
        assert_eq!(video.txop_limit_units_32_us, 94);

        let voice = parameters.access_category(WmmAccessCategory::Voice);
        assert!(voice.admission_control_mandatory);
        assert_eq!(voice.txop_limit_units_32_us, 47);
    }

    #[test]
    fn retains_the_standard_txop_high_byte_that_the_blob_drops() {
        let mut element = STANDARD_PARAMETER_ELEMENT;
        element[21] = 1;
        assert_eq!(
            parse_wmm_parameter_element(&element)
                .unwrap()
                .access_category(WmmAccessCategory::Video)
                .txop_limit_units_32_us,
            350
        );
    }

    #[test]
    fn rejects_wrong_identity_short_elements_and_duplicate_aci() {
        let mut element = STANDARD_PARAMETER_ELEMENT;
        element[2..6].copy_from_slice(&WMM_OUI_AND_TYPE);
        element[1] = (WMM_PARAMETER_BODY_LEN - 1) as u8;
        assert!(parse_wmm_parameter_element(&element).is_none());

        element = STANDARD_PARAMETER_ELEMENT;
        element[22] = 0x42;
        assert!(parse_wmm_parameter_element(&element).is_none());
    }

    #[test]
    fn typed_user_priorities_map_to_the_four_standard_access_categories() {
        let expected = [
            WmmAccessCategory::BestEffort,
            WmmAccessCategory::Background,
            WmmAccessCategory::Background,
            WmmAccessCategory::BestEffort,
            WmmAccessCategory::Video,
            WmmAccessCategory::Video,
            WmmAccessCategory::Voice,
            WmmAccessCategory::Voice,
        ];
        for (value, category) in expected.into_iter().enumerate() {
            let priority = WmmUserPriority::new(value as u8).unwrap();
            assert_eq!(priority.value(), value as u8);
            assert_eq!(priority.access_category(), category);
        }
        assert_eq!(WmmUserPriority::new(8), None);
    }

    #[test]
    fn station_dscp_mapping_bleaches_unassigned_and_network_control_values() {
        let cases = [
            (0, 0),
            (1, 1),
            (8, 1),
            (10, 0),
            (18, 3),
            (24, 4),
            (34, 4),
            (40, 5),
            (44, 6),
            (46, 6),
            (48, 0),
            (56, 0),
            (63, 0),
        ];
        for (dscp, priority) in cases {
            assert_eq!(
                Dscp::new(dscp).unwrap().station_user_priority().value(),
                priority
            );
        }
        assert_eq!(Dscp::new(64), None);
        assert_eq!(Dscp::new(46).unwrap().default_user_priority().value(), 5);
    }

    #[test]
    fn ethernet_classifier_uses_vlan_pcp_before_ipv4_dscp() {
        let mut frame = [0_u8; 22];
        frame[12..14].copy_from_slice(&0x8100_u16.to_be_bytes());
        frame[14..16].copy_from_slice(&(5_u16 << 13).to_be_bytes());
        frame[16..18].copy_from_slice(&0x0800_u16.to_be_bytes());
        frame[18] = 0x45;
        frame[19] = 46 << 2;

        let class = classify_ethernet_wmm(&frame);
        assert_eq!(class.user_priority, WmmUserPriority::UP5);
        assert_eq!(class.access_category, WmmAccessCategory::Video);
        assert_eq!(
            class.source,
            WmmClassificationSource::Ieee8021d(WmmUserPriority::UP5)
        );
    }

    #[test]
    fn ethernet_classifier_extracts_ipv4_and_ipv6_dscp_fail_closed() {
        let mut ipv4 = [0_u8; 16];
        ipv4[12..14].copy_from_slice(&0x0800_u16.to_be_bytes());
        ipv4[14] = 0x45;
        ipv4[15] = 46 << 2;
        let class = classify_ethernet_wmm(&ipv4);
        assert_eq!(class.user_priority, WmmUserPriority::UP6);
        assert_eq!(class.access_category, WmmAccessCategory::Voice);

        let mut ipv6 = [0_u8; 16];
        ipv6[12..14].copy_from_slice(&0x86dd_u16.to_be_bytes());
        let traffic_class = 40_u8 << 2;
        ipv6[14] = 0x60 | (traffic_class >> 4);
        ipv6[15] = traffic_class << 4;
        let class = classify_ethernet_wmm(&ipv6);
        assert_eq!(class.user_priority, WmmUserPriority::UP5);

        ipv6[14] = 0x40;
        assert_eq!(
            classify_ethernet_wmm(&ipv6).user_priority,
            WmmUserPriority::UP0
        );
        assert_eq!(
            classify_ethernet_wmm(&[0; 13]).user_priority,
            WmmUserPriority::UP0
        );
    }
}
