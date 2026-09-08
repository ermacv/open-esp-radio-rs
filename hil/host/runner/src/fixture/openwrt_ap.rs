//! One scoped OpenWrt AP profile, restored after the complete scenario.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    io::Write as _,
    process::{Command, Stdio},
    time::Duration,
};
use zeroize::Zeroizing;

use crate::{
    Result,
    lab::config::{OpenWrtConfig, StationConfig},
    scenario::PhyExpectation,
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct Observation {
    pub(crate) enabled: bool,
    pub(crate) channel: u8,
    pub(crate) geometry: String,
    pub(crate) htmode: String,
    pub(crate) ht: bool,
    pub(crate) he: bool,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub(crate) struct Profile {
    pub(crate) ht40_above: bool,
    pub(crate) phy: PhyExpectation,
    pub(crate) channel: u8,
}

impl Profile {
    pub(crate) fn new(config: &OpenWrtConfig, phy: PhyExpectation) -> Self {
        Self {
            ht40_above: config.ht40_above,
            phy,
            channel: config.channel,
        }
    }

    fn htmode(self) -> &'static str {
        match self.phy {
            PhyExpectation::He20 => "HE20",
            PhyExpectation::Ht20 => "HT20",
            PhyExpectation::Ht40 if self.ht40_above => "HT40+",
            PhyExpectation::Ht40 => "HT40-",
        }
    }

    pub(crate) fn verify(self, observed: &Observation) -> Result<()> {
        let width = if self.phy == PhyExpectation::Ht40 {
            40
        } else {
            20
        };
        let frequency = 2407 + u16::from(self.channel) * 5;
        let center = match self.htmode() {
            "HT40+" => frequency + 10,
            "HT40-" => frequency - 10,
            _ => frequency,
        };
        let geometry = format!(
            "channel {} ({frequency} MHz), width: {width} MHz, center1: {center} MHz",
            self.channel
        );
        if !observed.enabled
            || observed.channel != self.channel
            || observed.htmode != self.htmode()
            || !observed.ht
            || observed.he != (self.phy == PhyExpectation::He20)
            || !observed
                .geometry
                .lines()
                .any(|line| line.trim() == geometry)
        {
            return Err(format!("OpenWrt profile was not applied: expected {} channel {} center {center} MHz; observed {observed:?}", self.htmode(), self.channel).into());
        }
        Ok(())
    }

    fn options(self, config: &OpenWrtConfig, station: &StationConfig) -> Value {
        let (ssid, passphrase) = station.credentials();
        json!({
            &config.radio: {"channel": self.channel.to_string(), "htmode": self.htmode(), "disabled": "0"},
            &config.ap_section: {"mode": "ap", "ssid": ssid, "key": passphrase,
                "encryption": "psk2", "wmm": "1", "ieee80211w": "0", "disabled": "0", "hidden": "0", "ifname": config.wireless_interface}
        })
    }
}

pub(crate) trait Backend {
    fn invoke(
        &self,
        operation: &str,
        options: &Value,
        up: bool,
        pending: &Value,
        ap_enabled: Option<bool>,
    ) -> Result<Zeroizing<String>>;
    fn observe(&self) -> Result<Observation>;
}

pub(crate) struct Remote(OpenWrtConfig);
impl Backend for Remote {
    fn invoke(
        &self,
        operation: &str,
        options: &Value,
        up: bool,
        pending: &Value,
        ap_enabled: Option<bool>,
    ) -> Result<Zeroizing<String>> {
        invoke(&self.0, operation, options, up, pending, ap_enabled)
    }
    fn observe(&self) -> Result<Observation> {
        observe(&self.0)
    }
}

pub(crate) struct AccessPoint<B: Backend = Remote> {
    backend: B,
    profile: Profile,
    before: Zeroizing<String>,
    restored: bool,
    pub(crate) applied: Observation,
}

impl AccessPoint {
    pub(crate) fn start(
        config: &OpenWrtConfig,
        station: &StationConfig,
        phy: PhyExpectation,
    ) -> Result<Self> {
        let profile = Profile::new(config, phy);
        probe(config, profile)?;
        Self::prepare(
            Remote(config.clone()),
            profile,
            profile.options(config, station),
        )
    }
}

impl AccessPoint {
    pub(crate) fn report(&self) -> Result<Value> {
        let before: Value = serde_json::from_str(&self.before)?;
        let radio = &before["options"][&self.backend.0.radio];
        Ok(json!({"schema": 1, "requested": self.profile,
            "before": {"up": before["up"], "channel": radio["channel"], "htmode": radio["htmode"]},
            "applied": self.applied}))
    }
}

impl<B: Backend> AccessPoint<B> {
    fn prepare(backend: B, profile: Profile, options: Value) -> Result<Self> {
        let before = backend.invoke("snapshot", &options, true, &Value::Null, None)?;
        // Establish ownership before the first mutation, including a failed SSH reply.
        let mut owner = Self {
            backend,
            profile,
            before,
            restored: false,
            applied: Observation {
                enabled: false,
                channel: 0,
                geometry: String::new(),
                htmode: String::new(),
                ht: false,
                he: false,
            },
        };
        owner
            .backend
            .invoke("apply", &options, true, &Value::Null, None)?;
        owner.applied = owner.backend.observe()?;
        profile.verify(&owner.applied)?;
        Ok(owner)
    }

    pub(crate) fn stop(&mut self) -> Result<()> {
        self.backend
            .invoke("state", &json!({}), false, &Value::Null, None)
            .map(|_| ())
    }

    pub(crate) fn restart(&mut self) -> Result<()> {
        self.backend
            .invoke("state", &json!({}), true, &Value::Null, None)?;
        self.profile.verify(&self.backend.observe()?)
    }

    pub(crate) fn restore(&mut self) -> Result<()> {
        if self.restored {
            return Ok(());
        }
        let before: Value = serde_json::from_str(&self.before)?;
        let up = before["up"]
            .as_bool()
            .ok_or("OpenWrt snapshot omitted radio state")?;
        let ap_enabled = before["ap_enabled"]
            .as_bool()
            .ok_or("OpenWrt snapshot omitted AP state")?;
        let after = self.backend.invoke(
            "restore",
            &before["options"],
            up,
            &before["pending"],
            Some(ap_enabled),
        )?;
        let after: Value = serde_json::from_str(&after)?;
        if after != before {
            // Name the mismatched contract without logging credential values.
            let mismatched = ["up", "ap_enabled", "options", "pending"]
                .into_iter()
                .filter(|key| after[key] != before[key])
                .collect::<Vec<_>>()
                .join(", ");
            return Err(
                format!("OpenWrt restoration differs from its snapshot in: {mismatched}").into(),
            );
        }
        self.restored = true;
        Ok(())
    }
}

impl<B: Backend> Drop for AccessPoint<B> {
    fn drop(&mut self) {
        if !self.restored {
            super::cleanup::record("restore OpenWrt scenario profile", || self.restore());
        }
    }
}

pub(crate) fn observe(config: &OpenWrtConfig) -> Result<Observation> {
    let data = invoke(config, "observe", &json!({}), true, &Value::Null, None)?;
    serde_json::from_str(&data).map_err(Into::into)
}

pub(crate) fn probe(config: &OpenWrtConfig, profile: Profile) -> Result<()> {
    use oer_process::CommandExt as _;
    let script = format!(
        "set -eu; command -v ucode >/dev/null; command -v iw >/dev/null; \
         phy=$(ubus call iwinfo phyname '{{\"section\":\"{}\"}}' | jsonfilter -e '@.phyname'); \
         test -n \"$phy\"; iw phy \"$phy\" info",
        config.radio
    );
    let output = ssh(config, &script)
        .supervised_output()
        .and_then(super::Error::ssh_output)?;
    if !output.status.success() {
        return Err("cannot discover OpenWrt radio capabilities (ucode, iw, iwinfo and jsonfilter required)".into());
    }
    verify_capabilities(profile, &String::from_utf8(output.stdout)?)
}

fn verify_capabilities(profile: Profile, info: &str) -> Result<()> {
    super::channel::verify_ap_capabilities(
        profile.channel,
        (profile.phy == PhyExpectation::Ht40).then_some(profile.ht40_above),
        profile.phy == PhyExpectation::He20,
        info,
    )
}

fn ssh(config: &OpenWrtConfig, script: &str) -> Command {
    let mut command = Command::new("ssh");
    command
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=5"])
        .arg(&config.ssh_target)
        .arg(script);
    command
}

fn invoke(
    config: &OpenWrtConfig,
    operation: &str,
    options: &Value,
    up: bool,
    pending: &Value,
    ap_enabled: Option<bool>,
) -> Result<Zeroizing<String>> {
    let request = json!({"operation": operation, "radio": config.radio,
        "ap_section": config.ap_section, "interface": config.wireless_interface,
        "options": options, "up": up, "pending": pending, "ap_enabled": ap_enabled});
    let program = Zeroizing::new(format!(
        "let request = {};\n{}",
        request,
        include_str!("openwrt_ap/remote.uc")
    ));
    let mut command = ssh(config, "ucode -");
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = oer_process::owned::Child::spawn(&mut command)?;
    child
        .stdin
        .take()
        .ok_or("SSH stdin unavailable")?
        .write_all(program.as_bytes())?;
    let mut output =
        super::Error::ssh_output(child.wait_with_output_timeout(Some(Duration::from_secs(30)))?)?;
    if !output.status.success() {
        // Never include a ucode source excerpt containing the credential-bearing request.
        let error = String::from_utf8_lossy(&output.stderr);
        let message = error.lines().next().unwrap_or("remote operation failed");
        return Err(super::Error::new(format!("OpenWrt {operation}: {message}")).into());
    }
    Ok(Zeroizing::new(String::from_utf8(std::mem::take(
        &mut output.stdout,
    ))?))
}

#[cfg(test)]
mod tests;
