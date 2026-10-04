//! The HIL stand file, `~/.config/open-esp-radio/stand.toml`: the stand's
//! USB hubs, its pool of boards and the fixture sections, with the rules a
//! valid file follows. A board is a pool member, identified by its USB
//! serial number (the MAC of an Espressif USB Serial/JTAG port); the role it
//! plays in a run comes from the firmware the run flashes, so no board is
//! named a device under test or a peer here. No `/dev/tty*` path is stored.

#![forbid(unsafe_code)]

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt, fs,
    path::{Path, PathBuf},
};

use oer_chip_profile::{BluetoothMode, Profile, WifiBand};
use serde::Deserialize;

/// The stand file schema this build reads.
pub const SCHEMA: u32 = 1;

/// The fixture sections the HIL host reads from the stand file as it read
/// them from the lab file before; their own types live with their owner.
pub const FIXTURE_SECTIONS: [&str; 6] = [
    "bluetooth",
    "station",
    "access_point",
    "station_fixture",
    "air_observer",
    "legacy_bss",
];

/// Why a stand file is unusable.
#[derive(Debug)]
pub struct Error(String);

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

fn error(message: impl Into<String>) -> Error {
    Error(message.into())
}

/// The stand file.
#[derive(Clone, Debug, Deserialize)]
pub struct StandFile {
    pub schema: u32,
    pub stand: Stand,
    #[serde(default)]
    pub hub: Vec<Hub>,
    #[serde(default)]
    pub board: Vec<Board>,
    /// The fixture sections ([`FIXTURE_SECTIONS`]), as tables their owner
    /// reads.
    #[serde(flatten)]
    pub fixtures: BTreeMap<String, toml::Value>,
}

/// The stand's identity and its policies.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stand {
    pub id: String,
    pub air: AirPolicy,
}

/// How runs share the air.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum AirPolicy {
    /// A run that uses the air holds all of it while it executes.
    Exclusive,
}

/// One USB hub with per-port power switching, by its USB 2 location (`3-8`)
/// and that of its USB 3 half.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Hub {
    pub id: String,
    pub usb2: String,
    pub usb3: Option<String>,
    /// Ports no command may switch: a cascaded hub hangs on them.
    #[serde(default)]
    pub protected: Vec<u8>,
    /// Ports whose power `uhubctl` really cuts.
    #[serde(default)]
    pub switchable: Vec<u8>,
    /// The hub's touch button of each port, counted from the top of the
    /// stand: port → button.
    #[serde(default)]
    pub buttons: BTreeMap<String, u8>,
}

impl Hub {
    pub fn is_protected(&self, port: u8) -> bool {
        self.protected.contains(&port)
    }

    pub fn is_switchable(&self, port: u8) -> bool {
        self.switchable.contains(&port)
    }

    /// The touch button of `port`, if the hub has one there.
    pub fn button(&self, port: u8) -> Option<u8> {
        self.buttons.get(&port.to_string()).copied()
    }
}

/// One board of the pool.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Board {
    /// The name written on the board.
    pub id: String,
    /// Its USB serial number: the MAC of its USB Serial/JTAG port.
    pub usb_serial: String,
    /// Its chip, one with a profile in `platform/<chip>/chip.toml`.
    pub chip: String,
    /// The radios the board carries; a subset of its chip's.
    pub radios: Vec<Radio>,
    /// The roles a run may give it.
    pub roles: Vec<BoardRole>,
    /// The hub port it hangs on.
    pub port: PortRef,
    /// How the stand resets it, in the order it tries them.
    pub reset: Vec<ResetStep>,
    /// An image the board's startup flash holds, when it needs one.
    pub startup_artifact: Option<PathBuf>,
    /// A USB-to-UART bridge whose modem lines drive the chip's EN and BOOT
    /// pins, when one is wired.
    pub uart_bridge: Option<UartBridge>,
}

/// A USB-to-UART bridge wired to a board's EN and BOOT pins.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct UartBridge {
    /// The bridge's USB serial number.
    pub serial: String,
    /// The modem line that pulls EN low.
    pub en: ModemLine,
    /// The modem line that pulls the boot strap low.
    pub boot: ModemLine,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ModemLine {
    Rts,
    Dtr,
}

impl Board {
    pub fn has_role(&self, role: BoardRole) -> bool {
        self.roles.contains(&role)
    }

    /// Whether `serial` is this board's USB serial number.
    pub fn is_serial(&self, serial: &str) -> bool {
        self.usb_serial.eq_ignore_ascii_case(serial)
    }

    /// Check the board's chip and radios against the chip profiles `chips`.
    pub fn validate_chip(&self, chips: &[Profile]) -> Result<()> {
        let name = &self.id;
        let profile = chips
            .iter()
            .find(|profile| profile.id == self.chip)
            .ok_or_else(|| {
                error(format!(
                    "board `{name}`: chip `{}` has no profile (platform/{}/chip.toml)",
                    self.chip, self.chip
                ))
            })?;
        match self
            .radios
            .iter()
            .find(|radio| !radio.on(&profile.properties))
        {
            Some(radio) => Err(error(format!(
                "board `{name}`: an {} has no {} radio",
                self.chip,
                radio.as_str()
            ))),
            None => Ok(()),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PortRef {
    pub hub: String,
    pub port: u8,
}

/// A radio a board carries.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(rename_all = "kebab-case")]
pub enum Radio {
    #[serde(rename = "wifi-2g4")]
    Wifi2g4,
    #[serde(rename = "wifi-5g")]
    Wifi5g,
    Ble,
    Ieee802154,
}

impl Radio {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Wifi2g4 => "wifi-2g4",
            Self::Wifi5g => "wifi-5g",
            Self::Ble => "ble",
            Self::Ieee802154 => "ieee802154",
        }
    }

    /// Whether a chip with `properties` has this radio.
    fn on(self, properties: &oer_chip_profile::Properties) -> bool {
        match self {
            Self::Wifi2g4 => properties.wifi_bands.contains(&WifiBand::Band2g4),
            Self::Wifi5g => properties.wifi_bands.contains(&WifiBand::Band5g),
            Self::Ble => properties.bluetooth.contains(&BluetoothMode::Le),
            Self::Ieee802154 => properties.ieee802154,
        }
    }
}

/// A role a run gives a board: what it flashes on it.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(rename_all = "kebab-case")]
pub enum BoardRole {
    /// Our image under test.
    Dut,
    /// A vendor (ESP-IDF) firmware the device under test works against.
    Peer,
}

impl BoardRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dut => "dut",
            Self::Peer => "peer",
        }
    }
}

/// One way to reset a board.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(rename_all = "kebab-case")]
pub enum ResetStep {
    /// RTS of the USB Serial/JTAG port.
    UsbJtagRts,
    /// A reset through the USB Serial/JTAG port's JTAG.
    Jtag,
    /// A power cycle of the board's hub port.
    Power,
}

impl ResetStep {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UsbJtagRts => "usb-jtag-rts",
            Self::Jtag => "jtag",
            Self::Power => "power",
        }
    }
}

/// `$XDG_CONFIG_HOME/open-esp-radio/stand.toml`, or `~/.config/...`.
pub fn default_path() -> Result<PathBuf> {
    let base = match std::env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
        Some(base) => PathBuf::from(base),
        None => PathBuf::from(
            std::env::var_os("HOME")
                .ok_or_else(|| error("HOME is required to find the stand file"))?,
        )
        .join(".config"),
    };
    Ok(base.join("open-esp-radio/stand.toml"))
}

impl StandFile {
    /// Parse a stand file without validating it.
    pub fn parse(source: &str) -> Result<Self> {
        toml::from_str(source).map_err(|error| Error(format!("invalid stand file: {error}")))
    }

    /// Read, parse and validate the stand file at `path`, which holds
    /// credentials and so must be private to its owner. The boards' radios
    /// are checked against their chips by [`Self::validate_chips`].
    pub fn load(path: &Path) -> Result<Self> {
        require_private(path)?;
        let source = fs::read_to_string(path).map_err(|cause| {
            Error(format!(
                "cannot read the stand file `{}`: {cause}",
                path.display()
            ))
        })?;
        let file =
            Self::parse(&source).map_err(|cause| Error(format!("{}: {cause}", path.display())))?;
        file.validate()
            .map_err(|cause| Error(format!("{}: {cause}", path.display())))?;
        Ok(file)
    }

    /// Check every rule of the stand file but the boards' chips.
    pub fn validate(&self) -> Result<()> {
        if self.schema != SCHEMA {
            return Err(error(format!(
                "stand file schema {}; this build reads schema {SCHEMA}",
                self.schema
            )));
        }
        identifier("stand.id", &self.stand.id)?;
        for section in self.fixtures.keys() {
            if !FIXTURE_SECTIONS.contains(&section.as_str()) {
                return Err(error(format!(
                    "unknown section [{section}]; the stand file has [stand], [[hub]], [[board]] and {}",
                    FIXTURE_SECTIONS.map(|name| format!("[{name}]")).join(", ")
                )));
            }
        }
        self.validate_hubs()?;
        self.validate_boards()
    }

    /// Check every board's chip and radios against the chip profiles
    /// `chips` (`platform/<chip>/chip.toml` of the checkout), as the stand's
    /// doctor does. A run checks only the boards it takes
    /// ([`Board::validate_chip`]), so a board whose chip has no profile yet
    /// keeps the rest of the stand usable.
    pub fn validate_chips(&self, chips: &[Profile]) -> Result<()> {
        self.board
            .iter()
            .try_for_each(|board| board.validate_chip(chips))
    }

    fn validate_hubs(&self) -> Result<()> {
        let mut ids = BTreeSet::new();
        let mut locations = BTreeSet::new();
        let mut buttons = BTreeSet::new();
        for hub in &self.hub {
            identifier("hub.id", &hub.id)?;
            if !ids.insert(hub.id.as_str()) {
                return Err(error(format!("hub `{}` is described twice", hub.id)));
            }
            for location in std::iter::once(&hub.usb2).chain(&hub.usb3) {
                if location.is_empty() || !locations.insert(location.as_str()) {
                    return Err(error(format!(
                        "hub `{}`: USB location `{location}` is empty or another hub's",
                        hub.id
                    )));
                }
            }
            for &port in hub.protected.iter().chain(&hub.switchable) {
                if port == 0 {
                    return Err(error(format!("hub `{}`: ports count from 1", hub.id)));
                }
            }
            if let Some(port) = hub
                .protected
                .iter()
                .find(|port| hub.switchable.contains(port))
            {
                return Err(error(format!(
                    "hub `{}`: port {port} is both protected and switchable",
                    hub.id
                )));
            }
            for (port, &button) in &hub.buttons {
                if !port.parse::<u8>().is_ok_and(|port| port > 0) {
                    return Err(error(format!(
                        "hub `{}`: button port `{port}` is not a port",
                        hub.id
                    )));
                }
                if button == 0 || !buttons.insert(button) {
                    return Err(error(format!(
                        "hub `{}`: button {button} is zero or another port's",
                        hub.id
                    )));
                }
            }
        }
        Ok(())
    }

    fn validate_boards(&self) -> Result<()> {
        let mut ids = BTreeSet::new();
        let mut serials = BTreeSet::new();
        let mut ports = BTreeSet::new();
        for board in &self.board {
            identifier("board.id", &board.id)?;
            let name = &board.id;
            if !ids.insert(name.as_str()) {
                return Err(error(format!("board `{name}` is described twice")));
            }
            if board.usb_serial.is_empty() || !serials.insert(board.usb_serial.to_ascii_uppercase())
            {
                return Err(error(format!(
                    "board `{name}`: USB serial `{}` is empty or another board's",
                    board.usb_serial
                )));
            }
            identifier(&format!("board `{name}` chip"), &board.chip)?;
            distinct(name, "radios", &board.radios)?;
            distinct(name, "roles", &board.roles)?;
            distinct(name, "reset", &board.reset)?;
            let hub = self.hub(&board.port.hub).ok_or_else(|| {
                error(format!(
                    "board `{name}`: hub `{}` is not described",
                    board.port.hub
                ))
            })?;
            let port = board.port.port;
            if port == 0 || hub.is_protected(port) {
                return Err(error(format!(
                    "board `{name}`: port {port} of hub `{}` is not a board port",
                    hub.id
                )));
            }
            if board.reset.contains(&ResetStep::Power) && !hub.is_switchable(port) {
                return Err(error(format!(
                    "board `{name}`: resets by power, but port {port} of hub `{}` is not switchable",
                    hub.id
                )));
            }
            if let Some(bridge) = &board.uart_bridge
                && (bridge.en == bridge.boot
                    || bridge.serial.is_empty()
                    || self.board_by_serial(&bridge.serial).is_some())
            {
                return Err(error(format!(
                    "board `{name}`: the UART bridge needs its own serial and different EN and BOOT lines"
                )));
            }
            if !ports.insert((hub.id.as_str(), port)) {
                return Err(error(format!(
                    "board `{name}`: port {port} of hub `{}` holds another board",
                    hub.id
                )));
            }
        }
        Ok(())
    }

    pub fn hub(&self, id: &str) -> Option<&Hub> {
        self.hub.iter().find(|hub| hub.id == id)
    }

    pub fn board(&self, id: &str) -> Option<&Board> {
        self.board.iter().find(|board| board.id == id)
    }

    pub fn board_by_serial(&self, serial: &str) -> Option<&Board> {
        self.board.iter().find(|board| board.is_serial(serial))
    }

    /// A fixture section, as its owner reads it.
    pub fn fixture(&self, section: &str) -> Option<&toml::Value> {
        self.fixtures.get(section)
    }

    /// The board of `chip` a run gives `role`: `chosen` when the run names
    /// one, otherwise the only such board other than `excluding` (the board
    /// another role of the run already took). Several candidates need a
    /// choice until the stand's scheduler assigns boards.
    pub fn select_board(
        &self,
        chip: &str,
        role: BoardRole,
        chosen: Option<&str>,
        excluding: Option<&str>,
    ) -> Result<&Board> {
        if let Some(chosen) = chosen {
            let board = self
                .board(chosen)
                .ok_or_else(|| error(format!("board `{chosen}` is not in the stand file")))?;
            if board.chip != chip || !board.has_role(role) {
                return Err(error(format!(
                    "board `{chosen}` is an {} for {}; this run needs an {chip} for {}",
                    board.chip,
                    board
                        .roles
                        .iter()
                        .map(|role| role.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    role.as_str()
                )));
            }
            if Some(chosen) == excluding {
                return Err(error(format!(
                    "board `{chosen}` already has another role in this run"
                )));
            }
            return Ok(board);
        }
        let candidates = self
            .board
            .iter()
            .filter(|board| {
                board.chip == chip && board.has_role(role) && Some(board.id.as_str()) != excluding
            })
            .collect::<Vec<_>>();
        match candidates.as_slice() {
            [board] => Ok(board),
            [] => Err(error(format!(
                "the stand file has no {chip} board for {}{}",
                role.as_str(),
                excluding.map_or(String::new(), |board| format!(" besides `{board}`"))
            ))),
            several => Err(error(format!(
                "several {chip} boards can be the {} ({}); name one",
                role.as_str(),
                several
                    .iter()
                    .map(|board| board.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))),
        }
    }
}

fn identifier(field: &str, value: &str) -> Result<()> {
    if oer_chip_profile::valid_identifier(value) {
        Ok(())
    } else {
        Err(error(format!(
            "{field} `{value}` must start with a lowercase letter and hold only lowercase letters, digits and hyphens"
        )))
    }
}

fn distinct<T: Ord + Copy>(board: &str, field: &str, values: &[T]) -> Result<()> {
    if values.is_empty() {
        return Err(error(format!("board `{board}`: {field} is empty")));
    }
    if values.iter().copied().collect::<BTreeSet<_>>().len() != values.len() {
        return Err(error(format!("board `{board}`: {field} repeats a value")));
    }
    Ok(())
}

#[cfg(unix)]
fn require_private(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(path)
        .map_err(|cause| {
            Error(format!(
                "cannot read the stand file `{}`: {cause}",
                path.display()
            ))
        })?
        .permissions()
        .mode();
    if mode & 0o077 != 0 {
        return Err(error(format!(
            "the stand file `{}` holds credentials; make it private to its owner (chmod 600)",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(not(unix))]
fn require_private(_: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests;
