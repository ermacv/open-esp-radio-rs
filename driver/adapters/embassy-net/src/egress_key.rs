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
