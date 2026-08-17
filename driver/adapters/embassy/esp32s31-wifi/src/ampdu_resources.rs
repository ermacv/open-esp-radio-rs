//! Role-neutral retained A-MPDU descriptor arenas.
//!
//! AP and STA lease these exact owners; neither role may manufacture or
//! discard descriptor or network-lease retention storage at a transition.

use open_esp_radio_esp32s31_wifi_mac::tx_ampdu::{HtAmpduTxResources, RetainedAmpduDmaStorage};

/// Unique owner of the active and optional standby aggregate arenas.
///
/// AP and STA may attach different frame encoders and completion policies to
/// an arena, but the double-buffer ownership transition is identical: only
/// the active arena may be published, while the standby arena remains
/// software-owned until an explicit swap.
pub(crate) struct AggregateTxArenaPair<T> {
    active: T,
    standby: Option<T>,
}

impl<T> AggregateTxArenaPair<T> {
    pub(crate) const fn new(active: T, standby: Option<T>) -> Self {
        Self { active, standby }
    }

    pub(crate) const fn has_standby(&self) -> bool {
        self.standby.is_some()
    }

    pub(crate) const fn active(&self) -> &T {
        &self.active
    }

    pub(crate) fn active_mut(&mut self) -> &mut T {
        &mut self.active
    }

    pub(crate) const fn standby(&self) -> Option<&T> {
        self.standby.as_ref()
    }

    pub(crate) fn standby_mut(&mut self) -> Option<&mut T> {
        self.standby.as_mut()
    }

    /// Promote the software-owned standby arena to active ownership.
    ///
    /// The former active arena becomes the next standby. Callers must prove
    /// that its hardware transaction has completed before crossing this edge.
    pub(crate) fn swap_active_standby(&mut self) -> bool {
        let Some(standby) = self.standby.as_mut() else {
            return false;
        };
        core::mem::swap(&mut self.active, standby);
        true
    }

    pub(crate) fn into_parts(self) -> (T, Option<T>) {
        (self.active, self.standby)
    }
}

pub struct AggregateTxResources<'storage, B: 'storage, const SLOTS: usize, const BUFFER_SIZE: usize>
{
    pub(crate) primary: HtAmpduTxResources<'storage, SLOTS, BUFFER_SIZE>,
    pub(crate) primary_retention: &'storage mut RetainedAmpduDmaStorage<B, SLOTS>,
    pub(crate) standby: Option<HtAmpduTxResources<'storage, SLOTS, BUFFER_SIZE>>,
    pub(crate) standby_retention: Option<&'storage mut RetainedAmpduDmaStorage<B, SLOTS>>,
}

impl<'storage, B: 'storage, const SLOTS: usize, const BUFFER_SIZE: usize>
    AggregateTxResources<'storage, B, SLOTS, BUFFER_SIZE>
{
    pub const fn single(
        primary: HtAmpduTxResources<'storage, SLOTS, BUFFER_SIZE>,
        primary_retention: &'storage mut RetainedAmpduDmaStorage<B, SLOTS>,
    ) -> Self {
        Self {
            primary,
            primary_retention,
            standby: None,
            standby_retention: None,
        }
    }

    pub const fn pipelined(
        primary: HtAmpduTxResources<'storage, SLOTS, BUFFER_SIZE>,
        primary_retention: &'storage mut RetainedAmpduDmaStorage<B, SLOTS>,
        standby: HtAmpduTxResources<'storage, SLOTS, BUFFER_SIZE>,
        standby_retention: &'storage mut RetainedAmpduDmaStorage<B, SLOTS>,
    ) -> Self {
        Self {
            primary,
            primary_retention,
            standby: Some(standby),
            standby_retention: Some(standby_retention),
        }
    }

    pub const fn primary(&self) -> &HtAmpduTxResources<'storage, SLOTS, BUFFER_SIZE> {
        &self.primary
    }

    pub const fn standby(&self) -> Option<&HtAmpduTxResources<'storage, SLOTS, BUFFER_SIZE>> {
        self.standby.as_ref()
    }

    #[allow(clippy::type_complexity)]
    pub fn into_parts(
        self,
    ) -> (
        HtAmpduTxResources<'storage, SLOTS, BUFFER_SIZE>,
        &'storage mut RetainedAmpduDmaStorage<B, SLOTS>,
        Option<HtAmpduTxResources<'storage, SLOTS, BUFFER_SIZE>>,
        Option<&'storage mut RetainedAmpduDmaStorage<B, SLOTS>>,
    ) {
        (
            self.primary,
            self.primary_retention,
            self.standby,
            self.standby_retention,
        )
    }

    #[allow(clippy::type_complexity)]
    pub fn from_parts(
        primary: HtAmpduTxResources<'storage, SLOTS, BUFFER_SIZE>,
        primary_retention: &'storage mut RetainedAmpduDmaStorage<B, SLOTS>,
        standby: Option<HtAmpduTxResources<'storage, SLOTS, BUFFER_SIZE>>,
        standby_retention: Option<&'storage mut RetainedAmpduDmaStorage<B, SLOTS>>,
    ) -> Self {
        assert_eq!(
            standby.is_some(),
            standby_retention.is_some(),
            "standby descriptors and retention must cross role boundaries together"
        );
        Self {
            primary,
            primary_retention,
            standby,
            standby_retention,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AggregateTxArenaPair;

    #[test]
    fn arena_pair_swap_preserves_both_unique_owners() {
        let mut pair = AggregateTxArenaPair::new(1_u8, Some(2_u8));

        assert!(pair.swap_active_standby());
        assert_eq!(*pair.active(), 2);
        assert_eq!(pair.standby(), Some(&1));

        let (active, standby) = pair.into_parts();
        assert_eq!(active, 2);
        assert_eq!(standby, Some(1));
    }

    #[test]
    fn single_arena_cannot_cross_a_missing_standby_edge() {
        let mut pair = AggregateTxArenaPair::new(1_u8, None);

        assert!(!pair.swap_active_standby());
        assert_eq!(*pair.active(), 1);
        assert!(!pair.has_standby());
    }
}
