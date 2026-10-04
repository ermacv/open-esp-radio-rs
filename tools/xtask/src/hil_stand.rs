//! `cargo hil stand discover|doctor`: the stand file against the host.
//!
//! `discover` maps every Espressif USB Serial/JTAG device `uhubctl` reports
//! to its hub, port and touch button and compares the result with the stand
//! file: boards in place, moved, missing (with why the port is empty) and
//! new, each new one with a `[[board]]` fragment to paste. It never changes
//! the file. `doctor` checks the host around it: the stand file, `uhubctl`
//! without sudo through the repository's udev rule, and NetworkManager
//! leaving `wlan0` to the fixtures.

use std::{ffi::OsString, path::Path, time::Duration};

use oer_hil_stand_schema::{Hub, StandFile};

use crate::{Context, Result};

/// The USB vendor:product of an Espressif chip's USB Serial/JTAG port.
const ESPRESSIF_SERIAL_JTAG: &str = "303a:1001";
/// How long `--blink` keeps a port off.
const BLINK: Duration = Duration::from_secs(5);
/// Where the repository's host files are installed.
const UDEV_RULE: &str = "/etc/udev/rules.d/52-oer-uhubctl.rules";
const NETWORKMANAGER_CONF: &str = "/etc/NetworkManager/conf.d/oer-unmanaged.conf";

#[derive(clap::Parser)]
#[command(name = "cargo hil stand", no_binary_name = true)]
pub(crate) enum StandCli {
    /// Map each attached board to its hub port and button and compare the
    /// result with the stand file, which it never changes.
    Discover {
        /// Switch HUB:PORT (a stand-file hub id and port) off for five
        /// seconds under a lease, to see which button it is.
        #[arg(long, value_name = "HUB:PORT", conflicts_with = "verify_power")]
        blink: Option<String>,
        /// Cycle BOARD's hub port under its lease and require the ROM to
        /// report a power-on reset.
        #[arg(long, value_name = "BOARD")]
        verify_power: Option<String>,
    },
    /// Check the host around the stand file: the file itself, `uhubctl`
    /// without sudo and NetworkManager leaving `wlan0` alone.
    Doctor,
}

pub(crate) fn stand(
    ctx: &Context,
    owner: impl FnOnce() -> Result<String>,
    args: &[OsString],
) -> Result<std::process::ExitCode> {
    use clap::Parser as _;
    let path = oer_hil_stand_schema::default_path()?;
    match StandCli::try_parse_from(args)? {
        StandCli::Discover {
            blink: Some(target),
            ..
        } => {
            let stand = StandFile::load(&path)?;
            let (hub, port) = blink_target(&stand, &target)?;
            blink(&stand, hub, port, owner()?)?;
            println!("{}:{port} was off for {} s", hub.id, BLINK.as_secs());
        }
        StandCli::Discover {
            verify_power: Some(board),
            ..
        } => {
            let line = crate::hil_board::power_reset_line(owner()?, &board)?;
            match line.as_deref() {
                Some(line) if line.contains("POWERON") => {
                    println!("{board} lost its power: {line}");
                }
                other => {
                    return Err(format!(
                        "{board}'s hub port cycled, but its ROM reported no power-on reset: {}",
                        other.unwrap_or("no reset line")
                    )
                    .into());
                }
            }
        }
        StandCli::Discover { .. } => {
            let stand = StandFile::load(&path)?;
            let report = uhubctl_report()?;
            print!(
                "{}",
                describe(&stand, &discover(&stand, &parse_uhubctl(&report)))
            );
        }
        StandCli::Doctor => {
            let checks = doctor(ctx, &path);
            let mut failed = false;
            for check in &checks {
                match &check.failure {
                    None => println!("PASS {}", check.name),
                    Some(failure) => {
                        failed = true;
                        println!("FAIL {}: {failure}", check.name);
                    }
                }
            }
            if failed {
                return Ok(std::process::ExitCode::FAILURE);
            }
        }
    }
    Ok(std::process::ExitCode::SUCCESS)
}

/// One hub section of `uhubctl`'s report.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HubStatus {
    pub location: String,
    pub ports: Vec<PortStatus>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PortStatus {
    pub port: u8,
    pub powered: bool,
    pub connected: bool,
    /// The attached device's `vid:pid` and its USB serial number, if any.
    pub device: Option<(String, Option<String>)>,
}

/// `uhubctl`'s report: per hub its ports' power and connection state and
/// the device each holds.
pub(crate) fn parse_uhubctl(report: &str) -> Vec<HubStatus> {
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
            let mut words = device.split_whitespace();
            let id = words.next()?.to_owned();
            // An Espressif USB Serial/JTAG port reports its MAC as the last
            // word; other devices' descriptions end otherwise.
            let serial = device
                .split_whitespace()
                .last()
                .filter(|last| oer_hil_arbiter::normalize_mac(last).is_ok())
                .map(str::to_owned);
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

/// Where a stand-file hub port is, as discovery names it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Place {
    /// The stand-file hub, or the hub's USB location when the stand file
    /// does not describe it.
    pub hub: String,
    pub port: u8,
    pub button: Option<u8>,
}

/// What discovery found for one board or port.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Finding {
    /// A board of the stand file on its own port.
    InPlace { board: String, place: Place },
    /// A board of the stand file on another port than the file says.
    Moved {
        board: String,
        expected: Place,
        found: Place,
    },
    /// A board of the stand file no hub reports.
    Missing {
        board: String,
        expected: Place,
        why: Absence,
    },
    /// An Espressif board the stand file does not describe.
    New {
        serial: String,
        place: Place,
        switchable: bool,
    },
}

/// Why a board's port holds no board.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Absence {
    /// The port is off.
    PortOff,
    /// The port is powered but empty: its button is off, or nothing is
    /// plugged in.
    Empty,
    /// The port holds another device.
    Occupied,
    /// No hub of the report is the board's hub.
    HubAbsent,
}

fn place(stand: &StandFile, location: &str, port: u8) -> Place {
    match stand.hub.iter().find(|hub| hub.usb2 == location) {
        Some(hub) => Place {
            hub: hub.id.clone(),
            port,
            button: hub.button(port),
        },
        None => Place {
            hub: location.to_owned(),
            port,
            button: None,
        },
    }
}

/// Compare the report with the stand file.
pub(crate) fn discover(stand: &StandFile, report: &[HubStatus]) -> Vec<Finding> {
    let attached = report
        .iter()
        .flat_map(|hub| hub.ports.iter().map(move |port| (hub, port)))
        .filter_map(|(hub, port)| match &port.device {
            Some((id, Some(serial))) if id == ESPRESSIF_SERIAL_JTAG => {
                Some((serial.clone(), hub.location.as_str(), port.port))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut findings = Vec::new();
    for board in &stand.board {
        let hub = stand.hub(&board.port.hub);
        let expected = Place {
            hub: board.port.hub.clone(),
            port: board.port.port,
            button: hub.and_then(|hub| hub.button(board.port.port)),
        };
        match attached.iter().find(|(serial, ..)| board.is_serial(serial)) {
            Some((_, location, port)) => {
                let found = place(stand, location, *port);
                findings.push(
                    if found.hub == expected.hub && found.port == expected.port {
                        Finding::InPlace {
                            board: board.id.clone(),
                            place: found,
                        }
                    } else {
                        Finding::Moved {
                            board: board.id.clone(),
                            expected,
                            found,
                        }
                    },
                );
            }
            None => {
                let status = hub.and_then(|hub| {
                    report
                        .iter()
                        .find(|status| status.location == hub.usb2)?
                        .ports
                        .iter()
                        .find(|port| port.port == board.port.port)
                });
                let why = match (hub, status) {
                    (Some(_), Some(port)) if !port.powered => Absence::PortOff,
                    (Some(_), Some(port)) if port.connected => Absence::Occupied,
                    (Some(_), Some(_)) => Absence::Empty,
                    _ => Absence::HubAbsent,
                };
                findings.push(Finding::Missing {
                    board: board.id.clone(),
                    expected,
                    why,
                });
            }
        }
    }
    for (serial, location, port) in attached {
        if stand.board_by_serial(&serial).is_none() {
            let switchable = stand
                .hub
                .iter()
                .find(|hub| hub.usb2 == location)
                .is_some_and(|hub| hub.is_switchable(port));
            findings.push(Finding::New {
                serial,
                place: place(stand, location, port),
                switchable,
            });
        }
    }
    findings
}

fn at(place: &Place) -> String {
    match place.button {
        Some(button) => format!("{}:{} (button {button})", place.hub, place.port),
        None => format!("{}:{}", place.hub, place.port),
    }
}

/// The findings as discovery prints them.
pub(crate) fn describe(stand: &StandFile, findings: &[Finding]) -> String {
    let mut out = String::new();
    for finding in findings {
        out.push_str(&match finding {
            Finding::InPlace { board, place } => format!("ok      {board} on {}\n", at(place)),
            Finding::Moved {
                board,
                expected,
                found,
            } => format!(
                "moved   {board} on {}; the stand file says {}: update its `port`\n",
                at(found),
                at(expected)
            ),
            Finding::Missing {
                board,
                expected,
                why,
            } => format!(
                "missing {board} on {}: {}\n",
                at(expected),
                match (why, expected.button) {
                    (Absence::PortOff, _) => String::from("the port is off"),
                    (Absence::Empty, Some(button)) => {
                        format!("button {button} is off or the board is not plugged in")
                    }
                    (Absence::Empty, None) => String::from("the port is empty"),
                    (Absence::Occupied, _) => String::from("another device is on the port"),
                    (Absence::HubAbsent, _) => String::from("its hub is not attached"),
                }
            ),
            Finding::New {
                serial,
                place,
                switchable,
            } => {
                let port = if stand.hub(&place.hub).is_some() {
                    format!("{{ hub = \"{}\", port = {} }}", place.hub, place.port)
                } else {
                    format!(
                        "{{ hub = \"?\", port = {} }}  # hub {} is not in the stand file",
                        place.port, place.hub
                    )
                };
                format!(
                    "new     {serial} on {}; add it to the stand file:\n\n\
                     [[board]]\n\
                     id = \"?\"  # the name written on the board\n\
                     usb-serial = \"{serial}\"\n\
                     chip = \"?\"\n\
                     radios = []\n\
                     roles = [\"dut\", \"peer\"]\n\
                     port = {port}\n\
                     reset = [{}]\n\n",
                    at(place),
                    if *switchable {
                        "\"jtag\", \"power\""
                    } else {
                        "\"jtag\""
                    }
                )
            }
        });
    }
    out
}

/// The stand-file hub and port `--blink HUB:PORT` names: a switchable port,
/// never one a cascaded hub hangs on.
pub(crate) fn blink_target<'a>(stand: &'a StandFile, target: &str) -> Result<(&'a Hub, u8)> {
    let (hub, port) = target
        .split_once(':')
        .ok_or_else(|| format!("`{target}` is not HUB:PORT"))?;
    let port: u8 = port
        .parse()
        .map_err(|_| format!("`{port}` is not a port number"))?;
    let hub = stand
        .hub(hub)
        .ok_or_else(|| format!("hub `{hub}` is not in the stand file"))?;
    if hub.is_protected(port) {
        return Err(format!(
            "{}:{port} is protected: a cascaded hub hangs on it, and switching it drops every board behind it",
            hub.id
        )
        .into());
    }
    if !hub.is_switchable(port) {
        return Err(format!(
            "{}:{port} is not switchable: `uhubctl` does not cut its power",
            hub.id
        )
        .into());
    }
    Ok((hub, port))
}

/// Switch a port off for [`BLINK`] under a lease of the port and of the
/// board on it.
fn blink(stand: &StandFile, hub: &Hub, port: u8, owner: String) -> Result<()> {
    let arbiter = oer_hil_arbiter::Arbiter::open()?;
    let mut claims = vec![oer_hil_arbiter::Claim::exclusive(format!(
        "hub-port:{}:{port}",
        hub.usb2
    ))];
    if let Some(board) = stand
        .board
        .iter()
        .find(|board| board.port.hub == hub.id && board.port.port == port)
    {
        claims.push(oer_hil_arbiter::Claim::board(
            &oer_hil_arbiter::normalize_mac(&board.usb_serial)?,
        ));
    }
    let _grant = arbiter.acquire(&oer_hil_arbiter::Request {
        owner,
        work: format!("stand discover --blink {}:{port}", hub.id),
        scenarios: Vec::new(),
        claims,
    })?;
    oer_hil_arbiter::control::PowerControl {
        via: oer_hil_arbiter::control::PowerVia::Uhubctl,
        location: hub.usb2.clone(),
        port: u32::from(port),
    }
    .cycle_holding(BLINK)
}

fn uhubctl_report() -> Result<String> {
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
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// One check of the stand doctor.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct Check {
    pub name: &'static str,
    pub failure: Option<String>,
}

fn check(name: &'static str, result: Result<()>) -> Check {
    Check {
        name,
        failure: result.err().map(|error| error.to_string()),
    }
}

fn doctor(ctx: &Context, path: &Path) -> Vec<Check> {
    let stand = StandFile::load(path);
    let stand_check = match &stand {
        Ok(stand) => oer_chip_profile::Profile::all(&ctx.root)
            .map_err(|error| error.to_string().into())
            .and_then(|chips| stand.validate_chips(&chips).map_err(Into::into)),
        Err(error) => Err(error.to_string().into()),
    };
    let mut checks = vec![check("stand file", stand_check)];
    checks.push(check(
        "udev rule",
        same_file(
            &ctx.root.join("hil/stand/udev/52-oer-uhubctl.rules"),
            Path::new(UDEV_RULE),
        )
        .map_err(|error| format!("{error}; install it with sudo and reload udev").into()),
    ));
    checks.push(check(
        "uhubctl without sudo",
        match &stand {
            Ok(stand) => uhubctl_reaches(stand),
            Err(_) => Err("needs the stand file's hubs".into()),
        },
    ));
    checks.push(check(
        "NetworkManager configuration",
        same_file(
            &ctx.root.join("hil/stand/networkmanager/oer-unmanaged.conf"),
            Path::new(NETWORKMANAGER_CONF),
        )
        .map_err(|error| format!("{error}; install it with sudo and reload NetworkManager").into()),
    ));
    checks.push(check("wlan0 unmanaged", wlan0_unmanaged()));
    checks
}

/// Whether `installed` holds the repository's `source`.
pub(crate) fn same_file(source: &Path, installed: &Path) -> Result<()> {
    let wanted = std::fs::read(source)?;
    match std::fs::read(installed) {
        Ok(found) if found == wanted => Ok(()),
        Ok(_) => Err(format!("{} differs from {}", installed.display(), source.display()).into()),
        Err(_) => Err(format!("{} is not installed", installed.display()).into()),
    }
}

/// `uhubctl` reads every stand hub without sudo.
fn uhubctl_reaches(stand: &StandFile) -> Result<()> {
    let report = uhubctl_report()?;
    let hubs = parse_uhubctl(&report);
    let unreached = stand
        .hub
        .iter()
        .filter(|hub| !hubs.iter().any(|status| status.location == hub.usb2))
        .map(|hub| format!("{} ({})", hub.id, hub.usb2))
        .collect::<Vec<_>>();
    if unreached.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "uhubctl does not report {}: not attached, or not readable without sudo",
            unreached.join(", ")
        )
        .into())
    }
}

fn wlan0_unmanaged() -> Result<()> {
    let output = oer_process::output(
        std::process::Command::new("nmcli").args(["-t", "-f", "DEVICE,STATE", "device"]),
        Some(Duration::from_secs(10)),
    )?;
    wlan0_state(&String::from_utf8_lossy(&output.stdout))
}

/// `wlan0`'s state in `nmcli -t -f DEVICE,STATE device`: it must be
/// unmanaged, or absent from NetworkManager.
pub(crate) fn wlan0_state(report: &str) -> Result<()> {
    match report
        .lines()
        .find_map(|line| line.strip_prefix("wlan0:"))
        .map(str::trim)
    {
        None | Some("unmanaged") => Ok(()),
        Some(state) => Err(format!("NetworkManager manages wlan0 ({state})").into()),
    }
}

#[cfg(test)]
mod tests;
