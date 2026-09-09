use super::*;

#[test]
fn managed_capture_cleanup_never_takes_interface_ownership() {
    let directory = tempfile::tempdir().unwrap();
    let capture_dir = directory.path().join("capture");
    fs::create_dir(&capture_dir).unwrap();
    let remote = RemoteInterface {
        create_interface: false,
        interface: "test0".into(),
        directory: capture_dir.to_str().unwrap().into(),
    };
    fs::write(remote.capture(), b"capture").unwrap();
    // Execute the actual cleanup with an iw sentinel. A managed interface is
    // borrowed; cleanup must not even attempt an interface operation.
    let script = format!("iw() {{ exit 90; }}; {}", remote.cleanup_script());
    assert!(
        Command::new("sh")
            .args(["-c", &script])
            .status()
            .unwrap()
            .success()
    );
    assert!(!capture_dir.exists());
    // A repeated cleanup does not gain authority over an existing interface.
    assert!(
        Command::new("sh")
            .args(["-c", &script])
            .status()
            .unwrap()
            .success()
    );
}
