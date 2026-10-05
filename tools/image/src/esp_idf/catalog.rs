//! The catalog of tracked ESP-IDF firmware the stand's boards run, and its
//! builds.
//!
//! Every ESP-IDF project with a `firmware.toml` beside its `CMakeLists.txt`
//! is an entry: peers in `hil/peers/<project>/`, vendor references in
//! `verification/<chip>/hil-vendor/<project>/` and each chip's second-stage
//! bootloader in `hil/bootloaders/<chip>/`, which every image of that chip
//! is bundled with. The manifest names the image (its name in the board
//! journal), the target chip and the chip whose `artifacts.toml` pins the
//! ESP-IDF; every entry builds against that one pin through [`super::idf`].
use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::idf;
use crate::Result;

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
    /// Why the image must not be flashed now; flashing it is refused.
    #[serde(default)]
    hold: Option<String>,
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
    /// Why the image must not be flashed now.
    pub hold: Option<String>,
    /// Project directory, relative to the repository root.
    pub directory: PathBuf,
}

impl Entry {
    /// The entry's ESP-IDF project in the tree at `root`.
    pub fn project(&self, root: &Path) -> idf::Project {
        idf::Project {
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
            hold: manifest.hold,
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

pub fn entry<'a>(entries: &'a [Entry], image: &str) -> Result<&'a Entry> {
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

/// Build the catalog entry `image` of the tree at `root` against its pinned
/// ESP-IDF.
pub fn build(root: &Path, image: &str) -> Result<idf::Build> {
    let entries = entries(root)?;
    let entry = entry(&entries, image)?;
    idf::build(root, &entry.pins, &[entry.project(root)])?
        .pop()
        .ok_or_else(|| "the build produced no image".into())
}

/// The file a built `entry` provides and its SHA-256: the application of an
/// image, the bootloader of a bootloader entry.
pub fn product(root: &Path, entry: &Entry, build: &idf::Build) -> Result<(PathBuf, String)> {
    Ok(match entry.kind {
        Kind::Image => (
            PathBuf::from(&build.application),
            build.application_sha256.clone(),
        ),
        Kind::Bootloader => {
            let path = bootloader_path(root, entry);
            let sha256 = oer_durable::sha256_file(&path)?;
            (path, sha256)
        }
    })
}

/// Where a bootloader entry's build leaves its bootloader.
pub fn bootloader_path(root: &Path, entry: &Entry) -> PathBuf {
    idf::output(root, &entry.project(root)).join("build/bootloader/bootloader.bin")
}

/// The project second-stage bootloader of `chip`, built against the pinned
/// ESP-IDF, and its SHA-256.
pub fn bootloader(root: &Path, chip: &str) -> Result<(PathBuf, String)> {
    let entries = entries(root)?;
    let entry = entries
        .iter()
        .find(|entry| entry.kind == Kind::Bootloader && entry.chip == chip)
        .ok_or_else(|| {
            format!("no project bootloader for {chip}; add hil/bootloaders/{chip} to the catalog")
        })?;
    build(root, &entry.image)?;
    let path = bootloader_path(root, entry);
    let sha256 = oer_durable::sha256_file(&path)?;
    Ok((path, sha256))
}

/// The ESP-IDF build directory of `chip`'s project bootloader, built first:
/// its `flasher_args.json` names the bootloader, the partition table and the
/// application's offset.
pub fn bootloader_build(root: &Path, chip: &str) -> Result<PathBuf> {
    let (bootloader, _) = bootloader(root, chip)?;
    Ok(bootloader
        .parent()
        .and_then(Path::parent)
        .ok_or("the bootloader lies outside an ESP-IDF build directory")?
        .to_owned())
}

/// The image bundle of the built catalog image `entry` in `output`: its
/// application with the bootloader and partition table of its own ESP-IDF
/// build, at its chip's flash map. The build's `flasher_args.json` must place
/// them there and flash nothing else, so a bundle holds every byte
/// `idf.py flash` would write.
pub fn bundle(
    root: &Path,
    entry: &Entry,
    build: &idf::Build,
    output: &Path,
) -> Result<crate::ImageBundle> {
    if entry.kind != Kind::Image {
        return Err(format!("`{}` is a bootloader, not an image", entry.image).into());
    }
    let directory = idf::output(root, &entry.project(root)).join("build");
    only_boot_files(&directory)?;
    let flash = crate::profile(root, &entry.chip)?
        .flash
        .ok_or_else(|| format!("{} names no flash map", entry.chip))?;
    let (bootloader, partition_table) = super::flash_files(&directory, &flash)?;
    crate::bundle::around(
        root,
        &entry.chip,
        Path::new(&build.application),
        crate::bundle::BootFiles::Given {
            bootloader: &bootloader,
            partition_table: &partition_table,
        },
        output,
    )
}

/// The bundle in `output` of the ELF `elf` of an ESP-IDF-bootloader chip's
/// application built outside the image pipeline (a chip's first no_std
/// image): encoded for its chip's flash map with espflash's default
/// partition table and bundled with the chip's catalog bootloader.
pub fn bundle_elf(
    root: &Path,
    chip: &str,
    elf: &Path,
    output: &Path,
) -> Result<crate::ImageBundle> {
    let profile = crate::profile(root, chip)?;
    if profile.boot != crate::Boot::EspIdfBootloader {
        return Err(format!(
            "an {chip} image is flashed as the bundle its build made (`cargo xtask build \
             firmware`, `cargo hil image build`), not as an ELF"
        )
        .into());
    }
    let flash = profile.flash.clone().ok_or("the chip names no flash map")?;
    let (bootloader, _) = bootloader(root, chip)?;
    let encoded = crate::encode::encode(
        &std::fs::read(elf)?,
        crate::encode::chip(&profile.espflash_chip)?,
        crate::encode::Encoding::DEFAULT,
        None,
        flash.partition_table,
        None,
    )?;
    let encoded_files = output.join("encoded");
    std::fs::create_dir_all(&encoded_files)?;
    let application = encoded_files.join("application.bin");
    let partition_table = encoded_files.join("partition-table.bin");
    std::fs::write(&application, &encoded.application)?;
    std::fs::write(&partition_table, &encoded.partition_table)?;
    crate::bundle::around(
        root,
        chip,
        &application,
        crate::bundle::BootFiles::Given {
            bootloader: &bootloader,
            partition_table: &partition_table,
        },
        &output.join("bundle"),
    )
}

/// Refuse a build whose `flasher_args.json` flashes more than the
/// bootloader, the partition table and the application.
fn only_boot_files(build: &Path) -> Result<()> {
    let arguments: serde_json::Value =
        serde_json::from_slice(&std::fs::read(build.join("flasher_args.json"))?)?;
    let files = arguments["flash_files"]
        .as_object()
        .ok_or("flasher_args.json lists no flash_files")?;
    if files.len() != 3 {
        return Err(format!(
            "the catalog build flashes {} files ({}); an image bundle holds the bootloader, the \
             partition table and the application",
            files.len(),
            files
                .iter()
                .map(|(address, file)| format!("{address} {}", file.as_str().unwrap_or("?")))
                .collect::<Vec<_>>()
                .join(", ")
        )
        .into());
    }
    Ok(())
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
                    hold: None,
                    directory: "hil/peers/esp32c5-ieee802154".into(),
                },
                Entry {
                    image: "vendor-calibration".into(),
                    chip: "esp32s31".into(),
                    pins: "esp32s31".into(),
                    kind: Kind::Image,
                    hold: None,
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
    fn a_catalog_build_flashing_more_than_its_boot_files_is_refused() {
        let build = tempfile::tempdir().unwrap();
        let write = |files: &str| {
            std::fs::write(
                build.path().join("flasher_args.json"),
                format!(r#"{{"flash_files": {{{files}}}}}"#),
            )
            .unwrap();
        };
        write(
            r#""0x2000": "bootloader/bootloader.bin", "0x8000": "partition_table/partition-table.bin", "0x10000": "app.bin""#,
        );
        only_boot_files(build.path()).unwrap();
        write(
            r#""0x2000": "bootloader/bootloader.bin", "0x8000": "partition_table/partition-table.bin", "0xd000": "ota_data_initial.bin", "0x10000": "app.bin""#,
        );
        let error = only_boot_files(build.path()).unwrap_err().to_string();
        assert!(error.contains("ota_data_initial.bin"), "{error}");
    }
}
