//! Association RSN element validation and suite selection.
//!
//! An authenticator validates a station's Association element with it, and a
//! supplicant derives the suite of its own element. Peer queues, callbacks and
//! chip node pointers are deliberately outside this crate.

use oer_ieee80211_mac::security::rsn::{
    RSN_CAPABILITY_MFPR, RSN_CIPHER_CCMP, RsnElement, RsnSyntaxError, ieee_suite,
};

use crate::{
    Akm,
    frames::{OwnedRsnIe, RsnFrameError},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RsnElementError {
    Malformed,
    CapacityExceeded,
    UnsupportedVersion,
    UnsupportedGroupCipher,
    UnsupportedPairwiseCipher,
    UnsupportedAkm,
    ManagementFrameProtectionUnsupported,
    PmkidCachingUnsupported,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedRsnElement {
    owned: OwnedRsnIe,
    akm: Akm,
    capabilities: u16,
}

impl ValidatedRsnElement {
    /// The first supported suite of the element's AKM list.
    pub const fn akm(&self) -> Akm {
        self.akm
    }

    pub fn owned(&self) -> &OwnedRsnIe {
        &self.owned
    }

    pub const fn capabilities(&self) -> u16 {
        self.capabilities
    }

    pub fn into_owned(self) -> OwnedRsnIe {
        self.owned
    }
}

/// Validate the CCMP subset implemented by the handshake state machines and
/// select the first supported [`Akm`] of the element.
///
/// A syntactically incomplete element is [`RsnElementError::Malformed`]
/// before any suite, capability or PMKID policy is applied.
pub fn validate_rsn_element(bytes: &[u8]) -> Result<ValidatedRsnElement, RsnElementError> {
    let owned = OwnedRsnIe::try_copy(bytes).map_err(|error| match error {
        RsnFrameError::CapacityExceeded => RsnElementError::CapacityExceeded,
        _ => RsnElementError::Malformed,
    })?;
    let rsn = RsnElement::parse(bytes).map_err(|error| match error {
        RsnSyntaxError::Malformed => RsnElementError::Malformed,
        RsnSyntaxError::UnsupportedVersion => RsnElementError::UnsupportedVersion,
    })?;

    if rsn.group_data_cipher() != ieee_suite(RSN_CIPHER_CCMP) {
        return Err(RsnElementError::UnsupportedGroupCipher);
    }
    if !rsn.pairwise_ciphers().contains(ieee_suite(RSN_CIPHER_CCMP)) {
        return Err(RsnElementError::UnsupportedPairwiseCipher);
    }
    let akm = rsn
        .akm_suites()
        .iter()
        .find_map(Akm::from_suite_selector)
        .ok_or(RsnElementError::UnsupportedAkm)?;

    let capabilities = rsn.capabilities().unwrap_or(0);
    if capabilities & RSN_CAPABILITY_MFPR != 0 {
        return Err(RsnElementError::ManagementFrameProtectionUnsupported);
    }
    if rsn.pmkid_count().is_some_and(|count| count != 0) {
        return Err(RsnElementError::PmkidCachingUnsupported);
    }
    // Without PMF negotiation this profile defines no Group Management
    // Cipher Suite in an association element.
    if rsn.group_management_cipher().is_some() {
        return Err(RsnElementError::Malformed);
    }
    Ok(ValidatedRsnElement {
        owned,
        akm,
        capabilities,
    })
}

#[cfg(test)]
mod tests;
