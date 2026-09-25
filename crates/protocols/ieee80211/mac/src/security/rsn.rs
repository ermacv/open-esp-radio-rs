//! RSN element wire syntax (IEEE 802.11-2020 9.4.2.24).
//!
//! [`RsnElement::parse`] validates the complete version-1 field sequence and
//! returns borrowed views of it. It applies no suite or capability policy:
//! station candidate selection and the RSN authenticator/supplicant apply
//! their own acceptance rules to the same parsed element.

/// Element ID of the RSN element.
pub const RSN_ELEMENT_ID: u8 = 48;
/// The only RSN element version defined by IEEE 802.11.
pub const RSN_VERSION: u16 = 1;
/// OUI of the IEEE 802.11 cipher and AKM suite selectors.
pub const RSN_OUI: [u8; 3] = [0x00, 0x0f, 0xac];
/// Cipher suite type of CCMP-128.
pub const RSN_CIPHER_CCMP: u8 = 4;
/// AKM suite type of PSK with SHA-1 key derivation.
pub const RSN_AKM_PSK: u8 = 2;
/// RSN Capabilities: management frame protection required.
pub const RSN_CAPABILITY_MFPR: u16 = 1 << 6;
/// RSN Capabilities: management frame protection capable.
pub const RSN_CAPABILITY_MFPC: u16 = 1 << 7;
/// RSN Capabilities: signaling-and-payload-protected A-MSDU capable.
pub const RSN_CAPABILITY_SPP_AMSDU_CAPABLE: u16 = 1 << 10;
/// Length of one PMKID.
pub const RSN_PMKID_LEN: usize = 16;

const SUITE_LEN: usize = 4;

/// The suite selector for `suite_type` under the IEEE 802.11 OUI.
pub const fn ieee_suite(suite_type: u8) -> [u8; 4] {
    [RSN_OUI[0], RSN_OUI[1], RSN_OUI[2], suite_type]
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
    pmkid_count: Option<u16>,
    group_management_cipher: Option<[u8; 4]>,
}

impl<'a> RsnElement<'a> {
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
        let pmkid_count = reader.optional(|reader| {
            let count = reader.u16()?;
            reader.take(usize::from(count) * RSN_PMKID_LEN)?;
            Ok(count)
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
            pmkid_count,
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
        self.pmkid_count
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
