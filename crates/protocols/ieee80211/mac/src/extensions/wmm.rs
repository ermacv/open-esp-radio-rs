//! Bounded parsing of the vendor-specific WMM Parameter Element.
//!
//! This extension retains the advertised AC parameters. Shared user-priority
//! values and traffic classification live in `qos`; parsing does not admit TX.

use crate::qos::WmmAccessCategory;

const VENDOR_ELEMENT_ID: u8 = 221;
const WMM_PARAMETER_BODY_LEN: usize = 24;
const WMM_OUI_AND_TYPE: [u8; 4] = [0x00, 0x50, 0xf2, 0x02];
const WMM_PARAMETER_SUBTYPE: u8 = 1;
const WMM_VERSION: u8 = 1;

impl WmmAccessCategory {
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
mod tests;
