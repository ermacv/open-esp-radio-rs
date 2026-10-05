//! One board of the stand as the host reaches it: its port, its flash
//! writer, its start, its reset ladder and its power.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use oer_chip_profile::Start;
use oer_hil_stand_model::{BoardRef, ResetStep, StandFile};

use crate::{
    console,
    flash::{self, After, Segment},
    openocd::Openocd,
    power::{HubPower, PowerCycle},
    reset::{self, RecoveryStep, ResetPath, Rung},
};

/// How long a reset's console is read for the ROM's line.
const BANNER: Duration = Duration::from_secs(2);
/// How long a board's port may take to return after a reset.
const REATTACH: Duration = Duration::from_secs(10);
/// How long a returned port may take until the stand's user may open it.
const PORT_ACCESS: Duration = Duration::from_secs(3);

/// How a write reaches the board's flash.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Via {
    /// espflash and the USB Serial/JTAG reset lines.
    Usb,
    /// OpenOCD and the chip's debug module, which works over any running
    /// image; it resets into the written image.
    Jtag,
}

/// A board of the stand file.
#[derive(Clone, Debug)]
pub struct Board {
    pub reference: BoardRef,
    /// Where the board was last seen; a reset or power cycle can move it.
    port: PathBuf,
    /// The chip as espflash names it.
    espflash_chip: String,
    reset: Vec<ResetStep>,
    power: Option<HubPower>,
}

impl Board {
    /// `board` of `stand`, last seen at `port`, with its chip's profile from
    /// the repository at `root`.
    pub fn new(
        root: &Path,
        stand: &StandFile,
        board: &oer_hil_stand_model::Board,
        port: PathBuf,
    ) -> crate::Result<Self> {
        let profile = oer_chip_profile::Profile::load(root, &board.chip)?;
        Ok(Self {
            reference: board.reference()?,
            port,
            espflash_chip: profile.espflash_chip,
            reset: board.reset.clone(),
            power: stand.hub_port(board).map(HubPower::new),
        })
    }

    /// The attached board of `stand` that `query` names (an id, a chip with
    /// one board, or a MAC), at its `/dev/serial/by-id` port.
    pub fn attached(root: &Path, stand: &StandFile, query: &str) -> crate::Result<Self> {
        let board = stand.resolve(query)?;
        let port = crate::ports::port_of(&board.mac()?)?;
        Self::new(root, stand, board, port)
    }

    pub fn mac(&self) -> &str {
        &self.reference.mac
    }

    pub fn chip(&self) -> &str {
        &self.reference.chip
    }

    /// Where the board was last seen.
    pub fn port(&self) -> &Path {
        &self.port
    }

    /// The board's port now: where it was while that exists, else its port
    /// once it is attached again within `within`.
    pub fn current_port(&self, within: Duration) -> Option<PathBuf> {
        if self.port.exists() {
            return Some(self.port.clone());
        }
        crate::ports::wait_for(self.mac(), within)
    }

    /// Whether the board resets by its hub port's power.
    pub fn has_power(&self) -> bool {
        self.power.is_some()
    }

    /// Write `bundle`'s segments into the board's flash through `via`; the
    /// start the image still needs ([`Self::start`]): the bundle's start
    /// policy after a USB write, none after OpenOCD's program, which resets
    /// into the image. A board that is not on USB, because its image switched
    /// its USB Serial/JTAG off, is put into its ROM's download mode first
    /// through its hub port's power.
    pub fn write(&self, bundle: &oer_image::ImageBundle, via: Via) -> crate::Result<Start> {
        if bundle.chip != self.reference.chip {
            return Err(format!(
                "board `{}` is an {}; the bundle is an {} image",
                self.reference.id, self.reference.chip, bundle.chip
            )
            .into());
        }
        match via {
            Via::Usb => {
                let port = self.flashable_port()?;
                flash::write(
                    &port,
                    &self.espflash_chip,
                    &Segment::of(bundle)?,
                    After::of(bundle.flash.start),
                )?;
                Ok(bundle.flash.start)
            }
            Via::Jtag => {
                let files = bundle
                    .segments()
                    .into_iter()
                    .map(|segment| (segment.offset, segment.file))
                    .collect::<Vec<_>>();
                Openocd::locate()?.program(self.chip(), self.mac(), &files)?;
                Ok(Start::Reset)
            }
        }
    }

    /// Start the image a write left as `start` needs: nothing after the
    /// writer's own reset, else a power-on reset where the board resets by
    /// power and an RTS reset where it does not.
    pub fn start(&self, start: Start) -> crate::Result<()> {
        match (start, &self.power) {
            (Start::Reset, _) => Ok(()),
            (Start::PowerOn, Some(_)) => self.power_on(),
            (Start::PowerOn, None) => {
                let port = self
                    .current_port(REATTACH)
                    .ok_or("the board's port is gone")?;
                drop(reset::reset_into_application(&port)?);
                Ok(())
            }
        }
    }

    /// The port a write goes through: the board's port, or, when the board
    /// is not on USB, its port after a download entry.
    fn flashable_port(&self) -> crate::Result<PathBuf> {
        if let Some(port) = self.current_port(Duration::ZERO) {
            return Ok(port);
        }
        if self.power.is_none() {
            return Err(format!(
                "board `{}` is not on USB and does not reset by power; a person must reset it",
                self.reference.id
            )
            .into());
        }
        let banner = self.download_entry()?;
        eprintln!(
            "hil: board `{}` was not on USB; its ROM now waits for the flash: {}",
            self.reference.id,
            reset::reset_line(&banner)
                .as_deref()
                .unwrap_or("no reset line")
        );
        self.current_port(REATTACH)
            .ok_or_else(|| "the board's port did not return after its download entry".into())
    }

    /// The rungs of the board's reset ladder, in the stand file's order.
    pub fn rungs(&self) -> Vec<Box<dyn Rung + '_>> {
        self.reset
            .iter()
            .filter_map(|step| -> Option<Box<dyn Rung + '_>> {
                match step {
                    ResetStep::UsbJtagRts => Some(Box::new(Rts(self))),
                    ResetStep::Jtag => Some(Box::new(Jtag(self))),
                    ResetStep::Power => self.power.as_ref().map(|_| Box::new(Power(self)) as _),
                }
            })
            .collect()
    }

    /// Reset the board through `path` and return the ROM's reset line it
    /// printed: the rung of that kind, whether or not the board's ladder
    /// lists it.
    pub fn reset(&self, path: ResetPath) -> crate::Result<Option<String>> {
        let banner = match path {
            ResetPath::Rts => Rts(self).reset()?,
            ResetPath::Jtag => Jtag(self).reset()?,
            ResetPath::Power => {
                self.require_power()?;
                Power(self).reset()?
            }
            ResetPath::Download => Some(self.download_entry()?),
        };
        // A chip whose USB Serial/JTAG port re-enumerates prints its banner
        // before the port returns: read the console again once it is back.
        Ok(banner
            .as_deref()
            .and_then(reset::reset_line)
            .or_else(|| reset::reset_line(&self.console(BANNER))))
    }

    /// The board's console read for `watch` without resetting it, once its
    /// port is back.
    pub fn console(&self, watch: Duration) -> String {
        self.current_port(REATTACH)
            .and_then(|port| reset::open_without_reset(&port).ok())
            .map(|serial| console::read_for(serial, watch))
            .unwrap_or_default()
    }

    /// Open the board's console without a reset, retrying while the port is
    /// still held by the reader of a previous reset or returning after one.
    pub fn open_console(&self) -> crate::Result<crate::port::Port> {
        let port = self
            .current_port(REATTACH)
            .ok_or("the board's port is gone")?;
        retrying(PORT_ACCESS, || reset::open_without_reset(&port)).map_err(Into::into)
    }

    fn require_power(&self) -> crate::Result<&HubPower> {
        self.power.as_ref().ok_or_else(|| {
            "the board does not reset by power; add `power` to its `reset` in the stand file".into()
        })
    }

    /// Power the board off and on, watching it leave USB and return.
    pub fn power_cycle_observed(&self) -> crate::Result<PowerCycle> {
        self.require_power()?.cycle_observed(self.mac())
    }

    /// Power the board off and on and wait until its port is back and open
    /// to the stand's user: a power-on reset into the image in its flash.
    pub fn power_on(&self) -> crate::Result<()> {
        self.power_cycle_observed()?.verdict()?;
        let port = self
            .current_port(REATTACH)
            .ok_or("the board's port did not return after its power")?;
        // udev hands the returned port to the stand's group a moment after
        // it appears.
        retrying(PORT_ACCESS, || reset::open_without_reset(&port))
            .map(drop)
            .map_err(|_| format!("{} stayed closed after its power", port.display()).into())
    }

    /// Power the board off and on and, as soon as its USB returns, reset it
    /// into its ROM's download mode through the USB Serial/JTAG; the console
    /// the ROM printed. Only a board that resets by power has it.
    pub fn download_entry(&self) -> crate::Result<String> {
        self.power_cycle_observed()?.verdict()?;
        let port = self
            .current_port(REATTACH)
            .ok_or("the board's port did not return after its power")?;
        // udev hands the returned port to the stand's group a moment after
        // it appears: retry the open briefly, then reset at once.
        let serial = retrying(PORT_ACCESS, || reset::reset_into_download(&port))?;
        Ok(console::read_for(serial, BANNER))
    }

    /// [`Self::download_entry`] when the board resets by power.
    pub fn download_entry_if_powered(&self) -> Option<impl Fn() -> crate::Result<String> + '_> {
        self.power.as_ref().map(|_| move || self.download_entry())
    }
}

/// Retry `open` for `within`: a port that just returned, or that the reader
/// of a previous reset still holds, opens a moment later.
fn retrying<T, E>(
    within: Duration,
    mut open: impl FnMut() -> std::result::Result<T, E>,
) -> std::result::Result<T, E> {
    let deadline = std::time::Instant::now() + within;
    loop {
        match open() {
            Err(_) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            result => return result,
        }
    }
}

struct Rts<'a>(&'a Board);

impl Rung for Rts<'_> {
    fn step(&self) -> RecoveryStep {
        RecoveryStep::RtsReset
    }

    fn reset(&self) -> crate::Result<Option<String>> {
        let port = self
            .0
            .current_port(Duration::from_secs(5))
            .ok_or("the board's port is gone")?;
        let serial = retrying(PORT_ACCESS, || reset::reset_into_application(&port))?;
        Ok(Some(console::read_for(serial, BANNER)))
    }
}

struct Jtag<'a>(&'a Board);

impl Rung for Jtag<'_> {
    fn step(&self) -> RecoveryStep {
        RecoveryStep::JtagReset
    }

    /// The console is opened first, without touching the reset lines, so
    /// the read starts before the reset's banner.
    fn reset(&self) -> crate::Result<Option<String>> {
        let openocd = Openocd::locate()?;
        let serial = self
            .0
            .current_port(Duration::ZERO)
            .and_then(|port| reset::open_without_reset(&port).ok());
        openocd.reset(self.0.chip(), self.0.mac())?;
        Ok(serial.map(|serial| console::read_for(serial, BANNER)))
    }
}

struct Power<'a>(&'a Board);

impl Rung for Power<'_> {
    fn step(&self) -> RecoveryStep {
        RecoveryStep::PowerCycle
    }

    fn reset(&self) -> crate::Result<Option<String>> {
        self.0.require_power()?.cycle()?;
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stand() -> StandFile {
        StandFile::parse(include_str!("../../../stand/stand.example.toml")).unwrap()
    }

    fn board(id: &str) -> Board {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let stand = stand();
        Board::new(
            &root,
            &stand,
            stand.board(id).unwrap(),
            "/dev/absent".into(),
        )
        .unwrap()
    }

    #[test]
    fn the_rungs_follow_the_stand_file_s_ladder() {
        let s31 = board("s31-a");
        assert_eq!(
            s31.rungs()
                .iter()
                .map(|rung| rung.step())
                .collect::<Vec<_>>(),
            [
                RecoveryStep::RtsReset,
                RecoveryStep::JtagReset,
                RecoveryStep::PowerCycle
            ]
        );
        assert!(s31.has_power());
        assert_eq!(s31.espflash_chip, "esp32s31");
        let c5 = board("c5-a");
        assert_eq!(
            c5.rungs()
                .iter()
                .map(|rung| rung.step())
                .collect::<Vec<_>>(),
            [RecoveryStep::JtagReset, RecoveryStep::PowerCycle]
        );
    }

    #[test]
    fn a_bundle_of_another_chip_is_refused_before_the_port_is_touched() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let profile = oer_image::profile(&root, "esp32c5").unwrap();
        let directory = tempfile::tempdir().unwrap();
        let bundle =
            oer_image::ImageBundle::new(directory.path(), &profile, profile.flash.clone().unwrap());
        let error = board("s31-a")
            .write(&bundle, Via::Usb)
            .unwrap_err()
            .to_string();
        assert!(error.contains("is an esp32s31"), "{error}");
    }

    #[test]
    fn a_board_without_power_cannot_be_power_cycled() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let mut stand = stand();
        stand.board[1].reset = vec![ResetStep::Jtag];
        let c5 = Board::new(&root, &stand, &stand.board[1], "/dev/absent".into()).unwrap();
        assert!(!c5.has_power());
        assert!(c5.download_entry_if_powered().is_none());
        let error = c5.reset(ResetPath::Power).unwrap_err().to_string();
        assert!(error.contains("does not reset by power"), "{error}");
        // A flash of a board that is neither on USB nor powerable needs a person.
        let error = c5.flashable_port().unwrap_err().to_string();
        assert!(error.contains("a person must reset it"), "{error}");
    }
}
