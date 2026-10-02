//! Reciprocal Enhanced Open advertisements and directed discovery.
use super::*;
use oer_ieee80211_mac::{
    management::{MAC_ADDRESS_LEN, elements::Elements, is_group_address},
    owe::Transition,
    security::rsn::RSN_ELEMENT_ID,
    ssid::WifiSsid,
};

/// Actual BSS identity, distinct from a hidden/empty beacon SSID.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BssIdentity {
    bssid: MacAddress,
    ssid: WifiSsid,
}
impl BssIdentity {
    pub fn new(bssid: MacAddress, ssid: WifiSsid) -> Result<Self, Error> {
        if bssid == [0; MAC_ADDRESS_LEN] || is_group_address(bssid) {
            return Err(Error::WrongContext);
        }
        Ok(Self { bssid, ssid })
    }
    pub const fn bssid(self) -> MacAddress {
        self.bssid
    }
    pub const fn ssid(self) -> WifiSsid {
        self.ssid
    }
}

/// AP configuration of the two BSS identities. Beacon construction and VIF
/// ownership remain outside this protocol; both directions use one binding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TransitionPair {
    open: BssIdentity,
    owe: BssIdentity,
}
impl TransitionPair {
    pub fn new(open: BssIdentity, owe: BssIdentity) -> Result<Self, Error> {
        if open.bssid == owe.bssid {
            return Err(Error::WrongContext);
        }
        Ok(Self { open, owe })
    }
    /// The open BSS advertises the OWE counterpart's actual SSID/BSSID.
    pub fn open_advertisement(&self, channel_hint: Option<(u8, u8)>) -> Transition<'_> {
        Transition {
            bssid: self.owe.bssid,
            ssid: self.owe.ssid.as_bytes(),
            channel_hint,
        }
    }
    /// The OWE BSS advertises the open counterpart, including when its own
    /// ordinary SSID IE is hidden. The identities are never swapped by signal.
    pub fn owe_advertisement(&self, channel_hint: Option<(u8, u8)>) -> Transition<'_> {
        Transition {
            bssid: self.open.bssid,
            ssid: self.open.ssid.as_bytes(),
            channel_hint,
        }
    }
}

/// A directed probe/search target learned from the open BSS. This hint alone
/// never authorizes an OWE association or an automatic fallback to open mode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TransitionTarget {
    pair: TransitionPair,
    channel_hint: Option<(u8, u8)>,
}
impl TransitionTarget {
    pub fn discover(
        open: BssIdentity,
        privacy: bool,
        elements: Elements<'_>,
    ) -> Result<Option<Self>, Error> {
        if privacy || elements.unique(RSN_ELEMENT_ID)?.is_some() {
            return Err(Error::SecurityMismatch);
        }
        let Some(counterpart) = Transition::from_elements(elements).map_err(Error::Wire)? else {
            return Ok(None);
        };
        let ssid = WifiSsid::new(counterpart.ssid).map_err(|_| Error::WrongContext)?;
        let owe = BssIdentity::new(counterpart.bssid, ssid)?;
        Ok(Some(Self {
            pair: TransitionPair::new(open, owe)?,
            channel_hint: counterpart.channel_hint,
        }))
    }
    pub const fn identity(self) -> BssIdentity {
        self.pair.owe
    }
    pub const fn channel_hint(self) -> Option<(u8, u8)> {
        self.channel_hint
    }
    /// Validate an observed counterpart's identity, OWE security and reciprocal
    /// transition binding. A hidden SSID is explicit (`None`), not guessed.
    /// Regulatory/channel admission and scan age stay with the scan owner.
    pub fn confirm<'a>(
        self,
        bssid: MacAddress,
        observed_ssid: Option<WifiSsid>,
        privacy: bool,
        elements: Elements<'a>,
    ) -> Result<SecurityProfile<'a>, Error> {
        if bssid != self.pair.owe.bssid
            || observed_ssid.is_some_and(|ssid| ssid != self.pair.owe.ssid)
        {
            return Err(Error::WrongContext);
        }
        if !privacy {
            return Err(Error::SecurityMismatch);
        }
        let counterpart = Transition::from_elements(elements)
            .map_err(Error::Wire)?
            .ok_or(Error::SecurityMismatch)?;
        if counterpart.bssid != self.pair.open.bssid
            || counterpart.ssid != self.pair.open.ssid.as_bytes()
        {
            return Err(Error::WrongContext);
        }
        SecurityProfile::from_elements(elements)
    }
}
