use super::*;

fn compiled(source: &str) -> (tempfile::TempDir, std::path::PathBuf, Output) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("contract.rs");
    fs::write(&path, source).unwrap();
    let output = compile(
        &path,
        directory.path(),
        &oer_toolchain::host_target().unwrap(),
        &Libraries::default(),
        true,
    )
    .unwrap();
    (directory, path, output)
}

#[test]
fn a_different_real_rustc_error_does_not_satisfy_the_contract() {
    let source = "pub fn f() { let _: u8 = false; } // ui-error: E0599\n";
    let (_directory, path, output) = compiled(source);
    assert!(validate(&output, &path, Some(&expectations(source).unwrap())).is_err());
    let correct = expectations(&source.replace("E0599", "E0308")).unwrap();
    validate(&output, &path, Some(&correct)).unwrap();
}

#[test]
fn an_unrelated_error_cannot_hide_beside_the_expected_one() {
    let source =
        "pub fn f() { let _: u8 = false; } // ui-error: E0308\npub fn g() { missing(); }\n";
    let (_directory, path, output) = compiled(source);
    assert!(validate(&output, &path, Some(&expectations(source).unwrap())).is_err());
}

#[test]
fn the_error_must_point_to_the_intended_boundary() {
    let source = "pub fn f() { let _: u8 = false; }\n// ui-error: E0308\n";
    let (_directory, path, output) = compiled(source);
    assert!(validate(&output, &path, Some(&expectations(source).unwrap())).is_err());
}

#[test]
fn a_positive_twin_must_compile() {
    let (_directory, path, output) = compiled("pub fn f() { missing(); }\n");
    assert!(validate(&output, &path, None).is_err());
    let (_directory, path, output) = compiled("pub fn f() { let _: u8 = 0; }\n");
    validate(&output, &path, None).unwrap();
}
