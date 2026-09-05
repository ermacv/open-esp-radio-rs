use super::*;

#[test]
fn capability_contract_rejects_stale_installations() {
    assert!(require_capabilities(&format!("{REQUIRED_CAPABILITIES}\n")).is_ok());
    assert!(require_capabilities("schema=1 client=1 managed=1\n").is_err());
    assert!(
        require_capabilities("schema=5 station_ap=he20,ht40 client=1 observer=ht40 managed=1\n")
            .is_err()
    );
}

#[cfg(unix)]
#[test]
fn installed_helper_contract_distinguishes_radio_wait_from_control_failures() {
    let helper =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../linux-net/open-radio-net");
    for (case, expected) in [
        ("connected", 0),
        ("timeout", ASSOCIATION_TIMEOUT),
        ("control-error", 7),
        ("malformed", 1),
    ] {
        let output = Command::new("bash")
            .args(["-c", include_str!("tests/client-wait.sh")])
            .env("OER_TEST_HELPER", &helper)
            .env("OER_TEST_CLIENT", case)
            .supervised_output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(expected),
            "{case}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let capabilities = String::from_utf8(output.stdout).unwrap();
        require_capabilities(&capabilities).unwrap();
    }
}
