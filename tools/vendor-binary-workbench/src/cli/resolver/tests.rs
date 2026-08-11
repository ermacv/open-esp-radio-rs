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
    let svd = svd
        .map(|path| format!("svd = [\"{path}\"]\n"))
        .unwrap_or_default();
    std::fs::write(
        path,
        format!(
            "schema = 1\n\
             id = \"fixture\"\n\
             architecture = \"riscv32\"\n\
             calling-convention = \"riscv-ilp32\"\n\
             endianness = \"little\"\n\
             pointer-width = 32\n\
             rust-target = \"riscv32imafc-unknown-none-elf\"\n\
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
             target-spec = \"target.toml\"\n\
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
        "vendor-workbench-resolver-{}-{}.toml",
        std::process::id(),
        name
    ));
    std::fs::write(&path, format!("schema = 1\n{contents}")).unwrap();
    let run_spec = RunSpec::load(&path).unwrap();
    (path, run_spec)
}

#[test]
fn command_resources_are_classified_by_one_positive_plan() {
    assert_eq!(
        ResolutionNeeds::for_command(&Command::ProjectStatus(Default::default())),
        ResolutionNeeds::new(true, false, false, false, true, false, true)
    );
    assert_eq!(
        ResolutionNeeds::for_command(&Command::ProjectAnalyze(Default::default())),
        ResolutionNeeds::new(true, true, false, false, true, true, true).with_configured_harness()
    );
    assert_eq!(
        ResolutionNeeds::for_command(&Command::ProjectVerify(Default::default())),
        ResolutionNeeds::new(true, true, false, true, true, true, true).with_configured_harness()
    );
    assert_eq!(
        ResolutionNeeds::for_command(&Command::RegisterValidate(Default::default())),
        ResolutionNeeds::new(true, false, false, false, true, true, false)
    );
    assert_eq!(
        ResolutionNeeds::for_command(&Command::ExecuteRun(Default::default())),
        ResolutionNeeds::new(false, true, false, true, true, true, true)
    );
    assert_eq!(
        ResolutionNeeds::for_command(&Command::GenerateReference(Default::default())),
        ResolutionNeeds::new(true, true, true, true, true, true, true)
    );
}

#[test]
fn composite_analysis_requires_only_a_selected_harness() {
    let analysis = ResolutionNeeds::for_command(&Command::ProjectAnalyze(Default::default()));
    assert!(!analysis.requires_harness(false));
    assert!(analysis.requires_harness(true));

    let status = ResolutionNeeds::for_command(&Command::ProjectStatus(Default::default()));
    assert!(!status.requires_harness(true));
}

#[test]
fn explicit_cli_paths_win_over_all_run_spec_defaults() {
    let (path, run_spec) = run_spec(
        "explicit",
        "[[inputs]]\nrole = \"artifact\"\npath = \"default.elf\"\n\n[[inputs]]\nrole = \"companion\"\npath = \"first.a\"\n\n[[inputs]]\nrole = \"companion\"\npath = \"second.a\"\n",
    );
    let mut command = Command::GenerateReference(ReferenceArgs {
        artifact: Some("explicit.elf".into()),
        companion: vec!["explicit.a".into()],
        ..Default::default()
    });
    apply_run_spec_defaults(&mut command, &run_spec);
    std::fs::remove_file(path).unwrap();
    let Command::GenerateReference(arguments) = command else {
        panic!("unexpected argument type")
    };
    assert_eq!(arguments.artifact, Some("explicit.elf".into()));
    assert_eq!(arguments.companion, [PathBuf::from("explicit.a")]);
}

#[test]
fn all_default_companions_are_preserved_when_cli_has_none() {
    let (path, run_spec) = run_spec(
        "companions",
        "[[inputs]]\nrole = \"artifact\"\npath = \"default.elf\"\n\n[[inputs]]\nrole = \"companion\"\npath = \"first.a\"\n\n[[inputs]]\nrole = \"companion\"\npath = \"second.a\"\n",
    );
    let mut command = Command::GenerateReference(ReferenceArgs::default());
    apply_run_spec_defaults(&mut command, &run_spec);
    std::fs::remove_file(path).unwrap();
    let Command::GenerateReference(arguments) = command else {
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
        "[[inputs]]\nrole = \"source-artifact:rom\"\npath = \"default-rom.elf\"\n\n[[inputs]]\nrole = \"source-artifact:archive\"\npath = \"archive.elf\"\n",
    );
    let mut command = Command::ExportIr(IrExportArgs {
        artifact: vec!["rom=explicit-rom.elf".parse().unwrap()],
        ..Default::default()
    });
    apply_run_spec_defaults(&mut command, &run_spec);
    std::fs::remove_file(path).unwrap();
    let Command::ExportIr(arguments) = command else {
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
fn one_ir_source_receives_only_its_source_qualified_companion() {
    let (path, run_spec) = run_spec(
        "source-companion",
        "[[inputs]]\nrole = \"source-artifact:rom\"\npath = \"rom.elf\"\n\n[[inputs]]\nrole = \"source-companion:rom\"\npath = \"rom-companion.elf\"\n\n[[inputs]]\nrole = \"source-companion:archive\"\npath = \"archive-companion.elf\"\n",
    );
    let mut command = Command::ExportIr(IrExportArgs::default());
    apply_run_spec_defaults(&mut command, &run_spec);
    std::fs::remove_file(path).unwrap();
    let Command::ExportIr(arguments) = command else {
        panic!("unexpected argument type")
    };
    assert_eq!(arguments.artifact.len(), 1);
    assert_eq!(arguments.companion.len(), 1);
    assert!(arguments.companion[0].ends_with("rom-companion.elf"));
}

#[test]
fn discovered_project_resolves_project_owned_paths_from_the_manifest() {
    let directory = fixture_directory("discovery");
    let nested = directory.join("nested");
    std::fs::create_dir(&nested).unwrap();
    write_target(&directory.join("target.toml"), None);
    write_project(
        &directory.join(DEFAULT_PROJECT_MANIFEST),
        "run-spec = \"local.toml\"\n",
    );
    std::fs::write(
        directory.join("local.toml"),
        "schema = 1\n\n[[inputs]]\nrole = \"artifact\"\npath = \"artifacts/vendor.elf\"\n",
    )
    .unwrap();

    let resolved = resolve_from(parse(&["project", "status"]), &nested).unwrap();
    let ResolvedInvocation::ProjectStatus { session, .. } = resolved else {
        panic!("expected a resolved project-status command")
    };
    assert_eq!(session.manifest, directory.join(DEFAULT_PROJECT_MANIFEST));
    assert_eq!(session.target_path, directory.join("target.toml"));
    assert_eq!(session.run_spec_path, Some(directory.join("local.toml")));
    assert_eq!(
        session.run_spec.unwrap().inputs()[0].path,
        directory.join("artifacts/vendor.elf")
    );

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn project_browser_resolves_only_the_manifest_for_the_application_frontend() {
    let directory = fixture_directory("browser");
    write_target(&directory.join("target.toml"), None);
    write_project(&directory.join(DEFAULT_PROJECT_MANIFEST), "");

    let resolved = resolve_from(parse(&["project", "browse"]), &directory).unwrap();
    let ResolvedInvocation::ProjectBrowse { project_path, .. } = resolved else {
        panic!("expected a project-browser invocation")
    };
    assert_eq!(project_path, directory.join(DEFAULT_PROJECT_MANIFEST));

    let error = resolve_from(
        parse(&["project", "browse", "--svd", "override.svd"]),
        &directory,
    )
    .err()
    .expect("browser must reject CLI catalog overrides");
    assert!(
        error
            .to_string()
            .contains("not --target-spec, --run-spec or --svd")
    );

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn sibling_local_run_is_discovered_but_manifest_configuration_wins() {
    let directory = fixture_directory("local-run-discovery");
    write_target(&directory.join("target.toml"), None);
    write_project(&directory.join(DEFAULT_PROJECT_MANIFEST), "");
    std::fs::write(
        directory.join("local.toml"),
        "schema = 1\n\n[[inputs]]\nrole = \"artifact\"\npath = \"discovered.elf\"\n",
    )
    .unwrap();

    let resolved = resolve_from(
        parse(&[
            "project",
            "status",
            "--project",
            directory.join(DEFAULT_PROJECT_MANIFEST).to_str().unwrap(),
        ]),
        &directory,
    )
    .unwrap();
    let ResolvedInvocation::ProjectStatus { session, .. } = resolved else {
        panic!("expected a resolved project-status command")
    };
    assert_eq!(session.run_spec_path, Some(directory.join("local.toml")));
    assert!(
        session.run_spec.unwrap().inputs()[0]
            .path
            .ends_with("discovered.elf")
    );

    write_project(
        &directory.join(DEFAULT_PROJECT_MANIFEST),
        "run-spec = \"configured.toml\"\n",
    );
    std::fs::write(
        directory.join("configured.toml"),
        "schema = 1\n\n[[inputs]]\nrole = \"artifact\"\npath = \"configured.elf\"\n",
    )
    .unwrap();
    let resolved = resolve_from(
        parse(&[
            "project",
            "status",
            "--project",
            directory.join(DEFAULT_PROJECT_MANIFEST).to_str().unwrap(),
        ]),
        &directory,
    )
    .unwrap();
    let ResolvedInvocation::ProjectStatus { session, .. } = resolved else {
        panic!("expected a resolved project-status command")
    };
    assert_eq!(
        session.run_spec_path,
        Some(directory.join("configured.toml"))
    );

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn explicit_run_spec_and_svd_override_project_defaults() {
    let directory = fixture_directory("precedence");
    write_target(&directory.join("target.toml"), Some("target.svd"));
    write_project(
        &directory.join(DEFAULT_PROJECT_MANIFEST),
        "run-spec = \"project.toml\"\nsvd = [\"project.svd\"]\n",
    );
    std::fs::write(
        directory.join("project.toml"),
        "schema = 1\n\n[[inputs]]\nrole = \"artifact\"\npath = \"project.elf\"\n",
    )
    .unwrap();
    std::fs::write(
        directory.join("explicit.toml"),
        "schema = 1\n\n[[inputs]]\nrole = \"artifact\"\npath = \"explicit.elf\"\n",
    )
    .unwrap();

    let resolved = resolve_from(
        parse(&[
            "project",
            "status",
            "--project",
            directory.join(DEFAULT_PROJECT_MANIFEST).to_str().unwrap(),
            "--run-spec",
            directory.join("explicit.toml").to_str().unwrap(),
            "--svd",
            "cli.svd",
        ]),
        &directory,
    )
    .unwrap();
    let ResolvedInvocation::ProjectStatus { session, .. } = resolved else {
        panic!("expected a resolved project-status command")
    };
    assert_eq!(session.run_spec_path, Some(directory.join("explicit.toml")));
    assert!(
        session.run_spec.unwrap().inputs()[0]
            .path
            .ends_with("explicit.elf")
    );
    assert_eq!(session.svd_paths, [PathBuf::from("cli.svd")]);

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn explicit_cli_arguments_remain_authoritative_after_full_resolution() {
    let directory = fixture_directory("cli-input");
    write_target(&directory.join("target.toml"), None);
    write_project(
        &directory.join(DEFAULT_PROJECT_MANIFEST),
        "run-spec = \"project.toml\"\n",
    );
    std::fs::write(
        directory.join("project.toml"),
        "schema = 1\n\n[[inputs]]\nrole = \"artifact\"\npath = \"project.elf\"\n",
    )
    .unwrap();

    let resolved = resolve_from(
        parse(&[
            "advanced",
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
    let ResolvedInvocation::Target {
        command: TargetCommand::AuditImageTargets(ImageAuditArgs { artifact, .. }),
        ..
    } = resolved
    else {
        panic!("expected image-audit arguments")
    };
    assert_eq!(artifact, Some(PathBuf::from("cli.elf")));

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn project_symbol_inventory_supplies_the_default_report_path() {
    let directory = fixture_directory("symbol-inventory");
    write_target(&directory.join("target.toml"), None);
    write_project(
        &directory.join(DEFAULT_PROJECT_MANIFEST),
        "run-spec = \"project.toml\"\n[analysis.symbols]\noutput = \"generated/symbols.json\"\n",
    );
    std::fs::write(
        directory.join("project.toml"),
        "schema = 1\n\n[[inputs]]\nrole = \"artifact\"\npath = \"vendor.elf\"\n",
    )
    .unwrap();

    let resolved = resolve_from(
        parse(&[
            "advanced",
            "symbols",
            "inventory",
            "--check",
            "--project",
            directory.join(DEFAULT_PROJECT_MANIFEST).to_str().unwrap(),
        ]),
        &directory,
    )
    .unwrap();
    let ResolvedInvocation::SymbolInventory { arguments, .. } = resolved else {
        panic!("expected symbol-inventory arguments")
    };
    assert!(arguments.check);
    assert_eq!(
        arguments.output,
        Some(directory.join("generated/symbols.json"))
    );

    std::fs::remove_dir_all(directory).unwrap();
}
