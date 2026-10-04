//! Settings, keys and lifecycle of the lower-MAC port.

use oer_ieee80211_mac::{channel::Channel, sequence::SequenceNumber};

/// A 48-bit IEEE MAC address in transmission order.
pub type MacAddress = [u8; 6];

/// Index of one virtual interface of the backend, below
/// [`LowerMacCapabilities::vifs`](crate::LowerMacCapabilities::vifs).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct VifId(pub u8);

/// The role of a virtual interface, which decides the frames its address
/// filter admits and the TSF it keeps.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum VifRole {
    /// A non-AP station: its BSSID is the access point it joined.
    Station,
    /// An access point: its BSSID is its own address.
    AccessPoint,
}

/// A set of interface roles.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct VifRoleSet(u8);

impl VifRoleSet {
    pub const NONE: Self = Self(0);
    pub const STATION: Self = Self(1 << 0);
    pub const ACCESS_POINT: Self = Self(1 << 1);

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn contains(self, role: VifRole) -> bool {
        let bit = match role {
            VifRole::Station => Self::STATION.0,
            VifRole::AccessPoint => Self::ACCESS_POINT.0,
        };
        self.0 & bit != 0
    }
}

/// Which received frames the backend delivers for one interface.
///
/// The filter is a set of admission rules; a frame admitted by any rule of
/// any interface is delivered, and a frame no rule admits is not, whatever
/// superset the hardware passes. Frames addressed to the interface are
/// acknowledged by the hardware whatever the filter says about other
/// frames. The rules of [`Self::BSS_MEMBER`] belong to a BSS: a station
/// without a BSSID can request only [`Self::OTHER_BSS_MANAGEMENT`].
/// Receiving every frame regardless of address is
/// [`LowerMacMonitor`](crate::LowerMacMonitor), not a filter rule.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct ReceiveFilter(u8);

impl ReceiveFilter {
    /// Nothing.
    pub const NONE: Self = Self(0);
    /// Frames whose receiver address is the interface's address.
    pub const OWN_UNICAST: Self = Self(1 << 0);
    /// Group-addressed frames of the interface's BSS.
    pub const OWN_BSS_GROUP: Self = Self(1 << 1);
    /// Beacons of the interface's BSS.
    pub const OWN_BSS_BEACONS: Self = Self(1 << 2);
    /// Beacons and Probe Responses of every BSS, as a scan needs.
    pub const OTHER_BSS_MANAGEMENT: Self = Self(1 << 3);
    /// Control frames addressed to the interface (BlockAckReq, PS-Poll).
    pub const OWN_CONTROL: Self = Self(1 << 4);

    /// What a station or access point needs for its own BSS.
    pub const BSS_MEMBER: Self = Self(
        Self::OWN_UNICAST.0 | Self::OWN_BSS_GROUP.0 | Self::OWN_BSS_BEACONS.0 | Self::OWN_CONTROL.0,
    );

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// Configuration of one virtual interface.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct VifConfig {
    pub address: MacAddress,
    pub role: VifRole,
    /// The BSS the interface belongs to; `None` for a station that has not
    /// joined one.
    pub bssid: Option<MacAddress>,
    /// At most the role's
    /// [`LowerMacCapabilities::receive_filters`](crate::LowerMacCapabilities::receive_filters).
    pub receive: ReceiveFilter,
}

/// Frame Control type of management frames.
const TYPE_MANAGEMENT: u8 = 0;
/// Frame Control type of control frames.
const TYPE_CONTROL: u8 = 1;
/// Frame Control type of data frames.
const TYPE_DATA: u8 = 2;
const SUBTYPE_PROBE_RESPONSE: u8 = 5;
const SUBTYPE_BEACON: u8 = 8;

impl VifConfig {
    /// The BSS the interface belongs to: its BSSID, or an access point's
    /// own address.
    pub const fn bss(&self) -> Option<MacAddress> {
        match (self.role, self.bssid) {
            (_, Some(bssid)) => Some(bssid),
            (VifRole::AccessPoint, None) => Some(self.address),
            (VifRole::Station, None) => None,
        }
    }

    /// Whether the interface's receive filter admits `frame`, an MPDU
    /// without FCS. A backend whose hardware passes a superset of the
    /// requested rules narrows its reception with this.
    pub fn admits(&self, frame: &[u8]) -> bool {
        let filter = self.receive;
        let Some(address1) = address(frame, 4) else {
            return false;
        };
        let frame_type = (frame[0] >> 2) & 0b11;
        let subtype = frame[0] >> 4;
        if frame_type == TYPE_CONTROL {
            return filter.contains(ReceiveFilter::OWN_CONTROL) && address1 == self.address;
        }
        let management = frame_type == TYPE_MANAGEMENT;
        if management
            && matches!(subtype, SUBTYPE_BEACON | SUBTYPE_PROBE_RESPONSE)
            && filter.contains(ReceiveFilter::OTHER_BSS_MANAGEMENT)
        {
            return true;
        }
        if filter.contains(ReceiveFilter::OWN_UNICAST) && address1 == self.address {
            return true;
        }
        let Some(bss) = self.bss() else {
            return false;
        };
        let in_bss = frame_bssid(frame).is_some_and(|bssid| bssid == bss);
        if management && subtype == SUBTYPE_BEACON {
            return in_bss && filter.contains(ReceiveFilter::OWN_BSS_BEACONS);
        }
        address1[0] & 1 != 0 && in_bss && filter.contains(ReceiveFilter::OWN_BSS_GROUP)
    }
}

fn address(frame: &[u8], offset: usize) -> Option<MacAddress> {
    frame.get(offset..offset + 6)?.try_into().ok()
}

/// The BSSID a management or data frame carries.
fn frame_bssid(frame: &[u8]) -> Option<MacAddress> {
    match (frame[0] >> 2) & 0b11 {
        TYPE_MANAGEMENT => address(frame, 16),
        TYPE_DATA => match frame.get(1)? & 0b11 {
            0b00 => address(frame, 16),
            // From the DS: Address 2 is the BSSID.
            0b10 => address(frame, 10),
            // To the DS: Address 1 is the BSSID.
            0b01 => address(frame, 4),
            _ => None,
        },
        _ => None,
    }
}

/// Handle of one installed key, chosen by the backend.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct KeyHandle(pub u8);

/// The cipher of an installed key.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Cipher {
    /// CCMP-128, a 16-byte temporal key.
    Ccmp128,
}

/// Which frames a key protects.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum KeyScope {
    /// Individually addressed frames exchanged with `peer`.
    Pairwise { peer: MacAddress },
    /// Group-addressed frames under this key identifier.
    Group { key_id: u8 },
}

/// A key to install.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KeyInstall<'a> {
    pub vif: VifId,
    pub cipher: Cipher,
    pub scope: KeyScope,
    /// The temporal key; its length must match the cipher.
    pub key: &'a [u8],
}

/// A receive Block Ack agreement whose A-MPDUs the hardware acknowledges
/// with a BlockAck.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RxBlockAckAgreement {
    pub vif: VifId,
    /// The originator.
    pub peer: MacAddress,
    pub tid: u8,
    pub start_sequence: SequenceNumber,
    /// The buffer size in MPDUs.
    pub window: u16,
}

/// A setting the backend applies outside transmission attempts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LowerMacSetting {
    /// Tune to a channel; every interface shares it.
    Channel(Channel),
    /// Configure an interface, or remove it (`None`).
    Vif {
        vif: VifId,
        config: Option<VifConfig>,
    },
    /// Remove an installed key.
    RemoveKey(KeyHandle),
    /// Start acknowledging A-MPDUs of an agreement with BlockAcks.
    AddRxBlockAck(RxBlockAckAgreement),
    /// End the agreement of `peer` and `tid`.
    RemoveRxBlockAck {
        vif: VifId,
        peer: MacAddress,
        tid: u8,
    },
    /// Open or close the transmit gate of every queue, as the station's own
    /// doze or a coexistence slice needs. While the gate is closed the
    /// backend admits attempts and holds them unpublished; opening it
    /// publishes them. A backend may refuse to close the gate while an
    /// attempt is already published (`SettingError::Busy`), because the
    /// hardware could then end that attempt with a timeout
    /// ([`TxStatus::Aborted`](crate::TxStatus::Aborted)). Per-peer
    /// power-save buffering is software policy above the port.
    TxGate { open: bool },
    /// The EDCA parameters of the four access categories, as the access
    /// point's WMM Parameter Element advertises them: the AIFSN and TXOP
    /// limit each queue's attempts contend with, and `CWmin`/`CWmax` for a
    /// backend that draws its own backoff. The set applies whole: a record
    /// outside the backend's limits is `SettingError::Unsupported` and
    /// changes nothing. A backend starts with its own defaults.
    Edca(oer_ieee80211_mac::extensions::wmm::WmmParameterSet),
    /// The coexistence priority the hardware receives the access point's
    /// beacons with.
    RxBeaconPriority(RxBeaconPriority),
    /// The BSS color of an interface's HE BSS (0-63), which its HE PPDUs
    /// carry. An association sets it from the access point's HE Operation
    /// and again whenever the BSS changes its color; a value above 63 is
    /// `SettingError::Unsupported`. A backend without HE accepts it as a
    /// no-op.
    HeBssColor { vif: VifId, color: u8 },
}

/// The coexistence priority beacon reception requests from the radio
/// system the station shares its RF with.
///
/// The station decides when beacon reception asks for the air; the value
/// it asks with is the radio system's priority of its beacon-window event,
/// which only the backend sharing the RF with that system knows. A backend
/// alone on its RF accepts each as a no-op.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxBeaconPriority {
    /// Receive beacons at the radio system's priority of the beacon
    /// window.
    BeaconWindow,
    /// Receive beacons at priority zero.
    Zero,
    /// Withdraw the beacon receive priority request after a beacon.
    Cleared,
}

/// Why the backend refused a setting; nothing changed.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SettingError {
    /// The setting names an interface the backend does not have or has not
    /// configured.
    UnknownVif,
    /// The channel lies outside the backend's bands and widths.
    UnsupportedChannel,
    /// The interface role lies outside the backend's interfaces.
    UnsupportedRole,
    /// Every key slot is in use.
    NoKeySlot,
    /// The key handle is not installed.
    UnknownKey,
    /// The key does not match its cipher.
    InvalidKey,
    /// Every receive Block Ack agreement slot is in use.
    NoBlockAckSlot,
    /// The agreement's TID or window exceeds the backend's limits, or no
    /// such agreement exists.
    InvalidBlockAck,
    /// The setting needs work in flight to finish first.
    Busy,
    /// A value lies outside the limits the backend's capabilities declare.
    Unsupported,
}
