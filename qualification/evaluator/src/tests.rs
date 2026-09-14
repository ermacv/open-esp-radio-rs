use super::*;
use std::path::Path;

#[test]
fn command_requires_one_explicit_manifest() {
    let parsed = parse_arguments([
        "evaluate".to_owned(),
        "--manifest".to_owned(),
        "qualification/test.toml".to_owned(),
    ])
    .unwrap();
    assert_eq!(parsed.command, Command::Evaluate);
    assert_eq!(
        parsed.manifest.as_deref(),
        Some(Path::new("qualification/test.toml"))
    );
    assert!(parsed.json_report.is_none());
    assert!(parsed.output_directory.is_none());
}

#[test]
fn removed_check_command_is_rejected() {
    let error = parse_arguments([
        "check".to_owned(),
        "--manifest".to_owned(),
        "qualification/test.toml".to_owned(),
    ])
    .unwrap_err();
    assert!(error.to_string().contains("unknown qualification command"));
}

#[test]
fn command_rejects_silent_extra_options() {
    let error = parse_arguments([
        "gate".to_owned(),
        "--manifest".to_owned(),
        "test.toml".to_owned(),
        "--best-effort".to_owned(),
    ])
    .unwrap_err();
    assert!(error.to_string().contains("unknown option"));
}

#[test]
fn catalog_render_requires_an_output_directory() {
    let error = parse_arguments([
        "catalog".to_owned(),
        "render".to_owned(),
        "--manifest".to_owned(),
        "qualification/test.toml".to_owned(),
    ])
    .unwrap_err();
    assert!(error.to_string().contains("requires --out"));

    let parsed = parse_arguments([
        "catalog".to_owned(),
        "render".to_owned(),
        "--manifest".to_owned(),
        "qualification/test.toml".to_owned(),
        "--out".to_owned(),
        "target/qualification/catalog".to_owned(),
    ])
    .unwrap();
    assert_eq!(parsed.command, Command::CatalogRender);
    assert_eq!(
        parsed.output_directory.as_deref(),
        Some(Path::new("target/qualification/catalog"))
    );
}

#[test]
fn static_catalog_form_is_distinct_from_manifest_form() {
    let parsed = parse_arguments([
        "catalog".to_owned(),
        "check".to_owned(),
        "--catalog".to_owned(),
        "qualification/catalog/a.toml".to_owned(),
        "--catalog".to_owned(),
        "qualification/catalog/b.toml".to_owned(),
    ])
    .unwrap();
    assert!(parsed.manifest.is_none());
    assert_eq!(parsed.catalogs.len(), 2);

    let error = parse_arguments([
        "catalog".to_owned(),
        "check".to_owned(),
        "--manifest".to_owned(),
        "qualification/program.toml".to_owned(),
        "--catalog".to_owned(),
        "qualification/catalog/a.toml".to_owned(),
    ])
    .unwrap_err();
    assert!(error.to_string().contains("exactly one"));
}

#[test]
fn help_forms_are_explicit_and_finite() {
    assert!(is_help(&["--help".to_owned()]));
    assert!(is_help(&["catalog".to_owned(), "--help".to_owned()]));
    assert!(!is_help(&["evaluate".to_owned(), "--help".to_owned(),]));
}
