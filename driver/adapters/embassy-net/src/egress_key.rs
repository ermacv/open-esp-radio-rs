use core::num::{NonZeroU8, NonZeroU32};

/// Generation-bound radio queue identity understood on both cores.
///
/// The generic network stack retains only its opaque `EgressKey`. The open
/// radio adapter validates and translates that value before it enters the
/// radio-owned candidate/grant control plane.
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
