//! `cargo hil flash`: flash an image bundle, or an ELF, to one board through
//! the flash operation (`oer-hil-flash`: lease, write, journal, start), and
//! optionally capture its console for a bounded time.
//!
//! This is the manual cycle for images outside the HIL runner and the
//! ESP-IDF catalog: a bundle `cargo xtask build firmware` or `cargo hil
//! image build` made, or the ELF of an ESP-IDF-bootloader chip's first
//! no_std image, which `oer-image` bundles with the chip's project bootloader
//! from the catalog (`hil/bootloaders/<chip>`, built against the pinned
//! ESP-IDF). The journal records the bundle's application image, so another
//! owner sees what the board carries. The console capture ends at its
//! deadline or at an expected line, never with the lease, so the board is
//! released promptly.
use std::{ffi::OsString, path::PathBuf};

use crate::Result;
use oer_hil_board::{Via, console, reset};
use oer_process::Checkout;
#[derive(clap::Parser, Debug)]
#[command(name = "cargo hil flash", no_binary_name = true)]
pub(crate) struct FlashCli {
    /// Registered board name or MAC.
    #[arg(long, value_name = "NAME|MAC")]
    board: String,
    /// Name in the board journal; default: the bundle directory's or ELF's name.
    #[arg(long)]
    image: Option<String>,
    /// Capture the console for this long after flashing: 30s, 2m.
    #[arg(long, value_name = "DURATION")]
    monitor: Option<String>,
    /// End the capture at the first console line containing TEXT; the
    /// command fails when it never appears.
    #[arg(long, value_name = "TEXT", requires = "monitor")]
    until: Option<String>,
    /// Radio environment the image uses: `shared`, `exclusive` for RF
    /// measurements, or `none` for an image that never enables the radio,
    /// which then runs beside an exclusive air lease.
    #[arg(long, value_parser = parse_air, default_value = "shared")]
    air: Air,
    /// How to write and reset the board: `usb` through espflash and the USB
    /// Serial/JTAG reset lines, or `jtag` through OpenOCD and the chip's
    /// debug module, which works over any running image.
    #[arg(long, value_enum, default_value = "usb")]
    via: ViaArg,
    /// An image bundle's directory, or an ELF of an ESP-IDF-bootloader chip.
    #[arg(value_name = "BUNDLE|ELF")]
    image_path: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, clap::ValueEnum)]
enum ViaArg {
    Usb,
    Jtag,
}

impl ViaArg {
    fn via(self) -> Via {
        match self {
            Self::Usb => Via::Usb,
            Self::Jtag => Via::Jtag,
        }
    }
}

/// How an image uses the radio environment; `None` claims no air.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Air(Option<oer_hil_arbiter::Mode>);

fn parse_air(text: &str) -> std::result::Result<Air, String> {
    match text {
        "shared" => Ok(Air(Some(oer_hil_arbiter::Mode::Shared))),
        "exclusive" => Ok(Air(Some(oer_hil_arbiter::Mode::Exclusive))),
        "none" => Ok(Air(None)),
        _ => Err(String::from("use `shared`, `exclusive` or `none`")),
    }
}

pub fn run(ctx: &Checkout, owner: String, args: &[OsString]) -> Result<std::process::ExitCode> {
    use clap::Parser as _;
    let cli = FlashCli::try_parse_from(args)?;
    let monitor = cli
        .monitor
        .as_deref()
        .map(oer_hil_arbiter::parse_duration)
        .transpose()?;
    let source = std::path::absolute(&cli.image_path)?;
    let image = match cli.image {
        Some(image) => image,
        None => source
            .file_name()
            .ok_or("the image path names no file")?
            .to_string_lossy()
            .into_owned(),
    };
    let arbiter = oer_hil_arbiter::Arbiter::open()?;
    let board = oer_hil_board::Board::attached(&ctx.root, &arbiter.stand()?, &cli.board)?;
    let output = ctx
        .root
        .join("target/hil/flash")
        .join(oer_hil_stand_model::mac::compact(board.mac()));
    std::fs::create_dir_all(&output)?;

    // The bundle to write, made before queueing.
    let bundle = if source.is_dir() {
        oer_image::ImageBundle::load(&source)?
    } else {
        oer_image::esp_idf::catalog::bundle_elf(&ctx.root, board.chip(), &source, &output)?
    };
    let request = oer_hil_arbiter::Request {
        owner: owner.clone(),
        work: format!("flash {} --board {}", source.display(), cli.board),
        scenarios: Vec::new(),
        claims: std::iter::once(oer_hil_arbiter::Claim::board(board.mac()))
            .chain(cli.air.0.map(|mode| oer_hil_arbiter::Claim {
                resource: oer_hil_arbiter::AIR.to_owned(),
                mode,
            }))
            .collect(),
    };
    let lease = arbiter.lease_board(&request, board.mac())?;
    // Over JTAG the console is opened first, without touching the reset
    // lines, so the capture starts at the boot the programming ends with.
    let early = (cli.via == ViaArg::Jtag)
        .then(|| reset::open_without_reset(board.port()))
        .transpose()?;
    oer_hil_flash::flash(
        &arbiter,
        &owner,
        &lease.lock,
        &board,
        &oer_hil_flash::Image {
            bundle: &bundle,
            name: &image,
            revision: oer_hil_flash::Revision::of_checkout(&ctx.root, std::path::Path::new(".")),
            origin: format!("cargo hil flash {}", source.display()),
        },
        cli.via.via(),
    )?;
    eprintln!("hil-arbiter: recorded {image} on {}", board.mac());
    let Some(duration) = monitor else {
        return Ok(std::process::ExitCode::SUCCESS);
    };
    let serial = match early {
        Some(serial) => serial,
        None => board.open_console()?,
    };
    let log = output.join(format!("console-{}.log", oer_durable::unix_millis()));
    let seen = console::capture(console::lines(serial), duration, cli.until.as_deref(), &log)?;
    eprintln!("hil: console log {}", log.display());
    Ok(match (cli.until, seen) {
        (Some(text), false) => {
            eprintln!(
                "hil: `{text}` did not appear within {}",
                oer_hil_arbiter::format_duration(duration)
            );
            std::process::ExitCode::FAILURE
        }
        _ => std::process::ExitCode::SUCCESS,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_names_board_image_capture_and_path() {
        use clap::Parser as _;
        let cli = FlashCli::try_parse_from([
            "--board",
            "esp32c5",
            "--monitor",
            "30s",
            "--until",
            "READY",
            "app.elf",
        ])
        .unwrap();
        assert_eq!(cli.board, "esp32c5");
        assert_eq!(cli.image_path, PathBuf::from("app.elf"));
        assert_eq!(cli.air, Air(Some(oer_hil_arbiter::Mode::Shared)));
        assert_eq!(cli.via.via(), Via::Usb);
        let quiet = FlashCli::try_parse_from([
            "--board", "esp32c5", "--air", "none", "--via", "jtag", "app.elf",
        ])
        .unwrap();
        assert_eq!(quiet.air, Air(None));
        assert_eq!(quiet.via.via(), Via::Jtag);
        assert!(FlashCli::try_parse_from(["--board", "esp32c5", "--until", "X", "a.elf"]).is_err());
    }
}
