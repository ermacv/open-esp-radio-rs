use super::*;
use serde_json::json;

fn artifact(path: &str, test: bool) -> serde_json::Value {
    json!({
        "reason": "compiler-artifact", "package_id": "path+file:///phy#0.1.0",
        "manifest_path": "/phy/Cargo.toml",
        "target": {"kind": ["lib"], "crate_types": ["lib"], "name": "oer_esp32s31_phy",
            "src_path": "/phy/src/lib.rs", "edition": "2024", "doc": true, "doctest": true, "test": true},
        "profile": {"opt_level": "3", "debuginfo": 0, "debug_assertions": false, "overflow_checks": false, "test": test},
        "features": [], "filenames": [path], "executable": null, "fresh": true
    })
}

#[test]
fn selects_actual_library_output_and_ignores_test_artifacts() {
    let messages = format!(
        "{}\n{}\n",
        artifact("/arbitrary target/PHY.rlib", false),
        artifact("/test/PHY.rlib", true)
    );
    assert_eq!(
        phy_artifact(messages.as_bytes()).unwrap(),
        PathBuf::from("/arbitrary target/PHY.rlib")
    );
}

#[test]
fn missing_or_ambiguous_library_output_fails() {
    assert!(phy_artifact(b"{\"reason\":\"build-finished\",\"success\":true}\n").is_err());
    let messages = format!(
        "{}\n{}\n",
        artifact("/one.rlib", false),
        artifact("/two.rlib", false)
    );
    assert!(phy_artifact(messages.as_bytes()).is_err());
}

fn built(package: &str, kind: &str, file: &str) -> serde_json::Value {
    json!({
        "reason": "compiler-artifact", "package_id": format!("path+file:///{package}#0.1.0"),
        "manifest_path": format!("/{package}/Cargo.toml"),
        "target": {"kind": [kind], "crate_types": [kind], "name": package,
            "src_path": format!("/{package}/src/lib.rs"), "edition": "2024", "doc": true, "doctest": true, "test": true},
        "profile": {"opt_level": "3", "debuginfo": 0, "debug_assertions": false, "overflow_checks": false, "test": false},
        "features": [], "filenames": [file], "executable": null, "fresh": true
    })
}

fn names(packages: &[&str]) -> BTreeMap<Utf8PathBuf, String> {
    packages
        .iter()
        .map(|package| {
            (
                Utf8PathBuf::from(format!("/{package}/Cargo.toml")),
                (*package).to_owned(),
            )
        })
        .collect()
}

const TRIPLE: &str = "riscv32imafc-unknown-none-elf";

#[test]
fn only_packages_the_phy_build_compiles_are_checked() {
    // `syn` is in the workspace metadata (another package's derive enables
    // it) but the PHY build never compiles it: the check passes.
    let messages = format!(
        "{}\n{}\n",
        built(
            "bytemuck",
            "lib",
            &format!("/t/{TRIPLE}/release/deps/libbytemuck.rlib")
        ),
        built(
            "vcell",
            "lib",
            &format!("/t/{TRIPLE}/release/deps/libvcell.rlib")
        ),
    );
    let built = built_packages(
        messages.as_bytes(),
        &names(&["bytemuck", "vcell", "syn"]),
        TRIPLE,
    )
    .unwrap();
    assert_eq!(
        built.chip,
        BTreeSet::from(["bytemuck".to_owned(), "vcell".to_owned()])
    );
    assert!(built.host.is_empty());
    check_built_packages(&built).unwrap();
}

#[test]
fn a_chip_built_package_missing_from_the_list_fails() {
    let messages = format!(
        "{}\n",
        built(
            "unreviewed",
            "lib",
            &format!("/t/{TRIPLE}/release/deps/libunreviewed.rlib")
        ),
    );
    let built = built_packages(messages.as_bytes(), &names(&["unreviewed"]), TRIPLE).unwrap();
    assert!(check_built_packages(&built).is_err());
}

#[test]
fn a_proc_macro_the_build_runs_is_checked_as_a_host_package() {
    let messages = format!(
        "{}\n{}\n",
        built(
            "some_derive",
            "proc-macro",
            "/t/release/deps/libsome_derive.so"
        ),
        // A host library behind a proc macro or build script is not published.
        built("syn", "lib", "/t/release/deps/libsyn.rlib"),
    );
    let built =
        built_packages(messages.as_bytes(), &names(&["some_derive", "syn"]), TRIPLE).unwrap();
    assert_eq!(built.host, BTreeSet::from(["some_derive".to_owned()]));
    assert!(built.chip.is_empty());
    let error = check_built_packages(&built).unwrap_err().to_string();
    assert!(error.contains("host proc macro"), "{error}");
}
