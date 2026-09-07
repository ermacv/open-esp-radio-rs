//! Caller-selected AP advertisements, independent of silicon and peer state.
//!
//! Fixed record capacities preserve the implemented beacon/association format.
//! The chip profile chooses values; these types validate and encode them.

use crate::{
    extensions::wmm::WmmAcParameters, ht::HtLocalCapabilities, security::WifiSecurityMode,
};

/// The Supported Rates and Extended Supported Rates records of this AP.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyRates {
    supported: [u8; 8],
    extended: [u8; 4],
}

impl LegacyRates {
    /// Rates use 500-kbit/s units, with bit seven marking a basic rate.
    pub const fn new(supported: [u8; 8], extended: [u8; 4]) -> Self {
        Self {
            supported,
            extended,
        }
    }

    pub const fn supported(&self) -> &[u8; 8] {
        &self.supported
    }

    pub const fn extended(&self) -> &[u8; 4] {
        &self.extended
    }

    pub fn supports(&self, rate_500kbps: u8) -> bool {
        self.supported
            .iter()
            .chain(self.extended.iter())
            .any(|encoded| encoded & 0x7f == rate_500kbps)
    }
}

/// Advertised WMM access parameters in standard ACI order: BE, BK, VI, VO.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WmmParameters {
    parameter_set_count: u8,
    uapsd: bool,
    access_categories: [WmmAcParameters; 4],
}

impl WmmParameters {
    pub const fn new(
        parameter_set_count: u8,
        uapsd: bool,
        access_categories: [WmmAcParameters; 4],
    ) -> Self {
        assert!(parameter_set_count <= 15);
        let mut index = 0;
        while index < access_categories.len() {
            let category = access_categories[index];
            assert!(category.aifsn <= 15 && category.ecw_min <= 15 && category.ecw_max <= 15);
            index += 1;
        }
        Self {
            parameter_set_count,
            uapsd,
            access_categories,
        }
    }

    /// Encode a complete vendor-specific WMM Parameter Element.
    pub const fn element(self) -> [u8; 26] {
        let mut element = [0; 26];
        element[0] = 221;
        element[1] = 24;
        element[3] = 0x50;
        element[4] = 0xf2;
        element[5] = 2;
        element[6] = 1;
        element[7] = 1;
        element[8] = self.parameter_set_count | if self.uapsd { 0x80 } else { 0 };
        let mut index = 0;
        while index < self.access_categories.len() {
            let category = self.access_categories[index];
            let offset = 10 + index * 4;
            element[offset] = ((index as u8) << 5)
                | if category.admission_control_mandatory {
                    0x10
                } else {
                    0
                }
                | category.aifsn;
            element[offset + 1] = (category.ecw_max << 4) | category.ecw_min;
            let txop = category.txop_limit_units_32_us.to_le_bytes();
            element[offset + 2] = txop[0];
            element[offset + 3] = txop[1];
            index += 1;
        }
        element
    }
}

/// One coherent local advertisement used by AP discovery and association.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Advertisement {
    pub legacy_rates: LegacyRates,
    pub ht: HtLocalCapabilities,
    pub wmm: WmmParameters,
    capability_information: u16,
    erp_information: u8,
}

impl Advertisement {
    /// Privacy belongs to the selected security mode, not the hardware profile.
    pub const fn new(
        legacy_rates: LegacyRates,
        ht: HtLocalCapabilities,
        wmm: WmmParameters,
        capability_information: u16,
        erp_information: u8,
    ) -> Self {
        assert!(capability_information & 0x0010 == 0);
        assert!(erp_information & !0x07 == 0);
        Self {
            legacy_rates,
            ht,
            wmm,
            capability_information,
            erp_information,
        }
    }

    pub const fn capabilities(&self, security: WifiSecurityMode) -> u16 {
        self.capability_information
            | match security {
                WifiSecurityMode::Open => 0,
                WifiSecurityMode::Wpa2Personal => 0x0010,
            }
    }

    pub const fn erp_information(&self) -> u8 {
        self.erp_information
    }
}

#[cfg(test)]
pub(crate) mod tests;
