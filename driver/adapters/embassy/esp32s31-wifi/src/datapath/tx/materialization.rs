//! Radio-owned boundary between durable software backlog and physical TX.
//!
//! Network integrations may retain packets in different memory and ownership
//! models.  Radio policy sees only an affine software frame and this
//! synchronous batch materializer.  Selection therefore precedes scarce SRAM
//! admission without exposing Xarxa, Embassy tokens or a compatibility
//! adapter's queue representation to STA/AP policy.

use open_esp_radio_dma::StableDmaBacking;
use open_esp_radio_network::NetworkInterfaceId;

/// Two owners that must cross a materialization boundary atomically.
pub type FramePair<Frame> = (Frame, Frame);

/// Result of atomically materializing two software-owned frames.
pub type MaterializedPairResult<SoftwareFrame, PhysicalFrame> =
    Result<FramePair<PhysicalFrame>, FramePair<SoftwareFrame>>;

/// Diagnostic snapshot of the bounded physical materialization horizon.
#[cfg(feature = "tx-phase-telemetry")]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MaterializationOwnershipSnapshot {
    pub free: usize,
    pub radio_owned: usize,
}

/// One affine software-owned Ethernet frame selected for radio service.
///
/// Implementations retain their original allocation until materialization or
/// drop.  The radio may inspect bytes for peer/TID/lifecycle classification,
/// but physical DMA ownership is represented only by
/// [`SelectedBurstMaterializer::PhysicalFrame`].
pub trait SoftwareTxFrame {
    fn interface(&self) -> NetworkInterfaceId;

    fn ethernet(&self) -> &[u8];

    fn as_slice(&self) -> &[u8] {
        self.ethernet()
    }
}

/// Final DMA-stable owner containing one materialized Ethernet frame.
///
/// The radio encoder needs both the logical Ethernet view and its offset in
/// the stable backing when it builds descriptor references. Keeping this
/// contract beside materialization avoids depending on one SRAM-pool lease
/// type while retaining exact, typed DMA geometry.
pub trait MaterializedTxFrame: StableDmaBacking {
    fn ethernet(&self) -> &[u8];

    fn ethernet_offset(&self) -> usize;

    fn ethernet_length(&self) -> usize {
        self.ethernet().len()
    }

    fn as_slice(&self) -> &[u8] {
        self.ethernet()
    }

    fn storage_mut(&mut self) -> &mut [u8] {
        self.stable_dma_region().into_mut_slice()
    }
}

/// Synchronous, batch-oriented admission from software ownership into the
/// fixed physical radio horizon.
///
/// Implementations must reserve every requested destination before consuming
/// any source owner.  A failed operation returns or retains every source
/// unchanged.  No method may wait for another executor/core while holding a
/// physical credit.
pub trait SelectedBurstMaterializer {
    type SoftwareFrame: SoftwareTxFrame;
    type PhysicalFrame: MaterializedTxFrame;

    fn interface(&self) -> NetworkInterfaceId;

    fn queue_len(&self) -> usize;

    fn try_take(&self) -> Option<Self::SoftwareFrame>;

    fn try_materialize(
        &self,
        frame: Self::SoftwareFrame,
    ) -> Result<Self::PhysicalFrame, Self::SoftwareFrame>;

    /// Reserve one physical credit before removing the next software owner.
    fn try_materialize_next(&self) -> Option<Self::PhysicalFrame>;

    fn materialization_capacity(&self) -> usize;

    #[cfg(feature = "tx-phase-telemetry")]
    fn ownership_snapshot(&self) -> MaterializationOwnershipSnapshot;

    /// Materialize one selected batch atomically with respect to source
    /// ownership. `destinations` must be empty on entry. On `false`, it stays
    /// empty and every occupied source remains in place.
    fn try_materialize_batch<const BATCH: usize>(
        &self,
        sources: &mut [Option<Self::SoftwareFrame>; BATCH],
        destinations: &mut [Option<Self::PhysicalFrame>; BATCH],
    ) -> bool;

    fn try_materialize_pair(
        &self,
        first: Self::SoftwareFrame,
        second: Self::SoftwareFrame,
    ) -> MaterializedPairResult<Self::SoftwareFrame, Self::PhysicalFrame> {
        let mut sources = [Some(first), Some(second)];
        let mut destinations = [const { None }; 2];
        if !self.try_materialize_batch(&mut sources, &mut destinations) {
            return Err((
                sources[0].take().expect("failed pair retains first owner"),
                sources[1].take().expect("failed pair retains second owner"),
            ));
        }
        Ok((
            destinations[0]
                .take()
                .expect("successful pair publishes first owner"),
            destinations[1]
                .take()
                .expect("successful pair publishes second owner"),
        ))
    }
}
