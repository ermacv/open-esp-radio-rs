use core::num::{NonZeroU8, NonZeroU32};

use embassy_net_driver::EgressKey;

pub(crate) const KEY_FORMAT: u32 = 0x5700_0000;
pub(crate) const KEY_FORMAT_MASK: u32 = 0xff00_0000;
pub(crate) const TOPOLOGY_MASK: u32 = 0x00ff_0000;
pub(crate) const SINGLE_RADIO_PEER: u32 = 1 << 16;
pub(crate) const PER_LINK_DESTINATION: u32 = 2 << 16;
pub(crate) const ASSOCIATED_PEER: u32 = 3 << 16;

/// Generic, generation-bound identity decoded from an associated-peer key.
///
/// This is deliberately not a Wi-Fi TXQ or BlockAck key. The network adapter
/// owns the interface/peer classification and preserves the generic traffic
/// class. The radio role remains responsible for mapping that class to a TID
/// and for revalidating association, power-save and radio state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AssociatedEgressIdentity {
    interface: u8,
    schedule_epoch: u32,
    peer_slot: NonZeroU8,
    peer_generation: NonZeroU32,
    traffic_class: u8,
}

impl AssociatedEgressIdentity {
    pub const fn new(
        interface: u8,
        schedule_epoch: u32,
        peer_slot: NonZeroU8,
        peer_generation: NonZeroU32,
        traffic_class: u8,
    ) -> Self {
        Self {
            interface,
            schedule_epoch,
            peer_slot,
            peer_generation,
            traffic_class,
        }
    }

    pub(crate) fn decode(key: EgressKey) -> Option<Self> {
        let [header, schedule_epoch, generation, slot] = key.words();
        if header & KEY_FORMAT_MASK != KEY_FORMAT || header & TOPOLOGY_MASK != ASSOCIATED_PEER {
            return None;
        }
        Some(Self::new(
            ((header >> 8) & 0xff) as u8,
            schedule_epoch,
            u8::try_from(slot).ok().and_then(NonZeroU8::new)?,
            NonZeroU32::new(generation)?,
            header as u8,
        ))
    }

    pub const fn interface(self) -> u8 {
        self.interface
    }

    pub const fn schedule_epoch(self) -> u32 {
        self.schedule_epoch
    }

    pub const fn peer_slot(self) -> NonZeroU8 {
        self.peer_slot
    }

    pub const fn peer_generation(self) -> NonZeroU32 {
        self.peer_generation
    }

    pub const fn traffic_class(self) -> u8 {
        self.traffic_class
    }
}

/// Adapter-owned interpretation of an opaque Xarxa egress key.
///
/// Xarxa only compares and retains the key. The physical device adapter may
/// decode its own format when it needs to join stack readiness with current
/// radio state. Decoding is classification only: it does not revalidate a
/// scheduling epoch, association generation, power-save state or admission
/// credit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecodedEgressKey {
    SingleRadioPeer {
        interface: u8,
        schedule_epoch: u32,
        traffic_class: u8,
    },
    PerLinkDestination {
        interface: u8,
        schedule_epoch: u32,
        destination: [u8; 6],
        traffic_class: u8,
    },
    AssociatedPeer(AssociatedEgressIdentity),
}

impl DecodedEgressKey {
    /// Decode only keys emitted by this adapter's versioned key format.
    pub fn decode(key: EgressKey) -> Option<Self> {
        let [header, schedule_epoch, generation, slot] = key.words();
        if header & KEY_FORMAT_MASK != KEY_FORMAT {
            return None;
        }
        let interface = ((header >> 8) & 0xff) as u8;
        let traffic_class = header as u8;
        match header & TOPOLOGY_MASK {
            SINGLE_RADIO_PEER if generation == 0 && slot == 0 => Some(Self::SingleRadioPeer {
                interface,
                schedule_epoch,
                traffic_class,
            }),
            PER_LINK_DESTINATION => {
                let high = u16::try_from(slot).ok()?.to_le_bytes();
                let low = generation.to_le_bytes();
                Some(Self::PerLinkDestination {
                    interface,
                    schedule_epoch,
                    destination: [low[0], low[1], low[2], low[3], high[0], high[1]],
                    traffic_class,
                })
            }
            ASSOCIATED_PEER => AssociatedEgressIdentity::decode(key).map(Self::AssociatedPeer),
            _ => None,
        }
    }

    pub const fn interface(self) -> u8 {
        match self {
            Self::SingleRadioPeer { interface, .. }
            | Self::PerLinkDestination { interface, .. } => interface,
            Self::AssociatedPeer(identity) => identity.interface(),
        }
    }

    pub const fn schedule_epoch(self) -> u32 {
        match self {
            Self::SingleRadioPeer { schedule_epoch, .. }
            | Self::PerLinkDestination { schedule_epoch, .. } => schedule_epoch,
            Self::AssociatedPeer(identity) => identity.schedule_epoch(),
        }
    }

    pub const fn traffic_class(self) -> u8 {
        match self {
            Self::SingleRadioPeer { traffic_class, .. }
            | Self::PerLinkDestination { traffic_class, .. } => traffic_class,
            Self::AssociatedPeer(identity) => identity.traffic_class(),
        }
    }
}

/// Generation-bound radio queue identity understood on both cores.
///
/// The generic network stack retains only its opaque `EgressKey`. The open
/// radio adapter validates and translates that value before it enters the
/// radio-owned scheduling and diagnostic grant state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EgressGrantKey {
    interface: u8,
    peer_slot: NonZeroU8,
    peer_generation: NonZeroU32,
    tid: u8,
}

impl EgressGrantKey {
    pub const fn new(
        interface: u8,
        peer_slot: NonZeroU8,
        peer_generation: NonZeroU32,
        tid: u8,
    ) -> Self {
        Self {
            interface,
            peer_slot,
            peer_generation,
            tid,
        }
    }

    pub const fn interface(self) -> u8 {
        self.interface
    }

    pub const fn peer_slot(self) -> NonZeroU8 {
        self.peer_slot
    }

    pub const fn peer_generation(self) -> NonZeroU32 {
        self.peer_generation
    }

    pub const fn tid(self) -> u8 {
        self.tid
    }

    #[cfg(feature = "tx-phase-telemetry")]
    pub(crate) const fn packed(self) -> u32 {
        u32::from_le_bytes([self.interface, self.peer_slot.get(), self.tid, 0])
    }

    #[cfg(feature = "tx-phase-telemetry")]
    pub(crate) fn from_packed(packed: u32, generation: u32) -> Option<Self> {
        let [interface, peer_slot, tid, reserved] = packed.to_le_bytes();
        (reserved == 0).then_some(())?;
        Some(Self::new(
            interface,
            NonZeroU8::new(peer_slot)?,
            NonZeroU32::new(generation)?,
            tid,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn header(topology: u32, interface: u8, traffic_class: u8) -> u32 {
        KEY_FORMAT | topology | ((interface as u32) << 8) | traffic_class as u32
    }

    #[test]
    fn decodes_each_adapter_owned_topology_without_granting_authority() {
        assert_eq!(
            DecodedEgressKey::decode(EgressKey::from_words([
                header(SINGLE_RADIO_PEER, 2, 6),
                7,
                0,
                0,
            ])),
            Some(DecodedEgressKey::SingleRadioPeer {
                interface: 2,
                schedule_epoch: 7,
                traffic_class: 6,
            })
        );

        let destination = [0x02, 1, 2, 3, 4, 5];
        assert_eq!(
            DecodedEgressKey::decode(EgressKey::from_words([
                header(PER_LINK_DESTINATION, 3, 1),
                8,
                u32::from_le_bytes([
                    destination[0],
                    destination[1],
                    destination[2],
                    destination[3],
                ]),
                u16::from_le_bytes([destination[4], destination[5]]) as u32,
            ])),
            Some(DecodedEgressKey::PerLinkDestination {
                interface: 3,
                schedule_epoch: 8,
                destination,
                traffic_class: 1,
            })
        );

        let associated = DecodedEgressKey::decode(EgressKey::from_words([
            header(ASSOCIATED_PEER, 4, 0),
            9,
            11,
            5,
        ]))
        .unwrap();
        assert_eq!(associated.interface(), 4);
        assert_eq!(associated.schedule_epoch(), 9);
        assert_eq!(associated.traffic_class(), 0);
        assert!(matches!(associated, DecodedEgressKey::AssociatedPeer(_)));
    }

    #[test]
    fn malformed_or_foreign_keys_fail_closed() {
        assert_eq!(
            DecodedEgressKey::decode(EgressKey::from_words([0, 1, 0, 0])),
            None
        );
        assert_eq!(
            DecodedEgressKey::decode(EgressKey::from_words([
                header(SINGLE_RADIO_PEER, 0, 0),
                1,
                1,
                0,
            ])),
            None
        );
        assert_eq!(
            DecodedEgressKey::decode(EgressKey::from_words([
                header(PER_LINK_DESTINATION, 0, 0),
                1,
                0,
                u32::from(u16::MAX) + 1,
            ])),
            None
        );
        assert_eq!(
            DecodedEgressKey::decode(EgressKey::from_words([
                header(ASSOCIATED_PEER, 0, 0),
                1,
                0,
                1,
            ])),
            None
        );
    }
}
