//! Role-lifetime wrapper around the shared retained A-MPDU arenas.

use open_esp_radio_dma::StableDmaBacking;
use open_esp_radio_esp32s31_wifi_ap::ampdu::Esp32s31ApAmpduTx;
use open_esp_radio_esp32s31_wifi_mac::tx_ampdu::{HtAmpduTxResources, RetainedAmpduDmaStorage};

use crate::ampdu_resources::AggregateTxResources;

/// AP lease of the role-neutral aggregate arenas.
///
/// AP currently needs one hardware arena. The standby arena remains owned by
/// this wrapper and is returned unchanged at the role boundary; STA may use
/// both arenas again after AP teardown.
pub struct Esp32s31AccessPointAmpdu<
    'storage,
    B: 'storage,
    const SLOTS: usize,
    const BUFFER_SIZE: usize,
> {
    active: Esp32s31ApAmpduTx<'storage, B, SLOTS, BUFFER_SIZE>,
    standby: Option<HtAmpduTxResources<'storage, SLOTS, BUFFER_SIZE>>,
    standby_retention: Option<&'storage mut RetainedAmpduDmaStorage<B, SLOTS>>,
}

impl<'storage, B: StableDmaBacking + 'storage, const SLOTS: usize, const BUFFER_SIZE: usize>
    Esp32s31AccessPointAmpdu<'storage, B, SLOTS, BUFFER_SIZE>
{
    pub fn new(
        resources: AggregateTxResources<'storage, B, SLOTS, BUFFER_SIZE>,
        maximum_aggregate_bytes: u16,
        attempt_limit: u8,
    ) -> Self {
        let (primary, primary_retention, standby, standby_retention) = resources.into_parts();
        match Esp32s31ApAmpduTx::new(
            primary,
            primary_retention,
            maximum_aggregate_bytes,
            attempt_limit,
        ) {
            Ok(active) => Self {
                active,
                standby,
                standby_retention,
            },
            Err(error) => unreachable!(
                "static AP aggregate geometry is validated by the shared STA arena: {error:?}"
            ),
        }
    }

    pub fn active_mut(&mut self) -> &mut Esp32s31ApAmpduTx<'storage, B, SLOTS, BUFFER_SIZE> {
        &mut self.active
    }

    #[allow(clippy::result_large_err)]
    pub fn try_into_resources(
        self,
    ) -> Result<AggregateTxResources<'storage, B, SLOTS, BUFFER_SIZE>, Self> {
        let Self {
            active,
            standby,
            standby_retention,
        } = self;
        match active.try_into_resources() {
            Ok((primary, primary_retention)) => Ok(AggregateTxResources::from_parts(
                primary,
                primary_retention,
                standby,
                standby_retention,
            )),
            Err(active) => Err(Self {
                active,
                standby,
                standby_retention,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregate_role_handle_remains_a_small_borrowed_owner() {
        assert!(core::mem::size_of::<Esp32s31AccessPointAmpdu<'static, (), 32, 0>>() <= 256);
    }
}
