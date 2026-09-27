//! Flash and reset a board through its chip's JTAG with OpenOCD.
//!
//! A USB Serial/JTAG reset restarts only the HP system, and some chips then
//! misbehave (see the ESP32-C5 rev 1.0 entry in `docs/hardware-errata.md`).
//! OpenOCD reaches the same USB device's JTAG interface, writes flash through
//! its flasher stub and resets the CPU through the debug module, which none of
//! those faults affect. The OpenOCD build is the one the ESP-IDF tool
//! installation of the shared cache provides.
use std::path::{Path, PathBuf};

use crate::Result;

/// The OpenOCD executable and its script directory from the ESP-IDF tools.
pub fn openocd() -> Result<(PathBuf, PathBuf)> {
    let tools = crate::vendor_firmware::cache_directory()?.join("idf-tools/tools/openocd-esp32");
    let mut versions = std::fs::read_dir(&tools)
        .map_err(|_| {
            format!(
                "no OpenOCD in {}; build any ESP-IDF catalog image once (`cargo hil firmware build IMAGE`) to install the tools",
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
    Ok((root.join("bin/openocd"), root.join("share/openocd/scripts")))
}

/// The OpenOCD arguments that write `files` (flash offset, file) to the board
/// of `chip` with USB serial number `mac`, verify them and reset the CPU.
pub fn program_arguments(
    chip: &str,
    mac: &str,
    scripts: &Path,
    files: &[(u64, PathBuf)],
) -> Vec<String> {
    let mut arguments = vec![
        String::from("-s"),
        scripts.display().to_string(),
        String::from("-f"),
        format!("board/{chip}-builtin.cfg"),
        String::from("-c"),
        format!("adapter serial {mac}"),
    ];
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
pub fn program(chip: &str, mac: &str, files: &[(u64, PathBuf)]) -> Result<()> {
    if files.is_empty() {
        return Err("nothing to flash".into());
    }
    let (openocd, scripts) = openocd()?;
    crate::process::run(
        std::process::Command::new(openocd).args(program_arguments(chip, mac, &scripts, files)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_file_is_verified_and_only_the_last_resets() {
        let arguments = program_arguments(
            "esp32c5",
            "38:44:BE:AA:25:64",
            Path::new("/scripts"),
            &[(0x2000, "boot.bin".into()), (0x10000, "app.bin".into())],
        );
        assert_eq!(
            arguments,
            [
                "-s",
                "/scripts",
                "-f",
                "board/esp32c5-builtin.cfg",
                "-c",
                "adapter serial 38:44:BE:AA:25:64",
                "-c",
                "program_esp boot.bin 0x2000 verify",
                "-c",
                "program_esp app.bin 0x10000 verify reset exit",
            ]
        );
    }
}
