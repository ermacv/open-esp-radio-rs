//! IEEE 802.11r-2008 8.5.1.5.3–5 key hierarchy and FT MIC.
//!
//! SOURCE: hostap `src/common/wpa_common.c`, wpa_derive_pmk_r0,
//! wpa_derive_pmk_r1, wpa_pmk_r1_to_ptk and wpa_ft_mic.
use super::*;
use crate::keys::{CcmpKey, RSN_TK_LEN};
use crate::{Pmk, RSN_KCK_LEN, RSN_KEK_LEN};
use aes::Aes128;
use cmac::{Cmac, Mac};
use oer_ieee80211_mac::security::SaePwe;
use oer_ieee80211_mac::security::rsn::RSN_PMKID_LEN;
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, ZeroizeOnDrop};

const FT_ROOT_KEY_LEN: usize = 32;
const FT_ROOT_NAME_SALT_LEN: usize = 16;
const EAP_MSK_LEN: usize = 64;
const FT_PTK_LEN: usize = RSN_KCK_LEN + RSN_KEK_LEN + RSN_TK_LEN;

/// Initial FT key source, with the authentication suite inseparable from it.
/// SAE and EAP must have succeeded before the caller creates this value.
pub struct InitialKey {
    akm: FtAkm,
    key: Pmk,
    sae_pwe: Option<SaePwe>,
}
impl InitialKey {
    pub fn psk(key: Pmk) -> Self {
        Self {
            akm: FtAkm::Psk,
            key,
            sae_pwe: None,
        }
    }
    pub fn authenticated_sae(pmk: Pmk, pwe: SaePwe) -> Self {
        Self {
            akm: FtAkm::Sae,
            key: pmk,
            sae_pwe: Some(pwe),
        }
    }
    /// FT-802.1X uses MSK octets 32..64, not the ordinary EAP PMK (0..32).
    pub fn authenticated_eap_msk(msk: &[u8; EAP_MSK_LEN]) -> Self {
        Self {
            akm: FtAkm::Ieee8021X,
            sae_pwe: None,
            key: Pmk::from_bytes(msk[FT_ROOT_KEY_LEN..].try_into().expect("MSK second half")),
        }
    }
    pub const fn akm(&self) -> FtAkm {
        self.akm
    }
    pub const fn sae_pwe(&self) -> Option<SaePwe> {
        self.sae_pwe
    }
    pub fn derive_r0(self, context: RootContext) -> PmkR0 {
        let mut kdf_context = [0; 1
            + oer_ieee80211_mac::management::MAX_SSID_LEN
            + wire::MOBILITY_DOMAIN_ID_LEN
            + 1
            + wire::R0KH_ID_MAX_LEN
            + oer_ieee80211_mac::management::MAC_ADDRESS_LEN];
        let mut length = 0;
        for value in [
            &[context.ssid.len() as u8][..],
            context.ssid.as_bytes(),
            context.mobility_domain.0.as_slice(),
            &[context.r0kh.as_bytes().len() as u8],
            context.r0kh.as_bytes(),
            context.station.as_slice(),
        ] {
            kdf_context[length..length + value.len()].copy_from_slice(value);
            length += value.len();
        }
        let mut data = [0; FT_ROOT_KEY_LEN + FT_ROOT_NAME_SALT_LEN];
        crate::kdf::kdf_sha256(
            self.key.as_bytes(),
            b"FT-R0",
            &kdf_context[..length],
            &mut data,
        );
        let name = hash_name(&[b"FT-R0N", &data[FT_ROOT_KEY_LEN..]]);
        let key = Pmk::from_bytes(data[..FT_ROOT_KEY_LEN].try_into().expect("root key width"));
        data.zeroize();
        kdf_context.zeroize();
        PmkR0 {
            key,
            name,
            context,
            akm: self.akm,
            sae_pwe: self.sae_pwe,
        }
    }
}

pub struct PmkR0 {
    key: Pmk,
    name: [u8; RSN_PMKID_LEN],
    context: RootContext,
    akm: FtAkm,
    sae_pwe: Option<SaePwe>,
}
impl PmkR0 {
    pub const fn context(&self) -> RootContext {
        self.context
    }
    pub const fn akm(&self) -> FtAkm {
        self.akm
    }
    pub const fn sae_pwe(&self) -> Option<SaePwe> {
        self.sae_pwe
    }
    pub const fn name(&self) -> [u8; RSN_PMKID_LEN] {
        self.name
    }
    pub fn duplicate(&self) -> Self {
        Self {
            key: self.key.duplicate(),
            name: self.name,
            context: self.context,
            akm: self.akm,
            sae_pwe: self.sae_pwe,
        }
    }
    pub fn derive_r1(&self, r1kh: R1khId) -> PmkR1 {
        let mut context = [0; wire::R1KH_ID_LEN + oer_ieee80211_mac::management::MAC_ADDRESS_LEN];
        context[..wire::R1KH_ID_LEN].copy_from_slice(&r1kh.0);
        context[wire::R1KH_ID_LEN..].copy_from_slice(&self.context.station);
        let mut key = [0; FT_ROOT_KEY_LEN];
        crate::kdf::kdf_sha256(self.key.as_bytes(), b"FT-R1", &context, &mut key);
        let result = PmkR1 {
            key: Pmk::from_bytes(key),
            root_name: self.name,
            name: r1_name(self.name, r1kh, self.context.station),
            root: self.context,
            r1kh,
            akm: self.akm,
            sae_pwe: self.sae_pwe,
        };
        key.zeroize();
        context.zeroize();
        result
    }
}

/// Secret-bearing key delivery. Only an authenticated, authorized inter-AP
/// transport may construct it; this does not authenticate a wire packet.
pub struct PmkR1 {
    key: Pmk,
    root_name: [u8; RSN_PMKID_LEN],
    name: [u8; RSN_PMKID_LEN],
    root: RootContext,
    r1kh: R1khId,
    akm: FtAkm,
    sae_pwe: Option<SaePwe>,
}
impl PmkR1 {
    pub fn from_authenticated_transfer(
        key: Pmk,
        root: RootContext,
        root_name: [u8; RSN_PMKID_LEN],
        r1kh: R1khId,
        akm: FtAkm,
        sae_pwe: Option<SaePwe>,
    ) -> Result<Self, Error> {
        if (akm == FtAkm::Sae) != sae_pwe.is_some() {
            return Err(Error::WrongKeyContext);
        }
        Ok(Self {
            key,
            root,
            root_name,
            r1kh,
            akm,
            sae_pwe,
            name: r1_name(root_name, r1kh, root.station),
        })
    }
    /// Borrow only while sealing an authorized inter-AP delivery.
    pub fn transfer_key(&self) -> &[u8; FT_ROOT_KEY_LEN] {
        self.key.as_bytes()
    }
    pub const fn root_context(&self) -> RootContext {
        self.root
    }
    pub const fn root_name(&self) -> [u8; RSN_PMKID_LEN] {
        self.root_name
    }
    pub const fn r1kh(&self) -> R1khId {
        self.r1kh
    }
    pub const fn akm(&self) -> FtAkm {
        self.akm
    }
    pub const fn sae_pwe(&self) -> Option<SaePwe> {
        self.sae_pwe
    }
    pub const fn name(&self) -> [u8; RSN_PMKID_LEN] {
        self.name
    }
    pub fn duplicate(&self) -> Self {
        Self {
            key: self.key.duplicate(),
            root: self.root,
            root_name: self.root_name,
            r1kh: self.r1kh,
            akm: self.akm,
            name: self.name,
            sae_pwe: self.sae_pwe,
        }
    }
    pub fn derive_ptk(&self, context: FtPtkContext) -> Result<FtPtk, Error> {
        if context.addresses.station != self.root.station {
            return Err(Error::WrongKeyContext);
        }
        if context.snonce == [0; RSN_NONCE_LEN] || context.anonce == [0; RSN_NONCE_LEN] {
            return Err(Error::ZeroNonce);
        }
        let mut canonical =
            [0; 2 * RSN_NONCE_LEN + 2 * oer_ieee80211_mac::management::MAC_ADDRESS_LEN];
        let mut offset = 0;
        for value in [
            context.snonce.as_slice(),
            context.anonce.as_slice(),
            context.addresses.access_point.as_slice(),
            context.addresses.station.as_slice(),
        ] {
            canonical[offset..offset + value.len()].copy_from_slice(value);
            offset += value.len();
        }
        let mut bytes = [0; FT_PTK_LEN];
        crate::kdf::kdf_sha256(self.key.as_bytes(), b"FT-PTK", &canonical, &mut bytes);
        let name = hash_name(&[&self.name, b"FT-PTKN", &canonical]);
        let ptk = FtPtk {
            akm: self.akm,
            context,
            name,
            kck: bytes[..RSN_KCK_LEN].try_into().expect("KCK width"),
            kek: bytes[RSN_KCK_LEN..RSN_KCK_LEN + RSN_KEK_LEN]
                .try_into()
                .expect("KEK width"),
            temporal: CcmpKey::new(
                bytes[RSN_KCK_LEN + RSN_KEK_LEN..]
                    .try_into()
                    .expect("CCMP width"),
            ),
        };
        canonical.zeroize();
        bytes.zeroize();
        Ok(ptk)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FtPtkContext {
    pub addresses: wire::FtAddresses,
    pub snonce: [u8; RSN_NONCE_LEN],
    pub anonce: [u8; RSN_NONCE_LEN],
}

/// FT-specific authority: it cannot enter the ordinary PMK expansion path.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct FtPtk {
    #[zeroize(skip)]
    akm: FtAkm,
    #[zeroize(skip)]
    context: FtPtkContext,
    #[zeroize(skip)]
    name: [u8; RSN_PMKID_LEN],
    kck: [u8; RSN_KCK_LEN],
    kek: [u8; RSN_KEK_LEN],
    temporal: CcmpKey,
}
impl FtPtk {
    pub const fn akm(&self) -> FtAkm {
        self.akm
    }
    pub const fn context(&self) -> FtPtkContext {
        self.context
    }
    pub const fn name(&self) -> [u8; RSN_PMKID_LEN] {
        self.name
    }
    pub const fn temporal_key(&self) -> &CcmpKey {
        &self.temporal
    }
    pub(crate) const fn kck(&self) -> &[u8; RSN_KCK_LEN] {
        &self.kck
    }
    /// Borrow for a caller-selected AES key-wrap backend.
    pub const fn kek(&self) -> &[u8; RSN_KEK_LEN] {
        &self.kek
    }
    pub fn mic(
        &self,
        transaction: wire::MicTransaction,
        elements: wire::FtElements<'_>,
    ) -> Result<[u8; wire::FT_MIC_LEN], Error> {
        Ok(self
            .mic_state(transaction, elements)?
            .finalize()
            .into_bytes()
            .into())
    }
    pub fn verify_mic(
        &self,
        transaction: wire::MicTransaction,
        elements: wire::FtElements<'_>,
    ) -> Result<(), Error> {
        self.mic_state(transaction, elements)?
            .verify_slice(elements.ft.mic())
            .map_err(|_| Error::InvalidMic)
    }
    fn mic_state(
        &self,
        transaction: wire::MicTransaction,
        elements: wire::FtElements<'_>,
    ) -> Result<Cmac<Aes128>, Error> {
        if elements.ft.element_count() != elements.protected_element_count()? {
            return Err(Error::Wire(wire::WireError::InconsistentFields));
        }
        let mut mac = Cmac::<Aes128>::new_from_slice(&self.kck).expect("KCK is an AES-128 key");
        mac.update(&self.context.addresses.station);
        mac.update(&self.context.addresses.access_point);
        mac.update(&[transaction as u8]);
        mac.update(elements.rsn);
        mac.update(elements.md);
        mac.update(&elements.ft.encoded()[..wire::FT_MIC_OFFSET]);
        mac.update(&[0; wire::FT_MIC_LEN]);
        mac.update(&elements.ft.encoded()[wire::FT_ANONCE_OFFSET..]);
        mac.update(elements.ric);
        mac.update(elements.rsnxe);
        Ok(mac)
    }
}

fn r1_name(
    root_name: [u8; RSN_PMKID_LEN],
    r1kh: R1khId,
    station: MacAddress,
) -> [u8; RSN_PMKID_LEN] {
    hash_name(&[b"FT-R1N", &root_name, &r1kh.0, &station])
}
fn hash_name(parts: &[&[u8]]) -> [u8; RSN_PMKID_LEN] {
    let mut hash = Sha256::new();
    for part in parts {
        hash.update(part);
    }
    let mut digest = hash.finalize();
    let mut name = [0; RSN_PMKID_LEN];
    name.copy_from_slice(&digest[..RSN_PMKID_LEN]);
    digest.zeroize();
    name
}
