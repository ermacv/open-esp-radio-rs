//! RFC 8110 section 4.4, RFC 5869 and IEEE 802.11 pairwise key hierarchy.
use super::*;
use crate::crypto::PAIRWISE_KEY_EXPANSION_LABEL;
use crate::{HandshakeSuite, PtkContext, aes, akm::owe_key_geometry, kdf};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256, Sha384, Sha512};
use subtle::ConstantTimeEq;
use zeroize::{Zeroize, ZeroizeOnDrop};

const MAX_PMK_LEN: usize = owe_key_geometry(Group::P521).pmk_len();
pub(super) const MAX_COORDINATE_LEN: usize = Group::P521.coordinate_len();
const MAX_PTK_LEN: usize = owe_key_geometry(Group::P521).ptk_len();
const MAX_MIC_LEN: usize = owe_key_geometry(Group::P521).mic.octets();
const OWE_PMK_LABEL: &[u8] = b"OWE Key Generation";

/// Secret PMK of one OWE group and address pair, wiped on drop.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct OwePmk {
    #[zeroize(skip)]
    group: Group,
    #[zeroize(skip)]
    addresses: Addresses,
    bytes: [u8; MAX_PMK_LEN],
    pmkid: [u8; RSN_PMKID_LEN],
}
impl OwePmk {
    /// Called only after the curve owner has validated both compact public
    /// keys and derived a non-infinite DH result. It is not a public shortcut
    /// around peer-point validation.
    pub(super) fn derive(
        group: Group,
        addresses: Addresses,
        station_public: &[u8],
        ap_public: &[u8],
        secret: &[u8],
    ) -> Result<Self, Error> {
        addresses.validate()?;
        if station_public.len() != group.coordinate_len()
            || ap_public.len() != group.coordinate_len()
            || secret.len() != group.coordinate_len()
        {
            return Err(Error::InvalidKey);
        }
        let mut salt = [0; 2 * MAX_COORDINATE_LEN + core::mem::size_of::<u16>()];
        let coordinate_len = group.coordinate_len();
        salt[..coordinate_len].copy_from_slice(station_public);
        salt[coordinate_len..2 * coordinate_len].copy_from_slice(ap_public);
        let group_octets = group.number().to_le_bytes();
        let salt_len = 2 * coordinate_len + group_octets.len();
        salt[2 * coordinate_len..salt_len].copy_from_slice(&group_octets);
        let salt = &salt[..salt_len];
        let mut result = Self {
            group,
            addresses,
            bytes: [0; MAX_PMK_LEN],
            pmkid: [0; RSN_PMKID_LEN],
        };
        let destination = &mut result.bytes[..owe_key_geometry(group).pmk_len()];
        macro_rules! expand {
            ($hash:ty) => {{
                Hkdf::<$hash>::new(Some(salt), secret)
                    .expand(OWE_PMK_LABEL, destination)
                    .map_err(|_| Error::InvalidKey)?;
                let mut hash = <$hash>::new();
                Digest::update(&mut hash, station_public);
                Digest::update(&mut hash, ap_public);
                let mut digest = hash.finalize();
                result.pmkid.copy_from_slice(&digest[..RSN_PMKID_LEN]);
                digest.zeroize();
            }};
        }
        match group {
            Group::P256 => expand!(Sha256),
            Group::P384 => expand!(Sha384),
            Group::P521 => expand!(Sha512),
        }
        Ok(result)
    }
    pub const fn group(&self) -> Group {
        self.group
    }
    pub const fn addresses(&self) -> Addresses {
        self.addresses
    }
    pub const fn pmkid(&self) -> [u8; RSN_PMKID_LEN] {
        self.pmkid
    }
    pub fn duplicate(&self) -> Self {
        Self {
            group: self.group,
            addresses: self.addresses,
            bytes: self.bytes,
            pmkid: self.pmkid,
        }
    }
    pub fn derive_ptk(&self, context: PtkContext) -> Result<OwePtk, Error> {
        if context.authenticator_address != self.addresses.access_point
            || context.supplicant_address != self.addresses.station
            || context.authenticator_nonce.iter().all(|octet| *octet == 0)
            || context.supplicant_nonce.iter().all(|octet| *octet == 0)
        {
            return Err(Error::WrongContext);
        }
        let mut canonical = context.canonical();
        let mut result = OwePtk {
            group: self.group,
            context,
            bytes: [0; MAX_PTK_LEN],
        };
        let geometry = owe_key_geometry(self.group);
        let pmk = &self.bytes[..geometry.pmk_len()];
        let destination = &mut result.bytes[..geometry.ptk_len()];
        match self.group {
            Group::P256 => {
                kdf::kdf_sha256(pmk, PAIRWISE_KEY_EXPANSION_LABEL, &canonical, destination)
            }
            Group::P384 => {
                kdf::kdf_sha384(pmk, PAIRWISE_KEY_EXPANSION_LABEL, &canonical, destination)
            }
            Group::P521 => {
                kdf::kdf_sha512(pmk, PAIRWISE_KEY_EXPANSION_LABEL, &canonical, destination)
            }
        }
        canonical.zeroize();
        Ok(result)
    }
}

#[cfg(test)]
impl OwePtk {
    pub(super) fn test_bytes(&self) -> &[u8] {
        &self.bytes
            [..self.kck().len() + self.kek().len() + oer_ieee80211_mac::security::CCMP_128_KEY_LEN]
    }
}

/// Full KCK/KEK/TK, with the exact group geometry and peer scope attached.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct OwePtk {
    #[zeroize(skip)]
    group: Group,
    #[zeroize(skip)]
    context: PtkContext,
    bytes: [u8; MAX_PTK_LEN],
}
impl OwePtk {
    pub const fn group(&self) -> Group {
        self.group
    }
    pub const fn addresses(&self) -> Addresses {
        Addresses {
            station: self.context.supplicant_address,
            access_point: self.context.authenticator_address,
        }
    }
    pub const fn context(&self) -> PtkContext {
        self.context
    }
    pub(super) fn same_key(&self, other: &Self) -> bool {
        self.group == other.group && bool::from(self.bytes.ct_eq(&other.bytes))
    }
    fn kck(&self) -> &[u8] {
        &self.bytes[..self.group.eapol_mic_length().octets()]
    }
    fn kek(&self) -> &[u8] {
        let start = self.kck().len();
        &self.bytes[start..start + owe_key_geometry(self.group).kek_len]
    }
    pub fn temporal_key(&self) -> &[u8; oer_ieee80211_mac::security::CCMP_128_KEY_LEN] {
        let start = self.kck().len() + owe_key_geometry(self.group).kek_len;
        self.bytes[start..start + oer_ieee80211_mac::security::CCMP_128_KEY_LEN]
            .try_into()
            .expect("checked suite geometry")
    }
    fn mic(&self, parts: &[&[u8]]) -> [u8; MAX_MIC_LEN] {
        let mut result = [0; MAX_MIC_LEN];
        macro_rules! hmac {
            ($hash:ty) => {{
                let mut mac = <Hmac<$hash> as Mac>::new_from_slice(self.kck())
                    .expect("HMAC accepts KCK lengths");
                for part in parts {
                    Mac::update(&mut mac, part);
                }
                let mut digest = mac.finalize().into_bytes();
                result[..self.kck().len()].copy_from_slice(&digest[..self.kck().len()]);
                digest.zeroize();
            }};
        }
        match self.group {
            Group::P256 => hmac!(Sha256),
            Group::P384 => hmac!(Sha384),
            Group::P521 => hmac!(Sha512),
        }
        result
    }
    /// Validate the negotiated descriptor and geometry before authentication;
    /// the shared RSN automaton separately owns replay/nonce/phase admission.
    pub fn verify_eapol(&self, bytes: &[u8]) -> Result<(), Error> {
        let length = self.group.eapol_mic_length();
        let frame = crate::EapolKeyFrame::parse_with_mic_length(bytes, length)?;
        if frame.key_info().descriptor_version() != self.group.eapol_descriptor_version() {
            return Err(Error::WrongContext);
        }
        let zero = [0; MAX_MIC_LEN];
        let mut expected = self.mic(&[
            &bytes[..crate::eapol::EAPOL_KEY_MIC_START],
            &zero[..length.octets()],
            &bytes[length.end()..],
        ]);
        let matches = expected[..length.octets()].ct_eq(frame.mic());
        expected.zeroize();
        if bool::from(matches) {
            Ok(())
        } else {
            Err(Error::InvalidMic)
        }
    }
    /// Atomic validation precedes clearing and replacing the MIC field.
    pub fn authenticate_eapol(&self, bytes: &mut [u8]) -> Result<(), Error> {
        let length = self.group.eapol_mic_length();
        let frame = crate::EapolKeyFrame::parse_with_mic_length(bytes, length)?;
        if frame.key_info().descriptor_version() != self.group.eapol_descriptor_version() {
            return Err(Error::WrongContext);
        }
        bytes[crate::eapol::EAPOL_KEY_MIC_START..length.end()].fill(0);
        let mut mic = self.mic(&[bytes]);
        bytes[crate::eapol::EAPOL_KEY_MIC_START..length.end()]
            .copy_from_slice(&mic[..length.octets()]);
        mic.zeroize();
        Ok(())
    }
    pub fn wrap_key_data(&self, plaintext: &[u8]) -> Result<aes::RsnWrappedKeyData, Error> {
        match self.group {
            Group::P256 => aes::software_aes128_key_wrap(
                self.kek().try_into().expect("group 19 KEK"),
                plaintext,
            ),
            Group::P384 | Group::P521 => aes::software_aes256_key_wrap(
                self.kek().try_into().expect("group 20/21 KEK"),
                plaintext,
            ),
        }
        .map_err(Error::Wrap)
    }
    pub fn unwrap_key_data(&self, encrypted: &[u8]) -> Result<aes::RsnUnwrappedKeyData, Error> {
        match self.group {
            Group::P256 => aes::software_aes128_key_unwrap(
                self.kek().try_into().expect("group 19 KEK"),
                encrypted,
            ),
            Group::P384 | Group::P521 => aes::software_aes256_key_unwrap(
                self.kek().try_into().expect("group 20/21 KEK"),
                encrypted,
            ),
        }
        .map_err(Error::Unwrap)
    }
}

#[cfg(test)]
impl OwePmk {
    pub(super) fn test_bytes(&self) -> &[u8] {
        &self.bytes[..owe_key_geometry(self.group).pmk_len()]
    }
}
