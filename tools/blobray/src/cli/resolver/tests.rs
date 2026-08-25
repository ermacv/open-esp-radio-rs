use super::*;
use crate::{
    cli::{ImageAuditArgs, InspectFunctionArgs, IrExportArgs, ReferenceArgs},
    project::DEFAULT_PROJECT_MANIFEST,
};

fn fixture_directory(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!(
        "blobray-resolved-invocation-{}-{name}",
        std::process::id()
    ));
    if directory.exists() {
        std::fs::remove_dir_all(&directory).unwrap();
    }
    std::fs::create_dir_all(&directory).unwrap();
    directory
}

fn write_target(path: &std::path::Path) {
    std::fs::write(
        path,
        "schema = 3\n\
             id = \"fixture\"\n\
             architecture = \"riscv32\"\n\
             calling-convention = \"riscv-ilp32\"\n\
             endianness = \"little\"\n\
             pointer-width = 32\n\
             rust-target = \"riscv32imafc-unknown-none-elf\"\n",
    )
    .unwrap();
}

fn write_project(path: &std::path::Path, extra: &str) {
    std::fs::write(
        path,
        format!(
            "schema = 4\n\
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
        "blobray-resolver-{}-{}.toml",
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
        ResolutionNeeds::new(true, true, false, false, true, true, true)
            .with_review_context()
            .with_configured_knowledge_provider()
    );
    assert_eq!(
        ResolutionNeeds::for_command(&Command::ProjectVerify(Default::default())),
        ResolutionNeeds::new(true, true, false, true, true, true, true)
            .with_review_context()
            .with_configured_knowledge_provider()
    );
    assert_eq!(
        ResolutionNeeds::for_command(&Command::RegisterValidate(Default::default())),
        ResolutionNeeds::new(true, false, false, false, true, true, true).with_review_context()
    );
    assert_eq!(
        ResolutionNeeds::for_command(&Command::ExecuteRun(Default::default())),
        ResolutionNeeds::new(false, true, false, true, true, true, true).with_review_context()
    );
    assert_eq!(
        ResolutionNeeds::for_command(&Command::GenerateReference(Default::default())),
        ResolutionNeeds::new(true, true, true, true, true, true, true).with_review_context()
    );
}

#[test]
fn composite_analysis_requires_only_a_selected_harness() {
    let analysis = ResolutionNeeds::for_command(&Command::ProjectAnalyze(Default::default()));
    assert!(!analysis.requires_knowledge_provider(false));
    assert!(analysis.requires_knowledge_provider(true));

    let status = ResolutionNeeds::for_command(&Command::ProjectStatus(Default::default()));
    assert!(!status.requires_knowledge_provider(true));
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
fn function_investigation_receives_every_origin_archive_for_its_source() {
    let (path, run_spec) = run_spec(
        "function-origin-archives",
        "[[inputs]]\nrole = \"source-artifact:wifi\"\npath = \"wifi.elf\"\n\n[[inputs]]\nrole = \"source-inventory:wifi\"\npath = \"libnet80211.a\"\n\n[[inputs]]\nrole = \"source-inventory:wifi\"\npath = \"libpp.a\"\n\n[[inputs]]\nrole = \"source-inventory:other\"\npath = \"other.a\"\n",
    );
    let mut command = Command::InspectFunction(InspectFunctionArgs {
        selector: "wifi:lmacRxDone".to_owned(),
        ..Default::default()
    });
    apply_run_spec_defaults(&mut command, &run_spec);
    std::fs::remove_file(path).unwrap();
    let Command::InspectFunction(arguments) = command else {
        panic!("unexpected argument type")
    };
    assert!(arguments.artifact.unwrap().ends_with("wifi.elf"));
    assert_eq!(arguments.inventory.len(), 2);
    assert!(arguments.inventory[0].ends_with("libnet80211.a"));
    assert!(arguments.inventory[1].ends_with("libpp.a"));
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
    write_target(&directory.join("target.toml"));
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
    write_target(&directory.join("target.toml"));
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
fn project_cache_stats_resolves_only_the_manifest() {
    let directory = fixture_directory("cache-stats");
    write_project(
        &directory.join(DEFAULT_PROJECT_MANIFEST),
        "run-spec = \"missing-local.toml\"\n",
    );

    let resolved = resolve_from(parse(&["project", "cache", "stats"]), &directory).unwrap();
    let ResolvedInvocation::ProjectCacheStats { project_path } = resolved else {
        panic!("expected a project-cache stats invocation")
    };
    assert_eq!(project_path, directory.join(DEFAULT_PROJECT_MANIFEST));

    let error = resolve_from(
        parse(&["project", "cache", "stats", "--run-spec", "override.toml"]),
        &directory,
    )
    .err()
    .expect("cache stats must reject run-spec overrides");
    assert!(
        error
            .to_string()
            .contains("accepts a project manifest, not --target-spec, --run-spec or --svd")
    );

    let missing = directory.join("missing-vendor-project.toml");
    let error = resolve_from(
        parse(&[
            "project",
            "cache",
            "stats",
            "--project",
            missing.to_str().unwrap(),
        ]),
        &directory,
    )
    .err()
    .expect("cache stats must reject a missing manifest");
    assert!(error.to_string().contains("project manifest"));
    assert!(error.to_string().contains("is unavailable"));

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn project_cache_maintenance_resolves_only_the_manifest() {
    let directory = fixture_directory("cache-maintenance");
    write_project(
        &directory.join(DEFAULT_PROJECT_MANIFEST),
        "run-spec = \"missing-local.toml\"\n",
    );

    let resolved =
        resolve_from(parse(&["project", "cache", "gc", "--dry-run"]), &directory).unwrap();
    let ResolvedInvocation::ProjectCacheGc {
        arguments,
        project_path,
    } = resolved
    else {
        panic!("expected a project-cache gc invocation")
    };
    assert!(arguments.dry_run);
    assert_eq!(project_path, directory.join(DEFAULT_PROJECT_MANIFEST));

    let resolved = resolve_from(parse(&["project", "cache", "compact"]), &directory).unwrap();
    let ResolvedInvocation::ProjectCacheCompact { project_path, .. } = resolved else {
        panic!("expected a project-cache compact invocation")
    };
    assert_eq!(project_path, directory.join(DEFAULT_PROJECT_MANIFEST));

    let error = resolve_from(
        parse(&["project", "cache", "compact", "--run-spec", "override.toml"]),
        &directory,
    )
    .err()
    .expect("cache compact must reject run-spec overrides");
    assert!(
        error
            .to_string()
            .contains("accepts a project manifest, not --target-spec, --run-spec or --svd")
    );

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn sibling_local_run_is_discovered_but_manifest_configuration_wins() {
    let directory = fixture_directory("local-run-discovery");
    write_target(&directory.join("target.toml"));
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
fn explicit_target_run_spec_and_svd_override_project_defaults() {
    let directory = fixture_directory("precedence");
    write_target(&directory.join("target.toml"));
    let explicit_target = directory.join("explicit target's.toml");
    write_target(&explicit_target);
    std::fs::write(
        &explicit_target,
        std::fs::read_to_string(&explicit_target)
            .unwrap()
            .replace("id = \"fixture\"", "id = \"explicit-fixture\""),
    )
    .unwrap();
    write_project(
        &directory.join(DEFAULT_PROJECT_MANIFEST),
        "run-spec = \"project.toml\"\nchip-pack = \"chip.toml\"\n[analysis.symbols]\noutput = \"generated/symbols.json\"\n[analysis.navigation]\noutput = \"generated/navigation.json\"\n",
    );
    std::fs::write(
        directory.join("chip.toml"),
        "schema = 3\nid = \"fixture-chip\"\nsvd = [\"project.svd\"]\nknowledge-provider = \"none\"\nknowledge-packs = []\n",
    )
    .unwrap();
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
            "--target-spec",
            explicit_target.to_str().unwrap(),
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
    assert_eq!(session.target_path, explicit_target);
    assert_eq!(session.target.id, "explicit-fixture");
    assert_eq!(session.run_spec_path, Some(directory.join("explicit.toml")));
    assert!(
        session.run_spec.as_ref().unwrap().inputs()[0]
            .path
            .ends_with("explicit.elf")
    );
    assert_eq!(session.svd_paths, [PathBuf::from("cli.svd")]);
    assert_eq!(
        session.explicit_context.target_spec,
        Some(explicit_target.clone())
    );
    assert_eq!(
        session.explicit_context.run_spec,
        Some(directory.join("explicit.toml"))
    );
    assert_eq!(
        session.explicit_context.svd_paths,
        [PathBuf::from("cli.svd")]
    );

    let context = session.context();
    let action = context
        .follow_up_action(
            [
                "inspect",
                "function",
                "fixture:has space;$(touch must-not-run)",
            ],
            crate::application::ProjectContextRequirement::Analysis,
        )
        .unwrap();
    assert_eq!(action.working_directory, directory);
    assert_eq!(
        &action.argv[..4],
        [
            "blobray",
            "inspect",
            "function",
            "fixture:has space;$(touch must-not-run)",
        ]
    );
    assert!(
        action
            .argv
            .windows(2)
            .any(|pair| pair == ["--svd", "cli.svd"])
    );
    let analyze = context
        .follow_up_action(
            ["project", "analyze"],
            crate::application::ProjectContextRequirement::Analysis,
        )
        .unwrap();
    assert_eq!(
        analyze.argv,
        vec![
            "blobray".to_owned(),
            "project".to_owned(),
            "analyze".to_owned(),
            "--project".to_owned(),
            directory
                .join(DEFAULT_PROJECT_MANIFEST)
                .to_str()
                .unwrap()
                .to_owned(),
            "--target-spec".to_owned(),
            explicit_target.to_str().unwrap().to_owned(),
            "--run-spec".to_owned(),
            directory.join("explicit.toml").to_str().unwrap().to_owned(),
            "--svd".to_owned(),
            "cli.svd".to_owned(),
        ]
    );
    assert_eq!(
        context.inputs_init_help_action().unwrap().argv,
        vec![
            "blobray".to_owned(),
            "project".to_owned(),
            "inputs".to_owned(),
            "init".to_owned(),
            "--project".to_owned(),
            directory
                .join(DEFAULT_PROJECT_MANIFEST)
                .to_str()
                .unwrap()
                .to_owned(),
            "--output".to_owned(),
            directory.join("explicit.toml").to_str().unwrap().to_owned(),
            "--help".to_owned(),
        ]
    );
    assert_eq!(
        context
            .follow_up_action(
                ["project", "files"],
                crate::application::ProjectContextRequirement::Target,
            )
            .unwrap()
            .argv,
        vec![
            "blobray".to_owned(),
            "project".to_owned(),
            "files".to_owned(),
            "--project".to_owned(),
            directory
                .join(DEFAULT_PROJECT_MANIFEST)
                .to_str()
                .unwrap()
                .to_owned(),
            "--target-spec".to_owned(),
            explicit_target.to_str().unwrap().to_owned(),
        ]
    );

    let report = crate::application::status::collect(&context);
    let actions = report
        .phases
        .iter()
        .flat_map(|phase| &phase.components)
        .filter_map(|component| component.next_step.as_ref())
        .flat_map(|step| &step.commands)
        .collect::<Vec<_>>();
    assert!(
        actions
            .iter()
            .any(|candidate| candidate.argv == analyze.argv)
    );

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn explicit_cli_arguments_remain_authoritative_after_full_resolution() {
    let directory = fixture_directory("cli-input");
    write_target(&directory.join("target.toml"));
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
    write_target(&directory.join("target.toml"));
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
