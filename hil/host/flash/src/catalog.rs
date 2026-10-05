//! The flash of an ESP-IDF catalog image ([`oer_image::esp_idf::catalog`]):
//! built against the pinned ESP-IDF, bundled with its own bootloader and
//! partition table, and written by the one flash operation.

use std::path::Path;

use oer_hil_arbiter::{Arbiter, lock::BoardLock};
use oer_hil_board::Via;
use oer_image::esp_idf::catalog::{self, Kind};

use crate::{Flashed, Image, Revision, Target};

/// What a catalog flash is asked to do.
pub struct Request<'a> {
    /// The catalog image's name.
    pub image: &'a str,
    /// The chip of the board it goes to.
    pub chip: &'a str,
    /// Leave a board alone whose journal says it carries the current build.
    pub if_changed: bool,
    pub via: Via,
    /// The run or command that flashes, for the journal.
    pub origin: &'a str,
}

/// Build the catalog image of the repository at `root` and flash it into
/// `target` under `lock`, journaled for `owner`.
pub fn flash(
    root: &Path,
    journal: &Arbiter,
    owner: &str,
    lock: &BoardLock,
    target: &dyn Target,
    request: &Request<'_>,
) -> crate::Result<Flashed> {
    let entries = catalog::entries(root)?;
    let entry = refuse_unflashable(&entries, request.image)?;
    if entry.chip != request.chip {
        return Err(format!(
            "the board is an {}; `{}` targets {}",
            request.chip, entry.image, entry.chip
        )
        .into());
    }
    let build = catalog::build(root, &entry.image)?;
    let bundle = catalog::bundle(
        root,
        entry,
        &build,
        &root
            .join("target/hil/flash")
            .join(target.mac().replace(':', ""))
            .join(&entry.image),
    )?;
    let image = Image {
        bundle: &bundle,
        name: &entry.image,
        revision: Revision::of_checkout(root, &entry.directory),
        origin: format!(
            "{}, ESP-IDF {}",
            request.origin,
            build.idf_revision.get(..12).unwrap_or(&build.idf_revision)
        ),
    };
    if request.if_changed {
        crate::flash_if_changed(journal, owner, lock, target, &image, request.via)
    } else {
        crate::flash(journal, owner, lock, target, &image, request.via).map(|()| Flashed::Written)
    }
}

/// The catalog entry `image`, unless it is a bootloader or held.
fn refuse_unflashable<'a>(
    entries: &'a [catalog::Entry],
    image: &str,
) -> crate::Result<&'a catalog::Entry> {
    let entry = catalog::entry(entries, image)?;
    if entry.kind == Kind::Bootloader {
        return Err(format!("`{image}` is a bootloader; every flash writes it").into());
    }
    if let Some(reason) = &entry.hold {
        return Err(format!(
            "`{image}` is held and not flashed: {reason} ({}/firmware.toml)",
            entry.directory.display()
        )
        .into());
    }
    Ok(entry)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(root: &Path, directory: &str, manifest: &str) {
        let directory = root.join(directory);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("CMakeLists.txt"), "").unwrap();
        std::fs::write(directory.join("firmware.toml"), manifest).unwrap();
    }

    #[test]
    fn a_held_image_or_a_bootloader_is_refused_before_any_build() {
        let root = tempfile::tempdir().unwrap();
        project(
            root.path(),
            "hil/peers/held",
            "image = \"held-peer\"\nchip = \"esp32c5\"\npins = \"esp32s31\"\nhold = \"wedges USB\"\n",
        );
        project(
            root.path(),
            "hil/bootloaders/esp32c5",
            "image = \"esp32c5-bootloader\"\nchip = \"esp32c5\"\npins = \"esp32c5\"\nkind = \"bootloader\"\n",
        );
        let entries = catalog::entries(root.path()).unwrap();
        let held = refuse_unflashable(&entries, "held-peer")
            .err()
            .unwrap()
            .to_string();
        assert!(
            held.contains("held") && held.contains("wedges USB"),
            "{held}"
        );
        let bootloader = refuse_unflashable(&entries, "esp32c5-bootloader")
            .err()
            .unwrap()
            .to_string();
        assert!(bootloader.contains("is a bootloader"), "{bootloader}");
    }
}
