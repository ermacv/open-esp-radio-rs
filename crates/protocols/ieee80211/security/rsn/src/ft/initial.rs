//! Initial FT association binds the existing four-way exchange to R0/R1 keys.
//!
//! SOURCE: hostap `src/rsn_supp/wpa.c`, wpa_supplicant_send_2_of_4 and
//! ft_validate_{mdie,ftie,rsnie}; `src/ap/wpa_auth.c`, ft_check_msg_2_of_4.

use super::protocol::Packet;
use super::*;
use crate::frames::key_data::{FtKeyDataContext, parse_ft_gtk_key_data};
use crate::frames::{RsnGroupKeys, RsnPlainKeyData, RsnTxFrame};
use crate::state::{PtkContext, RsnApPhase, RsnApState, RsnStaPhase, RsnStaState};
use oer_ieee80211_mac::management::MAC_ADDRESS_LEN;
use oer_ieee80211_mac::management::elements::{Elements, MAX_ENCODED_ELEMENT_LEN};
use oer_ieee80211_mac::security::rsn::RsnElement;

/// Initial Association Response's FT binding. Its storage stays with the
/// association owner; no frame buffer or secret is duplicated per operation.
///
/// Drive `RsnStaState<FtAkm>` / `RsnApState<FtAkm>` as usual. Use this owner
/// for PTK derivation, Message 2 binding and authenticated Message 3 key data.
/// Key installation, transmission and completion retain their existing tickets.
pub struct InitialAssociation<'a> {
    security: SecurityProfile<'a>,
    selected: SecurityProfile<'a>,
    addresses: wire::FtAddresses,
    md: &'a [u8],
    ft: &'a [u8],
    r0: PmkR0,
    r1: PmkR1,
}
impl<'a> InitialAssociation<'a> {
    pub fn new(
        key: InitialKey,
        target: Target<'a>,
        station: MacAddress,
        association_response: &'a [u8],
        selected: SecurityProfile<'a>,
    ) -> Result<Self, Error> {
        if selected.akm() != target.security.akm()
            || selected.sae_pwe() != target.security.sae_pwe()
            || selected.protects_management() != target.security.protects_management()
            || RsnElement::parse(selected.rsn())
                .map_err(|_| Error::UnsupportedSecurity)?
                .akm_suites()
                .len()
                != 1
            || key.akm() != target.security.akm()
            || key.sae_pwe() != target.security.sae_pwe()
            || station == target.bssid
            || station == [0; MAC_ADDRESS_LEN]
            || target.bssid == [0; MAC_ADDRESS_LEN]
            || oer_ieee80211_mac::management::is_group_address(station)
            || oer_ieee80211_mac::management::is_group_address(target.bssid)
        {
            return Err(Error::WrongKeyContext);
        }
        let elements = Elements::parse(association_response).map_err(wire::WireError::from)?;
        let md = elements
            .unique(wire::MOBILITY_DOMAIN_ELEMENT_ID)
            .map_err(wire::WireError::from)?
            .ok_or(Error::WrongKeyContext)?
            .encoded();
        let ft = elements
            .unique(wire::FAST_TRANSITION_ELEMENT_ID)
            .map_err(wire::WireError::from)?
            .ok_or(Error::WrongKeyContext)?
            .encoded();
        let parsed = wire::FastTransitionElement::parse(ft)?;
        if MobilityDomain::parse(md)? != target.domain
            || parsed.element_count() != 0
            || parsed.mic() != &[0; wire::FT_MIC_LEN]
            || parsed.anonce() != &[0; RSN_NONCE_LEN]
            || parsed.snonce() != &[0; RSN_NONCE_LEN]
            || parsed.subelements().iter().any(|value| {
                ![wire::subelement_id::R0KH, wire::subelement_id::R1KH].contains(&value.id)
            })
        {
            return Err(Error::WrongKeyContext);
        }
        let r0 = key.derive_r0(RootContext {
            ssid: target.ssid,
            mobility_domain: target.domain.id,
            r0kh: parsed.r0kh()?.ok_or(Error::WrongKeyContext)?,
            station,
        });
        let r1 = r0.derive_r1(parsed.r1kh()?.ok_or(Error::WrongKeyContext)?);
        Ok(Self {
            security: target.security,
            selected,
            addresses: wire::FtAddresses {
                station,
                access_point: target.bssid,
            },
            md,
            ft,
            r0,
            r1,
        })
    }
    pub const fn akm(&self) -> FtAkm {
        self.r0.akm()
    }
    pub fn derive_ptk(&self, context: PtkContext) -> Result<FtPtk, Error> {
        if context.authenticator_address != self.addresses.access_point
            || context.supplicant_address != self.addresses.station
        {
            return Err(Error::WrongPeer);
        }
        self.r1.derive_ptk(FtPtkContext {
            addresses: self.addresses,
            snonce: context.supplicant_nonce,
            anonce: context.authenticator_nonce,
        })
    }
    fn bound_elements<const N: usize>(&self, selected: bool) -> Result<Packet<N>, Error> {
        let mut packet = Packet::empty();
        let profile = if selected {
            self.selected
        } else {
            self.security
        };
        let rsn = RsnElement::parse(profile.rsn()).map_err(|_| Error::UnsupportedSecurity)?;
        let mut bytes = [0; MAX_ENCODED_ELEMENT_LEN];
        let length = if selected {
            rsn.encode_selected_akm(self.akm().suite_selector(), &[self.r1.name()], &mut bytes)
        } else {
            rsn.encode_with_pmkids(&[self.r1.name()], &mut bytes)
        }
        .map_err(|_| Error::CapacityExceeded)?;
        packet.push(&bytes[..length])?;
        packet.push(self.md)?;
        packet.push(self.ft)?;
        packet.push(profile.rsnxe())?;
        Ok(packet)
    }
    /// Build Message 2 from the selected association RSNE. Only its PMKID list
    /// changes; the response MDIE/FTIE are copied exactly and covered by the MIC.
    pub fn message2<const N: usize>(
        &self,
        replay: u64,
        ptk: &FtPtk,
    ) -> Result<RsnTxFrame<N>, Error> {
        self.check_ptk(ptk)?;
        let ies = self.bound_elements::<N>(true)?;
        Ok(RsnTxFrame::message2_with_key_data(
            self.akm(),
            self.addresses.access_point,
            replay,
            ptk.context().snonce,
            ies.bytes(),
        )?
        .authenticate_ft(ptk)?)
    }
    /// AP admission for Message 2, after verifying its EAPOL MIC. Its association
    /// owner supplies the exact selected Association Request security profile.
    pub fn validate_message2(&self, bytes: &[u8]) -> Result<(), Error> {
        let expected = self.bound_elements::<{ crate::RSN_KEY_DATA_CAPACITY }>(true)?;
        if bytes != expected.bytes() {
            return Err(Error::SecurityMismatch);
        }
        Ok(())
    }
    /// AP plaintext for Message 3. Existing KDE framing, padding and zeroizing
    /// ownership are shared with the ordinary four-way handshake.
    pub fn message3_key_data<const N: usize>(
        &self,
        groups: &TransitionGroupKeys,
        reassociation_deadline_tu: u32,
        key_lifetime_seconds: u32,
    ) -> Result<RsnPlainKeyData<N>, Error> {
        if self.security.protects_management() != groups.igtk.is_some() {
            return Err(Error::InvalidGroupKeys);
        }
        let mut ies = self.bound_elements::<N>(false)?;
        ies.push(
            &wire::TimeoutInterval::ReassociationDeadline {
                tu: reassociation_deadline_tu,
            }
            .encode(),
        )?;
        ies.push(
            &wire::TimeoutInterval::KeyLifetime {
                seconds: key_lifetime_seconds,
            }
            .encode(),
        )?;
        Ok(RsnPlainKeyData::build(
            ies.bytes(),
            &groups.gtk,
            groups.igtk.as_ref(),
        )?)
    }
    /// Verify FT binding in authenticated, unwrapped Message 3 before passing
    /// any group key to the existing key-install completion.
    pub fn parse_message3_key_data(&self, bytes: &[u8]) -> Result<InitialGroupKeys, Error> {
        let expected = self.bound_elements::<{ crate::RSN_KEY_DATA_CAPACITY }>(false)?;
        let rsn = Elements::parse(expected.bytes())
            .map_err(wire::WireError::from)?
            .unique(oer_ieee80211_mac::security::rsn::RSN_ELEMENT_ID)
            .map_err(wire::WireError::from)?
            .ok_or(Error::WrongKeyContext)?
            .encoded();
        let parsed = parse_ft_gtk_key_data(
            bytes,
            rsn,
            self.security.rsnxe(),
            FtKeyDataContext {
                md: self.md,
                ft: self.ft,
            },
            self.security.protects_management(),
        )?;
        Ok(InitialGroupKeys {
            groups: parsed.groups,
            reassociation_deadline_tu: parsed
                .reassociation_deadline_tu
                .ok_or(Error::WrongKeyContext)?,
            key_lifetime_seconds: parsed.key_lifetime_seconds.ok_or(Error::WrongKeyContext)?,
        })
    }
    fn check_ptk(&self, ptk: &FtPtk) -> Result<(), Error> {
        if ptk.akm() != self.akm() || ptk.context().addresses != self.addresses {
            return Err(Error::WrongKeyContext);
        }
        // Compare the key name too: another PMK-R1 for this peer is insufficient.
        if self.r1.derive_ptk(ptk.context())?.name() != ptk.name() {
            return Err(Error::WrongKeyContext);
        }
        Ok(())
    }
    /// Release the root for future transitions only after the initial STA key
    /// transaction completed. The association owner still completes Message 4 TX.
    pub fn into_station_root(self, state: &RsnStaState<FtAkm>) -> Result<PmkR0, Error> {
        if state.phase() != RsnStaPhase::Completed
            || state.akm() != self.akm()
            || *state.peer() != self.addresses.access_point
            || *state.local_address() != self.addresses.station
        {
            return Err(Error::WrongPhase);
        }
        Ok(self.r0)
    }
    pub fn into_access_point_root(self, state: &RsnApState<FtAkm>) -> Result<PmkR0, Error> {
        if state.phase() != RsnApPhase::Authorized
            || state.akm() != self.akm()
            || *state.peer() != self.addresses.station
            || *state.local_address() != self.addresses.access_point
        {
            return Err(Error::WrongPhase);
        }
        Ok(self.r0)
    }
}

pub struct InitialGroupKeys {
    pub groups: RsnGroupKeys,
    pub reassociation_deadline_tu: u32,
    pub key_lifetime_seconds: u32,
}
