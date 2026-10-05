//! A board's hub port power through `uhubctl`, and `uhubctl`'s report, read
//! by one parser.
//!
//! Only stand code switches a hub port: the arbiter returning a board's port
//! to its working state when a lease changes hands, a reset ladder's `power`
//! rung, the power-on start of a written image and `cargo stand
//! discover --blink`. A lease runs no `uhubctl` itself.
#![forbid(unsafe_code)]

use std::time::{Duration, Instant};

use oer_stand_file::HubPort;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// How long a board may take to leave USB once its port is off.
const POWER_LEAVE: Duration = Duration::from_secs(5);
/// How long a board may take to come back once its port is on.
const POWER_RETURN: Duration = Duration::from_secs(10);

/// The power of one switchable hub port.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HubPower {
    pub port: HubPort,
}

impl HubPower {
    pub fn new(port: HubPort) -> Self {
        Self { port }
    }

    /// Whether the port is powered.
    pub fn is_on(&self) -> crate::Result<bool> {
        let output = oer_process::output(
            std::process::Command::new("uhubctl")
                .args(["--location", &self.port.location, "--ports"])
                .arg(self.port.port.to_string()),
            Some(Duration::from_secs(30)),
        )?;
        powered(
            &parse(&String::from_utf8_lossy(&output.stdout)),
            &self.port.location,
            self.port.port,
        )
        .ok_or_else(|| {
            format!(
                "uhubctl reports no port {} of hub {}",
                self.port.port, self.port.location
            )
            .into()
        })
    }

    /// Power the port on.
    pub fn on(&self) -> crate::Result<()> {
        self.action("on", 2)
    }

    /// Power the port off and on again.
    pub fn cycle(&self) -> crate::Result<()> {
        self.action("cycle", 2)
    }

    /// Power the port off for `off`, then on again: long enough for a person
    /// to see which button's light goes out.
    pub fn cycle_holding(&self, off: Duration) -> crate::Result<()> {
        self.action("cycle", off.as_secs().max(1))
    }

    /// Power the port off and on again while watching the board with `mac`
    /// on it. The board must leave once the port is off and come back once
    /// it is on; its own USB device leaving shows that the board, not only
    /// the hub, lost power.
    pub fn cycle_observed(&self, mac: &oer_device_mac::DeviceId) -> crate::Result<PowerCycle> {
        self.action("off", 2)?;
        let left = wait_until(POWER_LEAVE, &|| !oer_device_discovery::is_attached(mac));
        // From the moment the port is told to power on.
        let started = Instant::now();
        self.action("on", 2)?;
        let returned = wait_until(POWER_RETURN, &|| oer_device_discovery::is_attached(mac))
            .then(|| started.elapsed());
        Ok(PowerCycle { left, returned })
    }

    fn action(&self, action: &str, delay_secs: u64) -> crate::Result<()> {
        let output = oer_process::output(
            std::process::Command::new("uhubctl")
                .args(["--location", &self.port.location, "--ports"])
                .arg(self.port.port.to_string())
                .args(["--action", action, "--delay"])
                .arg(delay_secs.to_string()),
            Some(Duration::from_secs(30 + delay_secs)),
        )?;
        if !output.status.success() {
            return Err(format!(
                "uhubctl could not {action} {} port {}: {}",
                self.port.location,
                self.port.port,
                String::from_utf8_lossy(&output.stderr).trim()
            )
            .into());
        }
        Ok(())
    }
}

/// What a watched power cycle of a board's port showed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PowerCycle {
    /// The board's own USB device left once the port was off.
    pub left: bool,
    /// How long after the port was on the board came back, if it did.
    pub returned: Option<Duration>,
}

impl PowerCycle {
    /// Whether the board lost its power and came back, or why not.
    pub fn verdict(self) -> std::result::Result<Duration, String> {
        match (self.left, self.returned) {
            (true, Some(after)) => Ok(after),
            (false, _) => Err(String::from(
                "the board stayed on USB while its port was off: the port does not cut its power",
            )),
            (true, None) => Err(format!(
                "the board left USB but did not return within {} s of its port's power",
                POWER_RETURN.as_secs()
            )),
        }
    }
}

/// Poll `condition` until it holds or `within` passes.
pub fn wait_until(within: Duration, condition: &dyn Fn() -> bool) -> bool {
    let deadline = Instant::now() + within;
    loop {
        if condition() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// One hub section of `uhubctl`'s report.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HubStatus {
    pub location: String,
    pub ports: Vec<PortStatus>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PortStatus {
    pub port: u8,
    pub powered: bool,
    pub connected: bool,
    /// The attached device's `vid:pid` and, when its description ends in
    /// one, its MAC (an Espressif USB Serial/JTAG port's serial number).
    pub device: Option<(String, Option<oer_device_mac::DeviceId>)>,
}

/// `uhubctl`'s report of every hub it reaches.
pub fn report() -> crate::Result<Vec<HubStatus>> {
    let output = oer_process::output(
        &mut std::process::Command::new("uhubctl"),
        Some(Duration::from_secs(30)),
    )?;
    if !output.status.success() {
        return Err(format!(
            "uhubctl failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    Ok(parse(&String::from_utf8_lossy(&output.stdout)))
}

/// `uhubctl`'s report: per hub its ports' power and connection state and
/// the device each holds.
pub fn parse(report: &str) -> Vec<HubStatus> {
    let mut hubs: Vec<HubStatus> = Vec::new();
    for line in report.lines() {
        if let Some(rest) = line.strip_prefix("Current status for hub ") {
            if let Some(location) = rest.split_whitespace().next() {
                hubs.push(HubStatus {
                    location: location.to_owned(),
                    ports: Vec::new(),
                });
            }
            continue;
        }
        let Some(hub) = hubs.last_mut() else {
            continue;
        };
        let Some(rest) = line.trim_start().strip_prefix("Port ") else {
            continue;
        };
        let Some((number, state)) = rest.split_once(':') else {
            continue;
        };
        let Ok(port) = number.trim().parse::<u8>() else {
            continue;
        };
        let (flags, device) = match state.split_once('[') {
            Some((flags, device)) => (flags, Some(device.trim_end().trim_end_matches(']'))),
            None => (state, None),
        };
        let words = flags.split_whitespace().collect::<Vec<_>>();
        let device = device.and_then(|device| {
            let id = device.split_whitespace().next()?.to_owned();
            // An Espressif USB Serial/JTAG port reports its MAC as the last
            // word; other devices' descriptions end otherwise.
            let serial = device
                .split_whitespace()
                .last()
                .and_then(|last| oer_device_mac::DeviceId::parse(last).ok());
            Some((id, serial))
        });
        hub.ports.push(PortStatus {
            port,
            powered: words.contains(&"power"),
            connected: words.contains(&"connect"),
            device,
        });
    }
    hubs
}

/// Whether port `port` of hub `location` is powered in `report`: that
/// hub's own section, not its USB 3 companion's.
pub fn powered(report: &[HubStatus], location: &str, port: u8) -> Option<bool> {
    report
        .iter()
        .find(|hub| hub.location == location)?
        .ports
        .iter()
        .find(|status| status.port == port)
        .map(|status| status.powered)
}

#[cfg(test)]
mod tests;
