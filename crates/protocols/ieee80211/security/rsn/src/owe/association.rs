//! OWE-specific association automata. Existing MLME supplies admitted
//! peer context and frame framing; the existing RSN owner supplies four-way
//! transitions. Entropy, TX, key installation and peer slots remain external.
use super::{exchange::Exchange, *};
use oer_ieee80211_mac::{
    management::{STATUS_SUCCESS, STATUS_UNSUPPORTED_FINITE_CYCLIC_GROUP, elements::Elements},
    owe::DhParameter,
    security::rsn::RsnElement,
    ssid::WifiSsid,
};

/// Explicit preference/allow list, without duplicate groups or implicit
/// downgrade. RFC 8110 requires group 19 support on both roles.
#[derive(Clone, Copy)]
pub struct GroupPolicy<'a> {
    groups: &'a [Group],
}
impl<'a> GroupPolicy<'a> {
    pub fn new(groups: &'a [Group]) -> Result<Self, Error> {
        if !groups.contains(&Group::P256)
            || groups
                .iter()
                .enumerate()
                .any(|(index, group)| groups[..index].contains(group))
        {
            return Err(Error::InvalidConfiguration);
        }
        Ok(Self { groups })
    }
    fn first(self) -> Group {
        self.groups[0]
    }
    fn contains(self, group: Group) -> bool {
        self.groups.contains(&group)
    }
    fn next(self, group: Group) -> Option<Group> {
        self.groups
            .get(self.groups.iter().position(|value| *value == group)? + 1)
            .copied()
    }
}

struct Packet<const N: usize> {
    bytes: [u8; N],
    length: usize,
}
impl<const N: usize> Packet<N> {
    fn new(
        profile: SecurityProfile<'_>,
        pmkid: Option<[u8; RSN_PMKID_LEN]>,
        dh: Option<DhParameter<'_>>,
    ) -> Result<Self, Error> {
        let mut value = Self {
            bytes: [0; N],
            length: 0,
        };
        let rsn = RsnElement::parse(profile.rsn()).map_err(|_| Error::UnsupportedSecurity)?;
        // MAC owns the RSN re-encoding, including optional count/cipher fields.
        value.length = rsn
            .encode_with_pmkids(pmkid.as_slice(), &mut value.bytes)
            .map_err(|_| Error::CapacityExceeded)?;
        let end = value
            .length
            .checked_add(profile.rsnxe().len())
            .ok_or(Error::CapacityExceeded)?;
        value
            .bytes
            .get_mut(value.length..end)
            .ok_or(Error::CapacityExceeded)?
            .copy_from_slice(profile.rsnxe());
        value.length = end;
        if let Some(dh) = dh {
            value.length += dh
                .encode(&mut value.bytes[value.length..])
                .map_err(Error::Wire)?;
        }
        Ok(value)
    }
    fn request(profile: SecurityProfile<'_>, dh: DhParameter<'_>) -> Result<Self, Error> {
        // Preserve the exact selected RSNE/RSNXE for Message 2 binding.
        let mut value = Self {
            bytes: [0; N],
            length: 0,
        };
        value.length = profile.encode(&mut value.bytes)?;
        value.length += dh
            .encode(&mut value.bytes[value.length..])
            .map_err(Error::Wire)?;
        Ok(value)
    }
    fn bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}

/// Immutable association identity, BSS security and caller's timing policy.
/// Open System Authentication and SSID/address admission precede this owner.
#[derive(Clone, Copy)]
pub struct AssociationContext<'a> {
    pub id: ExchangeId,
    pub ssid: WifiSsid,
    pub advertisement: SecurityProfile<'a>,
    pub retry: RetryPolicy,
}

mod station;
pub use station::{Station, StationPhase};
mod access_point;
pub use access_point::{AccessPoint, AccessPointPhase};
