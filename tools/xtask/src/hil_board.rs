//! `cargo hil board reset|check|console` and `cargo hil peer send`: the
//! stand's own ways to reach a board's port.
//!
//! Opening a USB serial port with the default modem lines resets an
//! Espressif chip, and a UART bridge's lines drive EN and BOOT, so tools open
//! board ports only through these commands, which release RTS before DTR and
//! hold a lease of the board. Each command claims the board; `peer send`
//! also claims the air shared, since a peer command may transmit.

use std::{
    ffi::OsString,
    io::Write as _,
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant},
};

use crate::{Context, Result, hil_flash};

/// How long a reset's ROM banner is read.
const BANNER: Duration = Duration::from_secs(3);
/// How long a port that re-enumerates after a reset may take to return.
const REATTACH: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum Via {
    /// Pulse RTS on the chip's USB Serial/JTAG port, the boot strap released.
    Rts,
    /// Reset the CPU through the chip's JTAG with OpenOCD.
    Jtag,
    /// Pulse EN through the board's registered UART bridge.
    En,
}

/// `cargo hil board reset|check|console`.
#[derive(Debug, clap::Subcommand)]
pub(crate) enum BoardCommand {
    /// Reset a board and print the reset reason its ROM reports.
    Reset {
        #[arg(value_name = "NAME|MAC")]
        board: String,
        #[arg(long, value_enum, default_value = "rts")]
        via: Via,
        /// Hold the boot strap low: the ROM waits for a download (`--via en`).
        #[arg(long)]
        download: bool,
    },
    /// Report whether a board is attached and answers, what it runs and how
    /// the stand reaches it, without resetting it.
    Check {
        #[arg(value_name = "NAME|MAC")]
        board: String,
    },
    /// Print a board's console for a bounded time without resetting it.
    Console {
        #[arg(value_name = "NAME|MAC")]
        board: String,
        /// How long, e.g. 10s or 2m.
        #[arg(long = "for", default_value = "10s", value_parser = parse_duration)]
        duration: Duration,
        /// End at the first line containing this text; fail when none does.
        #[arg(long)]
        until: Option<String>,
    },
}

fn parse_duration(text: &str) -> std::result::Result<Duration, String> {
    oer_hil_arbiter::parse_duration(text).map_err(|error| error.to_string())
}

/// `cargo hil peer send BOARD LINE`.
#[derive(Debug, clap::Parser)]
#[command(name = "cargo hil peer", no_binary_name = true)]
pub(crate) enum PeerCli {
    /// Send one line to a peer's text protocol and print its answer, up to
    /// its `@OK` or `@ERR` line.
    Send {
        #[arg(value_name = "NAME|MAC")]
        board: String,
        /// The command line, e.g. `SYNC` or `CFG 15 4f45 ...`.
        line: Vec<String>,
        /// How long to wait for the answer.
        #[arg(long = "for", default_value = "5s", value_parser = parse_duration)]
        duration: Duration,
    },
}

struct Target {
    mac: String,
    port: PathBuf,
    chip: Option<String>,
    arbiter: oer_hil_arbiter::Arbiter,
}

fn target(board: &str) -> Result<Target> {
    let arbiter = oer_hil_arbiter::Arbiter::open()?;
    let resolved = crate::firmware_catalog::resolve_board(&arbiter, board)?;
    Ok(Target {
        mac: resolved.mac,
        port: resolved.port,
        chip: resolved.chip,
        arbiter,
    })
}

fn lease(
    target: &Target,
    owner: String,
    work: String,
    air: bool,
) -> Result<oer_hil_arbiter::Grant> {
    let claims = std::iter::once(oer_hil_arbiter::Claim::board(&target.mac))
        .chain(air.then(|| oer_hil_arbiter::Claim::shared(oer_hil_arbiter::AIR)))
        .collect();
    target.arbiter.acquire(&oer_hil_arbiter::Request {
        owner,
        work,
        scenarios: Vec::new(),
        claims,
    })
}

pub(crate) fn board(
    ctx: &Context,
    owner: String,
    command: BoardCommand,
) -> Result<std::process::ExitCode> {
    match command {
        BoardCommand::Reset {
            board,
            via,
            download,
        } => {
            let target = target(&board)?;
            if download && via != Via::En {
                return Err("--download needs --via en: only EN holds the boot strap".into());
            }
            let _grant = lease(
                &target,
                owner,
                format!("board reset {board} --via {via:?}"),
                false,
            )?;
            let line = reset(&target, via, download)?;
            println!(
                "{board} ({}) reset via {}: {}",
                target.mac,
                format!("{via:?}").to_lowercase(),
                line.as_deref()
                    .unwrap_or("no ROM reset line on its console")
            );
        }
        BoardCommand::Check { board } => {
            let target = target(&board)?;
            let _grant = lease(&target, owner, format!("board check {board}"), false)?;
            check(&target, &board)?;
        }
        BoardCommand::Console {
            board,
            duration,
            until,
        } => {
            let target = target(&board)?;
            let _grant = lease(&target, owner, format!("board console {board}"), false)?;
            let log = ctx
                .root
                .join("target/hil/console")
                .join(target.mac.replace(':', ""));
            std::fs::create_dir_all(&log)?;
            let log = log.join(format!("console-{}.log", unix_seconds()));
            let lines = hil_flash::serial_lines(hil_flash::open_without_reset(&target.port)?);
            let seen = hil_flash::capture(lines, duration, until.as_deref(), &log)?;
            eprintln!("hil: console of {board} written to {}", log.display());
            if until.is_some() && !seen {
                return Ok(std::process::ExitCode::FAILURE);
            }
        }
    }
    Ok(std::process::ExitCode::SUCCESS)
}

pub(crate) fn peer(owner: String, args: &[OsString]) -> Result<std::process::ExitCode> {
    use clap::Parser as _;
    let PeerCli::Send {
        board,
        line,
        duration,
    } = PeerCli::try_parse_from(args)?;
    let line = line.join(" ");
    if line.is_empty() {
        return Err("cargo hil peer send needs a command line".into());
    }
    let target = target(&board)?;
    let _grant = lease(&target, owner, format!("peer send {board} {line}"), true)?;
    let mut serial = hil_flash::open_without_reset(&target.port)?;
    // A fresh line ends whatever the peer's parser held.
    serial.write_all(format!("\n{line}\n").as_bytes())?;
    let lines = hil_flash::serial_lines(serial);
    let command = line.split_whitespace().next().unwrap_or_default();
    let answered = answer(&lines, command, duration, &mut |text| println!("{text}"));
    Ok(match answered {
        Some(true) => std::process::ExitCode::SUCCESS,
        Some(false) | None => std::process::ExitCode::FAILURE,
    })
}

/// Print `lines` until the `@OK` (true) or `@ERR` (false) answer to
/// `command`, or `None` after `duration`. An answer naming another command,
/// such as noise a peer read while it booted, is only printed.
fn answer(
    lines: &mpsc::Receiver<Vec<u8>>,
    command: &str,
    duration: Duration,
    print: &mut dyn FnMut(&str),
) -> Option<bool> {
    let deadline = Instant::now() + duration;
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        match lines.recv_timeout(remaining) {
            Ok(line) if line.is_empty() => {}
            Ok(line) => {
                let text = String::from_utf8_lossy(&line);
                print(&text);
                let mut words = text.split_whitespace();
                let (status, about) = (words.next(), words.next());
                if about.is_some_and(|about| about.eq_ignore_ascii_case(command)) {
                    match status {
                        Some("@OK") => return Some(true),
                        Some("@ERR") => return Some(false),
                        _ => {}
                    }
                }
            }
            Err(_) => return None,
        }
    }
    None
}

/// Reset the target and return its ROM's `rst:` line, reading the console
/// again when the reset made the port re-enumerate.
fn reset(target: &Target, via: Via, download: bool) -> Result<Option<String>> {
    let lines = match via {
        Via::Rts => hil_flash::serial_lines(hil_flash::reset_into_application(&target.port)?),
        Via::Jtag => {
            let lines = hil_flash::serial_lines(hil_flash::open_without_reset(&target.port)?);
            let chip = target.chip.as_deref().ok_or(
                "the board has no registered chip; `cargo hil devices set MAC --chip CHIP`",
            )?;
            crate::hil_jtag::reset(chip, &target.mac)?;
            lines
        }
        Via::En => {
            let control = target
                .arbiter
                .devices()?
                .into_iter()
                .find(|device| device.mac == target.mac)
                .and_then(|device| device.control?.reset)
                .ok_or("the board has no reset path; `cargo hil devices set --reset-uart`")?;
            let mode = if download {
                oer_hil_arbiter::BootMode::Download
            } else {
                oer_hil_arbiter::BootMode::Normal
            };
            let banner = control.reset(mode)?;
            return Ok(oer_hil_arbiter::control::reset_line(&banner).map(str::to_owned));
        }
    };
    if let Some(line) = rom_line(&lines, BANNER) {
        return Ok(Some(line));
    }
    // A chip whose USB Serial/JTAG port re-enumerates prints the banner
    // before the port returns.
    let deadline = Instant::now() + REATTACH;
    while Instant::now() < deadline {
        if let Ok(serial) = hil_flash::open_without_reset(&target.port) {
            return Ok(rom_line(&hil_flash::serial_lines(serial), BANNER));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Ok(None)
}

/// Whether `board` boots from flash after an RTS reset: its ROM reset line,
/// or why not. A quarantined board is checked without a lease, which nobody
/// else can hold.
pub(crate) fn boots(board: &str) -> Result<String> {
    let target = target(board)?;
    let line = reset(&target, Via::Rts, false)?
        .ok_or("no ROM reset line on its console after an RTS reset")?;
    if line.contains("DOWNLOAD") {
        return Err(format!("its ROM waits for a download: {line}").into());
    }
    Ok(line)
}

fn rom_line(lines: &mpsc::Receiver<Vec<u8>>, within: Duration) -> Option<String> {
    let deadline = Instant::now() + within;
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        match lines.recv_timeout(remaining) {
            Ok(line) => {
                let text = String::from_utf8_lossy(&line).trim().to_owned();
                if text.starts_with("rst:") && text.contains("boot:") {
                    return Some(text);
                }
            }
            Err(_) => return None,
        }
    }
    None
}

fn check(target: &Target, board: &str) -> Result<()> {
    let devices = target.arbiter.devices()?;
    let device = devices.iter().find(|device| device.mac == target.mac);
    println!(
        "{board} ({}): attached at {}, chip {}",
        target.mac,
        target.port.display(),
        target.chip.as_deref().unwrap_or("unknown")
    );
    match target.arbiter.latest_flash(&target.mac)? {
        Some(event) => println!("  firmware: {event}"),
        None => println!("  firmware: unknown"),
    }
    if let Some(maintenance) = target
        .arbiter
        .maintenance()?
        .into_iter()
        .find(|entry| entry.mac == target.mac)
    {
        println!(
            "  maintenance by {}: {}",
            maintenance.owner, maintenance.reason
        );
    }
    let control = device.and_then(|device| device.control.as_ref());
    println!(
        "  reset paths: rts, jtag{}",
        if control.is_some_and(|control| control.reset.is_some()) {
            ", en (UART bridge)"
        } else {
            ""
        }
    );
    // A text-protocol peer answers SYNC; a HIL runtime answers only its binary
    // protocol, which the runner speaks.
    let mut serial = hil_flash::open_without_reset(&target.port)?;
    serial.write_all(b"\nSYNC\n")?;
    let lines = hil_flash::serial_lines(serial);
    let mut heard = Vec::new();
    let answered = answer(&lines, "SYNC", Duration::from_secs(2), &mut |text| {
        heard.push(text.to_owned())
    });
    match answered {
        Some(true) => println!("  answers: the peer text protocol (@OK SYNC)"),
        _ if !heard.is_empty() => println!(
            "  answers: console output, no peer text protocol; last line: {}",
            heard.last().map(String::as_str).unwrap_or_default()
        ),
        _ => println!(
            "  answers: nothing within 2s (a HIL runtime speaks only the runner's protocol; \
             a hung or halted chip says nothing)"
        ),
    }
    Ok(())
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &[&str]) -> mpsc::Receiver<Vec<u8>> {
        let (sender, received) = mpsc::channel();
        for line in text {
            sender.send(line.as_bytes().to_vec()).unwrap();
        }
        std::mem::forget(sender);
        received
    }

    #[test]
    fn a_peer_answer_ends_at_its_ok_or_err_line() {
        let mut printed = Vec::new();
        let ok = answer(
            &lines(&["@READY", "@ERR n unknown", "@OK SYNC", "later"]),
            "SYNC",
            Duration::from_secs(1),
            &mut |text| printed.push(text.to_owned()),
        );
        assert_eq!(ok, Some(true));
        assert_eq!(printed, ["@READY", "@ERR n unknown", "@OK SYNC"]);
        let err = answer(
            &lines(&["@ERR BURST busy"]),
            "BURST",
            Duration::from_secs(1),
            &mut |_| {},
        );
        assert_eq!(err, Some(false));
        let silent = answer(
            &lines(&["boot"]),
            "SYNC",
            Duration::from_millis(200),
            &mut |_| {},
        );
        assert_eq!(silent, None);
    }

    #[test]
    fn the_rom_reset_line_is_picked_from_the_banner() {
        let banner = lines(&[
            "ESP-ROM:esp32c5-eco2-20250121",
            "rst:0x15 (USB_UART_HPSYS),boot:0x18 (SPI_FAST_FLASH_BOOT)",
        ]);
        assert_eq!(
            rom_line(&banner, Duration::from_secs(1)).as_deref(),
            Some("rst:0x15 (USB_UART_HPSYS),boot:0x18 (SPI_FAST_FLASH_BOOT)")
        );
        assert_eq!(rom_line(&lines(&["app"]), Duration::from_millis(200)), None);
    }

    #[test]
    fn a_download_reset_needs_the_en_path() {
        use clap::Parser as _;
        #[derive(clap::Parser)]
        struct Wrap {
            #[command(subcommand)]
            command: BoardCommand,
        }
        let parsed = Wrap::try_parse_from(["x", "reset", "esp32c5", "--via", "jtag"]).unwrap();
        assert!(matches!(
            parsed.command,
            BoardCommand::Reset {
                via: Via::Jtag,
                download: false,
                ..
            }
        ));
    }
}
