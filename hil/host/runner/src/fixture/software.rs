//! Shared software-generation ownership across one fixture operation or HIL run.

use open_esp_radio_hil_runner::fixture_install::{OperationalLease, Provider};

use crate::Result;

pub(crate) struct SoftwareLease {
    _leases: Vec<OperationalLease>,
}

impl SoftwareLease {
    pub(crate) fn acquire_for(
        lab: &crate::lab::config::LabConfig,
        required: crate::lab::requirements::Requirements,
    ) -> Result<Self> {
        let mut providers = Vec::new();
        if required.bluetooth_adapter {
            providers.push(Provider::LinuxBluetooth);
        }
        if required.local_radio()
            || (required.station_network
                && matches!(
                    lab.station_fixture,
                    crate::lab::config::StationFixtureConfig::LocalLinux(_)
                ))
        {
            providers.push(Provider::LinuxNet);
        }
        Self::acquire(providers)
    }

    pub(crate) fn acquire_one(provider: Provider) -> Result<Self> {
        Self::acquire([provider])
    }

    fn acquire(providers: impl IntoIterator<Item = Provider>) -> Result<Self> {
        #[cfg(target_os = "linux")]
        {
            let mut providers = providers.into_iter().collect::<Vec<_>>();
            providers.sort_by_key(|provider| provider.as_str());
            providers.dedup();
            let mut leases = Vec::new();
            for provider in providers {
                leases.push(open_esp_radio_hil_runner::fixture_install::admit_system(
                    provider,
                )?);
            }
            Ok(Self { _leases: leases })
        }
        #[cfg(not(target_os = "linux"))]
        {
            if providers.into_iter().next().is_some() {
                return Err("Linux fixture software leases require Linux".into());
            }
            Ok(Self {
                _leases: Vec::new(),
            })
        }
    }
}
