//! The one OpenOCD of the stand: located in the ESP-IDF tools of the shared
//! cache, it resets a chip through its builtin USB-JTAG, reads a halted
//! hart's registers and programs flash.
//!
//! A USB Serial/JTAG reset restarts only the HP system, and some chips then
//! misbehave (see `docs/hardware-errata.md`).
//! OpenOCD reaches the same USB device's JTAG interface, writes flash through
//! its flasher stub and resets the CPU through the debug module, which none of
//! those faults affect. A board is named by its MAC (`adapter serial`).

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

/// Longest an OpenOCD flash may take; programming several megabytes over
/// USB JTAG takes about a minute.
const PROGRAM_TIMEOUT: Duration = Duration::from_secs(600);
/// Longest a reset may take.
pub const RESET_TIMEOUT: Duration = Duration::from_secs(60);

/// The OpenOCD build that reaches a chip's builtin USB-JTAG.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Openocd {
    pub program: PathBuf,
    pub scripts: PathBuf,
}

impl Openocd {
    /// The newest OpenOCD of the ESP-IDF tools in the shared cache, which any
    /// ESP-IDF catalog build installs.
    pub fn locate() -> crate::Result<Self> {
        Self::locate_in(&oer_durable::xdg::esp_idf_cache()?.join("idf-tools"))
    }

    fn locate_in(idf_tools: &Path) -> crate::Result<Self> {
        let tools = idf_tools.join("tools/openocd-esp32");
        let mut versions = std::fs::read_dir(&tools)
            .map_err(|_| {
                format!(
                    "no OpenOCD in {}; build any ESP-IDF catalog image once (`cargo hil firmware \
                     build IMAGE`) to install the tools",
                    tools.display()
                )
            })?
            .flatten()
            .map(|entry| entry.path().join("openocd-esp32"))
            .filter(|root| root.join("bin/openocd").is_file())
            .collect::<Vec<_>>();
        versions.sort();
        let root = versions
            .pop()
            .ok_or("the ESP-IDF tools hold no OpenOCD build")?;
        Ok(Self {
            program: root.join("bin/openocd"),
            scripts: root.join("share/openocd/scripts"),
        })
    }

    fn board(&self, chip: &str, mac: &str) -> Vec<String> {
        vec![
            String::from("-s"),
            self.scripts.display().to_string(),
            String::from("-f"),
            format!("board/{chip}-builtin.cfg"),
            String::from("-c"),
            format!("adapter serial {mac}"),
        ]
    }

    /// The arguments of a `reset run` of the board of `chip` with `mac`.
    pub fn reset_arguments(&self, chip: &str, mac: &str) -> Vec<String> {
        let mut arguments = self.board(chip, mac);
        for command in ["init", "reset run", "shutdown"] {
            arguments.extend([String::from("-c"), String::from(command)]);
        }
        arguments
    }

    /// Reset the chip through its builtin USB-JTAG and let it run. On a
    /// staged chip this is a software system reset (`rst:0x3`) that also clears
    /// low-power and PMU state which an RTS reset through the USB
    /// Serial/JTAG port, and a reflash, leave in place (a powered-down MPLL
    /// kept the second-stage bootloader in a watchdog loop until it).
    pub fn reset(
        &self,
        chip: &str,
        mac: &str,
        lifetime: &oer_process::IoLifetime,
    ) -> crate::Result<()> {
        self.run(&self.reset_arguments(chip, mac), RESET_TIMEOUT, lifetime)
            .map(drop)
    }

    /// The arguments that halt the chip, print `registers` and let it run
    /// on, with no reset and no debugger server listening.
    pub fn register_arguments(&self, chip: &str, mac: &str, registers: &[&str]) -> Vec<String> {
        let mut commands = String::from("init; halt");
        for register in registers {
            commands.push_str(&format!("; echo \"{register} [reg {register}]\""));
        }
        commands.push_str("; resume; shutdown");
        let mut arguments = vec![
            String::from("-s"),
            self.scripts.display().to_string(),
            String::from("-c"),
            String::from("gdb_port disabled; telnet_port disabled; tcl_port disabled"),
        ];
        arguments.extend(self.board(chip, mac).into_iter().skip(2));
        arguments.extend([String::from("-c"), commands]);
        arguments
    }

    /// Halt the chip, read `registers` of its current hart and let it run
    /// on: what a target that stopped answering was doing, without the reset
    /// that would erase it.
    pub fn registers(
        &self,
        chip: &str,
        mac: &str,
        registers: &[&str],
        timeout: Duration,
        lifetime: &oer_process::IoLifetime,
    ) -> crate::Result<Vec<(String, u32)>> {
        let mut command = oer_process::command(&self.program);
        command.args(self.register_arguments(chip, mac, registers));
        lifetime.pin(&mut command)?;
        let output = oer_process::output(&mut command, Some(timeout))?;
        let log = String::from_utf8_lossy(&output.stderr).into_owned()
            + &String::from_utf8_lossy(&output.stdout);
        let values = parse_registers(&log, registers);
        if values.is_empty() {
            return Err(format!(
                "OpenOCD read no register ({}):\n{}",
                output.status,
                tail(&log, 12)
            )
            .into());
        }
        Ok(values)
    }

    /// The arguments that write `files` (flash offset, file), verify each
    /// and reset the CPU after the last.
    pub fn program_arguments(
        &self,
        chip: &str,
        mac: &str,
        files: &[(u32, PathBuf)],
    ) -> Vec<String> {
        let mut arguments = self.board(chip, mac);
        for (index, (offset, file)) in files.iter().enumerate() {
            let last = index + 1 == files.len();
            arguments.push(String::from("-c"));
            arguments.push(format!(
                "program_esp {} {offset:#x} verify{}",
                file.display(),
                if last { " reset exit" } else { "" }
            ));
        }
        arguments
    }

    /// Write `files` to the board and reset it into the application.
    pub fn program(
        &self,
        chip: &str,
        mac: &str,
        files: &[(u32, PathBuf)],
        lifetime: &oer_process::IoLifetime,
    ) -> crate::Result<()> {
        if files.is_empty() {
            return Err("nothing to flash".into());
        }
        self.run(
            &self.program_arguments(chip, mac, files),
            PROGRAM_TIMEOUT,
            lifetime,
        )
        .map(drop)
    }

    /// Run OpenOCD with `arguments`, terminating it after `timeout`: a debug
    /// session on a wedged target can block forever, even against `SIGTERM`.
    /// Its log is shown only when it fails.
    fn run(
        &self,
        arguments: &[String],
        timeout: Duration,
        lifetime: &oer_process::IoLifetime,
    ) -> crate::Result<std::process::Output> {
        let mut command = oer_process::command(&self.program);
        command.args(arguments);
        lifetime.pin(&mut command)?;
        let output = oer_process::output(&mut command, Some(timeout)).map_err(|error| {
            if error.is::<oer_process::owned::DeadlineExceeded>() {
                format!(
                    "OpenOCD did not finish within {}s and was stopped",
                    timeout.as_secs()
                )
                .into()
            } else {
                error
            }
        })?;
        if !output.status.success() {
            return Err(format!(
                "OpenOCD failed with {}:\n{}",
                output.status,
                tail(&String::from_utf8_lossy(&output.stderr), 20)
            )
            .into());
        }
        Ok(output)
    }
}

fn tail(log: &str, lines: usize) -> String {
    let mut tail = log.lines().rev().take(lines).collect::<Vec<_>>();
    tail.reverse();
    tail.join("\n")
}

/// The `NAME name (/32): 0x…` lines OpenOCD echoed for `registers`.
fn parse_registers(log: &str, registers: &[&str]) -> Vec<(String, u32)> {
    registers
        .iter()
        .filter_map(|register| {
            let line = log
                .lines()
                .find(|line| line.trim_start().starts_with(&format!("{register} ")))?;
            let value = line.rsplit(':').next()?.trim();
            let value = u32::from_str_radix(value.strip_prefix("0x")?, 16).ok()?;
            Some((String::from(*register), value))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn openocd() -> Openocd {
        Openocd {
            program: "openocd".into(),
            scripts: "/scripts".into(),
        }
    }

    #[test]
    fn every_file_is_verified_and_only_the_last_resets() {
        let arguments = openocd().program_arguments(
            "chip-b",
            "38:44:BE:AA:25:64",
            &[(0x2000, "boot.bin".into()), (0x10000, "app.bin".into())],
        );
        assert_eq!(
            arguments,
            [
                "-s",
                "/scripts",
                "-f",
                "board/chip-b-builtin.cfg",
                "-c",
                "adapter serial 38:44:BE:AA:25:64",
                "-c",
                "program_esp boot.bin 0x2000 verify",
                "-c",
                "program_esp app.bin 0x10000 verify reset exit",
            ]
        );
    }

    #[test]
    fn a_reset_names_the_board_by_its_mac_and_runs_it() {
        assert_eq!(
            openocd().reset_arguments("chip-a", "30:ED:A0:F3:F6:D0"),
            [
                "-s",
                "/scripts",
                "-f",
                "board/chip-a-builtin.cfg",
                "-c",
                "adapter serial 30:ED:A0:F3:F6:D0",
                "-c",
                "init",
                "-c",
                "reset run",
                "-c",
                "shutdown",
            ]
        );
    }

    #[test]
    fn registers_are_read_without_a_reset_and_a_missing_one_is_left_out() {
        let log = "Info : [chip-b] Target halted, PC=0x42010736\n\
                   pc pc (/32): 0x42010736\n\
                   mcause mcause (/32): 0x30000007\n\
                   Info : shutdown command invoked\n";
        assert_eq!(
            parse_registers(log, &["pc", "mcause", "mtval"]),
            [
                (String::from("pc"), 0x4201_0736),
                (String::from("mcause"), 0x3000_0007)
            ]
        );
        let arguments = openocd().register_arguments("chip-b", "38:44:BE:AA:25:64", &["pc"]);
        assert_eq!(
            &arguments[..4],
            [
                "-s",
                "/scripts",
                "-c",
                "gdb_port disabled; telnet_port disabled; tcl_port disabled"
            ]
        );
        assert!(arguments.contains(&String::from("adapter serial 38:44:BE:AA:25:64")));
        let commands = arguments.last().unwrap();
        // It reads without resetting and leaves the chip running.
        assert!(commands.contains("halt") && commands.ends_with("resume; shutdown"));
        assert!(!commands.contains("reset"));
    }

    #[test]
    fn the_newest_openocd_of_the_idf_tools_is_located() {
        let tools = tempfile::tempdir().unwrap();
        assert!(Openocd::locate_in(tools.path()).is_err());
        for version in ["v0.12.0-esp32-20240318", "v0.12.0-esp32-20250707"] {
            let root = tools
                .path()
                .join("tools/openocd-esp32")
                .join(version)
                .join("openocd-esp32");
            std::fs::create_dir_all(root.join("bin")).unwrap();
            std::fs::write(root.join("bin/openocd"), "").unwrap();
        }
        let located = Openocd::locate_in(tools.path()).unwrap();
        assert!(located.program.to_string_lossy().contains("20250707"));
        assert!(located.scripts.ends_with("share/openocd/scripts"));
    }
}
