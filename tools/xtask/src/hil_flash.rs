//! `cargo hil flash`: flash an ELF to one board under a lease of that board,
//! journal it, and optionally capture its console for a bounded time.
//!
//! This is the manual cycle for images outside the HIL runner and the
//! ESP-IDF catalog, such as a chip's first no_std images. `espflash` writes
//! the chip's project bootloader from the catalog (`hil/bootloaders/<chip>`,
//! built against the pinned ESP-IDF), the partition table and the
//! application for the board's registered chip. The journal records the
//! application image `espflash save-image` derives from the ELF and the
//! bootloader's digest, so another owner sees what the board carries. The console capture ends at its deadline or at an expected
//! line, never with the lease, so the board is released promptly.
use std::{
    ffi::OsString,
    io::Write as _,
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc,
    time::{Duration, Instant},
};

use crate::{Context, Result};

#[derive(clap::Parser, Debug)]
#[command(name = "cargo hil flash", no_binary_name = true)]
pub(crate) struct FlashCli {
    /// Registered board name or MAC.
    #[arg(long, value_name = "NAME|MAC")]
    board: String,
    /// Name in the board journal; default: the ELF's file name.
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
    via: Via,
    elf: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, clap::ValueEnum)]
enum Via {
    Usb,
    Jtag,
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

pub fn run(
    ctx: &Context,
    request: oer_hil_arbiter::Request,
    args: &[OsString],
) -> Result<std::process::ExitCode> {
    use clap::Parser as _;
    let cli = FlashCli::try_parse_from(args)?;
    let monitor = cli
        .monitor
        .as_deref()
        .map(oer_hil_arbiter::parse_duration)
        .transpose()?;
    let elf = std::path::absolute(&cli.elf)?;
    let image = match cli.image {
        Some(image) => image,
        None => elf
            .file_name()
            .ok_or("the ELF path names no file")?
            .to_string_lossy()
            .into_owned(),
    };
    let arbiter = oer_hil_arbiter::Arbiter::open()?;
    let board = crate::firmware_catalog::resolve_board(&arbiter, &cli.board)?;
    let chip = board.chip.clone().ok_or_else(|| {
        format!(
            "board `{}` has no registered chip; run `cargo hil devices set {} --chip CHIP`",
            cli.board, board.mac
        )
    })?;
    let output = ctx
        .root
        .join("target/hil/flash")
        .join(board.mac.replace(':', ""));
    std::fs::create_dir_all(&output)?;

    // Build the chip's project bootloader and derive the application image
    // before queueing.
    let (bootloader, bootloader_sha256) = crate::firmware_catalog::bootloader(ctx, &chip)?;
    let application = output.join("application.bin");
    crate::process::run(
        Command::new(espflash())
            .args(["save-image", "--chip", &chip])
            .arg(&elf)
            .arg(&application),
    )?;
    let application_sha256 = crate::vendor_fetch::sha256(&application)?;
    // JTAG writes one image from offset 0: bootloader, partition table and
    // application merged.
    let merged = output.join("merged.bin");
    if cli.via == Via::Jtag {
        crate::process::run(
            Command::new(espflash())
                .args(["save-image", "--chip", &chip, "--merge", "--bootloader"])
                .arg(&bootloader)
                .arg(&elf)
                .arg(&merged),
        )?;
    }

    let request = oer_hil_arbiter::Request {
        work: format!("flash {} --board {}", elf.display(), cli.board),
        claims: std::iter::once(oer_hil_arbiter::Claim::board(&board.mac))
            .chain(cli.air.0.map(|mode| oer_hil_arbiter::Claim {
                resource: oer_hil_arbiter::AIR.to_owned(),
                mode,
            }))
            .collect(),
        ..request
    };
    let _grant = arbiter.acquire(&request)?;
    let _device = oer_esp32s31_firmware::device::DeviceLease::acquire(&board.port)?;
    // Over JTAG the console is opened first, without touching the reset
    // lines, so the capture starts at the boot the programming ends with.
    let quiet = match cli.via {
        Via::Usb => {
            crate::process::run(
                Command::new(espflash())
                    .args([
                        "flash",
                        "--non-interactive",
                        "--chip",
                        &chip,
                        "--after",
                        "no-reset",
                        "--port",
                    ])
                    .arg(&board.port)
                    .arg("--bootloader")
                    .arg(&bootloader)
                    .arg(&elf),
            )?;
            None
        }
        Via::Jtag => {
            let serial = open_without_reset(&board.port)?;
            crate::hil_jtag::program(&chip, &board.mac, &[(0, merged.clone())])?;
            Some(serial)
        }
    };
    let (commit, dirty) = crate::firmware_catalog::source_revision(&ctx.root, Path::new("."));
    arbiter.record_board_by(
        request.owner.clone(),
        Some(board.mac.clone()),
        oer_hil_arbiter::BoardEventKind::Flashed {
            image: image.clone(),
            application_sha256,
            commit,
            dirty,
            origin: format!(
                "cargo hil flash {}, bootloader {}",
                elf.display(),
                &bootloader_sha256[..12]
            ),
        },
    )?;
    eprintln!("hil-arbiter: recorded {image} on {}", board.mac);

    let serial = match quiet {
        Some(serial) => serial,
        None => reset_into_application(&board.port)?,
    };
    let Some(duration) = monitor else {
        return Ok(std::process::ExitCode::SUCCESS);
    };
    let started = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?;
    let log = output.join(format!("console-{}.log", started.as_millis()));
    let seen = capture(serial_lines(serial), duration, cli.until.as_deref(), &log)?;
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

fn espflash() -> OsString {
    std::env::var_os("ESPFLASH").unwrap_or_else(|| "espflash".into())
}

/// The stand's board-port openers, re-exported for the stand commands.
pub(crate) use oer_hil_runner_core::session::reset::{open_without_reset, reset_into_application};

/// The lines `serial` receives, until the receiver is dropped.
pub(crate) fn serial_lines(mut serial: Box<dyn serialport::SerialPort>) -> mpsc::Receiver<Vec<u8>> {
    let (lines, received) = mpsc::channel();
    std::thread::spawn(move || {
        let mut pending = Vec::new();
        let mut buffer = [0; 4096];
        loop {
            match serial.read(&mut buffer) {
                Ok(0) => {}
                Ok(read) => pending.extend_from_slice(&buffer[..read]),
                Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
                Err(_) => return,
            }
            while let Some(end) = pending.iter().position(|&byte| byte == b'\n') {
                let mut line = pending.drain(..=end).collect::<Vec<_>>();
                line.pop();
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                if lines.send(line).is_err() {
                    return;
                }
            }
            // A dropped receiver is noticed at the next line; stop polling an
            // idle console once nobody listens.
            if lines.send(Vec::new()).is_err() {
                return;
            }
        }
    });
    received
}

/// Copy `lines` to the terminal and `log` until `duration` passes or a line
/// contains `until`; returns whether it did. Empty messages only keep the
/// source alive and are not lines.
pub(crate) fn capture(
    lines: mpsc::Receiver<Vec<u8>>,
    duration: Duration,
    until: Option<&str>,
    log: &Path,
) -> Result<bool> {
    let mut file = std::fs::File::create(log)?;
    let deadline = Instant::now() + duration;
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        match lines.recv_timeout(remaining) {
            Ok(line) if line.is_empty() => {}
            Ok(line) => {
                file.write_all(&line)?;
                file.write_all(b"\n")?;
                let text = String::from_utf8_lossy(&line);
                println!("{text}");
                if until.is_some_and(|until| text.contains(until)) {
                    return Ok(true);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                break;
            }
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &[&str], keep_open: Duration) -> mpsc::Receiver<Vec<u8>> {
        let (sender, received) = mpsc::channel();
        let text = text
            .iter()
            .map(|line| line.as_bytes().to_vec())
            .collect::<Vec<_>>();
        std::thread::spawn(move || {
            for line in text {
                sender.send(Vec::new()).unwrap();
                sender.send(line).unwrap();
            }
            std::thread::sleep(keep_open);
        });
        received
    }

    #[test]
    fn the_capture_ends_at_the_expected_line_or_its_deadline() {
        let directory = tempfile::tempdir().unwrap();
        let log = directory.path().join("console.log");
        let started = Instant::now();
        let seen = capture(
            lines(&["boot", "READY 1", "later"], Duration::from_secs(30)),
            Duration::from_secs(20),
            Some("READY"),
            &log,
        )
        .unwrap();
        assert!(seen);
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "boot\nREADY 1\n");

        let started = Instant::now();
        let seen = capture(
            lines(&["boot"], Duration::from_secs(30)),
            Duration::from_millis(300),
            Some("READY"),
            &log,
        )
        .unwrap();
        assert!(!seen);
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "boot\n");
    }

    #[test]
    fn the_command_names_board_elf_and_capture() {
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
        assert_eq!(cli.elf, Path::new("app.elf"));
        assert_eq!(cli.air, Air(Some(oer_hil_arbiter::Mode::Shared)));
        let quiet =
            FlashCli::try_parse_from(["--board", "esp32c5", "--air", "none", "app.elf"]).unwrap();
        assert_eq!(quiet.air, Air(None));
        assert!(FlashCli::try_parse_from(["--board", "esp32c5", "--until", "X", "a.elf"]).is_err());
    }
}
