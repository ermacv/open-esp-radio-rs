//! Association RSN element validation and suite selection.
//!
//! An authenticator validates a station's Association element with it, and a
//! supplicant derives the suite of its own element. Peer queues, callbacks and
//! chip node pointers are deliberately outside this crate.

use crate::{
    Akm,
    frames::{OwnedRsnIe, RsnFrameError},
};

const RSN_ELEMENT_ID: u8 = 0x30;
const RSN_VERSION: u16 = 1;
const RSN_CAPABILITY_MFPR: u16 = 1 << 6;
const RSN_OUI: [u8; 3] = [0x00, 0x0f, 0xac];
const RSN_CIPHER_CCMP: u8 = 4;

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

fn read_u16(bytes: &[u8], offset: &mut usize) -> Result<u16, RsnElementError> {
    let value = bytes
        .get(*offset..*offset + 2)
        .ok_or(RsnElementError::Malformed)?;
    *offset += 2;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}

fn read_suite(bytes: &[u8], offset: &mut usize) -> Result<[u8; 4], RsnElementError> {
    let suite = bytes
        .get(*offset..*offset + 4)
        .ok_or(RsnElementError::Malformed)?;
    *offset += 4;
    Ok([suite[0], suite[1], suite[2], suite[3]])
}

fn supported_suite(suite: [u8; 4], selector: u8) -> bool {
    suite[..3] == RSN_OUI && suite[3] == selector
}

/// Validate the CCMP subset implemented by the handshake state machines and
/// select the first supported [`Akm`] of the element.
pub fn validate_rsn_element(bytes: &[u8]) -> Result<ValidatedRsnElement, RsnElementError> {
    let owned = OwnedRsnIe::try_copy(bytes).map_err(|error| match error {
        RsnFrameError::CapacityExceeded => RsnElementError::CapacityExceeded,
        _ => RsnElementError::Malformed,
    })?;
    if bytes.first() != Some(&RSN_ELEMENT_ID) {
        return Err(RsnElementError::Malformed);
    }

    let body = &bytes[2..];
    let mut offset = 0;
    if read_u16(body, &mut offset)? != RSN_VERSION {
        return Err(RsnElementError::UnsupportedVersion);
    }
    if !supported_suite(read_suite(body, &mut offset)?, RSN_CIPHER_CCMP) {
        return Err(RsnElementError::UnsupportedGroupCipher);
    }

    let pairwise_count = usize::from(read_u16(body, &mut offset)?);
    if pairwise_count == 0 {
        return Err(RsnElementError::UnsupportedPairwiseCipher);
    }
    let mut pairwise_ccmp = false;
    for _ in 0..pairwise_count {
        pairwise_ccmp |= supported_suite(read_suite(body, &mut offset)?, RSN_CIPHER_CCMP);
    }
    if !pairwise_ccmp {
        return Err(RsnElementError::UnsupportedPairwiseCipher);
    }

    let akm_count = usize::from(read_u16(body, &mut offset)?);
    if akm_count == 0 {
        return Err(RsnElementError::UnsupportedAkm);
    }
    let mut akm = None;
    for _ in 0..akm_count {
        let selected = Akm::from_suite_selector(read_suite(body, &mut offset)?);
        akm = akm.or(selected);
    }
    let akm = akm.ok_or(RsnElementError::UnsupportedAkm)?;

    let capabilities = if offset < body.len() {
        let capabilities = read_u16(body, &mut offset)?;
        if capabilities & RSN_CAPABILITY_MFPR != 0 {
            return Err(RsnElementError::ManagementFrameProtectionUnsupported);
        }
        capabilities
    } else {
        0
    };
    if offset < body.len() && read_u16(body, &mut offset)? != 0 {
        return Err(RsnElementError::PmkidCachingUnsupported);
    }
    if offset != body.len() {
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
