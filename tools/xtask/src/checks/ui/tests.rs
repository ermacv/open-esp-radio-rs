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

#[test]
fn an_uncoded_error_requires_null_code_and_the_exact_message() {
    let source = "pub fn f<'a, 'b>(b: &'b ()) -> &'a () { b } // ui-error: null lifetime may not live long enough\n";
    let (_directory, path, output) = compiled(source);
    validate(&output, &path, Some(&expectations(source).unwrap())).unwrap();
    for wrong in ["E0521", "null another lifetime failure"] {
        let expected =
            expectations(&source.replace("null lifetime may not live long enough", wrong)).unwrap();
        assert!(validate(&output, &path, Some(&expected)).is_err());
    }
}

#[test]
fn multiple_errors_at_one_boundary_require_one_expectation_each() {
    let source =
        "pub fn f() { let _: u8 = false; let _: u8 = (); } // ui-error: E0308 // ui-error: E0308\n";
    let (_directory, path, output) = compiled(source);
    let mut expected = expectations(source).unwrap();
    validate(&output, &path, Some(&expected)).unwrap();
    expected.pop();
    assert!(validate(&output, &path, Some(&expected)).is_err());
    expected.push(expected[0].clone());
    expected.push(expected[0].clone());
    assert!(validate(&output, &path, Some(&expected)).is_err());
}
