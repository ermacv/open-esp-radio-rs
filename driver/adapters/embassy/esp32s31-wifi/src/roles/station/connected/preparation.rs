//! Owner-preserving preparation of a successful join for connected service.
//!
//! The associated peer, selected VIF and connected policy must be validated
//! before runtime, network or key owners are split by a composition root.  A
//! failed policy check returns the complete original handoff so callers cannot
//! accidentally turn configuration failure into a partially consumed epoch.

use open_esp_radio_esp32s31_wifi_sta::attempt::{
    Esp32s31StaAttemptSecurity, Esp32s31StaInstalledSecurity,
};
use open_esp_radio_wifi_embassy::station_network::{
    StationNetworkLink, StationNetworkResources, start_station_network,
};

use crate::roles::station::connected::port::{
    Esp32s31ConnectedStaConfigError, Esp32s31ConnectedStaPlan, Esp32s31ConnectedStaPort,
};

use super::{Esp32s31ConnectedServiceParts, Esp32s31ConnectedServiceResources};

/// Validated connected plan paired with every owner returned by join.
pub struct Esp32s31PreparedConnectedService<'security, R, E, N> {
    runtime: R,
    epoch: E,
    network: N,
    plan: Esp32s31ConnectedStaPlan,
    installed_security: Esp32s31StaInstalledSecurity,
    security: Esp32s31StaAttemptSecurity<'security>,
}

impl<'security, R, E, N> Esp32s31PreparedConnectedService<'security, R, E, N> {
    pub const fn plan(&self) -> &Esp32s31ConnectedStaPlan {
        &self.plan
    }

    pub const fn epoch(&self) -> &E {
        &self.epoch
    }

    pub fn into_parts(self) -> Esp32s31PreparedConnectedServiceParts<'security, R, E, N> {
        Esp32s31PreparedConnectedServiceParts {
            runtime: self.runtime,
            epoch: self.epoch,
            network: self.network,
            plan: self.plan,
            installed_security: self.installed_security,
            security: self.security,
        }
    }
}

/// Validated service after its persistent network owner entered the connected
/// link state.
pub struct Esp32s31ConnectedNetworkStarted<'security, R, E, S, N, T> {
    runtime: R,
    epoch: E,
    stack: S,
    network: N,
    initial_network_task: Option<T>,
    plan: Esp32s31ConnectedStaPlan,
    installed_security: Esp32s31StaInstalledSecurity,
    security: Esp32s31StaAttemptSecurity<'security>,
}

impl<'security, R, E, S, N, T> Esp32s31ConnectedNetworkStarted<'security, R, E, S, N, T> {
    /// Borrow the still-coherent station runtime before the connected graph is
    /// decomposed for driver assembly. Platform activation failures can then
    /// return this complete owner instead of rebuilding it from loose fields.
    pub fn runtime_mut(&mut self) -> &mut R {
        &mut self.runtime
    }

    /// Exact agreement proof retained before a paired composition splits the
    /// connected owner graph. A mismatch must quarantine this whole value;
    /// it must not be repaired by treating a key-bearing attempt as Open.
    pub const fn security_modes(
        &self,
    ) -> (
        open_esp_radio_ieee80211::security::WifiSecurityMode,
        open_esp_radio_ieee80211::security::WifiSecurityMode,
    ) {
        (self.installed_security.mode(), self.security.mode())
    }

    pub fn into_parts(self) -> Esp32s31ConnectedNetworkStartedParts<'security, R, E, S, N, T> {
        Esp32s31ConnectedNetworkStartedParts {
            runtime: self.runtime,
            epoch: self.epoch,
            stack: self.stack,
            network: self.network,
            initial_network_task: self.initial_network_task,
            plan: self.plan,
            installed_security: self.installed_security,
            security: self.security,
        }
    }

    /// Borrow the runtime and hardware frontier together for the final
    /// pre-activation ownership transaction.
    pub fn runtime_and_epoch_mut(&mut self) -> (&mut R, &mut E) {
        (&mut self.runtime, &mut self.epoch)
    }
}

/// Named network-started decomposition used by the concrete driver assembler.
pub struct Esp32s31ConnectedNetworkStartedParts<'security, R, E, S, N, T> {
    pub runtime: R,
    pub epoch: E,
    pub stack: S,
    pub network: N,
    pub initial_network_task: Option<T>,
    pub plan: Esp32s31ConnectedStaPlan,
    pub installed_security: Esp32s31StaInstalledSecurity,
    pub security: Esp32s31StaAttemptSecurity<'security>,
}

impl<'security, R, E, D, N, S>
    Esp32s31PreparedConnectedService<'security, R, E, StationNetworkResources<D, N, S>>
where
    N: StationNetworkLink,
{
    /// Start the IP stack exactly once and publish link-up for every later
    /// association while the complete radio/security handoff remains owned.
    ///
    /// The initializer may borrow runtime resource bindings (for example one
    /// static stack arena) but cannot consume or replace the runtime owner.
    pub fn start_network<T>(
        mut self,
        start: impl FnOnce(&mut R, D, &Esp32s31ConnectedStaPlan) -> (S, T),
    ) -> Esp32s31ConnectedNetworkStarted<'security, R, E, S, N, T> {
        let network = start_station_network(self.network, |device| {
            start(&mut self.runtime, device, &self.plan)
        });
        let (stack, network, initial_network_task) = network.into_parts();
        Esp32s31ConnectedNetworkStarted {
            runtime: self.runtime,
            epoch: self.epoch,
            stack,
            network,
            initial_network_task,
            plan: self.plan,
            installed_security: self.installed_security,
            security: self.security,
        }
    }
}

/// Named decomposition after plan validation has succeeded.
pub struct Esp32s31PreparedConnectedServiceParts<'security, R, E, N> {
    pub runtime: R,
    pub epoch: E,
    pub network: N,
    pub plan: Esp32s31ConnectedStaPlan,
    pub installed_security: Esp32s31StaInstalledSecurity,
    pub security: Esp32s31StaAttemptSecurity<'security>,
}

/// Configuration failure retaining the complete join handoff.
pub struct Esp32s31ConnectedServicePrepareFailure<'security, R, E, N> {
    pub error: Esp32s31ConnectedStaConfigError,
    resources: Esp32s31ConnectedServiceResources<'security, R, E, N>,
}

impl<'security, R, E, N> Esp32s31ConnectedServicePrepareFailure<'security, R, E, N> {
    pub fn into_resources(self) -> Esp32s31ConnectedServiceResources<'security, R, E, N> {
        self.resources
    }
}

/// Validate the selected VIF, associated peer and connected policy before any
/// hardware, network, key or reusable-storage owner moves independently.
#[allow(clippy::result_large_err)]
pub fn prepare_esp32s31_connected_service<
    const AGGREGATE_SLOTS: usize,
    const RX_REORDER_SLOTS: usize,
    R,
    E,
    N,
>(
    resources: Esp32s31ConnectedServiceResources<'_, R, E, N>,
) -> Result<
    Esp32s31PreparedConnectedService<'_, R, E, N>,
    Esp32s31ConnectedServicePrepareFailure<'_, R, E, N>,
> {
    let Esp32s31ConnectedServiceParts {
        runtime,
        epoch,
        network,
        interface,
        config,
        peer,
        installed_security,
        security,
    } = resources.into_parts();
    let installed_mode = installed_security.mode();
    let material_mode = security.mode();
    if installed_mode != material_mode {
        return Err(Esp32s31ConnectedServicePrepareFailure {
            error: Esp32s31ConnectedStaConfigError::SecurityModeMismatch {
                installed: installed_mode,
                material: material_mode,
            },
            resources: Esp32s31ConnectedServiceResources::new(
                runtime,
                epoch,
                network,
                interface,
                config,
                peer,
                installed_security,
                security,
            ),
        });
    }
    match Esp32s31ConnectedStaPort::prepare_for_interface_with_storage_and_security::<
        AGGREGATE_SLOTS,
        RX_REORDER_SLOTS,
    >(peer, config, interface, installed_mode)
    {
        Ok(plan) => Ok(Esp32s31PreparedConnectedService {
            runtime,
            epoch,
            network,
            plan,
            installed_security,
            security,
        }),
        Err(failure) => Err(Esp32s31ConnectedServicePrepareFailure {
            error: failure.error,
            resources: Esp32s31ConnectedServiceResources::new(
                runtime,
                epoch,
                network,
                interface,
                config,
                failure.peer,
                installed_security,
                security,
            ),
        }),
    }
}
