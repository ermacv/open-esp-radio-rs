//! Uniform session evidence for a managed station access-point fixture.

use std::{net::Ipv4Addr, time::Duration};

use crate::{
    Result,
    lab_config::StationFixtureConfig,
    local_linux_fixture::{LocalLinuxRxCapture, LocalLinuxRxEvidence},
    openwrt_fixture::{OpenWrtRxCapture, OpenWrtRxEvidence},
    scenario::PhyExpectation,
};

pub(crate) enum RxCapture {
    LocalLinux(LocalLinuxRxCapture),
    OpenWrt(OpenWrtRxCapture),
}

#[derive(Debug)]
pub(crate) enum RxEvidence {
    LocalLinux(LocalLinuxRxEvidence),
    OpenWrt(OpenWrtRxEvidence),
}

impl RxCapture {
    pub(crate) fn start(
        fixture: &StationFixtureConfig,
        target: Ipv4Addr,
        port: u16,
        duration: Duration,
        phy: PhyExpectation,
    ) -> Result<Option<Self>> {
        match fixture {
            StationFixtureConfig::LocalLinux(config) => Ok(Some(Self::LocalLinux(
                LocalLinuxRxCapture::start(config, target, port, duration, phy)?,
            ))),
            StationFixtureConfig::OpenWrt(config) => Ok(Some(Self::OpenWrt(
                OpenWrtRxCapture::start(config, target, port, duration, phy)?,
            ))),
            StationFixtureConfig::External(_) => Ok(None),
        }
    }

    pub(crate) fn finish(self) -> Result<RxEvidence> {
        match self {
            Self::LocalLinux(capture) => capture.finish().map(RxEvidence::LocalLinux),
            Self::OpenWrt(capture) => capture.finish().map(RxEvidence::OpenWrt),
        }
    }
}

impl RxEvidence {
    pub(crate) const fn wireless_packets(&self) -> u64 {
        match self {
            Self::LocalLinux(evidence) => evidence.udp_packets,
            Self::OpenWrt(evidence) => evidence.wireless_packets,
        }
    }

    pub(crate) fn markdown(&self) -> String {
        match self {
            Self::LocalLinux(evidence) => format!(
                "- Local Linux AP filtered UDP / station/interface TX packets: `{}` / `{}` / `{}`; retries/failed: `{}` / `{}`; channel width: `{}` MHz; TX duration: `{}` us; TX/RX bitrate: `{}` / `{}`\n",
                evidence.udp_packets,
                evidence.station_tx_packets,
                evidence.interface_tx_packets,
                evidence.station_tx_retries,
                evidence.station_tx_failed,
                evidence.channel_width_mhz,
                evidence.station_tx_duration_micros,
                evidence.tx_bitrate,
                evidence.rx_bitrate,
            ),
            Self::OpenWrt(evidence) => format!(
                "- OpenWrt filtered Ethernet ingress (diagnostic) / Wi-Fi egress (exact): `{}` / `{}`; interface RX/TX: `{}` / `{}`; station TX/retries/failed: `{}` / `{}` / `{}`; pre-air TID-0 AQM drops: `{}`; channel width: `{}` MHz\n",
                evidence.ingress_packets,
                evidence.wireless_packets,
                evidence.ingress_interface_rx_packets,
                evidence.wireless_interface_tx_packets,
                evidence.station_tx_packets,
                evidence.station_tx_retries,
                evidence.station_tx_failed,
                evidence.station_tid0_aqm_drops,
                evidence.channel_width_mhz,
            ),
        }
    }
}
