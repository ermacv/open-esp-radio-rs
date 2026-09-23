use super::*;
use sha2::{Digest, Sha256};

#[test]
fn omitted_dropped_failed_or_duplicate_outputs_cannot_complete_work() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("output");
    let set = OutputSet::new(std::slice::from_ref(&file), false).unwrap();
    assert!(set.require_complete().is_err());
    assert!(set.file(1, "undeclared").is_err());
    {
        let _unused = set.file(0, "dropped").unwrap();
    }
    assert!(set.require_complete().is_err());
    assert!(set.clone().file(0, "duplicate").is_err());
    assert!(!file.exists());

    let failed = OutputSet::new(std::slice::from_ref(&file), false).unwrap();
    std::fs::create_dir(&file).unwrap();
    assert!(failed.file(0, "failed").unwrap().text("data").is_err());
    assert!(failed.require_complete().is_err());
    assert!(OutputSet::new(&[file.clone(), file], false).is_err());
}

#[test]
fn successful_emission_and_check_complete_only_their_declared_slots() {
    let dir = tempfile::tempdir().unwrap();
    let files = [dir.path().join("a"), dir.path().join("b")];
    let set = OutputSet::new(&files, false).unwrap();
    set.file(0, "first").unwrap().text("first").unwrap();
    assert!(set.require_complete().is_err());
    set.clone()
        .file(1, "second")
        .unwrap()
        .bytes(b"second")
        .unwrap();
    set.require_complete().unwrap();
    let check = OutputSet::new(&files, true).unwrap();
    check.file(0, "first").unwrap().text("first").unwrap();
    assert!(check.file(1, "second").unwrap().text("wrong").is_err());
    assert!(check.require_complete().is_err());
    assert_eq!(fs::read(&files[1]).unwrap(), b"second");
}

#[test]
fn cached_reuse_requires_actual_matching_bytes_and_does_not_consume_a_miss() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("result");
    let digest = format!("{:x}", Sha256::digest(b"expected"));
    let set = OutputSet::new(std::slice::from_ref(&path), false).unwrap();
    assert!(!set.reuse(0, &digest).unwrap());
    fs::write(&path, b"different").unwrap();
    assert!(!set.reuse(0, &digest).unwrap());
    assert!(set.require_complete().is_err());
    set.file(0, "restore")
        .unwrap()
        .verified_stream(b"expected".as_slice(), 8, &digest)
        .unwrap();
    set.require_complete().unwrap();
    let next = OutputSet::new(std::slice::from_ref(&path), false).unwrap();
    assert!(next.reuse(0, &digest).unwrap());
    next.require_complete().unwrap();
    assert!(next.file(0, "duplicate").is_err());
}

fn bundle_paths(root: &Path) -> Vec<PathBuf> {
    crate::artifacts::bundle_files(root)
        .chain(std::iter::once(root.join(super::super::coverage::FILE)))
        .collect()
}

#[test]
fn bundle_publication_requires_all_declared_members_before_touching_destination() {
    let dir = tempfile::tempdir().unwrap();
    let destination = dir.path().join("bundle");
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("previous"), "old").unwrap();
    let paths = bundle_paths(&destination);
    for missing_declaration in [true, false] {
        let stage = StagedLinkedIrBundle::fixture(dir.path().join("stage"));
        let set = if missing_declaration {
            OutputSet::new(&paths[..paths.len() - 1], false).unwrap()
        } else {
            fs::remove_file(stage.path().join(super::super::coverage::FILE)).unwrap();
            OutputSet::new(&paths, false).unwrap()
        };
        assert!(set.bundle(&destination, stage).is_err());
        assert!(set.require_complete().is_err());
        assert_eq!(
            fs::read_to_string(destination.join("previous")).unwrap(),
            "old"
        );
        assert!(!dir.path().join("stage").exists());
    }
}

#[test]
fn bundle_completion_requires_successful_swap_or_complete_comparison() {
    let dir = tempfile::tempdir().unwrap();
    let destination = dir.path().join("bundle");
    let paths = bundle_paths(&destination);
    let set = OutputSet::new(&paths, false).unwrap();
    assert!(
        set.bundle(
            &destination,
            StagedLinkedIrBundle::fixture(dir.path().join("stage"))
        )
        .unwrap()
        .is_empty()
    );
    set.require_complete().unwrap();
    assert!(set.file(0, "duplicate member").is_err());
    let check = OutputSet::new(&paths, true).unwrap();
    assert!(
        check
            .bundle(
                &destination,
                StagedLinkedIrBundle::fixture(dir.path().join("stage"))
            )
            .unwrap()
            .is_empty()
    );
    check.require_complete().unwrap();
    let coverage = destination.join(super::super::coverage::FILE);
    fs::write(&coverage, "stale").unwrap();
    let stale = OutputSet::new(&paths, true).unwrap();
    assert_eq!(
        stale
            .bundle(
                &destination,
                StagedLinkedIrBundle::fixture(dir.path().join("stage"))
            )
            .unwrap(),
        vec![coverage]
    );
    assert!(stale.require_complete().is_err());
}

#[test]
fn receipts_keep_emitted_content_even_when_collected_after_a_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("output");
    let set = OutputSet::new(std::slice::from_ref(&file), false).unwrap();
    set.file(0, "fixture").unwrap().text("original").unwrap();
    fs::write(&file, "modified").unwrap();
    let receipts = set.receipts().unwrap();
    assert_eq!(
        receipts[0].sha256(),
        format!("{:x}", Sha256::digest(b"original"))
    );
    assert!(
        receipts[0]
            .validate()
            .unwrap_err()
            .to_string()
            .contains("changed after emission")
    );
    fs::remove_file(file).unwrap();
    assert!(
        receipts[0]
            .validate()
            .unwrap_err()
            .to_string()
            .contains("unavailable")
    );

    let root = dir.path().join("bundle");
    let set = OutputSet::new(&bundle_paths(&root), false).unwrap();
    set.bundle(
        &root,
        StagedLinkedIrBundle::fixture(dir.path().join("stage")),
    )
    .unwrap();
    let member = crate::artifacts::bundle_files(&root).next().unwrap();
    fs::write(&member, "changed").unwrap();
    let receipts = set.receipts().unwrap();
    assert!(
        receipts
            .iter()
            .find(|receipt| receipt.path() == member)
            .unwrap()
            .validate()
            .is_err()
    );
}
