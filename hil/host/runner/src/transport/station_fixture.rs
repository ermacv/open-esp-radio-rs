//! Uniform session evidence for a managed station access-point fixture.

use std::{net::Ipv4Addr, time::Duration};

use crate::{
    Result,
    qualification::scenario::PhyExpectation,
    transport::lab_config::StationFixtureConfig,
    transport::local_linux_fixture::{LocalLinuxRxCapture, LocalLinuxRxEvidence},
    transport::openwrt_fixture::{OpenWrtRxCapture, OpenWrtRxEvidence},
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
                "- OpenWrt filtered Ethernet ingress (diagnostic) / Wi-Fi egress (exact): `{}` / `{}`; interface RX/TX: `{}` / `{}`; station TX/retries/failed: `{}` / `{}` / `{}`; pre-air TID-0 AQM drops: `{}`; channel width: `{}` MHz; AP TX/RX bitrate: `{}` / `{}`\n",
                evidence.ingress_packets,
                evidence.wireless_packets,
                evidence.ingress_interface_rx_packets,
                evidence.wireless_interface_tx_packets,
                evidence.station_tx_packets,
                evidence.station_tx_retries,
                evidence.station_tx_failed,
                evidence.station_tid0_aqm_drops,
                evidence.channel_width_mhz,
                evidence.tx_bitrate,
                evidence.rx_bitrate,
            ),
        }
    }

    pub(crate) fn require_ht40_downlink(&self) -> Result<()> {
        match self {
            Self::LocalLinux(evidence) => require_ht40_mcs7(
                "STA RX/AP TX",
                evidence.channel_width_mhz,
                &evidence.tx_bitrate,
            ),
            Self::OpenWrt(evidence) => require_ht40_mcs7(
                "STA RX/AP TX",
                evidence.channel_width_mhz,
                &evidence.tx_bitrate,
            ),
        }
    }
}

/// Require the HT40 MCS7 ceiling family rather than comparing throughput
/// samples produced at different widths or MCS values. The fixture snapshot
/// is only the peer's most recent rate, so guard interval remains reported
/// evidence instead of a whole-interval precondition.
pub(crate) fn require_ht40_mcs7(
    direction: &str,
    interface_width_mhz: u8,
    bitrate: &str,
) -> Result<()> {
    let fields = bitrate.split_whitespace().collect::<Vec<_>>();
    let observed_mbps = fields
        .first()
        .ok_or_else(|| format!("{direction} link snapshot omitted bitrate"))?
        .parse::<f64>()
        .map_err(|error| format!("invalid {direction} bitrate `{bitrate}`: {error}"))?;
    let mcs = fields
        .windows(2)
        .find_map(|pair| (pair[0] == "MCS").then_some(pair[1]))
        .ok_or_else(|| format!("{direction} link snapshot omitted MCS: `{bitrate}`"))?
        .parse::<u8>()
        .map_err(|error| format!("invalid {direction} MCS in `{bitrate}`: {error}"))?;
    let vector_width_mhz = fields
        .iter()
        .find_map(|field| field.strip_suffix("MHz"))
        .ok_or_else(|| format!("{direction} link snapshot omitted channel width: `{bitrate}`"))?
        .parse::<u8>()
        .map_err(|error| format!("invalid {direction} channel width in `{bitrate}`: {error}"))?;
    let mcs7_rate =
        (134.9..=135.1).contains(&observed_mbps) || (149.9..=150.1).contains(&observed_mbps);
    if interface_width_mhz != 40 || vector_width_mhz != 40 || mcs != 7 || !mcs7_rate {
        return Err(format!(
            "{direction} link precondition failed: required=MCS 7 40MHz (135.0 MBit/s long GI or 150.0 MBit/s short GI) observed_interface_width={interface_width_mhz} observed=`{bitrate}`"
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::require_ht40_mcs7;

    #[test]
    fn ht40_ceiling_vector_requires_mcs7_and_width_but_accepts_either_gi() {
        require_ht40_mcs7("STA RX/AP TX", 40, "150.0 MBit/s MCS 7 40MHz short GI").unwrap();
        require_ht40_mcs7("STA RX/AP TX", 40, "135.0 MBit/s MCS 7 40MHz").unwrap();
        assert!(
            require_ht40_mcs7("STA RX/AP TX", 20, "150.0 MBit/s MCS 7 40MHz short GI",).is_err()
        );
        assert!(
            require_ht40_mcs7("STA RX/AP TX", 40, "135.0 MBit/s MCS 6 40MHz short GI",).is_err()
        );
        assert!(require_ht40_mcs7("STA RX/AP TX", 40, "120.0 MBit/s MCS 7 40MHz").is_err());
    }
}
