use super::*;

#[test]
fn admission_authenticates_and_each_operation_retains_exclusion() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("device.lock");
    let token = "a".repeat(TOKEN_BYTES);
    let broker =
        LockBroker::start(FileLock::acquire(&path, Mode::Exclusive).unwrap(), &token).unwrap();
    assert!(LockBroker::operation(broker.endpoint(), &"b".repeat(TOKEN_BYTES)).is_err());
    let first = LockBroker::operation(broker.endpoint(), &token).unwrap();
    let second = LockBroker::operation(broker.endpoint(), &token).unwrap();
    let endpoint = broker.endpoint().to_owned();
    drop(broker);
    assert!(LockBroker::operation(&endpoint, &token).is_err());
    drop(first);
    assert!(
        FileLock::try_acquire(&path, Mode::Exclusive)
            .unwrap()
            .is_none()
    );
    drop(second);
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while FileLock::try_acquire(&path, Mode::Exclusive)
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
