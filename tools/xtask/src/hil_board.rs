//! `cargo hil board reset|check|console|soak` and `cargo hil peer send`: the
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
/// How long a port may take to return after its hub port's power cycle.
const POWER_REATTACH: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum Via {
    /// Pulse RTS on the chip's USB Serial/JTAG port, the boot strap released.
    Rts,
    /// Reset the CPU through the chip's JTAG with OpenOCD.
    Jtag,
    /// Cycle the power of the board's hub port: off, then on. The
    /// stand's only way to switch a hub port; a lease runs no `uhubctl`.
    Power,
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
    /// Reset a board again and again through each path, stopping at the
    /// first reset after which it does not boot; the result is journaled.
    #[command(group(clap::ArgGroup::new("length").required(true).args(["cycles", "duration"])))]
    Soak {
        #[arg(value_name = "NAME|MAC")]
        board: String,
        /// Cycles to run; one cycle resets through each path once.
        #[arg(long)]
        cycles: Option<u32>,
        /// How long to run, e.g. 8h.
        #[arg(long = "for", value_parser = parse_duration)]
        duration: Option<Duration>,
        /// Reset paths, each once per cycle.
        #[arg(long, value_enum, value_delimiter = ',', default_value = "rts")]
        via: Vec<Via>,
        /// Cycles per lease; other owners may use the board between them.
        #[arg(long, default_value_t = 10)]
        batch: u32,
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
        BoardCommand::Reset { board, via } => {
            let target = target(&board)?;
            let _grant = lease(
                &target,
                owner,
                format!("board reset {board} --via {via:?}"),
                false,
            )?;
            let line = reset(&target, via)?;
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
        BoardCommand::Soak {
            board,
            cycles,
            duration,
            via,
            batch,
        } => return soak(ctx, owner, &board, cycles, duration, &via, batch.max(1)),
    }
    Ok(std::process::ExitCode::SUCCESS)
}

/// Cycle `board`'s hub port under its lease and return the reset line its
/// ROM reports (`cargo hil stand discover --verify-power`).
pub(crate) fn power_reset_line(owner: String, board: &str) -> Result<Option<String>> {
    let target = target(board)?;
    let _grant = lease(
        &target,
        owner,
        format!("stand discover --verify-power {board}"),
        false,
    )?;
    reset(&target, Via::Power)
}

/// Whether a reset's ROM line shows the board booting from flash, or why not.
fn booted(line: Option<&str>) -> std::result::Result<(), String> {
    match line {
        None => Err(String::from("no ROM reset line on its console")),
        Some(line) if line.contains("DOWNLOAD") => {
            Err(format!("its ROM waits for a download: {line}"))
        }
        Some(_) => Ok(()),
    }
}

fn reset_path(via: Via) -> oer_hil_arbiter::ResetPath {
    match via {
        Via::Rts => oer_hil_arbiter::ResetPath::Rts,
        Via::Jtag => oer_hil_arbiter::ResetPath::Jtag,
        Via::Power => oer_hil_arbiter::ResetPath::Power,
    }
}

/// `cargo hil board soak`: batches of cycles under their own leases, ending
/// at `cycles`, after `duration`, or at the first reset that did not boot.
fn soak(
    ctx: &Context,
    owner: String,
    board: &str,
    cycles: Option<u32>,
    duration: Option<Duration>,
    via: &[Via],
    batch: u32,
) -> Result<std::process::ExitCode> {
    let target = target(board)?;
    let started = Instant::now();
    let done = |cycle: u32| {
        cycles.is_some_and(|cycles| cycle >= cycles)
            || duration.is_some_and(|duration| started.elapsed() >= duration)
    };
    let (mut cycle, mut resets, mut failure) = (0_u32, 0_u32, None);
    'soak: while !done(cycle) {
        let _grant = lease(
            &target,
            owner.clone(),
            format!("board soak {board} cycles {}..", cycle + 1),
            false,
        )?;
        for _ in 0..batch {
            if done(cycle) {
                break;
            }
            cycle += 1;
            for &path in via {
                resets += 1;
                let line = reset(&target, path)?;
                let name = format!("{path:?}").to_lowercase();
                match booted(line.as_deref()) {
                    Ok(()) => println!("{cycle} {name}: {}", line.unwrap_or_default()),
                    Err(why) => {
                        println!("{cycle} {name}: FAILED: {why}");
                        // What the board prints next is the evidence.
                        let log = ctx
                            .root
                            .join("target/hil/console")
                            .join(target.mac.replace(':', ""))
                            .join(format!("soak-failure-{}.log", unix_seconds()));
                        std::fs::create_dir_all(log.parent().ok_or("no parent")?)?;
                        if let Ok(serial) = hil_flash::open_without_reset(&target.port) {
                            let lines = hil_flash::serial_lines(serial);
                            let _ = hil_flash::capture(lines, Duration::from_secs(5), None, &log);
                            eprintln!("hil: console after the failure in {}", log.display());
                        }
                        failure = Some(format!("cycle {cycle} via {name}: {why}"));
                        break 'soak;
                    }
                }
            }
        }
    }
    target.arbiter.record_board_by(
        owner,
        Some(target.mac.clone()),
        oer_hil_arbiter::BoardEventKind::Soaked {
            paths: via.iter().copied().map(reset_path).collect(),
            cycles: cycle,
            resets,
            failure: failure.clone(),
        },
    )?;
    println!(
        "{board}: {resets} resets in {cycle} cycles: {}",
        failure.as_deref().unwrap_or("every reset booted")
    );
    Ok(if failure.is_some() {
        std::process::ExitCode::FAILURE
    } else {
        std::process::ExitCode::SUCCESS
    })
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
fn reset(target: &Target, via: Via) -> Result<Option<String>> {
    let lines = match via {
        Via::Rts => hil_flash::serial_lines(retrying(|| {
            hil_flash::reset_into_application(&target.port)
        })?),
        Via::Jtag => {
            let lines =
                hil_flash::serial_lines(retrying(|| hil_flash::open_without_reset(&target.port))?);
            let chip = target
                .chip
                .as_deref()
                .ok_or("the board is not in the stand file")?;
            crate::hil_jtag::reset(chip, &target.mac)?;
            lines
        }
        Via::Power => {
            let power = target
                .arbiter
                .devices()?
                .into_iter()
                .find(|device| device.mac == target.mac)
                .and_then(|device| device.power)
                .ok_or(
                    "the board does not reset by power; add `power` to its `reset` in the stand file",
                )?;
            power.cycle()?;
            return Ok(reattached_rom_line(target, POWER_REATTACH));
        }
    };
    if let Some(line) = rom_line(&lines, BANNER) {
        return Ok(Some(line));
    }
    // A chip whose USB Serial/JTAG port re-enumerates prints the banner
    // before the port returns.
    Ok(reattached_rom_line(target, REATTACH))
}

/// The ROM's `rst:` line once the board's port returns within `within`.
fn reattached_rom_line(target: &Target, within: Duration) -> Option<String> {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if let Ok(serial) = hil_flash::open_without_reset(&target.port) {
            return rom_line(&hil_flash::serial_lines(serial), BANNER);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    None
}

/// Open a board's port, retrying for two seconds: the console reader of the
/// previous reset releases the port only at its next read timeout.
fn retrying<T, E>(
    mut open: impl FnMut() -> std::result::Result<T, E>,
) -> std::result::Result<T, E> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match open() {
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            result => return result,
        }
    }
}

/// Whether `board` boots from flash after an RTS reset: its ROM reset line,
/// or why not. A quarantined board is checked without a lease, which nobody
/// else can hold.
pub(crate) fn boots(board: &str) -> Result<String> {
    let target = target(board)?;
    let line =
        reset(&target, Via::Rts)?.ok_or("no ROM reset line on its console after an RTS reset")?;
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
    println!(
        "  reset paths: rts, jtag{}",
        if device.is_some_and(|device| device.power.is_some()) {
            ", power (hub port)"
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

    #[test]
    fn a_soak_counts_only_resets_that_boot_from_flash() {
        assert!(
            booted(Some(
                "rst:0x15 (USB_UART_HPSYS),boot:0x18 (SPI_FAST_FLASH_BOOT)"
            ))
            .is_ok()
        );
        assert!(
            booted(Some("rst:0x1 (POWERON),boot:0x4 (DOWNLOAD(USB/UART0))"))
                .unwrap_err()
                .contains("download")
        );
        assert!(booted(None).is_err());
    }

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
    fn a_reset_names_its_path() {
        use clap::Parser as _;
        #[derive(clap::Parser)]
        struct Wrap {
            #[command(subcommand)]
            command: BoardCommand,
        }
        let parsed = Wrap::try_parse_from(["x", "reset", "esp32c5", "--via", "jtag"]).unwrap();
        assert!(matches!(
            parsed.command,
            BoardCommand::Reset { via: Via::Jtag, .. }
        ));
        assert!(Wrap::try_parse_from(["x", "reset", "esp32c5", "--via", "en"]).is_err());
    }
}
