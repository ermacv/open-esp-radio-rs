//! `cargo hil flash`: flash an ELF to one board under a lease of that board,
//! journal it, and optionally capture its console for a bounded time.
//!
//! This is the manual cycle for images outside the HIL runner and the
//! ESP-IDF catalog, such as a chip's first no_std images. `espflash` writes
//! the ESP-IDF second-stage bootloader, partition table and application for
//! the board's registered chip. The journal records the application image
//! `espflash save-image` derives from the ELF, so another owner sees what the
//! board carries. The console capture ends at its deadline or at an expected
//! line, never with the lease, so the board is released promptly.
use std::{
    ffi::OsString,
    io::{BufRead as _, Write as _},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

use crate::{Context, Result};

#[derive(clap::Parser, Debug)]
#[command(name = "cargo hil flash", no_binary_name = true)]
struct FlashCli {
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
    /// Radio environment the image uses; exclusive for RF measurements.
    #[arg(long, value_parser = parse_air, default_value = "shared")]
    air: oer_hil_arbiter::Mode,
    elf: PathBuf,
}

fn parse_air(text: &str) -> std::result::Result<oer_hil_arbiter::Mode, String> {
    match text {
        "shared" => Ok(oer_hil_arbiter::Mode::Shared),
        "exclusive" => Ok(oer_hil_arbiter::Mode::Exclusive),
        _ => Err(String::from("use `shared` or `exclusive`")),
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

    // Derive the application image before queueing.
    let application = output.join("application.bin");
    crate::process::run(
        Command::new(espflash())
            .args(["save-image", "--chip", &chip])
            .arg(&elf)
            .arg(&application),
    )?;
    let application_sha256 = crate::vendor_fetch::sha256(&application)?;

    let request = oer_hil_arbiter::Request {
        work: format!("flash {} --board {}", elf.display(), cli.board),
        claims: vec![
            oer_hil_arbiter::Claim::board(&board.mac),
            oer_hil_arbiter::Claim {
                resource: oer_hil_arbiter::AIR.to_owned(),
                mode: cli.air,
            },
        ],
        ..request
    };
    let _grant = arbiter.acquire(&request)?;
    let _device = oer_esp32s31_firmware::device::DeviceLease::acquire(&board.port)?;
    crate::process::run(
        Command::new(espflash())
            .args(["flash", "--non-interactive", "--chip", &chip, "--port"])
            .arg(&board.port)
            .arg(&elf),
    )?;
    let (commit, dirty) = crate::firmware_catalog::source_revision(&ctx.root, Path::new("."));
    arbiter.record_board_by(
        request.owner.clone(),
        Some(board.mac.clone()),
        oer_hil_arbiter::BoardEventKind::Flashed {
            image: image.clone(),
            application_sha256,
            commit,
            dirty,
            origin: format!("cargo hil flash {}", elf.display()),
        },
    )?;
    eprintln!("hil-arbiter: recorded {image} on {}", board.mac);

    let Some(duration) = monitor else {
        return Ok(std::process::ExitCode::SUCCESS);
    };
    let started = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?;
    let log = output.join(format!("console-{}.log", started.as_millis()));
    let mut console = Command::new(espflash());
    console
        .args(["monitor", "--non-interactive", "--chip", &chip, "--port"])
        .arg(&board.port)
        .arg("--elf")
        .arg(&elf);
    let seen = capture(console, duration, cli.until.as_deref(), &log)?;
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

/// Copy `command`'s output lines to the terminal and `log` until `duration`
/// passes or a line contains `until`; returns whether it did. The command is
/// then terminated.
fn capture(
    mut command: Command,
    duration: Duration,
    until: Option<&str>,
    log: &Path,
) -> Result<bool> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    let stdout = child.stdout.take().ok_or("the console has no output")?;
    let (lines, received) = mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout).split(b'\n') {
            let Ok(line) = line else { break };
            if lines.send(line).is_err() {
                break;
            }
        }
    });
    let mut file = std::fs::File::create(log)?;
    let deadline = Instant::now() + duration;
    let mut seen = false;
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        match received.recv_timeout(remaining) {
            Ok(line) => {
                file.write_all(&line)?;
                file.write_all(b"\n")?;
                let text = String::from_utf8_lossy(&line);
                println!("{text}");
                if until.is_some_and(|until| text.contains(until)) {
                    seen = true;
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => break,
            // The console exited on its own.
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    Ok(seen)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn printer(lines: &str, then_sleep: &str) -> Command {
        let mut command = Command::new("sh");
        command.args(["-c", &format!("printf '{lines}'; sleep {then_sleep}")]);
        command
    }

    #[test]
    fn the_capture_ends_at_the_expected_line_or_its_deadline() {
        let directory = tempfile::tempdir().unwrap();
        let log = directory.path().join("console.log");
        let started = Instant::now();
        let seen = capture(
            printer("boot\\nREADY 1\\nlater\\n", "30"),
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
            printer("boot\\n", "30"),
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
        assert_eq!(cli.air, oer_hil_arbiter::Mode::Shared);
        assert!(FlashCli::try_parse_from(["--board", "esp32c5", "--until", "X", "a.elf"]).is_err());
    }
}
