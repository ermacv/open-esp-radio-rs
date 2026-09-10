//! Independent passive capture on a dedicated, initially idle OpenWrt PHY.
//! Uses the same event-driven capture owner and air decoder as other fixtures.
use super::{
    local_air_monitor,
    openwrt_capture::{RemoteCapture, ssh_target},
};
use crate::{
    Result,
    lab::config::{LabConfig, StationFixtureConfig},
};
use oer_process::CommandExt as _;
use std::{fs, net::Ipv4Addr, path::Path, time::Duration};

pub(crate) struct Capture {
    remote: RemoteCapture,
    target_mac: String,
    setup_only: bool,
}

impl Capture {
    pub(crate) fn start(
        lab: &LabConfig,
        target: Option<Ipv4Addr>,
        duration: Duration,
        output: &Path,
    ) -> Result<Option<Self>> {
        let Some(observer) = &lab.air_observer else {
            return Ok(None);
        };
        let StationFixtureConfig::OpenWrt(ap) = &lab.station_fixture else {
            return Err("independent OpenWrt observer requires a managed AP".into());
        };
        let boot = |target: &str| -> Result<String> {
            let result =
                ssh_target(target, "cat /proc/sys/kernel/random/boot_id").supervised_output()?;
            if !result.status.success() {
                return Err("cannot identify independent observer hardware".into());
            }
            let id = String::from_utf8(result.stdout)?.trim().to_owned();
            if id.is_empty() {
                return Err("empty hardware boot identity".into());
            }
            Ok(id)
        };
        let ap_boot = boot(&ap.ssh_target)?;
        let observer_boot = boot(&observer.ssh_target)?;
        if ap_boot == observer_boot {
            return Err(
                "independent observer must be a different host from the transmitting AP".into(),
            );
        }
        let geometry = local_air_monitor::resolve_observer_action(ap)?;
        let target_mac = target
            .map(|address| super::openwrt_fixture::resolve_station_mac(ap, address))
            .transpose()?
            .unwrap_or_else(|| "02:00:00:00:00:01".into());
        let filter = if target.is_some() {
            format!("wlan host {target_mac} or type ctl")
        } else {
            "type mgt".into()
        };
        let remote = RemoteCapture::start_independent(
            observer,
            geometry,
            &filter,
            output.join("independent-openwrt-air.pcap"),
            duration,
        )?;
        fs::write(
            output.join("independent-openwrt-fixture.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema": 1, "ap_boot": ap_boot, "observer_boot": observer_boot,
                "phy": observer.phy, "interface": observer.interface, "target_mac": target_mac,
                "frequency": geometry.frequency, "width_mhz": geometry.width, "center_frequency": geometry.center,
                "readiness": "tcpdump listening and verified channel", "initial_phy_state": "no interfaces",
            }))?,
        )?;
        Ok(Some(Self {
            remote,
            target_mac,
            setup_only: target.is_none(),
        }))
    }

    pub(crate) fn finish(mut self) -> Result<()> {
        let result = self.remote.finish_capture();
        let path = self.remote.output_path().with_extension("json");
        match result {
            Ok((captured, dropped)) => {
                let mut evidence =
                    local_air_monitor::parse_capture(self.remote.output_path(), &self.target_mac)?;
                evidence.captured_frames = captured;
                evidence.kernel_dropped = dropped;
                fs::write(path, serde_json::to_vec_pretty(&evidence)?)?;
                if captured == 0 && !self.setup_only {
                    return Err("independent OpenWrt observer captured no frames".into());
                }
                Ok(())
            }
            Err(error) => {
                fs::write(
                    path,
                    serde_json::to_vec_pretty(
                        &serde_json::json!({"failure": error.to_string(), "complete": false}),
                    )?,
                )?;
                Err(error)
            }
        }
    }
}
