//! One scoped OpenWrt AP profile, restored after the complete scenario.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    io::Write as _,
    process::{Command, Stdio},
    time::Duration,
};
use zeroize::Zeroizing;

use crate::Result;
use hil_core::{
    lab::config::OpenWrtConfig,
    lab::config::StationConfig,
    lab::link::{AccessPointSecurity, ManagementFrameProtection, PhyExpectation},
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Observation {
    pub enabled: bool,
    pub channel: u8,
    pub geometry: String,
    pub htmode: String,
    pub ht: bool,
    pub he: bool,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Profile {
    pub ht40_above: bool,
    pub phy: PhyExpectation,
    pub channel: u8,
    pub management_frame_protection: ManagementFrameProtection,
    pub access_point_security: AccessPointSecurity,
}

impl Profile {
    pub fn new(
        config: &OpenWrtConfig,
        phy: PhyExpectation,
        management_frame_protection: ManagementFrameProtection,
        access_point_security: AccessPointSecurity,
    ) -> Self {
        Self {
            ht40_above: config.ht40_above,
            phy,
            channel: config.channel,
            management_frame_protection,
            access_point_security,
        }
    }

    /// The OpenWrt `encryption` option.
    fn encryption(self) -> &'static str {
        match self.access_point_security {
            AccessPointSecurity::Wpa2Personal => "psk2",
            AccessPointSecurity::Wpa3Personal => "sae",
            AccessPointSecurity::Wpa3Transition => "sae-mixed",
        }
    }

    /// The OpenWrt `ieee80211w` option; with protection OpenWrt also offers
    /// PSK-SHA256 beside PSK. WPA3 requires protection, and its transition
    /// mode at least offers it.
    fn ieee80211w(self) -> &'static str {
        match (self.access_point_security, self.management_frame_protection) {
            (AccessPointSecurity::Wpa3Personal, _) | (_, ManagementFrameProtection::Required) => {
                "2"
            }
            (AccessPointSecurity::Wpa3Transition, _) | (_, ManagementFrameProtection::Optional) => {
                "1"
            }
            (AccessPointSecurity::Wpa2Personal, ManagementFrameProtection::Disabled) => "0",
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

    pub fn verify(self, observed: &Observation) -> Result<()> {
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
            || !(observed.htmode == self.htmode()
                || (self.phy == PhyExpectation::Ht40 && observed.htmode == "HT40"))
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
                "encryption": self.encryption(), "wmm": "1", "ieee80211w": self.ieee80211w(), "disabled": "0", "hidden": "0", "ifname": config.wireless_interface}
        })
    }
}

pub trait Backend {
    fn invoke(
        &self,
        operation: &str,
        options: &Value,
        up: bool,
        ap_enabled: Option<bool>,
    ) -> Result<Zeroizing<String>>;
    fn observe(&self) -> Result<Observation>;
}

pub struct Remote(OpenWrtConfig);
impl Backend for Remote {
    fn invoke(
        &self,
        operation: &str,
        options: &Value,
        up: bool,
        ap_enabled: Option<bool>,
    ) -> Result<Zeroizing<String>> {
        if self.0.read_only && !matches!(operation, "snapshot" | "observe" | "verify") {
            return Err("read-only OpenWrt fixture forbids AP mutation".into());
        }
        invoke(&self.0, operation, options, up, ap_enabled)
    }
    fn observe(&self) -> Result<Observation> {
        observe(&self.0)
    }
}

pub struct AccessPoint<B: Backend = Remote> {
    backend: B,
    profile: Profile,
    before: Zeroizing<String>,
    restored: bool,
    read_only: bool,
    pub applied: Observation,
}

impl AccessPoint {
    pub fn start(
        config: &OpenWrtConfig,
        station: &StationConfig,
        phy: PhyExpectation,
        management_frame_protection: ManagementFrameProtection,
        access_point_security: AccessPointSecurity,
    ) -> Result<Self> {
        let profile = Profile::new(
            config,
            phy,
            management_frame_protection,
            access_point_security,
        );
        probe(config, profile)?;
        if config.read_only {
            return Self::attach(
                Remote(config.clone()),
                profile,
                profile.options(config, station),
            );
        }
        Self::prepare(
            Remote(config.clone()),
            profile,
            profile.options(config, station),
        )
    }
}

impl AccessPoint {
    pub fn report(&self) -> Result<Value> {
        let before: Value = serde_json::from_str(&self.before)?;
        let radio = &before["options"][&self.backend.0.radio];
        Ok(json!({"schema": 1, "requested": self.profile,
            "read_only": self.read_only,
            "before": {"up": before["up"], "channel": radio["channel"], "htmode": radio["htmode"]},
            "applied": self.applied}))
    }
}

impl<B: Backend> AccessPoint<B> {
    fn attach(backend: B, profile: Profile, options: Value) -> Result<Self> {
        let before = backend.invoke("verify", &options, true, None)?;
        let applied = backend.observe()?;
        profile.verify(&applied)?;
        Ok(Self {
            backend,
            profile,
            before,
            restored: true,
            read_only: true,
            applied,
        })
    }

    fn prepare(backend: B, profile: Profile, options: Value) -> Result<Self> {
        let before = backend.invoke("snapshot", &options, true, None)?;
        refuse_uncommitted_changes(&serde_json::from_str(&before)?)?;
        // Establish ownership before the first mutation, including a failed SSH reply.
        let mut owner = Self {
            backend,
            profile,
            before,
            restored: false,
            read_only: false,
            applied: Observation {
                enabled: false,
                channel: 0,
                geometry: String::new(),
                htmode: String::new(),
                ht: false,
                he: false,
            },
        };
        owner.backend.invoke("apply", &options, true, None)?;
        owner.applied = owner.backend.observe()?;
        profile.verify(&owner.applied)?;
        Ok(owner)
    }

    pub fn stop(&mut self) -> Result<()> {
        if self.read_only {
            return Err("read-only OpenWrt AP cannot be stopped".into());
        }
        self.backend
            .invoke("state", &json!({}), false, None)
            .map(|_| ())
    }

    pub fn restart(&mut self) -> Result<()> {
        if self.read_only {
            return Err("read-only OpenWrt AP cannot be restarted".into());
        }
        self.backend.invoke("state", &json!({}), true, None)?;
        self.profile.verify(&self.backend.observe()?)
    }

    pub fn restore(&mut self) -> Result<()> {
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
        let after = self
            .backend
            .invoke("restore", &before["options"], up, Some(ap_enabled))?;
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
            hil_core::fixture::cleanup::record("restore OpenWrt scenario profile", || {
                self.restore()
            });
        }
    }
}

/// The scenario profile is applied as temporary UCI changes and restored by
/// reverting them, so the router's own configuration must be committed:
/// restoration cannot reproduce another owner's uncommitted change.
fn refuse_uncommitted_changes(snapshot: &Value) -> Result<()> {
    let pending = snapshot["pending"]
        .as_object()
        .ok_or("OpenWrt snapshot omitted its uncommitted UCI changes")?
        .iter()
        .flat_map(|(section, keys)| {
            keys.as_object()
                .into_iter()
                .flatten()
                .filter(|(_, pending)| pending.as_bool() == Some(true))
                .map(move |(key, _)| format!("{section}.{key}"))
        })
        .collect::<Vec<_>>();
    if pending.is_empty() {
        return Ok(());
    }
    Err(crate::fixture::Error::new(format!(
        "OpenWrt has uncommitted UCI changes: {}; run `uci revert wireless` on the router",
        pending.join(", ")
    ))
    .into())
}

pub fn observe(config: &OpenWrtConfig) -> Result<Observation> {
    let data = invoke(config, "observe", &json!({}), true, None)?;
    serde_json::from_str(&data).map_err(Into::into)
}

pub fn probe(config: &OpenWrtConfig, profile: Profile) -> Result<()> {
    use oer_process::CommandExt as _;
    let script = format!(
        "set -eu; command -v ucode >/dev/null; command -v iw >/dev/null; \
         phy=$(ubus call iwinfo phyname '{{\"section\":\"{}\"}}' | jsonfilter -e '@.phyname'); \
         test -n \"$phy\"; iw phy \"$phy\" info",
        config.radio
    );
    let output = ssh(config, &script)
        .supervised_output()
        .and_then(crate::fixture::Error::ssh_output)?;
    if !output.status.success() {
        return Err("cannot discover OpenWrt radio capabilities (ucode, iw, iwinfo and jsonfilter required)".into());
    }
    verify_capabilities(profile, &String::from_utf8(output.stdout)?)
}

fn verify_capabilities(profile: Profile, info: &str) -> Result<()> {
    crate::fixture::channel::verify_ap_capabilities(
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
    ap_enabled: Option<bool>,
) -> Result<Zeroizing<String>> {
    let request = json!({"operation": operation, "radio": config.radio,
        "ap_section": config.ap_section, "interface": config.wireless_interface,
        "options": options, "up": up, "ap_enabled": ap_enabled});
    let program = Zeroizing::new(format!(
        "let request = {};\n{}",
        request,
        include_str!("ap/remote.uc")
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
    let mut output = crate::fixture::Error::ssh_output(
        child.wait_with_output_timeout(Some(Duration::from_secs(30)))?,
    )?;
    if !output.status.success() {
        // Never include a ucode source excerpt containing the credential-bearing request.
        let error = String::from_utf8_lossy(&output.stderr);
        let message = error.lines().next().unwrap_or("remote operation failed");
        return Err(crate::fixture::Error::new(format!("OpenWrt {operation}: {message}")).into());
    }
    Ok(Zeroizing::new(String::from_utf8(std::mem::take(
        &mut output.stdout,
    ))?))
}

#[cfg(test)]
mod tests;
