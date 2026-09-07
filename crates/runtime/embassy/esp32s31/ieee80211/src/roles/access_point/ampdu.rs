//! Role-lifetime wrapper around the shared retained A-MPDU arenas.

use crate::datapath::tx::resources::{AggregateTxArenaPair, AggregateTxResources};

use oer_memory::StableDmaBacking;

use oer_esp32s31_wifi_ap::ampdu::ApAmpduTx;

/// AP lease of the role-neutral aggregate arenas.
///
/// Both retained arenas stay role-owned. One may be prepared from queued
/// Ethernet leases while hardware transmits the other; swapping them is an
/// ownership transition, not a second hardware publication.
pub struct AccessPointAmpdu<'storage, B: 'storage, const SLOTS: usize, const BUFFER_SIZE: usize> {
    arenas: AggregateTxArenaPair<ApAmpduTx<'storage, B, SLOTS, BUFFER_SIZE>>,
    maximum_aggregate_bytes: u16,
    attempt_limit: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AccessPointAmpduParked {
    maximum_aggregate_bytes: u16,
    attempt_limit: u8,
}

impl<'storage, B: StableDmaBacking + 'storage, const SLOTS: usize, const BUFFER_SIZE: usize>
    AccessPointAmpdu<'storage, B, SLOTS, BUFFER_SIZE>
{
    pub fn new(
        resources: AggregateTxResources<'storage, B, SLOTS, BUFFER_SIZE>,
        maximum_aggregate_bytes: u16,
        attempt_limit: u8,
    ) -> Self {
        let (primary, primary_retention, standby, standby_retention) = resources.into_parts();
        let active = match ApAmpduTx::new(
            primary,
            primary_retention,
            maximum_aggregate_bytes,
            attempt_limit,
        ) {
            Ok(active) => active,
            Err(error) => unreachable!(
                "static AP aggregate geometry is validated by the shared STA arena: {error:?}"
            ),
        };
        let standby = match (standby, standby_retention) {
            (Some(resources), Some(retention)) => Some(
                ApAmpduTx::new(resources, retention, maximum_aggregate_bytes, attempt_limit)
                    .unwrap_or_else(|error| {
                        unreachable!("standby AP aggregate geometry is static: {error:?}")
                    }),
            ),
            (None, None) => None,
            _ => unreachable!("standby aggregate resources and retention move together"),
        };
        Self {
            arenas: AggregateTxArenaPair::new(active, standby),
            maximum_aggregate_bytes,
            attempt_limit,
        }
    }

    pub fn active_mut(&mut self) -> &mut ApAmpduTx<'storage, B, SLOTS, BUFFER_SIZE> {
        self.arenas.active_mut()
    }

    pub fn standby_mut(&mut self) -> Option<&mut ApAmpduTx<'storage, B, SLOTS, BUFFER_SIZE>> {
        self.arenas.standby_mut()
    }

    pub const fn has_standby(&self) -> bool {
        self.arenas.has_standby()
    }

    pub fn publish_standby<P, E, T, const ORDINARY_BUFFER_SIZE: usize, H>(
        &mut self,
        ordinary: &mut oer_esp32s31_wifi_ap::tx::ApTx<'_, P, E, T, ORDINARY_BUFFER_SIZE>,
        hardware: &mut H,
    ) -> Result<
        oer_esp32s31_wifi_ap::ampdu::ApPreparedAmpdu,
        oer_esp32s31_wifi_ap::ampdu::ApAmpduError,
    >
    where
        P: oer_esp32s31_wifi::ordinary_tx::WifiTxPowerProfile,
        E: oer_esp32s31_wifi::ordinary_tx::WifiTxEntropy,
        T: oer_esp32s31_wifi::ordinary_tx::WifiTxTimer,
        H: oer_esp32s31_wifi_mac::tx::ampdu::HtAmpduHardware,
    {
        let standby = self
            .arenas
            .standby_mut()
            .ok_or(oer_esp32s31_wifi_ap::ampdu::ApAmpduError::Idle)?;
        let prepared = standby.publish(ordinary, hardware)?;
        assert!(
            self.arenas.swap_active_standby(),
            "standby publication preserves the checked arena"
        );
        Ok(prepared)
    }

    #[allow(clippy::result_large_err)]
    pub fn try_park(
        self,
    ) -> Result<
        (
            AggregateTxResources<'storage, B, SLOTS, BUFFER_SIZE>,
            AccessPointAmpduParked,
        ),
        Self,
    > {
        let Self {
            arenas,
            maximum_aggregate_bytes,
            attempt_limit,
        } = self;
        let (active, standby) = arenas.into_parts();
        match active.try_into_resources() {
            Ok((primary, primary_retention)) => {
                let (standby, standby_retention) = match standby {
                    Some(standby) => match standby.try_into_resources() {
                        Ok((standby, retention)) => (Some(standby), Some(retention)),
                        Err(_) => unreachable!("idle active arena implies idle standby arena"),
                    },
                    None => (None, None),
                };
                Ok((
                    AggregateTxResources::from_parts(
                        primary,
                        primary_retention,
                        standby,
                        standby_retention,
                    ),
                    AccessPointAmpduParked {
                        maximum_aggregate_bytes,
                        attempt_limit,
                    },
                ))
            }
            Err(active) => Err(Self {
                arenas: AggregateTxArenaPair::new(active, standby),
                maximum_aggregate_bytes,
                attempt_limit,
            }),
        }
    }

    pub fn resume(
        resources: AggregateTxResources<'storage, B, SLOTS, BUFFER_SIZE>,
        parked: AccessPointAmpduParked,
    ) -> Self {
        Self::new(
            resources,
            parked.maximum_aggregate_bytes,
            parked.attempt_limit,
        )
    }

    #[allow(clippy::result_large_err)]
    pub fn try_into_resources(
        self,
    ) -> Result<AggregateTxResources<'storage, B, SLOTS, BUFFER_SIZE>, Self> {
        self.try_park().map(|(resources, _)| resources)
    }
}

#[cfg(test)]
mod tests;
