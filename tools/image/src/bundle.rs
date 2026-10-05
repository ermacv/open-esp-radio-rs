//! The image bundle: everything one build made, in one directory, described
//! by its `bundle.json`.
//!
//! A bundle holds the runtime ELF, the encoded flash contents (application,
//! bootloader, partition table and, where the chip selects slots by OTA
//! data, the OTA selection), the gate reports, the effective lock files and
//! `source-inputs.json`. [`ImageBundle::segments`] is what a flash writes:
//! the files at the chip's flash map offsets, the OTA selection last.

use std::{
    collections::BTreeSet,
    num::NonZeroU32,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{Boot, FlashMap, Result};

/// The bundle's description, beside its files.
pub const MANIFEST: &str = "bundle.json";
/// Format of [`MANIFEST`].
pub const SCHEMA: u32 = 2;

/// Every file name a bundle uses.
pub mod files {
    pub const RUNTIME_ELF: &str = "runtime.elf";
    pub const RUNTIME_BIN: &str = "runtime.bin";
    pub const BOOTSTRAP_ELF: &str = "bootstrap.elf";
    pub const APPLICATION: &str = "application.bin";
    pub const BOOTLOADER: &str = "bootloader.bin";
    pub const PARTITIONS: &str = "partitions.bin";
    pub const OTADATA: &str = "otadata.bin";
    pub const SOURCE_INPUTS: &str = "source-inputs.json";
    pub const RUNTIME_LOCK: &str = "runtime-Cargo.lock";
    pub const BOOTSTRAP_LOCK: &str = "bootstrap-Cargo.lock";
    pub const PLACEMENT_REPORT: &str = "placement.txt";
    pub const BUILD_LOG: &str = "build.log";
}

/// One build's outputs.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ImageBundle {
    schema: u32,
    /// The bundle's directory; not recorded, it is where the manifest lies.
    #[serde(skip)]
    pub directory: PathBuf,
    pub chip: String,
    pub rust_target: String,
    pub boot: Boot,
    /// Where the flash contents go.
    pub flash: FlashMap,
    /// The staged boot's packed runtime and bootstrap ELF.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staged: Option<Staged>,
    /// Whether the bundle holds the OTA selection (`otadata.bin`).
    pub otadata: bool,
    /// Each effective lock file: the committed catalog it copies, and its
    /// file in the bundle.
    pub locks: Vec<Lock>,
    /// The gate reports, by file name.
    pub reports: Vec<String>,
    /// The seed the runtime's link order was shuffled by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout_seed: Option<NonZeroU32>,
    /// The reviewed ROM summaries the stack analysis applied.
    pub rom_summaries: BTreeSet<String>,
    /// What the gates passed with: partial or conditional bounds.
    pub warnings: Vec<String>,
}

/// The staged boot's own files.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Staged {
    /// The bootstrap's package, which embeds the packed runtime.
    pub bootstrap_package: String,
}

/// An effective lock file of a build.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Lock {
    /// The committed `Cargo.lock`, relative to the repository root.
    pub committed: PathBuf,
    /// Its effective copy's file name in the bundle.
    pub file: String,
}

/// One region a flash writes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Segment {
    pub offset: u32,
    pub file: PathBuf,
    pub description: &'static str,
}

impl ImageBundle {
    /// An empty bundle of `profile`'s chip in `directory`, which a pipeline
    /// fills: no files yet, no gate run.
    pub fn new(directory: &Path, profile: &oer_chip_profile::Profile, flash: FlashMap) -> Self {
        Self {
            schema: SCHEMA,
            directory: directory.to_owned(),
            chip: profile.id.clone(),
            rust_target: profile.rust_target.clone(),
            boot: profile.boot,
            flash,
            staged: None,
            otadata: false,
            locks: Vec::new(),
            reports: Vec::new(),
            layout_seed: None,
            rom_summaries: BTreeSet::new(),
            warnings: Vec::new(),
        }
    }

    /// The bundle in `directory`.
    pub fn load(directory: &Path) -> Result<Self> {
        let path = directory.join(MANIFEST);
        let value: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?,
        )
        .map_err(|error| format!("{}: {error}", path.display()))?;
        if value["schema"].as_u64() != Some(u64::from(SCHEMA)) {
            return Err(format!(
                "{} has schema {}; this tool reads schema {SCHEMA}: build the bundle again",
                path.display(),
                value["schema"]
            )
            .into());
        }
        let mut bundle: Self = serde_json::from_value(value)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        bundle.directory = directory.to_owned();
        for segment in bundle.segments() {
            if !segment.file.is_file() {
                return Err(format!(
                    "the bundle in {} lacks its {}",
                    directory.display(),
                    segment.description
                )
                .into());
            }
        }
        Ok(bundle)
    }

    /// Record the bundle's manifest in its directory.
    pub fn write(&self) -> Result<()> {
        oer_durable::atomic_json(&self.directory.join(MANIFEST), self)?;
        Ok(())
    }

    /// The bundle's file `name`.
    pub fn path(&self, name: &str) -> PathBuf {
        self.directory.join(name)
    }

    pub fn runtime_elf(&self) -> PathBuf {
        self.path(files::RUNTIME_ELF)
    }

    /// The encoded ESP application image.
    pub fn application(&self) -> PathBuf {
        self.path(files::APPLICATION)
    }

    pub fn bootloader(&self) -> PathBuf {
        self.path(files::BOOTLOADER)
    }

    pub fn partitions(&self) -> PathBuf {
        self.path(files::PARTITIONS)
    }

    /// The OTA selection of the first application slot, when the chip's
    /// bootloader selects slots by OTA data.
    pub fn otadata(&self) -> Option<PathBuf> {
        self.otadata.then(|| self.path(files::OTADATA))
    }

    /// The staged boot's packed runtime.
    pub fn runtime_bin(&self) -> Option<PathBuf> {
        self.staged.as_ref().map(|_| self.path(files::RUNTIME_BIN))
    }

    /// The staged boot's bootstrap ELF.
    pub fn bootstrap_elf(&self) -> Option<PathBuf> {
        self.staged
            .as_ref()
            .map(|_| self.path(files::BOOTSTRAP_ELF))
    }

    pub fn source_inputs(&self) -> PathBuf {
        self.path(files::SOURCE_INPUTS)
    }

    /// The effective lock file that copies the committed `committed`.
    pub fn lock(&self, committed: &Path) -> Option<PathBuf> {
        self.locks
            .iter()
            .find(|lock| lock.committed == committed)
            .map(|lock| self.path(&lock.file))
    }

    /// The bundle of this bundle's flash files in `output`, with the bytes
    /// of `application` (an archived copy of this bundle's) as its
    /// application: what a run flashes, so it writes exactly the bytes it
    /// archived even when a later build replaces this bundle. The ELF files,
    /// reports and locks stay here.
    pub fn with_application(&self, application: &Path, output: &Path) -> Result<Self> {
        if std::fs::read(application)? != std::fs::read(self.application())? {
            return Err(format!(
                "{} is not the application of the bundle in {}",
                application.display(),
                self.directory.display()
            )
            .into());
        }
        std::fs::create_dir_all(output)?;
        let mut bundle = Self::new_like(self, output);
        std::fs::copy(application, bundle.application())?;
        for file in [
            Some(files::BOOTLOADER),
            Some(files::PARTITIONS),
            self.otadata.then_some(files::OTADATA),
        ]
        .into_iter()
        .flatten()
        {
            std::fs::copy(self.path(file), output.join(file))?;
        }
        bundle.otadata = self.otadata;
        bundle.write()?;
        Ok(bundle)
    }

    fn new_like(other: &Self, directory: &Path) -> Self {
        Self {
            schema: SCHEMA,
            directory: directory.to_owned(),
            chip: other.chip.clone(),
            rust_target: other.rust_target.clone(),
            boot: other.boot,
            flash: other.flash.clone(),
            staged: None,
            otadata: false,
            locks: Vec::new(),
            reports: Vec::new(),
            layout_seed: other.layout_seed,
            rom_summaries: BTreeSet::new(),
            warnings: Vec::new(),
        }
    }

    /// What a flash writes, in order: the bootloader, the partition table,
    /// the application, and the OTA selection last, so an interrupted write
    /// leaves the previous selection pointing at an image whose checksum no
    /// longer validates instead of a half-written one.
    pub fn segments(&self) -> Vec<Segment> {
        let mut segments = vec![
            Segment {
                offset: self.flash.bootloader,
                file: self.bootloader(),
                description: "bootloader",
            },
            Segment {
                offset: self.flash.partition_table,
                file: self.partitions(),
                description: "partition table",
            },
            Segment {
                offset: self.flash.application,
                file: self.application(),
                description: "application",
            },
        ];
        if let (Some(offset), Some(file)) = (self.flash.otadata, self.otadata()) {
            segments.push(Segment {
                offset,
                file,
                description: "OTA selection",
            });
        }
        segments
    }
}

/// The boot files of a bundle made around an application encoded before.
pub enum BootFiles<'a> {
    /// A staged chip: the bootloader is encoded from an ELF of the
    /// application (its bytes do not depend on the ELF's code), the
    /// partition table from the chip's CSV.
    Encode { elf: &'a Path },
    /// An ESP-IDF bootloader chip: its catalog bootloader and partition
    /// table, as a build archived them.
    Given {
        bootloader: &'a Path,
        partition_table: &'a Path,
    },
}

/// The bundle in `output` of an `application` image encoded earlier (a
/// run's archive, or a vendor ESP-IDF build) for `chip` of the repository at
/// `root`, with the boot files `boot` names: what a replay or a foreign
/// application flashes, encoded here so a flash still only writes bundle
/// files.
pub fn around(
    root: &Path,
    chip: &str,
    application: &Path,
    boot: BootFiles<'_>,
    output: &Path,
) -> Result<ImageBundle> {
    let profile = crate::profile(root, chip)?;
    let flash = profile.flash.clone().ok_or("the chip names no flash map")?;
    std::fs::create_dir_all(output)?;
    let mut bundle = ImageBundle::new(output, &profile, flash);
    std::fs::copy(application, bundle.application())?;
    match (profile.boot, boot) {
        (Boot::Staged, BootFiles::Encode { elf }) => {
            crate::staged::encode_boot_files(root, &profile, &std::fs::read(elf)?, &mut bundle)?;
        }
        (
            Boot::EspIdfBootloader,
            BootFiles::Given {
                bootloader,
                partition_table,
            },
        ) => {
            std::fs::copy(bootloader, bundle.bootloader())?;
            std::fs::copy(partition_table, bundle.partitions())?;
        }
        (boot, _) => {
            return Err(format!("a {boot:?} chip's bundle takes the other boot files").into());
        }
    }
    bundle.write()?;
    Ok(bundle)
}

#[cfg(test)]
mod tests;
