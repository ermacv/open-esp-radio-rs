//! The catalog of tracked ESP-IDF firmware the stand's boards run.
//!
//! Every ESP-IDF project with a `firmware.toml` beside its `CMakeLists.txt`
//! is an entry: peers in `hil/peers/<project>/`, vendor references in
//! `verification/<chip>/hil-vendor/<project>/` and each chip's second-stage
//! bootloader in `hil/bootloaders/<chip>/`, which every flash of the stand
//! writes to a board of that chip. The manifest names the image
//! (its name in the board journal), the target chip and the chip whose
//! `artifacts.toml` pins the ESP-IDF; every entry builds against that one pin
//! through [`crate::vendor_firmware`]. Flashing leases only the named board
//! and journals the image with the digest from its `build.json`.
use std::{
    path::{Path, PathBuf},
    process::Command,
};

use serde::Deserialize;

use crate::{Context, Result, vendor_firmware};

/// Manifest file of a catalog entry.
const MANIFEST: &str = "firmware.toml";

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Manifest {
    /// Name of the image in the board journal.
    image: String,
    /// ESP-IDF target chip.
    chip: String,
    /// Chip whose `artifacts.toml` pins the ESP-IDF and vendor archives.
    pins: String,
    #[serde(default)]
    kind: Kind,
}

/// What an entry provides.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    /// An application image flashed as a whole.
    #[default]
    Image,
    /// The chip's second-stage bootloader, written with other images.
    Bootloader,
}

#[derive(Debug, PartialEq)]
pub struct Entry {
    pub image: String,
    pub chip: String,
    pub pins: String,
    pub kind: Kind,
    /// Project directory, relative to the repository root.
    pub directory: PathBuf,
}

impl Entry {
    fn project(&self, root: &Path) -> vendor_firmware::Project {
        vendor_firmware::Project {
            name: self
                .directory
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            source: root.join(&self.directory),
            chip: self.chip.clone(),
        }
    }
}

/// Every catalog entry below `root`, by image name.
pub fn entries(root: &Path) -> Result<Vec<Entry>> {
    let mut directories = subdirectories(&root.join("hil/peers"))?;
    directories.extend(subdirectories(&root.join("hil/bootloaders"))?);
    for chip in subdirectories(&root.join("verification"))? {
        directories.extend(subdirectories(&chip.join("hil-vendor"))?);
    }
    let mut entries = Vec::new();
    for directory in directories {
        let path = directory.join(MANIFEST);
        if !path.is_file() {
            continue;
        }
        let manifest: Manifest = toml::from_str(&std::fs::read_to_string(&path)?)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        if entries
            .iter()
            .any(|entry: &Entry| entry.image == manifest.image)
        {
            return Err(format!("firmware image `{}` is defined twice", manifest.image).into());
        }
        entries.push(Entry {
            image: manifest.image,
            chip: manifest.chip,
            pins: manifest.pins,
            kind: manifest.kind,
            directory: directory.strip_prefix(root)?.to_owned(),
        });
    }
    entries.sort_by(|a, b| a.image.cmp(&b.image));
    Ok(entries)
}

fn subdirectories(directory: &Path) -> Result<Vec<PathBuf>> {
    let Ok(listing) = std::fs::read_dir(directory) else {
        return Ok(Vec::new());
    };
    let mut directories = Vec::new();
    for entry in listing {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            directories.push(entry.path());
        }
    }
    directories.sort();
    Ok(directories)
}

fn entry<'a>(entries: &'a [Entry], image: &str) -> Result<&'a Entry> {
    entries
        .iter()
        .find(|entry| entry.image == image)
        .ok_or_else(|| {
            let known = entries
                .iter()
                .map(|entry| entry.image.as_str())
                .collect::<Vec<_>>();
            format!("no firmware image `{image}`; catalog: {}", known.join(", ")).into()
        })
}

/// `cargo hil firmware list`: image, chip, project and last build.
pub fn list(ctx: &Context) -> Result<()> {
    for entry in entries(&ctx.root)? {
        let built = std::fs::read(
            vendor_firmware::output(&ctx.root, &entry.project(&ctx.root)).join("build.json"),
        )
        .ok()
        .and_then(|bytes| serde_json::from_slice::<vendor_firmware::Build>(&bytes).ok())
        .map_or_else(
            || String::from("not built"),
            |build| {
                let digest = match entry.kind {
                    Kind::Image => Some(build.application_sha256),
                    Kind::Bootloader => {
                        crate::vendor_fetch::sha256(&bootloader_path(ctx, &entry)).ok()
                    }
                };
                digest.map_or_else(
                    || String::from("not built"),
                    |digest| format!("built {}", &digest[..12]),
                )
            },
        );
        println!(
            "{:<24} {:<10} {:<48} {built}",
            entry.image,
            entry.chip,
            entry.directory.display()
        );
    }
    Ok(())
}

/// `cargo hil firmware build IMAGE`: build against the pinned ESP-IDF.
pub fn build(ctx: &Context, image: &str) -> Result<vendor_firmware::Build> {
    let entries = entries(&ctx.root)?;
    let entry = entry(&entries, image)?;
    let build = vendor_firmware::build(ctx, &entry.pins, &[entry.project(&ctx.root)])?
        .pop()
        .ok_or("the build produced no image")?;
    let (digest, path) = match entry.kind {
        Kind::Image => (build.application_sha256.clone(), build.application.clone()),
        Kind::Bootloader => {
            let path = bootloader_path(ctx, entry);
            (
                crate::vendor_fetch::sha256(&path)?,
                path.display().to_string(),
            )
        }
    };
    println!("{:<24} {digest} {path}", entry.image);
    Ok(build)
}

fn bootloader_path(ctx: &Context, entry: &Entry) -> PathBuf {
    vendor_firmware::output(&ctx.root, &entry.project(&ctx.root))
        .join("build/bootloader/bootloader.bin")
}

/// The project second-stage bootloader of `chip`, built against the pinned
/// ESP-IDF, and its SHA-256.
pub fn bootloader(ctx: &Context, chip: &str) -> Result<(PathBuf, String)> {
    let entries = entries(&ctx.root)?;
    let entry = entries
        .iter()
        .find(|entry| entry.kind == Kind::Bootloader && entry.chip == chip)
        .ok_or_else(|| {
            format!("no project bootloader for {chip}; add hil/bootloaders/{chip} to the catalog")
        })?;
    build(ctx, &entry.image)?;
    let path = bootloader_path(ctx, entry);
    let sha256 = crate::vendor_fetch::sha256(&path)?;
    Ok((path, sha256))
}

/// The files `idf.py flash` writes, by address, from the build's
/// `flasher_args.json`.
fn flash_files(build: &Path) -> Result<Vec<(String, PathBuf)>> {
    let arguments: serde_json::Value =
        serde_json::from_slice(&std::fs::read(build.join("flasher_args.json"))?)?;
    let files = arguments["flash_files"]
        .as_object()
        .ok_or("flasher_args.json lists no flash_files")?;
    let mut files = files
        .iter()
        .map(|(address, file)| {
            let file = file.as_str().ok_or("flash file is not a path")?;
            Ok((address.clone(), build.join(file)))
        })
        .collect::<Result<Vec<_>>>()?;
    files
        .sort_by_key(|(address, _)| u64::from_str_radix(address.trim_start_matches("0x"), 16).ok());
    Ok(files)
}

/// `cargo hil firmware flash IMAGE --board BOARD`: build, lease only that
/// board, write every flash file of the build and journal the image.
pub fn flash(
    ctx: &Context,
    image: &str,
    board: &str,
    request: oer_hil_arbiter::Request,
) -> Result<()> {
    let entries = entries(&ctx.root)?;
    let entry = entry(&entries, image)?;
    if entry.kind == Kind::Bootloader {
        return Err(format!("`{image}` is a bootloader; every flash writes it").into());
    }
    let arbiter = oer_hil_arbiter::Arbiter::open()?;
    let Board { mac, port, chip } = resolve_board(&arbiter, board)?;
    if let Some(chip) = chip
        && chip != entry.chip
    {
        return Err(format!(
            "board `{board}` is an {chip}; `{image}` targets {}",
            entry.chip
        )
        .into());
    }
    let built = build(ctx, image)?;
    let files =
        flash_files(&vendor_firmware::output(&ctx.root, &entry.project(&ctx.root)).join("build"))?;
    let request = oer_hil_arbiter::Request {
        claims: vec![
            oer_hil_arbiter::Claim::board(&mac),
            oer_hil_arbiter::Claim::shared(oer_hil_arbiter::AIR),
        ],
        ..request
    };
    let _grant = arbiter.acquire(&request)?;
    let _device = oer_esp32s31_firmware::device::DeviceLease::acquire(&port)?;
    for (index, (address, file)) in files.iter().enumerate() {
        let after = if index + 1 == files.len() {
            "hard-reset"
        } else {
            "no-reset"
        };
        let mut command =
            ctx.command(std::env::var_os("ESPFLASH").unwrap_or_else(|| "espflash".into()));
        command
            .args([
                "write-bin",
                "--chip",
                &entry.chip,
                "--non-interactive",
                "--port",
            ])
            .arg(&port)
            .args(["--after", after, address])
            .arg(file);
        crate::process::run(&mut command)?;
    }
    arbiter.register_device(oer_hil_arbiter::Device {
        mac: mac.clone(),
        chip: Some(entry.chip.clone()),
        name: None,
    })?;
    let (commit, dirty) = source_revision(&ctx.root, &entry.directory);
    arbiter.record_board_by(
        request.owner.clone(),
        Some(mac.clone()),
        oer_hil_arbiter::BoardEventKind::Flashed {
            image: entry.image.clone(),
            application_sha256: built.application_sha256,
            commit,
            dirty,
            origin: format!(
                "cargo hil firmware flash, ESP-IDF {}",
                &built.idf_revision[..12]
            ),
        },
    )?;
    eprintln!("hil-arbiter: recorded {} on {mac}", entry.image);
    Ok(())
}

/// An attached board of the stand.
pub struct Board {
    pub mac: String,
    pub port: PathBuf,
    /// Its chip, when registered.
    pub chip: Option<String>,
}

/// The attached board `board` names: a registered name or a MAC.
pub fn resolve_board(arbiter: &oer_hil_arbiter::Arbiter, board: &str) -> Result<Board> {
    let devices = arbiter.devices()?;
    let mac = oer_hil_arbiter::board_mac(&devices, board)?;
    let chip = devices
        .iter()
        .find(|device| device.mac == mac)
        .and_then(|device| device.chip.clone());
    let port = oer_hil_arbiter::attached_ports()
        .into_iter()
        .find(|port| port.mac.as_deref() == Some(mac.as_str()))
        .map(|port| PathBuf::from(port.port))
        .ok_or_else(|| format!("board `{board}` ({mac}) is not attached"))?;
    Ok(Board { mac, port, chip })
}

/// The repository commit and whether `directory` differs from it.
pub(crate) fn source_revision(root: &Path, directory: &Path) -> (Option<String>, Option<bool>) {
    let git = |args: &[&str]| {
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    };
    let commit = git(&["rev-parse", "HEAD"]);
    let dirty = git(&["status", "--porcelain", "--", &directory.to_string_lossy()])
        .map(|status| !status.is_empty());
    (commit, dirty)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(root: &Path, directory: &str, manifest: &str) {
        let directory = root.join(directory);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("CMakeLists.txt"), "").unwrap();
        std::fs::write(directory.join(MANIFEST), manifest).unwrap();
    }

    #[test]
    fn peers_and_vendor_references_form_one_catalog() {
        let root = tempfile::tempdir().unwrap();
        project(
            root.path(),
            "hil/peers/esp32c5-ieee802154",
            "image = \"ieee802154-peer\"\nchip = \"esp32c5\"\npins = \"esp32s31\"\n",
        );
        project(
            root.path(),
            "verification/esp32s31/hil-vendor/calibration",
            "image = \"vendor-calibration\"\nchip = \"esp32s31\"\npins = \"esp32s31\"\n",
        );
        std::fs::create_dir_all(root.path().join("hil/peers/untracked")).unwrap();
        let entries = entries(root.path()).unwrap();
        assert_eq!(
            entries,
            [
                Entry {
                    image: "ieee802154-peer".into(),
                    chip: "esp32c5".into(),
                    pins: "esp32s31".into(),
                    kind: Kind::Image,
                    directory: "hil/peers/esp32c5-ieee802154".into(),
                },
                Entry {
                    image: "vendor-calibration".into(),
                    chip: "esp32s31".into(),
                    pins: "esp32s31".into(),
                    kind: Kind::Image,
                    directory: "verification/esp32s31/hil-vendor/calibration".into(),
                },
            ]
        );
        assert_eq!(entries[0].project(root.path()).name, "esp32c5-ieee802154");
        assert!(entry(&entries, "missing").is_err());
        project(
            root.path(),
            "hil/peers/copy",
            "image = \"ieee802154-peer\"\nchip = \"esp32c5\"\npins = \"esp32s31\"\n",
        );
        assert!(
            super::entries(root.path()).is_err(),
            "image names are unique"
        );
    }

    #[test]
    fn the_tracked_catalog_names_its_projects() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let entries = entries(&root).unwrap();
        for image in ["ieee802154-peer", "vendor-calibration"] {
            assert!(entries.iter().any(|entry| entry.image == image), "{image}");
        }
        assert!(
            entries
                .iter()
                .any(|entry| entry.kind == Kind::Bootloader && entry.chip == "esp32c5"),
            "esp32c5 has a project bootloader"
        );
    }

    #[test]
    fn flash_files_are_written_in_address_order() {
        let build = tempfile::tempdir().unwrap();
        std::fs::write(
            build.path().join("flasher_args.json"),
            r#"{"flash_files": {"0x10000": "app.bin", "0x0": "bootloader/bootloader.bin", "0x8000": "partition_table/partition-table.bin"}}"#,
        )
        .unwrap();
        let files = flash_files(build.path()).unwrap();
        assert_eq!(
            files
                .iter()
                .map(|(address, _)| address.as_str())
                .collect::<Vec<_>>(),
            ["0x0", "0x8000", "0x10000"]
        );
        assert_eq!(files[2].1, build.path().join("app.bin"));
    }
}
