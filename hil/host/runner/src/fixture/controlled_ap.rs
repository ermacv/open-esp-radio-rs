//! Lifetime-safe access to the repository-controlled HIL access point.

use crate::{
    Result,
    lab::config::{StationConfig, StationFixtureConfig},
    scenario::PhyExpectation,
};

/// Restores the selected AP frontier on every normal or error return.
pub(crate) enum ControlledAp {
    Local(super::local_ap::AccessPoint),
    OpenWrt(Box<super::openwrt_ap::AccessPoint>),
    External,
}

impl ControlledAp {
    pub(crate) fn start(
        station: &StationConfig,
        fixture: &StationFixtureConfig,
        phy: PhyExpectation,
    ) -> Result<Self> {
        match fixture {
            StationFixtureConfig::LocalLinux(config) => Ok(Self::Local(
                super::local_ap::AccessPoint::start(config, station, phy)
                    .map_err(super::Error::context)?,
            )),
            StationFixtureConfig::OpenWrt(openwrt) => Ok(Self::OpenWrt(Box::new(
                super::openwrt_ap::AccessPoint::start(openwrt, station, phy)
                    .map_err(super::Error::context)?,
            ))),
            StationFixtureConfig::External(_) => {
                require_station_credentials(station)?;
                Ok(Self::External)
            }
        }
    }

    pub(crate) fn stop(&mut self) -> Result<()> {
        match self {
            Self::Local(ap) => ap.stop(),
            Self::OpenWrt(ap) => ap.stop(),
            Self::External => Err(crate::fixture::Error::new(
                "an external station fixture cannot be stopped by HIL",
            )
            .into()),
        }
    }

    pub(crate) fn restart(&mut self) -> Result<()> {
        match self {
            Self::Local(ap) => ap.restart(),
            Self::OpenWrt(ap) => ap.restart(),
            Self::External => Err(crate::fixture::Error::new(
                "an external station fixture cannot be restarted by HIL",
            )
            .into()),
        }
    }
}

pub(crate) fn require_station_credentials(station: &StationConfig) -> Result<()> {
    let _credentials = station.credentials();
    Ok(())
}
