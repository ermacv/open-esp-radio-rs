use super::*;

#[test]
fn admission_authenticates_and_each_operation_retains_exclusion() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("device.lock");
    let token = format!(
        "{:_<64}",
        directory.path().file_name().unwrap().to_string_lossy()
    );
    let broker =
        LockBroker::start(FileLock::acquire(&path, Mode::Exclusive).unwrap(), &token).unwrap();
    let mut unauthenticated =
        UnixStream::connect_addr(&address(&std::fs::metadata(&path).unwrap()).unwrap()).unwrap();
    unauthenticated.set_read_timeout(Some(HANDSHAKE)).unwrap();
    unauthenticated.write_all(&[b'b'; TOKEN_BYTES]).unwrap();
    assert!(unauthenticated.read_exact(&mut [0]).is_err());
    let first = LockBroker::operation(&path, &token).unwrap();
    let second = LockBroker::operation(&path, &token).unwrap();
    drop(broker);
    assert!(LockBroker::operation(&path, &token).is_err());
    drop(first);
    assert!(
        FileLock::try_acquire(&path, Mode::Exclusive)
            .unwrap()
            .is_none()
    );
    drop(second);
    wait_for_release(&path);
}

fn wait_for_release(path: &Path) {
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while FileLock::try_acquire(path, Mode::Exclusive)
        .unwrap()
        .is_none()
    {
        assert!(
            std::time::Instant::now() < deadline,
            "broker retained completed I/O"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn release_ignores_unrelated_file_description_copies_after_io_drains() {
    let directory = tempfile::tempdir().unwrap();
    for draining in [false, true] {
        let path = directory.path().join(format!("device-{draining}.lock"));
        let token = "a".repeat(TOKEN_BYTES);
        let lock = FileLock::acquire(&path, Mode::Exclusive).unwrap();
        // A concurrent fork can retain this same open file description.
        // Keep a duplicate alive to make close-only release fail reliably.
        let unrelated = lock.file().try_clone().unwrap();
        let broker = LockBroker::start(lock, &token).unwrap();
        let operation = draining.then(|| LockBroker::operation(&path, &token).unwrap());
        drop(broker);
        if operation.is_some() {
            assert!(
                FileLock::try_acquire(&path, Mode::Exclusive)
                    .unwrap()
                    .is_none()
            );
        }
        drop(operation);
        wait_for_release(&path);
        drop(unrelated);
    }
}

#[test]
fn long_lock_paths_and_identical_filenames_have_independent_brokers() {
    let directory = tempfile::tempdir().unwrap();
    let long = directory.path().join("x".repeat(120));
    let first = long.join("first/device.lock");
    let second = long.join("second/device.lock");
    let token = format!(
        "{:_<64}",
        directory.path().file_name().unwrap().to_string_lossy()
    );
    let first_broker =
        LockBroker::start(FileLock::acquire(&first, Mode::Exclusive).unwrap(), &token).unwrap();
    let second_broker =
        LockBroker::start(FileLock::acquire(&second, Mode::Exclusive).unwrap(), &token).unwrap();
    let first_operation = LockBroker::operation(&first, &token).unwrap();
    let second_operation = LockBroker::operation(&second, &token).unwrap();
    drop(first_broker);
    assert!(LockBroker::operation(&first, &token).is_err());
    assert!(LockBroker::operation(&second, &token).is_ok());
    assert!(
        FileLock::try_acquire(&first, Mode::Exclusive)
            .unwrap()
            .is_none()
    );
    drop(first_operation);
    drop(second_operation);
    drop(second_broker);
    wait_for_release(&first);
    wait_for_release(&second);
}
