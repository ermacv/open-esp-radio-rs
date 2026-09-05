use super::*;
use std::sync::mpsc;

#[test]
fn completed_bundle_survives_cache_reuse_and_failed_build_cleanup() {
    let root = tempfile::tempdir().unwrap();
    let first = Workspace::acquire(root.path()).unwrap();
    fs::create_dir_all(first.cache()).unwrap();
    let cached = first.cache().join("runtime");
    fs::write(&cached, b"first feature selection").unwrap();
    let runtime = first.snapshot(&cached, "runtime.elf").unwrap();
    fs::write(first.output().join("application.bin"), b"first image").unwrap();
    let first = first.finish();

    let second = Workspace::acquire(root.path()).unwrap();
    fs::write(&cached, b"second feature selection").unwrap();
    second.snapshot(&cached, "runtime.elf").unwrap();
    fs::write(second.output().join("application.bin"), b"second image").unwrap();
    let incomplete = second.output().to_owned();
    drop(second);

    assert!(!incomplete.exists());
    assert_eq!(fs::read(runtime).unwrap(), b"first feature selection");
    assert_eq!(
        fs::read(first.directory().join("application.bin")).unwrap(),
        b"first image"
    );
    assert_eq!(fs::read(cached).unwrap(), b"second feature selection");
}

#[test]
fn artifact_lease_spans_snapshotting_and_releases_before_flash() {
    let root = tempfile::tempdir().unwrap();
    let first = Workspace::acquire(root.path()).unwrap();
    let contender = OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.path().join("build.lock"))
        .unwrap();
    assert_eq!(
        contender.try_lock_exclusive().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    let build = first.finish();
    contender.try_lock_exclusive().unwrap();
    // Keeping the immutable bundle selected for flash does not retain the
    // compilation lease: another invocation can use the cache immediately.
    assert!(build.directory().is_dir());
    FileExt::unlock(&contender).unwrap();
}

#[test]
fn different_examples_can_hold_build_workspaces_concurrently() {
    let root = tempfile::tempdir().unwrap();
    let (ready, received) = mpsc::channel();
    std::thread::scope(|scope| {
        let mut releases = Vec::new();
        for name in ["station", "access-point"] {
            let root = root.path();
            let ready = ready.clone();
            let (release, finish) = mpsc::channel::<()>();
            releases.push(release);
            scope.spawn(move || {
                let workspace = Workspace::acquire(&root.join(name)).unwrap();
                ready.send(()).unwrap();
                let _ = finish.recv();
                assert!(workspace.output().is_dir());
            });
        }
        let first = received.recv_timeout(Duration::from_secs(5));
        let second = received.recv_timeout(Duration::from_secs(5));
        drop(releases);
        assert!(first.is_ok() && second.is_ok());
    });
}
