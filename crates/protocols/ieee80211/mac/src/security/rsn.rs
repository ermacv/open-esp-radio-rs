//! RSN element wire syntax (IEEE 802.11-2020 9.4.2.24).
//!
//! [`RsnElement::parse`] validates the complete version-1 field sequence and
//! returns borrowed views of it. It applies no suite or capability policy:
//! station candidate selection and the RSN authenticator/supplicant apply
//! their own acceptance rules to the same parsed element.

/// Element ID of the RSN element.
pub const RSN_ELEMENT_ID: u8 = 48;
/// RSN Extension element shared by association and FT integrity processing.
pub const RSNXE_ELEMENT_ID: u8 = 244;
/// RSNXE first-octet Capability Field Length (encoded as length minus one).
pub const RSNXE_LENGTH_MASK: u8 = 0x0f;
/// SAE Hash-to-Element capability in the first RSNXE octet.
pub const RSNXE_SAE_H2E: u8 = 1 << 5;
/// The only RSN element version defined by IEEE 802.11.
pub const RSN_VERSION: u16 = 1;
/// OUI of the IEEE 802.11 cipher and AKM suite selectors.
pub const RSN_OUI: [u8; 3] = [0x00, 0x0f, 0xac];
/// Cipher suite type of CCMP-128.
pub const RSN_CIPHER_CCMP: u8 = 4;
/// AKM suite type of PSK with SHA-1 key derivation.
pub const RSN_AKM_PSK: u8 = 2;
/// `00-0F-AC:6`: PSK with SHA-256 key derivation.
pub const RSN_AKM_PSK_SHA256: u8 = 6;
/// `00-0F-AC:8`: SAE.
pub const RSN_AKM_SAE: u8 = 8;
/// `00-0F-AC:18`: Opportunistic Wireless Encryption (RFC 8110).
pub const RSN_AKM_OWE: u8 = 18;
pub const RSN_AKM_FT_8021X: u8 = 3;
pub const RSN_AKM_FT_PSK: u8 = 4;
pub const RSN_AKM_FT_SAE: u8 = 9;
/// `00-0F-AC:6`: BIP-CMAC-128, the default group management cipher.
pub const RSN_CIPHER_BIP_CMAC_128: u8 = 6;
/// RSN Capabilities: management frame protection required.
pub const RSN_CAPABILITY_MFPR: u16 = 1 << 6;
/// RSN Capabilities: management frame protection capable.
pub const RSN_CAPABILITY_MFPC: u16 = 1 << 7;
/// RSN Capabilities: signaling-and-payload-protected A-MSDU capable.
pub const RSN_CAPABILITY_SPP_AMSDU_CAPABLE: u16 = 1 << 10;
/// Length of one PMKID.
pub const RSN_PMKID_LEN: usize = 16;

const SUITE_LEN: usize = 4;

/// FT suites using the SHA-256 hierarchy and a 128-bit CMAC.
///
/// This subset requires PMK-R0/R1 ownership. It cannot be passed to the
/// ordinary [`Akm`] PMK expansion or silently selected by an existing role.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FtAkm {
    Ieee8021X,
    Psk,
    Sae,
}

impl FtAkm {
    pub const fn suite_selector(self) -> [u8; 4] {
        ieee_suite(match self {
            Self::Ieee8021X => RSN_AKM_FT_8021X,
            Self::Psk => RSN_AKM_FT_PSK,
            Self::Sae => RSN_AKM_FT_SAE,
        })
    }

    pub const fn from_suite_selector(selector: [u8; 4]) -> Option<Self> {
        match selector {
            [0x00, 0x0f, 0xac, RSN_AKM_FT_8021X] => Some(Self::Ieee8021X),
            [0x00, 0x0f, 0xac, RSN_AKM_FT_PSK] => Some(Self::Psk),
            [0x00, 0x0f, 0xac, RSN_AKM_FT_SAE] => Some(Self::Sae),
            _ => None,
        }
    }
}

/// The suite selector for `suite_type` under the IEEE 802.11 OUI.
pub const fn ieee_suite(suite_type: u8) -> [u8; 4] {
    [RSN_OUI[0], RSN_OUI[1], RSN_OUI[2], suite_type]
}

/// Authentication and key management suite named by an RSN element.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Akm {
    /// `00-0F-AC:2`, PSK with PRF-SHA1 key expansion and HMAC-SHA1-128 MIC
    /// (key descriptor version 2).
    Psk,
    /// `00-0F-AC:6`, PSK with the SHA-256 key derivation function and
    /// AES-128-CMAC MIC (key descriptor version 3), as protected management
    /// frames select.
    PskSha256,
    /// `00-0F-AC:8`, SAE: the PMK comes from the SAE exchange; the SHA-256
    /// key derivation function and AES-128-CMAC MIC under the AKM-defined
    /// key descriptor version 0.
    Sae,
}

impl Akm {
    /// The suite named by an RSN element AKM selector, if supported.
    pub const fn from_suite_selector(selector: [u8; 4]) -> Option<Self> {
        match selector {
            [0x00, 0x0f, 0xac, RSN_AKM_PSK] => Some(Self::Psk),
            [0x00, 0x0f, 0xac, RSN_AKM_PSK_SHA256] => Some(Self::PskSha256),
            [0x00, 0x0f, 0xac, RSN_AKM_SAE] => Some(Self::Sae),
            _ => None,
        }
    }

    pub const fn suite_selector(self) -> [u8; 4] {
        ieee_suite(match self {
            Self::Psk => RSN_AKM_PSK,
            Self::PskSha256 => RSN_AKM_PSK_SHA256,
            Self::Sae => RSN_AKM_SAE,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RsnSyntaxError {
    /// The header, a counted list or an optional field is incomplete, or
    /// bytes remain after the last defined field.
    Malformed,
    /// The element is well framed but not version 1, so its body layout is
    /// unknown.
    UnsupportedVersion,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RsnEncodeError {
    ElementTooLong,
    SuiteNotAdvertised,
    OutputTooSmall { required: usize },
}

/// A counted list of four-byte suite selectors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RsnSuiteList<'a> {
    bytes: &'a [u8],
}

impl<'a> RsnSuiteList<'a> {
    pub const fn len(&self) -> usize {
        self.bytes.len() / SUITE_LEN
    }

    pub const fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = [u8; 4]> + 'a {
        self.bytes
            .chunks_exact(SUITE_LEN)
            .map(|suite| [suite[0], suite[1], suite[2], suite[3]])
    }

    pub fn contains(&self, suite: [u8; 4]) -> bool {
        self.iter().any(|candidate| candidate == suite)
    }
}

/// A syntactically complete version-1 RSN element.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RsnElement<'a> {
    group_data_cipher: [u8; 4],
    pairwise_ciphers: RsnSuiteList<'a>,
    akm_suites: RsnSuiteList<'a>,
    capabilities: Option<u16>,
    pmkids: Option<&'a [u8]>,
    group_management_cipher: Option<[u8; 4]>,
}

impl<'a> RsnElement<'a> {
    /// Encode the caller's selected suite from this advertisement.
    pub fn encode_selected_akm(
        self,
        akm: [u8; SUITE_LEN],
        pmkids: &[[u8; RSN_PMKID_LEN]],
        output: &mut [u8],
    ) -> Result<usize, RsnEncodeError> {
        if !self.akm_suites.contains(akm) {
            return Err(RsnEncodeError::SuiteNotAdvertised);
        }
        RsnElement {
            group_data_cipher: self.group_data_cipher,
            pairwise_ciphers: self.pairwise_ciphers,
            akm_suites: RsnSuiteList { bytes: &akm },
            capabilities: self.capabilities,
            pmkids: self.pmkids,
            group_management_cipher: self.group_management_cipher,
        }
        .encode_with_pmkids(pmkids, output)
    }
    /// Preserve suites and capabilities while replacing the PMKID list.
    ///
    /// FT uses PMKR0Name during authentication and PMKR1Name during
    /// reassociation. This wire operation applies no FT or PMKSA policy.
    pub fn encode_with_pmkids(
        self,
        pmkids: &[[u8; RSN_PMKID_LEN]],
        output: &mut [u8],
    ) -> Result<usize, RsnEncodeError> {
        let fixed = 2
            + 2
            + SUITE_LEN
            + 2
            + self.pairwise_ciphers.bytes.len()
            + 2
            + self.akm_suites.bytes.len()
            + 2
            + 2;
        let length = fixed
            .checked_add(
                pmkids
                    .len()
                    .checked_mul(RSN_PMKID_LEN)
                    .ok_or(RsnEncodeError::ElementTooLong)?,
            )
            .and_then(|length| {
                length.checked_add(usize::from(self.group_management_cipher.is_some()) * SUITE_LEN)
            })
            .ok_or(RsnEncodeError::ElementTooLong)?;
        if length - 2 > u8::MAX as usize {
            return Err(RsnEncodeError::ElementTooLong);
        }
        let output = output
            .get_mut(..length)
            .ok_or(RsnEncodeError::OutputTooSmall { required: length })?;
        output[0] = RSN_ELEMENT_ID;
        output[1] = (length - 2) as u8;
        let mut offset = 2;
        for bytes in [
            RSN_VERSION.to_le_bytes().as_slice(),
            self.group_data_cipher.as_slice(),
            (self.pairwise_ciphers.len() as u16)
                .to_le_bytes()
                .as_slice(),
            self.pairwise_ciphers.bytes,
            (self.akm_suites.len() as u16).to_le_bytes().as_slice(),
            self.akm_suites.bytes,
            self.capabilities.unwrap_or(0).to_le_bytes().as_slice(),
            (pmkids.len() as u16).to_le_bytes().as_slice(),
        ] {
            output[offset..offset + bytes.len()].copy_from_slice(bytes);
            offset += bytes.len();
        }
        for pmkid in pmkids {
            output[offset..offset + RSN_PMKID_LEN].copy_from_slice(pmkid);
            offset += RSN_PMKID_LEN;
        }
        if let Some(suite) = self.group_management_cipher {
            output[offset..].copy_from_slice(&suite);
        }
        Ok(length)
    }
    /// Parse one complete element, including its ID and length octets.
    ///
    /// The fields after the AKM list are optional only as a trailing
    /// sequence; every field that is started must be complete, and no bytes
    /// may follow the Group Management Cipher Suite.
    pub fn parse(element: &'a [u8]) -> Result<Self, RsnSyntaxError> {
        let [RSN_ELEMENT_ID, length, body @ ..] = element else {
            return Err(RsnSyntaxError::Malformed);
        };
        if usize::from(*length) != body.len() {
            return Err(RsnSyntaxError::Malformed);
        }

        let mut reader = Reader { bytes: body };
        if reader.u16()? != RSN_VERSION {
            return Err(RsnSyntaxError::UnsupportedVersion);
        }
        let group_data_cipher = reader.suite()?;
        let pairwise_ciphers = reader.suite_list()?;
        let akm_suites = reader.suite_list()?;
        let capabilities = reader.optional(Reader::u16)?;
        let pmkids = reader.optional(|reader| {
            let count = reader.u16()?;
            reader.take(usize::from(count) * RSN_PMKID_LEN)
        })?;
        let group_management_cipher = reader.optional(Reader::suite)?;
        if !reader.bytes.is_empty() {
            return Err(RsnSyntaxError::Malformed);
        }

        Ok(Self {
            group_data_cipher,
            pairwise_ciphers,
            akm_suites,
            capabilities,
            pmkids,
            group_management_cipher,
        })
    }

    pub const fn group_data_cipher(&self) -> [u8; 4] {
        self.group_data_cipher
    }

    pub const fn pairwise_ciphers(&self) -> RsnSuiteList<'a> {
        self.pairwise_ciphers
    }

    pub const fn akm_suites(&self) -> RsnSuiteList<'a> {
        self.akm_suites
    }

    /// RSN Capabilities, or `None` when the element ends after the AKM list.
    pub const fn capabilities(&self) -> Option<u16> {
        self.capabilities
    }

    /// Number of PMKIDs, or `None` when the PMKID Count field is absent.
    pub const fn pmkid_count(&self) -> Option<u16> {
        match self.pmkids {
            Some(pmkids) => Some((pmkids.len() / RSN_PMKID_LEN) as u16),
            None => None,
        }
    }

    /// The listed PMKIDs, in order.
    pub fn pmkids(&self) -> impl Iterator<Item = [u8; RSN_PMKID_LEN]> + use<'a> {
        self.pmkids
            .unwrap_or(&[])
            .chunks_exact(RSN_PMKID_LEN)
            .map(|pmkid| pmkid.try_into().expect("a PMKID chunk has its length"))
    }

    pub const fn group_management_cipher(&self) -> Option<[u8; 4]> {
        self.group_management_cipher
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    fn take(&mut self, length: usize) -> Result<&'a [u8], RsnSyntaxError> {
        if length > self.bytes.len() {
            return Err(RsnSyntaxError::Malformed);
        }
        let (taken, rest) = self.bytes.split_at(length);
        self.bytes = rest;
        Ok(taken)
    }

    fn u16(&mut self) -> Result<u16, RsnSyntaxError> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn suite(&mut self) -> Result<[u8; 4], RsnSyntaxError> {
        let bytes = self.take(SUITE_LEN)?;
        Ok([bytes[0], bytes[1], bytes[2], bytes[3]])
    }

    fn suite_list(&mut self) -> Result<RsnSuiteList<'a>, RsnSyntaxError> {
        let count = usize::from(self.u16()?);
        Ok(RsnSuiteList {
            bytes: self.take(count * SUITE_LEN)?,
        })
    }

    fn optional<T>(
        &mut self,
        read: impl FnOnce(&mut Self) -> Result<T, RsnSyntaxError>,
    ) -> Result<Option<T>, RsnSyntaxError> {
        if self.bytes.is_empty() {
            Ok(None)
        } else {
            read(self).map(Some)
        }
    }
}

#[cfg(test)]
mod tests;
