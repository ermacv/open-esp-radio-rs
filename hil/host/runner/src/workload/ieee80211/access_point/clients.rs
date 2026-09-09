//! Scoped ownership of AP client fixtures.

use crate::execution::context::Context;
use std::net::Ipv4Addr;

use open_esp_radio_hil_protocol::WifiAccessPointSecurity;

use crate::workload::ieee80211::access_point::with_cleanup_errors;
use crate::{
    Result,
    fixture::{
        controlled_client::ControlledClient,
        controlled_openwrt_client::{ControlledOpenWrtClient, OpenWrtClientLinkObservation},
    },
    lab::config::StationFixtureConfig,
    scenario::AccessPointClient,
};

pub(super) enum ConnectedClients {
    Laptop {
        primary: ControlledClient,
        secondary: Option<ControlledOpenWrtClient>,
        management_capture:
            Option<Box<crate::fixture::openwrt_tx_monitor::OpenWrtManagementCapture>>,
    },
    OpenWrt {
        primary: ControlledOpenWrtClient,
    },
}

impl ConnectedClients {
    pub(super) fn openwrt_primary(&self) -> Option<&ControlledOpenWrtClient> {
        match self {
            Self::OpenWrt { primary } => Some(primary),
            Self::Laptop { .. } => None,
        }
    }

    pub(super) fn secondary(&self) -> Option<&ControlledOpenWrtClient> {
        match self {
            Self::Laptop { secondary, .. } => secondary.as_ref(),
            Self::OpenWrt { .. } => None,
        }
    }

    pub(super) fn traffic_target(&self, target: Ipv4Addr) -> Result<Ipv4Addr> {
        match self {
            Self::Laptop { .. } => Ok(target),
            Self::OpenWrt { primary } => primary.forward_address().ok_or_else(|| {
                "OpenWrt primary client omitted its wired forwarding address".into()
            }),
        }
    }

    pub(super) fn begin_primary_link_observation(
        &self,
    ) -> Result<Option<OpenWrtClientLinkObservation>> {
        self.openwrt_primary()
            .map(ControlledOpenWrtClient::begin_link_observation)
            .transpose()
    }

    pub(super) fn begin_secondary_link_observation(
        &self,
    ) -> Result<Option<OpenWrtClientLinkObservation>> {
        self.secondary()
            .map(ControlledOpenWrtClient::begin_link_observation)
            .transpose()
    }
}

pub(super) fn connect_clients(
    config: &super::Config,
    minimum_clients: u8,
    context: &Context<'_>,
    output: &std::path::Path,
) -> Result<ConnectedClients> {
    let client = config.client;
    let security = config.security;
    let openwrt_client_fixed_ht_mcs = config.openwrt_client_fixed_ht_mcs;
    let openwrt_client_fixed_guard_interval = config.openwrt_client_fixed_guard_interval;
    let timeout = config.timeout;
    let openwrt_fixture = || -> Result<&crate::lab::config::OpenWrtConfig> {
        match &context.lab.station_fixture {
            StationFixtureConfig::OpenWrt(fixture) => Ok(fixture),
            _ => Err("AP OpenWrt client requires the OpenWrt station fixture".into()),
        }
    };
    match client {
        AccessPointClient::OpenWrt => Ok(ConnectedClients::OpenWrt {
            primary: ControlledOpenWrtClient::connect_primary(
                &context.lab.access_point,
                openwrt_fixture()?,
                security,
                openwrt_client_fixed_ht_mcs,
                openwrt_client_fixed_guard_interval,
            )?,
        }),
        AccessPointClient::Laptop => {
            // Associate the observable OpenWrt peer first in two-client runs.
            // This gives debugfs evidence for the first BA bank and exercises
            // the laptop on the next independently allocated peer slot.
            let secondary = if minimum_clients >= 2 {
                Some(ControlledOpenWrtClient::connect(
                    &context.lab.access_point,
                    openwrt_fixture()?,
                    security,
                    openwrt_client_fixed_ht_mcs,
                    openwrt_client_fixed_guard_interval,
                )?)
            } else {
                None
            };
            if security == WifiAccessPointSecurity::Open {
                return Err("open AP qualification requires the controlled OpenWrt client".into());
            }
            let connect = || -> Result<_> {
                let capture = match &context.lab.station_fixture {
                    StationFixtureConfig::OpenWrt(config) if config.monitor_interface.is_some() => {
                        Some(Box::new(
                            crate::fixture::openwrt_tx_monitor::OpenWrtManagementCapture::start(
                                config, output, timeout,
                            )?,
                        ))
                    }
                    _ => None,
                };
                let result = ControlledClient::connect(
                    &context.lab.access_point,
                    &output.join("linux-client"),
                );
                match result {
                    Ok(primary) => Ok((primary, capture)),
                    Err(error) => {
                        let evidence = capture.map(|capture| capture.finish()).transpose().err();
                        Err(with_cleanup_errors(error, evidence, None, None, None))
                    }
                }
            };
            let (primary, management_capture) = match connect() {
                Ok(primary) => primary,
                Err(error) => {
                    let restore = secondary
                        .map(ControlledOpenWrtClient::restore)
                        .transpose()
                        .err();
                    return Err(with_cleanup_errors(error, restore, None, None, None));
                }
            };
            Ok(ConnectedClients::Laptop {
                primary,
                secondary,
                management_capture,
            })
        }
    }
}

pub(super) fn restore_clients(clients: ConnectedClients) -> Result<()> {
    match clients {
        ConnectedClients::OpenWrt { primary } => primary.restore(),
        ConnectedClients::Laptop {
            primary,
            secondary,
            management_capture,
        } => {
            let capture = management_capture
                .map(|capture| capture.finish())
                .transpose();
            let secondary = secondary.map(ControlledOpenWrtClient::restore).transpose();
            let primary = primary.restore();
            let restore = match (primary, secondary) {
                (Ok(()), Ok(_)) => Ok(()),
                (Err(primary), Ok(_)) => Err(primary),
                (Ok(()), Err(secondary)) => Err(secondary),
                (Err(primary), Err(secondary)) => Err(format!(
                    "primary client restore failed: {primary}; secondary client restore failed: {secondary}",
                )
                .into()),
            };
            match (restore, capture) {
                (Ok(()), Ok(_)) => Ok(()),
                (Err(error), Ok(_)) | (Ok(()), Err(error)) => Err(error),
                (Err(error), Err(capture)) => {
                    Err(format!("{error}; management capture failed: {capture}").into())
                }
            }
        }
    }
}
