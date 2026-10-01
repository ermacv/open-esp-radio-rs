use super::{BodyReader, Elements, MAX_ELEMENT_BODY_LEN, WireError, element_id};
use core::mem::size_of;
const RM_CAPABILITIES_LEN: usize = 5;
const BSS_LOAD_BODY_LEN: usize = 2 * size_of::<u16>() + size_of::<u8>();
// Bit positions in IEEE 802.11-2012 RM Enabled / Extended Capabilities IEs.
#[repr(u8)]
enum RmCapability {
    LinkMeasurement = 0,
    NeighborReport = 1,
    BeaconPassive = 4,
    BeaconActive = 5,
    BeaconTable = 6,
    ChannelLoad = 9,
    NoiseHistogram = 10,
}
#[repr(u16)]
enum ExtendedCapability {
    EventReporting = 7,
    DiagnosticReporting = 8,
    TrafficFiltering = 16,
    WnmSleep = 17,
    BssTransition = 19,
    DirectedMulticast = 26,
}

/// Entire advertised RM and Extended Capabilities payloads. Unknown bits are
/// retained; advertised support is separate from local backend support.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PeerCapabilities {
    rm: Option<[u8; RM_CAPABILITIES_LEN]>,
    extended: [u8; MAX_ELEMENT_BODY_LEN],
    extended_len: u8,
}
impl PeerCapabilities {
    pub fn parse(elements: Elements<'_>) -> Result<Self, WireError> {
        let rm = elements
            .unique(element_id::RM_ENABLED_CAPABILITIES)?
            .map(|value| {
                value.try_into().map_err(|_| {
                    WireError::InvalidElementLength(element_id::RM_ENABLED_CAPABILITIES)
                })
            })
            .transpose()?;
        let extended = elements
            .unique(element_id::EXTENDED_CAPABILITIES)?
            .unwrap_or(&[]);
        let mut stored = [0; MAX_ELEMENT_BODY_LEN];
        stored[..extended.len()].copy_from_slice(extended);
        Ok(Self {
            rm,
            extended: stored,
            extended_len: extended.len() as u8,
        })
    }
    pub const fn radio_measurement(&self) -> Option<&[u8; RM_CAPABILITIES_LEN]> {
        self.rm.as_ref()
    }
    pub fn extended(&self) -> &[u8] {
        &self.extended[..usize::from(self.extended_len)]
    }
    pub fn rm_bit(&self, bit: u8) -> bool {
        self.rm.as_ref().is_some_and(|rm| {
            usize::from(bit) < RM_CAPABILITIES_LEN * u8::BITS as usize
                && rm[usize::from(bit / u8::BITS as u8)] & (1 << (bit % u8::BITS as u8)) != 0
        })
    }
    pub fn extended_bit(&self, bit: u16) -> bool {
        self.extended()
            .get(usize::from(bit / u8::BITS as u16))
            .is_some_and(|byte| byte & (1 << (bit % u8::BITS as u16)) != 0)
    }
    pub fn neighbor_report(&self) -> bool {
        self.rm_bit(RmCapability::NeighborReport as u8)
    }
    pub fn link_measurement(&self) -> bool {
        self.rm_bit(RmCapability::LinkMeasurement as u8)
    }
    pub fn bss_transition(&self) -> bool {
        self.extended_bit(ExtendedCapability::BssTransition as u16)
    }
    pub fn beacon_passive(&self) -> bool {
        self.rm_bit(RmCapability::BeaconPassive as u8)
    }
    pub fn beacon_active(&self) -> bool {
        self.rm_bit(RmCapability::BeaconActive as u8)
    }
    pub fn beacon_table(&self) -> bool {
        self.rm_bit(RmCapability::BeaconTable as u8)
    }
    pub fn channel_load(&self) -> bool {
        self.rm_bit(RmCapability::ChannelLoad as u8)
    }
    pub fn noise_histogram(&self) -> bool {
        self.rm_bit(RmCapability::NoiseHistogram as u8)
    }
    pub fn wnm_sleep(&self) -> bool {
        self.extended_bit(ExtendedCapability::WnmSleep as u16)
    }
    pub fn event_reporting(&self) -> bool {
        self.extended_bit(ExtendedCapability::EventReporting as u16)
    }
    pub fn diagnostic_reporting(&self) -> bool {
        self.extended_bit(ExtendedCapability::DiagnosticReporting as u16)
    }
    pub fn traffic_filtering(&self) -> bool {
        self.extended_bit(ExtendedCapability::TrafficFiltering as u16)
    }
    pub fn directed_multicast(&self) -> bool {
        self.extended_bit(ExtendedCapability::DirectedMulticast as u16)
    }
}

/// BSS Load IE values, including admission capacity in units of 32 us/s.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BssLoad {
    pub station_count: u16,
    pub channel_utilization: u8,
    pub available_admission_capacity: u16,
}
impl BssLoad {
    pub fn parse(elements: Elements<'_>) -> Result<Option<Self>, WireError> {
        let Some(value) = elements.unique(element_id::BSS_LOAD)? else {
            return Ok(None);
        };
        if value.len() != BSS_LOAD_BODY_LEN {
            return Err(WireError::InvalidElementLength(element_id::BSS_LOAD));
        }
        let mut fields = BodyReader::new(value);
        Ok(Some(Self {
            station_count: u16::from_le_bytes(fields.array()?),
            channel_utilization: fields.u8()?,
            available_admission_capacity: u16::from_le_bytes(fields.array()?),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn event_diagnostic_and_tfs_advertisement_bits_are_independent() {
        for (octet, mask) in [(0, 0x80), (1, 1), (2, 1)] {
            let mut wire = [127, 3, 0, 0, 0];
            wire[2 + octet] = mask;
            let caps = PeerCapabilities::parse(Elements::parse(&wire).unwrap()).unwrap();
            assert_eq!(caps.event_reporting(), octet == 0);
            assert_eq!(caps.diagnostic_reporting(), octet == 1);
            assert_eq!(caps.traffic_filtering(), octet == 2);
            assert!(!caps.wnm_sleep());
            assert!(!caps.bss_transition());
        }
        let caps = PeerCapabilities::parse(Elements::EMPTY).unwrap();
        assert!(!caps.event_reporting());
        assert!(!caps.diagnostic_reporting());
        assert!(!caps.traffic_filtering());
    }
}
