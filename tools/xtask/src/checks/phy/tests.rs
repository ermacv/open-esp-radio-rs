use super::*;
use serde_json::json;

/// A chip's PHY library as its profile names it.
const PHY_PACKAGE: &str = "oer-chip-a-phy";

fn library() -> oer_repo::chips::profile::PhyLibrary {
    oer_repo::chips::profile::PhyLibrary {
        package: PHY_PACKAGE.to_owned(),
        packages: vec![PHY_PACKAGE.to_owned()],
    }
}

fn artifact(path: &str, test: bool) -> serde_json::Value {
    json!({
        "reason": "compiler-artifact", "package_id": "path+file:///phy#0.1.0",
        "manifest_path": "/phy/Cargo.toml",
        "target": {"kind": ["lib"], "crate_types": ["lib"], "name": "oer_chip_a_phy",
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
        phy_artifact(messages.as_bytes(), PHY_PACKAGE).unwrap(),
        PathBuf::from("/arbitrary target/PHY.rlib")
    );
}

#[test]
fn missing_or_ambiguous_library_output_fails() {
    assert!(
        phy_artifact(
            b"{\"reason\":\"build-finished\",\"success\":true}\n",
            PHY_PACKAGE
        )
        .is_err()
    );
    let messages = format!(
        "{}\n{}\n",
        artifact("/one.rlib", false),
        artifact("/two.rlib", false)
    );
    assert!(phy_artifact(messages.as_bytes(), PHY_PACKAGE).is_err());
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

fn chip(package: &str) -> serde_json::Value {
    built(
        package,
        "lib",
        &format!("/t/{TRIPLE}/release/deps/lib{package}.rlib"),
    )
}

fn stream(artifacts: &[serde_json::Value]) -> String {
    artifacts
        .iter()
        .map(|artifact| format!("{artifact}\n"))
        .collect()
}

#[test]
fn only_packages_the_phy_build_compiles_are_checked() {
    // `syn` is in the workspace metadata (another package's derive enables
    // it) but the PHY build never compiles it: the check passes.
    let messages = stream(&[chip("bytemuck"), chip(PHY_PACKAGE)]);
    let built = built_packages(
        messages.as_bytes(),
        &names(&["bytemuck", PHY_PACKAGE, "syn"]),
        TRIPLE,
    )
    .unwrap();
    assert_eq!(
        built.chip,
        BTreeSet::from(["bytemuck".to_owned(), PHY_PACKAGE.to_owned()])
    );
    assert!(built.host.is_empty());
    check_built_packages(&built, &library()).unwrap();
}

#[test]
fn a_chip_built_package_missing_from_the_list_fails() {
    let messages = stream(&[chip("unreviewed"), chip(PHY_PACKAGE)]);
    let built = built_packages(
        messages.as_bytes(),
        &names(&["unreviewed", PHY_PACKAGE]),
        TRIPLE,
    )
    .unwrap();
    let error = check_built_packages(&built, &library())
        .unwrap_err()
        .to_string();
    assert!(error.contains("unreviewed"), "{error}");
}

#[test]
fn a_proc_macro_the_build_runs_is_checked_as_a_host_package() {
    let messages = stream(&[
        built(
            "some_derive",
            "proc-macro",
            "/t/release/deps/libsome_derive.so",
        ),
        // A host library behind a proc macro or build script is not published.
        built("syn", "lib", "/t/release/deps/libsyn.rlib"),
        chip(PHY_PACKAGE),
    ]);
    let built = built_packages(
        messages.as_bytes(),
        &names(&["some_derive", "syn", PHY_PACKAGE]),
        TRIPLE,
    )
    .unwrap();
    assert_eq!(built.host, BTreeSet::from(["some_derive".to_owned()]));
    assert_eq!(built.chip, BTreeSet::from([PHY_PACKAGE.to_owned()]));
    let error = check_built_packages(&built, &library())
        .unwrap_err()
        .to_string();
    assert!(error.contains("host proc macro"), "{error}");
}

#[test]
fn a_build_whose_outputs_the_target_rule_misses_fails_instead_of_passing_empty() {
    // The PHY's rlib outside a target-triple directory: nothing is recognized
    // as chip-built, which must not read as a clean audit.
    let messages = stream(&[built(PHY_PACKAGE, "lib", "/t/release/deps/libphy.rlib")]);
    let built = built_packages(messages.as_bytes(), &names(&[PHY_PACKAGE]), TRIPLE).unwrap();
    assert!(built.chip.is_empty());
    let error = check_built_packages(&built, &library())
        .unwrap_err()
        .to_string();
    assert!(error.contains(PHY_PACKAGE), "{error}");
}
