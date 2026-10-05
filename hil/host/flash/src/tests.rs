use std::cell::RefCell;

use super::*;

const MAC: &str = "38:44:BE:AA:25:64";

/// A board that records what the operation asks of it, and what the
/// journal held when it was asked to start.
struct Fake<'a> {
    mac: &'static str,
    journal: &'a Arbiter,
    write: std::result::Result<Start, &'static str>,
    calls: RefCell<Vec<String>>,
}

impl Target for Fake<'_> {
    fn mac(&self) -> &str {
        self.mac
    }

    fn write(&self, bundle: &ImageBundle, via: Via) -> Result<Start> {
        let journaled = self.journal.latest_flash(self.mac)?.is_some();
        self.calls.borrow_mut().push(format!(
            "write {} {via:?} journaled={journaled}",
            bundle.chip
        ));
        self.write.map_err(Into::into)
    }

    fn start(&self, start: Start) -> Result<()> {
        let journaled = self.journal.latest_flash(self.mac)?.is_some();
        self.calls
            .borrow_mut()
            .push(format!("start {start:?} journaled={journaled}"));
        Ok(())
    }
}

fn bundle(directory: &Path) -> ImageBundle {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let profile = oer_image::profile(&root, "esp32c5").unwrap();
    let bundle = ImageBundle::new(directory, &profile, profile.flash.clone().unwrap());
    std::fs::write(bundle.application(), b"application").unwrap();
    bundle
}

fn image(bundle: &ImageBundle) -> Image<'_> {
    Image {
        bundle,
        name: "ieee802154-peer",
        revision: Revision {
            commit: Some("c0ffee".into()),
            dirty: Some(false),
        },
        origin: String::from("test"),
    }
}

#[test]
fn the_flash_writes_under_the_lock_then_journals_then_starts() {
    let directory = tempfile::tempdir().unwrap();
    let journal = Arbiter::at(directory.path().join("arbiter")).unwrap();
    let lock = BoardLock::try_acquire_in(&directory.path().join("locks"), MAC).unwrap();
    let bundle = bundle(directory.path());
    let board = Fake {
        mac: MAC,
        journal: &journal,
        write: Ok(Start::PowerOn),
        calls: RefCell::new(Vec::new()),
    };
    flash(&journal, "peer", &lock, &board, &image(&bundle), Via::Usb).unwrap();
    assert_eq!(
        *board.calls.borrow(),
        [
            "write esp32c5 Usb journaled=false",
            "start PowerOn journaled=true"
        ]
    );
    let flashed = journal.latest_flash(MAC).unwrap().unwrap();
    assert_eq!(flashed.owner, "peer");
    assert!(
        journal
            .carries(
                MAC,
                "ieee802154-peer",
                &oer_durable::sha256_bytes(b"application")
            )
            .unwrap()
    );
}

#[test]
fn a_failed_write_is_not_journaled_nor_started() {
    let directory = tempfile::tempdir().unwrap();
    let journal = Arbiter::at(directory.path().join("arbiter")).unwrap();
    let lock = BoardLock::try_acquire_in(&directory.path().join("locks"), MAC).unwrap();
    let bundle = bundle(directory.path());
    let board = Fake {
        mac: MAC,
        journal: &journal,
        write: Err("Protocol error"),
        calls: RefCell::new(Vec::new()),
    };
    assert!(flash(&journal, "peer", &lock, &board, &image(&bundle), Via::Usb).is_err());
    assert_eq!(board.calls.borrow().len(), 1);
    assert!(journal.latest_flash(MAC).unwrap().is_none());
}

#[test]
fn another_board_s_lock_writes_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let journal = Arbiter::at(directory.path().join("arbiter")).unwrap();
    let lock =
        BoardLock::try_acquire_in(&directory.path().join("locks"), "30:ED:A0:F3:F6:D0").unwrap();
    let bundle = bundle(directory.path());
    let board = Fake {
        mac: MAC,
        journal: &journal,
        write: Ok(Start::Reset),
        calls: RefCell::new(Vec::new()),
    };
    let error = flash(&journal, "peer", &lock, &board, &image(&bundle), Via::Usb)
        .unwrap_err()
        .to_string();
    assert!(error.contains("does not cover"), "{error}");
    assert!(board.calls.borrow().is_empty());
}

#[test]
fn an_image_the_board_carries_is_not_written_again() {
    let directory = tempfile::tempdir().unwrap();
    let journal = Arbiter::at(directory.path().join("arbiter")).unwrap();
    let lock = BoardLock::try_acquire_in(&directory.path().join("locks"), MAC).unwrap();
    let bundle = bundle(directory.path());
    let board = Fake {
        mac: MAC,
        journal: &journal,
        write: Ok(Start::Reset),
        calls: RefCell::new(Vec::new()),
    };
    assert_eq!(
        flash_if_changed(&journal, "peer", &lock, &board, &image(&bundle), Via::Jtag).unwrap(),
        Flashed::Written
    );
    assert_eq!(
        flash_if_changed(&journal, "peer", &lock, &board, &image(&bundle), Via::Jtag).unwrap(),
        Flashed::Carried
    );
    assert_eq!(board.calls.borrow().len(), 2, "one write and its start");
    // A new build is written.
    std::fs::write(bundle.application(), b"rebuilt").unwrap();
    assert_eq!(
        flash_if_changed(&journal, "peer", &lock, &board, &image(&bundle), Via::Jtag).unwrap(),
        Flashed::Written
    );
}
