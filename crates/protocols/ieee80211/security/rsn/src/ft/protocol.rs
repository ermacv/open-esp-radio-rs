//! FT security admission, protected IE construction and group-key delivery.
use super::*;
use crate::aes::{RFC3394_OVERHEAD_LEN, software_aes128_key_unwrap, software_aes128_key_wrap};
use crate::frames::{RSN_GTK_LEN, RSN_IGTK_LEN, RSN_IPN_LEN, RsnGtk, RsnIgtk};
use oer_ieee80211_mac::management::elements::{
    ELEMENT_HEADER_LEN, Elements, MAX_ENCODED_ELEMENT_LEN,
};
use oer_ieee80211_mac::security::SaePwe;
use oer_ieee80211_mac::security::rsn::{
    RSN_CAPABILITY_MFPC, RSN_CAPABILITY_MFPR, RSN_CIPHER_BIP_CMAC_128, RSN_CIPHER_CCMP,
    RSN_PMKID_LEN, RSNXE_ELEMENT_ID, RsnElement, ieee_suite,
};

const GTK_RSC_LEN: usize = 8;
const KEY_ID_LEN: usize = 2;
const KEY_LENGTH_LEN: usize = 1;
const GTK_PREFIX_LEN: usize = KEY_ID_LEN + KEY_LENGTH_LEN + GTK_RSC_LEN;
const IGTK_PREFIX_LEN: usize = KEY_ID_LEN + RSN_IPN_LEN + KEY_LENGTH_LEN;
const GTK_SUBELEMENT_LEN: usize =
    ELEMENT_HEADER_LEN + GTK_PREFIX_LEN + RSN_GTK_LEN + RFC3394_OVERHEAD_LEN;
const IGTK_SUBELEMENT_LEN: usize =
    ELEMENT_HEADER_LEN + IGTK_PREFIX_LEN + RSN_IGTK_LEN + RFC3394_OVERHEAD_LEN;

/// Selected FT/CCMP security and the immutable scan/advertisement bytes.
/// Caller-owned storage is borrowed instead of copied into every AP peer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SecurityProfile<'a> {
    rsn: &'a [u8],
    rsnxe: &'a [u8],
    akm: FtAkm,
    management: bool,
    sae_pwe: Option<SaePwe>,
}
impl<'a> SecurityProfile<'a> {
    pub fn new(
        rsn: &'a [u8],
        rsnxe: &'a [u8],
        akm: FtAkm,
        management: bool,
        sae_pwe: Option<SaePwe>,
    ) -> Result<Self, Error> {
        if (akm == FtAkm::Sae) != sae_pwe.is_some() {
            return Err(Error::InvalidConfiguration);
        }
        let profile = Self {
            rsn,
            rsnxe,
            akm,
            management,
            sae_pwe,
        };
        profile.accept_rsn(rsn)?;
        if !rsnxe.is_empty() {
            let list = Elements::parse(rsnxe).map_err(wire::WireError::from)?;
            let value = list
                .unique(RSNXE_ELEMENT_ID)
                .map_err(wire::WireError::from)?
                .ok_or(Error::UnsupportedSecurity)?;
            if value.encoded() != rsnxe
                || value.body.is_empty()
                || usize::from(value.body[0] & oer_ieee80211_mac::security::rsn::RSNXE_LENGTH_MASK)
                    + 1
                    != value.body.len()
            {
                return Err(Error::UnsupportedSecurity);
            }
        }
        if profile.rsnxe_used()
            && (rsnxe.len() < 3 || rsnxe[2] & oer_ieee80211_mac::security::rsn::RSNXE_SAE_H2E == 0)
        {
            return Err(Error::UnsupportedSecurity);
        }
        Ok(profile)
    }
    pub const fn sae_pwe(self) -> Option<SaePwe> {
        self.sae_pwe
    }
    pub fn rsnxe_used(self) -> bool {
        self.sae_pwe == Some(SaePwe::HashToElement)
    }
    pub(crate) fn without_rsnxe(self) -> Self {
        Self { rsnxe: &[], ..self }
    }
    pub const fn akm(self) -> FtAkm {
        self.akm
    }
    pub const fn protects_management(self) -> bool {
        self.management
    }
    pub const fn rsn(self) -> &'a [u8] {
        self.rsn
    }
    pub const fn rsnxe(self) -> &'a [u8] {
        self.rsnxe
    }
    pub(crate) fn accept_rsn(self, bytes: &[u8]) -> Result<(), Error> {
        let rsn = RsnElement::parse(bytes).map_err(|_| Error::UnsupportedSecurity)?;
        let flags = rsn.capabilities().unwrap_or(0);
        if rsn.group_data_cipher() != ieee_suite(RSN_CIPHER_CCMP)
            || rsn.pairwise_ciphers().len() != 1
            || !rsn.pairwise_ciphers().contains(ieee_suite(RSN_CIPHER_CCMP))
            || !rsn.akm_suites().contains(self.akm.suite_selector())
            || (flags & RSN_CAPABILITY_MFPR != 0 && !self.management)
            || (self.management && flags & RSN_CAPABILITY_MFPC == 0)
            || (self.akm == FtAkm::Sae && !self.management)
            || rsn
                .group_management_cipher()
                .is_some_and(|cipher| cipher != ieee_suite(RSN_CIPHER_BIP_CMAC_128))
        {
            return Err(Error::UnsupportedSecurity);
        }
        Ok(())
    }
    pub(crate) fn accept(
        self,
        elements: wire::FtElements<'_>,
        name: [u8; RSN_PMKID_LEN],
        response: bool,
    ) -> Result<(), Error> {
        self.accept_rsn(elements.rsn)?;
        let rsn = RsnElement::parse(elements.rsn).map_err(|_| Error::UnsupportedSecurity)?;
        if rsn.pmkid_count() != Some(1) || rsn.pmkids().next() != Some(name) {
            return Err(Error::WrongKeyContext);
        }
        if response {
            if elements.rsnxe != self.rsnxe {
                return Err(Error::SecurityMismatch);
            }
            let advertised = RsnElement::parse(self.rsn).map_err(|_| Error::UnsupportedSecurity)?;
            if rsn.capabilities().unwrap_or(0) != advertised.capabilities().unwrap_or(0)
                || rsn.group_management_cipher() != advertised.group_management_cipher()
                || !rsn.akm_suites().iter().eq(advertised.akm_suites().iter())
            {
                return Err(Error::SecurityMismatch);
            }
        }
        Ok(())
    }
}

/// Group keys authenticated and unwrapped together before any installation.
pub struct TransitionGroupKeys {
    pub gtk: RsnGtk,
    pub receive_sequence: [u8; GTK_RSC_LEN],
    pub igtk: Option<RsnIgtk>,
}

pub(crate) struct Packet<const N: usize> {
    bytes: [u8; N],
    len: usize,
}
impl<const N: usize> Packet<N> {
    pub(crate) fn bytes_for_encoding(&mut self) -> &mut [u8] {
        &mut self.bytes
    }
    pub(crate) fn set_encoded_length(&mut self, len: usize) {
        assert!(len <= N);
        self.len = len;
    }
    pub(crate) const fn empty() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
        }
    }
    pub(crate) fn copy(bytes: &[u8]) -> Result<Self, Error> {
        let mut packet = Self::empty();
        if bytes.len() > N {
            return Err(Error::CapacityExceeded);
        }
        packet.bytes[..bytes.len()].copy_from_slice(bytes);
        packet.len = bytes.len();
        Ok(packet)
    }
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let end = self
            .len
            .checked_add(bytes.len())
            .ok_or(Error::CapacityExceeded)?;
        self.bytes
            .get_mut(self.len..end)
            .ok_or(Error::CapacityExceeded)?
            .copy_from_slice(bytes);
        self.len = end;
        Ok(())
    }
    pub(crate) fn element(&mut self, id: u8, body: &[u8]) -> Result<(), Error> {
        let length = u8::try_from(body.len()).map_err(|_| Error::CapacityExceeded)?;
        self.push(&[id, length])?;
        self.push(body)
    }
}

pub(crate) struct IeRequest<'a> {
    pub security: SecurityProfile<'a>,
    pub selected_rsn: bool,
    pub md: MobilityDomain,
    pub root_name: [u8; RSN_PMKID_LEN],
    pub r0kh: R0khId,
    pub r1kh: Option<R1khId>,
    pub snonce: [u8; RSN_NONCE_LEN],
    pub anonce: [u8; RSN_NONCE_LEN],
    pub ric: &'a [u8],
    pub groups: Option<&'a TransitionGroupKeys>,
    pub protected: Option<(&'a FtPtk, wire::MicTransaction)>,
    pub reassociation_deadline_tu: Option<u32>,
}
pub(crate) fn build_ies<const N: usize>(request: IeRequest<'_>) -> Result<Packet<N>, Error> {
    let mut packet = Packet::empty();
    let rsn = RsnElement::parse(request.security.rsn).map_err(|_| Error::UnsupportedSecurity)?;
    let mut rsn_bytes = [0; MAX_ENCODED_ELEMENT_LEN];
    let length = if request.selected_rsn {
        rsn.encode_selected_akm(
            request.security.akm.suite_selector(),
            &[request.root_name],
            &mut rsn_bytes,
        )
    } else {
        rsn.encode_with_pmkids(&[request.root_name], &mut rsn_bytes)
    }
    .map_err(|_| Error::CapacityExceeded)?;
    packet.push(&rsn_bytes[..length])?;
    packet.push(&request.md.encode())?;
    let mut subelements = Packet::<{ MAX_ENCODED_ELEMENT_LEN }>::empty();
    if let Some(r1kh) = request.r1kh {
        subelements.element(wire::subelement_id::R1KH, &r1kh.0)?;
    }
    subelements.element(wire::subelement_id::R0KH, request.r0kh.as_bytes())?;
    if let Some(groups) = request.groups {
        let (ptk, _) = request.protected.ok_or(Error::WrongPhase)?;
        let wrapped = software_aes128_key_wrap(ptk.kek(), groups.gtk.key())
            .map_err(|_| Error::InvalidGroupKeys)?;
        let mut body = Packet::<{ GTK_SUBELEMENT_LEN }>::empty();
        body.push(&u16::from(groups.gtk.key_id()).to_le_bytes())?;
        body.push(&[RSN_GTK_LEN as u8])?;
        body.push(&groups.receive_sequence)?;
        body.push(wrapped.as_bytes())?;
        subelements.element(wire::subelement_id::GTK, body.bytes())?;
        if let Some(igtk) = &groups.igtk {
            let wrapped = software_aes128_key_wrap(ptk.kek(), igtk.key())
                .map_err(|_| Error::InvalidGroupKeys)?;
            let mut body = Packet::<{ IGTK_SUBELEMENT_LEN }>::empty();
            body.push(&u16::from(igtk.key_id()).to_le_bytes())?;
            body.push(&igtk.packet_number())?;
            body.push(&[RSN_IGTK_LEN as u8])?;
            body.push(wrapped.as_bytes())?;
            subelements.element(wire::subelement_id::IGTK, body.bytes())?;
        }
    }
    let resources = wire::RicElements::parse(request.ric)?.elements();
    let count = if request.protected.is_some() {
        u8::try_from(
            usize::from(wire::FT_BASE_PROTECTED_ELEMENT_COUNT)
                + resources.iter().count()
                + usize::from(!request.security.rsnxe.is_empty()),
        )
        .map_err(|_| Error::CapacityExceeded)?
    } else {
        0
    };
    let mut ft = [0; MAX_ENCODED_ELEMENT_LEN];
    let length = wire::FastTransitionFields {
        element_count: count,
        rsnxe_used: request.protected.is_some() && request.security.rsnxe_used(),
        mic: [0; wire::FT_MIC_LEN],
        anonce: request.anonce,
        snonce: request.snonce,
        subelements: Elements::parse(subelements.bytes()).map_err(wire::WireError::from)?,
    }
    .encode(&mut ft)?;
    let ft_offset = packet.len;
    packet.push(&ft[..length])?;
    packet.push(request.ric)?;
    packet.push(request.security.rsnxe)?;
    if let Some(tu) = request.reassociation_deadline_tu {
        packet.push(&wire::TimeoutInterval::ReassociationDeadline { tu }.encode())?;
    }
    let parsed = wire::FtElements::parse(packet.bytes())?;
    if parsed.ric != request.ric {
        return Err(Error::Wire(wire::WireError::InconsistentFields));
    }
    if let Some((ptk, transaction)) = request.protected {
        let mic = ptk.mic(transaction, parsed)?;
        packet.bytes[ft_offset + wire::FT_MIC_OFFSET..ft_offset + wire::FT_ANONCE_OFFSET]
            .copy_from_slice(&mic);
    }
    Ok(packet)
}

pub(crate) fn unwrap_groups(
    ptk: &FtPtk,
    ft: wire::FastTransitionElement<'_>,
    management: bool,
) -> Result<TransitionGroupKeys, Error> {
    let subelements = ft.subelements();
    if subelements
        .unique(wire::subelement_id::BIGTK)
        .map_err(wire::WireError::from)?
        .is_some()
        || subelements
            .unique(wire::subelement_id::OCI)
            .map_err(wire::WireError::from)?
            .is_some()
    {
        return Err(Error::UnsupportedSecurity);
    }
    let gtk = subelements
        .unique(wire::subelement_id::GTK)
        .map_err(wire::WireError::from)?
        .ok_or(Error::InvalidGroupKeys)?
        .body;
    if gtk.len() != GTK_PREFIX_LEN + RSN_GTK_LEN + RFC3394_OVERHEAD_LEN
        || usize::from(gtk[KEY_ID_LEN]) != RSN_GTK_LEN
    {
        return Err(Error::InvalidGroupKeys);
    }
    let info = u16::from_le_bytes(gtk[..KEY_ID_LEN].try_into().expect("GTK prefix"));
    let clear = software_aes128_key_unwrap(ptk.kek(), &gtk[GTK_PREFIX_LEN..])
        .map_err(|_| Error::InvalidGroupKeys)?;
    let key = RsnGtk::new(
        (info & 3) as u8,
        false,
        clear
            .as_bytes()
            .try_into()
            .map_err(|_| Error::InvalidGroupKeys)?,
    )
    .map_err(|_| Error::InvalidGroupKeys)?;
    let receive_sequence = gtk[KEY_ID_LEN + KEY_LENGTH_LEN..GTK_PREFIX_LEN]
        .try_into()
        .expect("GTK RSC");
    let wrapped_igtk = subelements
        .unique(wire::subelement_id::IGTK)
        .map_err(wire::WireError::from)?;
    if management != wrapped_igtk.is_some() {
        return Err(Error::InvalidGroupKeys);
    }
    let igtk = if let Some(value) = wrapped_igtk {
        let body = value.body;
        if body.len() != IGTK_PREFIX_LEN + RSN_IGTK_LEN + RFC3394_OVERHEAD_LEN
            || usize::from(body[IGTK_PREFIX_LEN - KEY_LENGTH_LEN]) != RSN_IGTK_LEN
        {
            return Err(Error::InvalidGroupKeys);
        }
        let key_id = u16::from_le_bytes(body[..KEY_ID_LEN].try_into().expect("IGTK prefix"));
        let clear = software_aes128_key_unwrap(ptk.kek(), &body[IGTK_PREFIX_LEN..])
            .map_err(|_| Error::InvalidGroupKeys)?;
        Some(
            RsnIgtk::new(
                u8::try_from(key_id).map_err(|_| Error::InvalidGroupKeys)?,
                body[KEY_ID_LEN..KEY_ID_LEN + RSN_IPN_LEN]
                    .try_into()
                    .expect("IGTK IPN"),
                clear
                    .as_bytes()
                    .try_into()
                    .map_err(|_| Error::InvalidGroupKeys)?,
            )
            .map_err(|_| Error::InvalidGroupKeys)?,
        )
    } else {
        None
    };
    Ok(TransitionGroupKeys {
        gtk: key,
        receive_sequence,
        igtk,
    })
}
