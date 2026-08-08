use super::*;
use crate::{
    cli::{ImageAuditArgs, IrExportArgs, ReferenceArgs},
    project::DEFAULT_PROJECT_MANIFEST,
};

fn fixture_directory(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!(
        "vendor-workbench-resolved-invocation-{}-{name}",
        std::process::id()
    ));
    if directory.exists() {
        std::fs::remove_dir_all(&directory).unwrap();
    }
    std::fs::create_dir_all(&directory).unwrap();
    directory
}

fn write_target(path: &std::path::Path, svd: Option<&str>) {
    let svd = svd.map(|path| format!("svd {path}\n")).unwrap_or_default();
    std::fs::write(
        path,
        format!(
            "schema 1\n\
             target fixture\n\
             architecture riscv32\n\
             calling-convention riscv-ilp32\n\
             endianness little\n\
             pointer-width 32\n\
             rust-target riscv32imafc-unknown-none-elf\n\
             {svd}"
        ),
    )
    .unwrap();
}

fn write_project(path: &std::path::Path, extra: &str) {
    std::fs::write(
        path,
        format!(
            "schema = 1\n\
             id = \"fixture-project\"\n\
             target-spec = \"target.spec\"\n\
             {extra}"
        ),
    )
    .unwrap();
}

fn parse(arguments: &[&str]) -> ParsedInvocation {
    ParsedInvocation::parse(arguments.iter().map(|value| (*value).to_owned())).unwrap()
}

fn run_spec(name: &str, contents: &str) -> (std::path::PathBuf, RunSpec) {
    let path = std::env::temp_dir().join(format!(
        "vendor-workbench-resolver-{}-{}.run",
        std::process::id(),
        name
    ));
    std::fs::write(&path, format!("schema 1\n{contents}")).unwrap();
    let run_spec = RunSpec::load(&path).unwrap();
    (path, run_spec)
}

#[test]
fn explicit_cli_paths_win_over_all_run_spec_defaults() {
    let (path, run_spec) = run_spec(
        "explicit",
        "input artifact default.elf\ninput companion first.a\ninput companion second.a\n",
    );
    let mut arguments = CommandArguments::Reference(ReferenceArgs {
        artifact: Some("explicit.elf".into()),
        companion: vec!["explicit.a".into()],
        ..Default::default()
    });
    apply_run_spec_defaults(Command::GenerateReference, &mut arguments, &run_spec);
    std::fs::remove_file(path).unwrap();
    let CommandArguments::Reference(arguments) = arguments else {
        panic!("unexpected argument type")
    };
    assert_eq!(arguments.artifact, Some("explicit.elf".into()));
    assert_eq!(arguments.companion, [PathBuf::from("explicit.a")]);
}

#[test]
fn all_default_companions_are_preserved_when_cli_has_none() {
    let (path, run_spec) = run_spec(
        "companions",
        "input artifact default.elf\ninput companion first.a\ninput companion second.a\n",
    );
    let mut arguments = CommandArguments::Reference(ReferenceArgs::default());
    apply_run_spec_defaults(Command::GenerateReference, &mut arguments, &run_spec);
    std::fs::remove_file(path).unwrap();
    let CommandArguments::Reference(arguments) = arguments else {
        panic!("unexpected argument type")
    };
    assert!(arguments.artifact.unwrap().ends_with("default.elf"));
    assert_eq!(arguments.companion.len(), 2);
    assert!(arguments.companion[0].ends_with("first.a"));
    assert!(arguments.companion[1].ends_with("second.a"));
}

#[test]
fn source_qualified_cli_artifacts_override_only_the_same_source() {
    let (path, run_spec) = run_spec(
        "sources",
        "input source-artifact:rom default-rom.elf\ninput source-artifact:archive archive.elf\n",
    );
    let mut arguments = CommandArguments::IrExport(IrExportArgs {
        artifact: vec!["rom=explicit-rom.elf".parse().unwrap()],
        ..Default::default()
    });
    apply_run_spec_defaults(Command::ExportIr, &mut arguments, &run_spec);
    std::fs::remove_file(path).unwrap();
    let CommandArguments::IrExport(arguments) = arguments else {
        panic!("unexpected argument type")
    };
    assert_eq!(arguments.artifact[0].source.as_str(), "rom");
    assert_eq!(
        arguments.artifact[0].path,
        PathBuf::from("explicit-rom.elf")
    );
    assert_eq!(arguments.artifact[1].source.as_str(), "archive");
    assert!(arguments.artifact[1].path.ends_with("archive.elf"));
}

#[test]
fn discovered_project_resolves_project_owned_paths_from_the_manifest() {
    let directory = fixture_directory("discovery");
    let nested = directory.join("nested");
    std::fs::create_dir(&nested).unwrap();
    write_target(&directory.join("target.spec"), None);
    write_project(
        &directory.join(DEFAULT_PROJECT_MANIFEST),
        "run-spec = \"local.run\"\n",
    );
    std::fs::write(
        directory.join("local.run"),
        "schema 1\ninput artifact artifacts/vendor.elf\n",
    )
    .unwrap();

    let resolved = resolve_from(parse(&["project", "status"]), &nested).unwrap();
    let ResolvedInvocation::Command(resolved) = resolved else {
        panic!("expected an ordinary resolved command")
    };
    assert_eq!(
        resolved.project_path,
        Some(directory.join(DEFAULT_PROJECT_MANIFEST))
    );
    assert_eq!(resolved.target_path, directory.join("target.spec"));
    assert_eq!(resolved.run_spec_path, Some(directory.join("local.run")));
    assert_eq!(
        resolved.run_spec.unwrap().inputs()[0].path,
        directory.join("artifacts/vendor.elf")
    );

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn explicit_run_spec_and_svd_override_project_defaults() {
    let directory = fixture_directory("precedence");
    write_target(&directory.join("target.spec"), Some("target.svd"));
    write_project(
        &directory.join(DEFAULT_PROJECT_MANIFEST),
        "run-spec = \"project.run\"\nsvd = [\"project.svd\"]\n",
    );
    std::fs::write(
        directory.join("project.run"),
        "schema 1\ninput artifact project.elf\n",
    )
    .unwrap();
    std::fs::write(
        directory.join("explicit.run"),
        "schema 1\ninput artifact explicit.elf\n",
    )
    .unwrap();

    let resolved = resolve_from(
        parse(&[
            "project",
            "status",
            "--project",
            directory.join(DEFAULT_PROJECT_MANIFEST).to_str().unwrap(),
            "--run-spec",
            directory.join("explicit.run").to_str().unwrap(),
            "--svd",
            "cli.svd",
        ]),
        &directory,
    )
    .unwrap();
    let ResolvedInvocation::Command(resolved) = resolved else {
        panic!("expected an ordinary resolved command")
    };
    assert_eq!(resolved.run_spec_path, Some(directory.join("explicit.run")));
    assert!(
        resolved.run_spec.unwrap().inputs()[0]
            .path
            .ends_with("explicit.elf")
    );
    assert_eq!(resolved.svd_paths, [PathBuf::from("cli.svd")]);

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn explicit_cli_arguments_remain_authoritative_after_full_resolution() {
    let directory = fixture_directory("cli-input");
    write_target(&directory.join("target.spec"), None);
    write_project(
        &directory.join(DEFAULT_PROJECT_MANIFEST),
        "run-spec = \"project.run\"\n",
    );
    std::fs::write(
        directory.join("project.run"),
        "schema 1\ninput artifact project.elf\n",
    )
    .unwrap();

    let resolved = resolve_from(
        parse(&[
            "image",
            "audit-targets",
            "--project",
            directory.join(DEFAULT_PROJECT_MANIFEST).to_str().unwrap(),
            "--artifact",
            "cli.elf",
        ]),
        &directory,
    )
    .unwrap();
    let ResolvedInvocation::Command(resolved) = resolved else {
        panic!("expected an ordinary resolved command")
    };
    let CommandArguments::ImageAudit(ImageAuditArgs { artifact, .. }) = resolved.arguments else {
        panic!("expected image-audit arguments")
    };
    assert_eq!(artifact, Some(PathBuf::from("cli.elf")));

    std::fs::remove_dir_all(directory).unwrap();
}
