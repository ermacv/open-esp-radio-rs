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

#[test]
fn independent_observer_refuses_busy_phy_and_cleans_failed_channel_setup() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    for (name, script) in [
        (
            "iw",
            r#"#!/bin/sh
case "$*" in
  'phy phy0 info') echo ' * monitor';;
  'dev') if [ "$OER_TEST_BUSY" = 1 ]; then printf 'phy#0\n\tInterface existing-ap\n'; fi;;
  'dev observe0 info') exit 1;;
  'phy phy0 interface add observe0 type monitor') touch "$OER_TEST_CREATED";;
  'dev observe0 set freq 2472 HT40-') exit 5;;
  'dev observe0 del') rm -f "$OER_TEST_CREATED";;
  *) exit 91;;
esac
"#,
        ),
        ("ip", "#!/bin/sh\nexit 0\n"),
        ("tcpdump", "#!/bin/sh\nexit 92\n"),
    ] {
        let path = bin.join(name);
        fs::write(&path, script).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    for busy in [true, false] {
        let remote = RemoteInterface {
            create_interface: true,
            interface: "observe0".into(),
            directory: temp
                .path()
                .join(if busy { "busy" } else { "failed" })
                .to_str()
                .unwrap()
                .into(),
        };
        let config = crate::lab::config::AirObserverConfig {
            ssh_target: "unused".into(),
            phy: "phy0".into(),
            interface: "observe0".into(),
        };
        let script = remote
            .independent_script(
                &config,
                super::super::channel::Geometry {
                    frequency: 2472,
                    width: 40,
                    center: 2462,
                },
                "type mgt",
                Duration::from_secs(1),
            )
            .unwrap();
        let result = Command::new("sh")
            .args(["-c", &script])
            .env(
                "PATH",
                format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
            )
            .env("OER_TEST_BUSY", if busy { "1" } else { "0" })
            .env("OER_TEST_CREATED", temp.path().join("created"))
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(!temp.path().join("created").exists());
        assert_eq!(Path::new(&remote.directory).exists(), !busy);
    }
}

#[test]
fn managed_capture_retains_bytes_through_acknowledged_stop() {
    let directory = tempfile::tempdir().unwrap();
    let remote = RemoteInterface {
        create_interface: false,
        interface: "egress0".into(),
        directory: directory.path().join("owned").to_str().unwrap().into(),
    };
    let config = OpenWrtConfig {
        ht40_above: false,
        radio: "radio0".into(),
        ap_section: "ap0".into(),
        channel: 13,
        ssh_target: "unused".into(),
        wireless_interface: "egress0".into(),
        ingress_interface: "lan0".into(),
        monitor_interface: None,
        phys: vec![],
        read_only: false,
        independent_laptop_monitor: false,
    };
    // Fake the packet source, but execute the real remote lifetime/control
    // script. Readiness follows the file write; stop is a signal, not a delay.
    let source = r#"import pathlib,signal,sys
args=sys.argv[1:]
pathlib.Path(args[args.index('-w')+1]).write_bytes(b'pcap\x00retained')
def stop(*_):
    print('1 packets captured\n0 packets dropped by kernel',file=sys.stderr,flush=True)
    sys.exit(0)
signal.signal(signal.SIGTERM,stop)
print('tcpdump: listening on egress0,',file=sys.stderr,flush=True)
signal.pause()
"#;
    // start_script uses sh -c through timeout; export the source in a private
    // executable instead of relying on shell function export semantics.
    use std::os::unix::fs::PermissionsExt;
    let program = directory.path().join("tcpdump");
    fs::write(
        &program,
        format!(
            "#!/bin/sh\nexec python3 -c {} \"$@\"\n",
            capture_process::quote(source)
        ),
    )
    .unwrap();
    fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
    let mut command = Command::new("sh");
    command.args([
        "-c",
        &remote.start_script(&config, "udp", true, Duration::from_secs(1)),
    ]);
    command.env(
        "PATH",
        format!(
            "{}:{}",
            directory.path().display(),
            std::env::var("PATH").unwrap()
        ),
    );
    let child = capture_process::Capture::start(
        &mut command,
        "tcpdump: listening on egress0,".into(),
        Duration::from_secs(10),
    )
    .unwrap();
    assert_eq!(fs::read(remote.capture()).unwrap(), b"pcap\0retained");
    let result = child.finish().unwrap();
    assert!(result.status.success());
    assert_eq!(
        summary_value(
            std::str::from_utf8(&result.stderr).unwrap(),
            "packets captured"
        ),
        Some(1)
    );
    assert_eq!(fs::read(remote.capture()).unwrap(), b"pcap\0retained");
    assert!(
        Command::new("sh")
            .args(["-c", &remote.cleanup_script()])
            .status()
            .unwrap()
            .success()
    );
    assert!(!Path::new(&remote.directory).exists());
}
