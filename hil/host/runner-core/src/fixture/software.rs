//! Shared software-generation ownership across one fixture operation or HIL run.
//!
//! A provider's installed software is also a stand resource,
//! `fixture-software:<provider>`: runs claim it shared and an installation
//! claims it exclusively as stand maintenance, so an installation goes ahead
//! of every waiting request, preempts the runs using the provider, and later
//! runs queue behind it. Runs take the software lease only
//! once the stand granted their claims, never while they wait.

use oer_hil_fixture_install::{OperationalLease, Provider};

use crate::Result;

pub struct SoftwareLease {
    _leases: Vec<OperationalLease>,
}

impl SoftwareLease {
    pub fn acquire_for(
        lab: &crate::lab::config::LabConfig,
        required: oer_hil_scenario::requirements::Requirements,
    ) -> Result<Self> {
        Self::acquire(providers(lab, required))
    }

    pub fn acquire_one(provider: Provider) -> Result<Self> {
        Self::acquire([provider])
    }

    pub(crate) fn acquire(providers: impl IntoIterator<Item = Provider>) -> Result<Self> {
        #[cfg(target_os = "linux")]
        {
            let mut providers = providers.into_iter().collect::<Vec<_>>();
            providers.sort_by_key(|provider| provider.as_str());
            providers.dedup();
            let mut leases = Vec::new();
            for provider in providers {
                leases.push(oer_hil_fixture_install::admit_system(provider)?);
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

/// The fixture software providers `required` runs on.
pub fn providers(
    lab: &crate::lab::config::LabConfig,
    required: oer_hil_scenario::requirements::Requirements,
) -> Vec<Provider> {
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
    providers
}

/// The stand resource of `provider`'s installed software.
pub fn resource(provider: Provider) -> String {
    format!("fixture-software:{}", provider.as_str())
}

/// Take the stand for installing `provider`: stand maintenance, served
/// before every waiting request, and every run that uses the provider is
/// preempted with the ordinary cleanup and a notice. The grant keeps later
/// runs of the provider queued until it is dropped.
pub fn install_grant(provider: Provider) -> Result<oer_hil_arbiter::Grant> {
    let mut request = oer_hil_arbiter::Request::from_environment(format!(
        "fixture install --provider {}",
        provider.as_str()
    ))?;
    request.claims = vec![oer_hil_arbiter::Claim::exclusive(resource(provider))];
    let reason = format!(
        "fixture maintenance: {} install by {}",
        provider.as_str(),
        request.owner
    );
    oer_hil_arbiter::Arbiter::open()?.acquire_maintenance(&request, &reason)
}
