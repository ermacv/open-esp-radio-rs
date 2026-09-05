//! Portable IEEE 802.11 priority values and Ethernet traffic classification.
//!
//! UP-to-AC and DSCP mappings describe traffic intent. The WMM vendor element
//! parser lives in `extensions::wmm`; the transmitter still owns admission and
//! queue selection using these values.

/// Standard WMM access-category identifier carried in ACI bits 5..6.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum WmmAccessCategory {
    #[default]
    BestEffort = 0,
    Background = 1,
    Video = 2,
    Voice = 3,
}

impl WmmAccessCategory {
    pub const fn from_aci(aci: u8) -> Option<Self> {
        match aci {
            0 => Some(Self::BestEffort),
            1 => Some(Self::Background),
            2 => Some(Self::Video),
            3 => Some(Self::Voice),
            _ => None,
        }
    }

    /// Next lower access category used when the requested AC requires an
    /// admission which the station does not own.
    pub const fn downgrade(self) -> Option<Self> {
        match self {
            Self::Voice => Some(Self::Video),
            Self::Video => Some(Self::BestEffort),
            Self::BestEffort => Some(Self::Background),
            Self::Background => None,
        }
    }

    /// Canonical user priority used when ACM policy downgrades into this AC.
    pub const fn canonical_user_priority(self) -> WmmUserPriority {
        match self {
            Self::Voice => WmmUserPriority::UP7,
            // The recovered station classifier enters VI at UP5.
            Self::Video => WmmUserPriority::UP5,
            Self::BestEffort => WmmUserPriority::UP0,
            Self::Background => WmmUserPriority::UP1,
        }
    }
}

/// Valid IEEE 802.1D / IEEE 802.11 user priority and QoS TID.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WmmUserPriority(u8);

impl WmmUserPriority {
    pub const UP0: Self = Self(0);
    pub const UP1: Self = Self(1);
    pub const UP2: Self = Self(2);
    pub const UP3: Self = Self(3);
    pub const UP4: Self = Self(4);
    pub const UP5: Self = Self(5);
    pub const UP6: Self = Self(6);
    pub const UP7: Self = Self(7);

    pub const fn new(value: u8) -> Option<Self> {
        if value <= 7 { Some(Self(value)) } else { None }
    }

    pub const fn value(self) -> u8 {
        self.0
    }

    /// Standard UP-to-AC mapping used by WMM and the existing data encoder.
    pub const fn access_category(self) -> WmmAccessCategory {
        match self.0 {
            0 | 3 => WmmAccessCategory::BestEffort,
            1 | 2 => WmmAccessCategory::Background,
            4 | 5 => WmmAccessCategory::Video,
            6 | 7 => WmmAccessCategory::Voice,
            _ => unreachable!(),
        }
    }
}

/// Valid six-bit IPv4/IPv6 Differentiated Services Code Point.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Dscp(u8);

impl Dscp {
    pub const fn new(value: u8) -> Option<Self> {
        if value <= 0x3f {
            Some(Self(value))
        } else {
            None
        }
    }

    pub const fn value(self) -> u8 {
        self.0
    }

    /// Widely deployed default mapping based on the three DSCP MSBs.
    pub const fn default_user_priority(self) -> WmmUserPriority {
        WmmUserPriority(self.0 >> 3)
    }

    /// RFC 8325 station-safe DSCP mapping.
    ///
    /// Only the standardized service-class codepoints receive elevated
    /// priority. Unknown codepoints and endpoint-sourced CS6/CS7 are bleached
    /// to UP0, preventing an application marking from acquiring the reserved
    /// network-control priority. RFC 8622's Lower Effort codepoint joins CS1
    /// in the background category.
    pub const fn station_user_priority(self) -> WmmUserPriority {
        match self.0 {
            // Lower Effort and CS1.
            1 | 8 => WmmUserPriority::UP1,
            // AF2x low-latency data.
            18 | 20 | 22 => WmmUserPriority::UP3,
            // AF3x, CS3, AF4x and CS4 video classes.
            24 | 26 | 28 | 30 | 32 | 34 | 36 | 38 => WmmUserPriority::UP4,
            // CS5 signaling.
            40 => WmmUserPriority::UP5,
            // VOICE-ADMIT and EF telephony.
            44 | 46 => WmmUserPriority::UP6,
            // DF, OAM, AF1x, CS6/CS7 and unassigned values fail closed to BE.
            _ => WmmUserPriority::UP0,
        }
    }
}

/// Header field which selected one network frame's WMM class.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WmmClassificationSource {
    /// Untagged non-IP traffic or a truncated network header.
    #[default]
    Unmarked,
    /// The outer IEEE 802.1Q/802.1ad priority-code-point field.
    Ieee8021d(WmmUserPriority),
    /// IPv4 or IPv6 DSCP mapped with the station-safe policy.
    Dscp(Dscp),
}

/// Complete portable WMM classification for one network frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WmmTrafficClass {
    pub user_priority: WmmUserPriority,
    pub access_category: WmmAccessCategory,
    pub source: WmmClassificationSource,
}

impl WmmTrafficClass {
    pub const BEST_EFFORT: Self = Self {
        user_priority: WmmUserPriority::UP0,
        access_category: WmmAccessCategory::BestEffort,
        source: WmmClassificationSource::Unmarked,
    };

    pub const fn from_user_priority(
        user_priority: WmmUserPriority,
        source: WmmClassificationSource,
    ) -> Self {
        Self {
            user_priority,
            access_category: user_priority.access_category(),
            source,
        }
    }
}

const ETHERNET_HEADER_LEN: usize = 14;
const ETHER_TYPE_IPV4: u16 = 0x0800;
const ETHER_TYPE_VLAN: u16 = 0x8100;
const ETHER_TYPE_PROVIDER_VLAN: u16 = 0x88a8;
const ETHER_TYPE_IPV6: u16 = 0x86dd;

/// Classify an Ethernet frame from an outer 802.1D PCP or an IP DSCP.
///
/// An explicit VLAN PCP takes precedence over an encapsulated DSCP. Untagged
/// non-IP and truncated headers remain best effort. The function never
/// invents priority from transport ports or payload bytes.
pub fn classify_ethernet_wmm(ethernet: &[u8]) -> WmmTrafficClass {
    if ethernet.len() < ETHERNET_HEADER_LEN {
        return WmmTrafficClass::BEST_EFFORT;
    }

    let ether_type = u16::from_be_bytes([ethernet[12], ethernet[13]]);
    let payload_offset = ETHERNET_HEADER_LEN;
    if matches!(ether_type, ETHER_TYPE_VLAN | ETHER_TYPE_PROVIDER_VLAN) {
        let Some(tag) = ethernet.get(payload_offset..payload_offset + 4) else {
            return WmmTrafficClass::BEST_EFFORT;
        };
        let tci = u16::from_be_bytes([tag[0], tag[1]]);
        let user_priority = WmmUserPriority((tci >> 13) as u8);
        // The explicit outer PCP is authoritative even when it is zero.
        return WmmTrafficClass::from_user_priority(
            user_priority,
            WmmClassificationSource::Ieee8021d(user_priority),
        );
    }

    let dscp = match ether_type {
        ETHER_TYPE_IPV4 => {
            let Some(header) = ethernet.get(payload_offset..payload_offset + 2) else {
                return WmmTrafficClass::BEST_EFFORT;
            };
            if header[0] >> 4 != 4 {
                return WmmTrafficClass::BEST_EFFORT;
            }
            Dscp(header[1] >> 2)
        }
        ETHER_TYPE_IPV6 => {
            let Some(header) = ethernet.get(payload_offset..payload_offset + 2) else {
                return WmmTrafficClass::BEST_EFFORT;
            };
            if header[0] >> 4 != 6 {
                return WmmTrafficClass::BEST_EFFORT;
            }
            Dscp(((header[0] & 0x0f) << 2) | (header[1] >> 6))
        }
        _ => return WmmTrafficClass::BEST_EFFORT,
    };
    let user_priority = dscp.station_user_priority();
    WmmTrafficClass::from_user_priority(user_priority, WmmClassificationSource::Dscp(dscp))
}

#[cfg(test)]
mod tests;
