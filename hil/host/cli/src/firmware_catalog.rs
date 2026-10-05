//! `cargo hil firmware`: list, build and flash the catalog of tracked
//! ESP-IDF firmware ([`oer_image::esp_idf::catalog`], which builds it).
//! Flashing leases only the named board and goes through the flash
//! operation's catalog flash (`oer_hil_flash::catalog`).
use crate::Result;
use oer_image::esp_idf::{
    catalog::{self, Kind, bootloader_path, entries, entry},
    idf,
};
use oer_process::Checkout;

/// `cargo hil firmware list`: image, chip, project and last build.
pub fn list(ctx: &Checkout) -> Result<()> {
    for entry in entries(&ctx.root)? {
        let built =
            std::fs::read(idf::output(&ctx.root, &entry.project(&ctx.root)).join("build.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<idf::Build>(&bytes).ok())
                .map_or_else(
                    || String::from("not built"),
                    |build| {
                        let digest = match entry.kind {
                            Kind::Image => Some(build.application_sha256),
                            Kind::Bootloader => {
                                oer_durable::sha256_file(&bootloader_path(&ctx.root, &entry)).ok()
                            }
                        };
                        digest.map_or_else(
                            || String::from("not built"),
                            |digest| format!("built {}", &digest[..12]),
                        )
                    },
                );
        let held = entry
            .hold
            .as_deref()
            .map_or_else(String::new, |reason| format!(" HELD: {reason}"));
        println!(
            "{:<24} {:<10} {:<48} {built}{held}",
            entry.image,
            entry.chip,
            entry.directory.display()
        );
    }
    Ok(())
}

/// `cargo hil firmware build IMAGE`: build against the pinned ESP-IDF and
/// print the image, its digest and its file.
pub fn build(ctx: &Checkout, image: &str) -> Result<idf::Build> {
    let entries = entries(&ctx.root)?;
    let entry = entry(&entries, image)?;
    let build = catalog::build(&ctx.root, image)?;
    let (path, digest) = catalog::product(&ctx.root, entry, &build)?;
    println!("{:<24} {digest} {}", entry.image, path.display());
    Ok(build)
}

/// `cargo hil firmware flash IMAGE --board BOARD`: build, lease only that
/// board, and flash the image through the flash operation.
pub fn flash(
    ctx: &Checkout,
    image: &str,
    board: &str,
    via: oer_hil_board::Via,
    if_changed: bool,
    owner: String,
) -> Result<()> {
    let arbiter = oer_hil_arbiter::Arbiter::open()?;
    let target = oer_hil_board::Board::attached(&ctx.root, &arbiter.stand()?, board)?;
    let request = oer_hil_arbiter::Request {
        owner: owner.clone(),
        work: format!("firmware flash {image} --board {board}"),
        scenarios: Vec::new(),
        claims: vec![
            oer_hil_arbiter::Claim::board(target.mac()),
            oer_hil_arbiter::Claim::shared(oer_hil_arbiter::AIR),
        ],
    };
    let lease = arbiter.lease_board(&request, target.mac())?;
    let flashed = oer_hil_flash::catalog::flash(
        &ctx.root,
        &arbiter,
        &owner,
        &lease.lock,
        &target,
        &oer_hil_flash::catalog::Request {
            image,
            chip: target.chip(),
            if_changed,
            via,
            origin: "cargo hil firmware flash",
        },
    )?;
    match flashed {
        oer_hil_flash::Flashed::Written => {
            eprintln!("hil-arbiter: recorded {image} on {}", target.mac());
        }
        oer_hil_flash::Flashed::Carried => {
            eprintln!("hil: {board} already carries the current build of {image}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalog_lists_its_peers_and_bootloaders() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let entries = entries(&root).unwrap();
        assert!(entries.iter().any(|entry| entry.image == "ieee802154-peer"));
        assert!(entries.iter().any(|entry| entry.kind == Kind::Bootloader));
    }
}
