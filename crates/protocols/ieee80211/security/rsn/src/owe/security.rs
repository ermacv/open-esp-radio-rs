//! OWE/CCMP admission. Wire syntax remains with MAC; role policy lives here.
use super::*;
use crate::element::ManagementFrameProtection;
use oer_ieee80211_mac::{
    management::elements::Elements,
    security::rsn::{
        RSN_AKM_OWE, RSN_CAPABILITY_MFPC, RSN_CAPABILITY_MFPR, RSN_CIPHER_BIP_CMAC_128,
        RSN_CIPHER_CCMP, RSN_ELEMENT_ID, RSNXE_ELEMENT_ID, RSNXE_LENGTH_MASK, RsnElement,
        ieee_suite,
    },
};

/// Borrowed OWE advertisement or selected association security elements.
/// Only CCMP-128/BIP-CMAC-128 are implemented by this RSN owner. Unknown AKMs
/// in an advertisement are preserved; an Association Request selects OWE.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SecurityProfile<'a> {
    rsn: &'a [u8],
    rsnxe: &'a [u8],
    protection: ManagementFrameProtection,
    selected: bool,
}
impl<'a> SecurityProfile<'a> {
    /// Extract security fields from a completely framed IE list. Other IEs
    /// remain with their association owner, including the OWE DH parameter.
    pub fn from_elements(elements: Elements<'a>) -> Result<Self, Error> {
        let rsn = elements
            .unique(RSN_ELEMENT_ID)?
            .ok_or(Error::UnsupportedSecurity)?;
        let rsnxe = elements.unique(RSNXE_ELEMENT_ID)?;
        if let Some(element) = rsnxe {
            let first = *element.body.first().ok_or(Error::UnsupportedSecurity)?;
            if usize::from(first & RSNXE_LENGTH_MASK) + 1 != element.body.len() {
                return Err(Error::UnsupportedSecurity);
            }
        }
        let parsed = RsnElement::parse(rsn.encoded()).map_err(|_| Error::UnsupportedSecurity)?;
        let capabilities = parsed.capabilities().unwrap_or(0);
        let protection = ManagementFrameProtection {
            capable: capabilities & RSN_CAPABILITY_MFPC != 0,
            required: capabilities & RSN_CAPABILITY_MFPR != 0,
        };
        if parsed.group_data_cipher() != ieee_suite(RSN_CIPHER_CCMP)
            || parsed.pairwise_ciphers().len() != 1
            || !parsed
                .pairwise_ciphers()
                .contains(ieee_suite(RSN_CIPHER_CCMP))
            || !parsed.akm_suites().contains(ieee_suite(RSN_AKM_OWE))
            || (protection.required && !protection.capable)
            || parsed.group_management_cipher().is_some_and(|cipher| {
                !protection.capable || cipher != ieee_suite(RSN_CIPHER_BIP_CMAC_128)
            })
        {
            return Err(Error::UnsupportedSecurity);
        }
        Ok(Self {
            rsn: rsn.encoded(),
            rsnxe: rsnxe.map_or(&[], |element| element.encoded()),
            protection,
            selected: parsed.akm_suites().len() == 1,
        })
    }
    pub const fn rsn(self) -> &'a [u8] {
        self.rsn
    }
    pub const fn rsnxe(self) -> &'a [u8] {
        self.rsnxe
    }
    pub const fn protection(self) -> ManagementFrameProtection {
        self.protection
    }
    pub const fn is_selected(self) -> bool {
        self.selected
    }

    /// RFC 8110 does not itself require PMF. An Enhanced Open policy can
    /// require it through MFPR; contradictory MFPR/MFPC is always rejected.
    pub fn negotiate(self, selected: Self) -> Result<bool, Error> {
        if !selected.selected
            || (self.protection.required && !selected.protection.capable)
            || (selected.protection.required && !self.protection.capable)
        {
            return Err(Error::SecurityMismatch);
        }
        Ok(self.protection.negotiated(selected.protection))
    }

    /// Serialize only security IEs, for exact Association/EAPOL binding.
    /// Capacity is checked before changing caller storage.
    pub fn encode(self, output: &mut [u8]) -> Result<usize, Error> {
        let length = self.rsn.len() + self.rsnxe.len();
        let output = output.get_mut(..length).ok_or(Error::CapacityExceeded)?;
        output[..self.rsn.len()].copy_from_slice(self.rsn);
        output[self.rsn.len()..].copy_from_slice(self.rsnxe);
        Ok(length)
    }
}
