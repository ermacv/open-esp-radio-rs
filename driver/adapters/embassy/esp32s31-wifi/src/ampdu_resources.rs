//! Role-neutral retained A-MPDU descriptor arenas.
//!
//! AP and STA lease these exact owners; neither role may manufacture or
//! discard descriptor or network-lease retention storage at a transition.

use open_esp_radio_esp32s31_wifi_mac::tx_ampdu::{HtAmpduTxResources, RetainedAmpduDmaStorage};

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
