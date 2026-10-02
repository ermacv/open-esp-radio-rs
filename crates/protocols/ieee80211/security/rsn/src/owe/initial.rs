//! Exact association-to-four-way binding, shared by the OWE role procedures.
use super::*;
use crate::{
    PtkContext, RSN_KEY_DATA_CAPACITY,
    frames::{RsnGroupKeys, RsnPlainKeyData, RsnTxFrame, parse_gtk_key_data},
    state::{RsnApPhase, RsnApState, RsnStaPhase, RsnStaState},
};
use oer_ieee80211_mac::{
    management::elements::Elements, security::rsn::RsnElement, ssid::WifiSsid,
};

/// An established DH/cache result and the exact association security fields.
/// The shared RSN automata own replay, nonce, duplicate and key-install tickets.
/// This owner supplies OWE key operations and validates security/key data before
/// the corresponding RSN completion. It performs no transmission or key IO.
// CAPABILITY: wifi-security-opportunistic-wireless-encryption-owe
pub struct InitialAssociation<'a> {
    key: OwePmk,
    advertisement: SecurityProfile<'a>,
    selected: SecurityProfile<'a>,
    scope: CacheScope,
}
impl<'a> InitialAssociation<'a> {
    pub fn new(
        key: OwePmk,
        ssid: WifiSsid,
        advertisement: SecurityProfile<'a>,
        selected: SecurityProfile<'a>,
    ) -> Result<Self, Error> {
        let management_protection = advertisement.negotiate(selected)?;
        let scope = CacheScope {
            addresses: key.addresses(),
            ssid,
            management_protection,
        };
        Ok(Self {
            key,
            advertisement,
            selected,
            scope,
        })
    }
    pub const fn group(&self) -> Group {
        self.key.group()
    }
    pub const fn addresses(&self) -> Addresses {
        self.key.addresses()
    }
    pub const fn management_protection(&self) -> bool {
        self.scope.management_protection
    }
    pub const fn scope(&self) -> CacheScope {
        self.scope
    }
    /// Resume only the exact offered cache scope and the single matching
    /// PMKID echoed by the AP. RFC 8110 4.5 requires ignoring a DH IE in this
    /// case; only its outer IE framing has been checked by `Elements`.
    pub fn resume(
        cached: CachedPmk,
        scope: CacheScope,
        advertisement: SecurityProfile<'a>,
        selected: SecurityProfile<'a>,
        response: Elements<'_>,
        now_us: u64,
    ) -> Result<Self, Error> {
        cached.validate_at(now_us)?;
        let response = SecurityProfile::from_elements(response)?;
        let parsed = RsnElement::parse(response.rsn()).map_err(|_| Error::UnsupportedSecurity)?;
        let offered = RsnElement::parse(selected.rsn()).map_err(|_| Error::UnsupportedSecurity)?;
        if cached.scope() != scope
            || advertisement.negotiate(selected)? != scope.management_protection
            || response.negotiate(selected)? != scope.management_protection
            || parsed.pmkid_count() != Some(1)
            || parsed.pmkids().next() != Some(cached.pmkid())
        {
            return Err(Error::WrongContext);
        }
        if offered.pmkid_count() != Some(1) || offered.pmkids().next() != Some(cached.pmkid()) {
            return Err(Error::WrongContext);
        }
        Self::new(cached.into_key(), scope.ssid, advertisement, selected)
    }
    pub const fn pmkid(&self) -> [u8; RSN_PMKID_LEN] {
        self.key.pmkid()
    }
    pub fn derive_ptk(&self, context: PtkContext) -> Result<OwePtk, Error> {
        self.key.derive_ptk(context)
    }
    fn check_ptk(&self, ptk: &OwePtk) -> Result<(), Error> {
        let expected = self.key.derive_ptk(ptk.context())?;
        if !ptk.same_key(&expected) {
            return Err(Error::WrongContext);
        }
        Ok(())
    }
    pub fn message2<const N: usize>(
        &self,
        replay: u64,
        ptk: &OwePtk,
    ) -> Result<RsnTxFrame<N>, Error> {
        self.check_ptk(ptk)?;
        let mut bytes = [0; RSN_KEY_DATA_CAPACITY];
        let length = self.selected.encode(&mut bytes)?;
        Ok(RsnTxFrame::message2_with_key_data(
            self.group(),
            self.addresses().access_point,
            replay,
            ptk.context().supplicant_nonce,
            &bytes[..length],
        )?
        .authenticate_owe(ptk)?)
    }
    /// AP admission after EAPOL MIC verification. Compare the exact security
    /// fields selected in Association Request, without reconstructing policy.
    pub fn validate_message2_key_data(&self, bytes: &[u8]) -> Result<(), Error> {
        let mut expected = [0; RSN_KEY_DATA_CAPACITY];
        let length = self.selected.encode(&mut expected)?;
        if bytes != &expected[..length] {
            return Err(Error::SecurityMismatch);
        }
        Ok(())
    }
    pub fn message3_key_data<const N: usize>(
        &self,
        groups: &RsnGroupKeys,
    ) -> Result<RsnPlainKeyData<N>, Error> {
        if groups.igtk.is_some() != self.management_protection() {
            return Err(Error::SecurityMismatch);
        }
        let mut bytes = [0; RSN_KEY_DATA_CAPACITY];
        let length = self.advertisement.encode(&mut bytes)?;
        Ok(RsnPlainKeyData::build(
            &bytes[..length],
            &groups.gtk,
            groups.igtk.as_ref(),
        )?)
    }
    /// STA admission after MIC verification and unwrapping, before key install.
    pub fn parse_message3_key_data(&self, bytes: &[u8]) -> Result<RsnGroupKeys, Error> {
        Ok(parse_gtk_key_data(
            bytes,
            self.advertisement.rsn(),
            self.advertisement.rsnxe(),
            self.management_protection(),
        )?)
    }
    fn cache(self, now_us: u64, expires_at_us: u64) -> Result<CachedPmk, Error> {
        CachedPmk::new(self.scope, self.key, now_us, expires_at_us)
    }
    /// Release a cache entry only after the shared STA key-install transaction
    /// succeeded. Message 4 transmission retains its existing owner/ticket.
    pub fn cache_station(
        self,
        state: &RsnStaState<Group>,
        now_us: u64,
        expires_at_us: u64,
    ) -> Result<CachedPmk, Error> {
        if state.phase() != RsnStaPhase::Completed
            || state.akm() != self.group()
            || *state.peer() != self.addresses().access_point
            || *state.local_address() != self.addresses().station
        {
            return Err(Error::WrongPhase);
        }
        self.cache(now_us, expires_at_us)
    }
    pub fn cache_access_point(
        self,
        state: &RsnApState<Group>,
        now_us: u64,
        expires_at_us: u64,
    ) -> Result<CachedPmk, Error> {
        if state.phase() != RsnApPhase::Authorized
            || state.akm() != self.group()
            || *state.peer() != self.addresses().station
            || *state.local_address() != self.addresses().access_point
        {
            return Err(Error::WrongPhase);
        }
        self.cache(now_us, expires_at_us)
    }
}
