//! Settings, keys, TSF and lifecycle of the lower-MAC port.

use oer_ieee80211_mac::{channel::Channel, sequence::SequenceNumber};
use oer_radio_coex::CoexPriority;

use crate::tx::TxId;

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

/// Which received frames the backend delivers for one interface.
///
/// The filter is a set of admission rules; a frame admitted by any rule is
/// delivered. Frames addressed to the interface are acknowledged by the
/// hardware whatever the filter says about other frames.
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
    /// Every frame the PHY decodes with a valid FCS.
    pub const PROMISCUOUS: Self = Self(1 << 5);

    /// What a station or access point needs for its own BSS.
    pub const BSS_MEMBER: Self = Self(
        Self::OWN_UNICAST.0 | Self::OWN_BSS_GROUP.0 | Self::OWN_BSS_BEACONS.0 | Self::OWN_CONTROL.0,
    );

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
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
    pub receive: ReceiveFilter,
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

/// A value of an interface's Timing Synchronization Function in
/// microseconds (IEEE 802.11-2020 11.1.3).
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Tsf(pub u64);

/// When the backend reports target beacon transmission times.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TbttSchedule {
    /// The beacon interval in time units of 1024 µs.
    pub beacon_interval_tu: u16,
    /// The next target beacon transmission time.
    pub next: Tsf,
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
    /// Set an interface's TSF.
    SetTsf { vif: VifId, tsf: Tsf },
    /// Report target beacon transmission times of an interface, or stop
    /// (`None`).
    Tbtt {
        vif: VifId,
        schedule: Option<TbttSchedule>,
    },
    /// Hold or release attempts to `peer`, or to every receiver of the
    /// interface when `peer` is `None`, while power save keeps them
    /// asleep. A held attempt waits in the backend; submission still admits
    /// it.
    PowerSaveTxBlock {
        vif: VifId,
        peer: Option<MacAddress>,
        blocked: bool,
    },
    /// How urgently Wi-Fi needs the shared antenna now; the backend maps it
    /// onto its coexistence arbitration.
    CoexPriority(CoexPriority),
}

/// Why the backend refused a setting; nothing changed.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SettingError {
    /// The setting names an interface the backend does not have or has not
    /// configured.
    UnknownVif,
    /// The backend cannot tune to the channel.
    UnsupportedChannel,
    /// The backend does not implement the interface role.
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
    /// The setting needs the port quiesced and attempts are in flight.
    Busy,
    /// The backend does not implement the setting.
    Unsupported,
}

/// Lifecycle commands; each ends with a terminal event.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LifecycleCommand {
    /// Start receiving and admitting attempts; ends with
    /// [`LifecycleEvent::Enabled`].
    Enable,
    /// Stop admitting attempts, abort those in flight and stop receiving;
    /// ends with [`LifecycleEvent::Disabled`] after the completion of every
    /// admitted attempt.
    Disable,
    /// Stop admitting attempts and let those in flight complete; ends with
    /// [`LifecycleEvent::Quiesced`] after the last completion. Enable
    /// resumes admission.
    Quiesce,
    /// End one admitted attempt; its terminal event is its completion,
    /// [`TxStatus::Aborted`](crate::TxStatus::Aborted) unless it had
    /// already completed.
    Cancel(TxId),
}

/// Terminal events of lifecycle commands.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LifecycleEvent {
    Enabled,
    Disabled,
    Quiesced,
}

/// Why the backend refused a lifecycle command; nothing changed.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LifecycleError {
    /// The port is already in the requested state.
    AlreadyInState,
    /// No admitted attempt has this identity.
    UnknownAttempt,
}
