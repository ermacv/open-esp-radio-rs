//! Bounded WMM Parameter Element parsing.

const VENDOR_ELEMENT_ID: u8 = 221;
const WMM_PARAMETER_BODY_LEN: usize = 24;
const WMM_OUI_AND_TYPE: [u8; 4] = [0x00, 0x50, 0xf2, 0x02];
const WMM_PARAMETER_SUBTYPE: u8 = 1;
const WMM_VERSION: u8 = 1;

/// Standard WMM access-category identifier carried in ACI bits 5..6.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum WmmAccessCategory {
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
/// `_oracles/libnet80211.a[ieee80211_sta.o]::ieee80211_parse_wmeparams`
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
        WMM_OUI_AND_TYPE, WMM_PARAMETER_BODY_LEN, WmmAccessCategory, parse_wmm_parameter_element,
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
}
