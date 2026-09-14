use super::*;
use std::ffi::OsStr;

fn tiny_crate(source: &str) -> (tempfile::TempDir, Context, common::CargoConfiguration) {
    let repository = tempfile::tempdir().unwrap();
    fs::create_dir(repository.path().join("src")).unwrap();
    fs::write(
        repository.path().join("Cargo.toml"),
        "[package]\nname='doc-fixture'\nversion='0.0.0'\nedition='2024'\n[workspace]\n",
    )
    .unwrap();
    fs::write(repository.path().join("src/lib.rs"), source).unwrap();
    let context = Context::new(repository.path()).unwrap();
    process::run(context.cargo().args(["generate-lockfile", "--offline"])).unwrap();
    let manifest = repository.path().join("Cargo.toml").canonicalize().unwrap();
    let host = String::from_utf8(
        process::capture(context.command("rustc").arg("-vV"))
            .unwrap()
            .stdout,
    )
    .unwrap()
    .lines()
    .find_map(|line| line.strip_prefix("host: "))
    .unwrap()
    .to_owned();
    let configuration = common::CargoConfiguration {
        manifest: manifest.clone(),
        workspace_manifest: manifest,
        package: "doc-fixture".into(),
        target: host,
        features: Vec::new(),
        build_profile: common::CargoBuildProfile::Dev,
        cargo_target: common::CargoTargetSelection::Lib("doc_fixture".into()),
    };
    (repository, context, configuration)
}

#[test]
fn public_private_rustdoc_are_distinct_and_broken_links_fail_with_diagnostics() {
    let (_repository, context, configuration) =
        tiny_crate("//! A valid crate.\npub struct Item;\n");
    let output = acquire_output(&context).unwrap();
    let public = run_rustdoc(
        &context,
        &output,
        &Job {
            purpose: Purpose::PublicRustdoc,
            configuration: configuration.clone(),
        },
    )
    .unwrap();
    let private = run_rustdoc(
        &context,
        &output,
        &Job {
            purpose: Purpose::PrivateRustdoc,
            configuration: configuration.clone(),
        },
    )
    .unwrap();
    assert_ne!(public, private);
    assert!(public.join("doc_fixture/index.html").is_file());
    assert!(private.join("doc_fixture/index.html").is_file());
    assert!(public.join("static.files").is_dir());
    assert!(private.join("static.files").is_dir());

    fs::write(
        context.root.join("src/lib.rs"),
        "//! This [`Missing`] does not resolve.\npub struct Item;\n",
    )
    .unwrap();
    let error = run_rustdoc(
        &context,
        &output,
        &Job {
            purpose: Purpose::PublicRustdoc,
            configuration,
        },
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("unresolved link"), "{error}");
    assert!(error.contains("Missing"), "{error}");
}

#[test]
fn a_real_failing_host_doctest_is_not_reported_as_success() {
    let (_repository, context, configuration) =
        tiny_crate("//! ```\n//! assert_eq!(1, 2);\n//! ```\npub struct Item;\n");
    let output = acquire_output(&context).unwrap();
    let error = run_doctest(
        &context,
        &output,
        &Job {
            purpose: Purpose::HostDoctest,
            configuration,
        },
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("doctest failed"), "{error}");
    assert!(
        error.contains("assertion `left == right` failed"),
        "{error}"
    );
}

#[test]
fn mcu_compile_consumer_checks_the_body_without_running_it() {
    let (_repository, context, mut configuration) =
        tiny_crate("#![no_std]\npub fn example() -> u32 { 42 }\n");
    configuration.target = TARGET.into();
    let output = acquire_output(&context).unwrap();
    let job = Job {
        purpose: Purpose::McuCompileConsumer,
        configuration: configuration.clone(),
    };
    run_consumer(&context, &output, &job).unwrap();
    fs::write(
        context.root.join("src/lib.rs"),
        "#![no_std]\npub fn example() -> u32 { missing_api() }\n",
    )
    .unwrap();
    let error = run_consumer(
        &context,
        &output,
        &Job {
            purpose: Purpose::McuCompileConsumer,
            configuration,
        },
    )
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("cannot find function `missing_api`"),
        "{error}"
    );
}

#[test]
fn markdown_parser_checks_references_paths_anchors_and_source_lines() {
    let repository = tempfile::tempdir().unwrap();
    fs::write(repository.path().join("Cargo.toml"), "[workspace]\n").unwrap();
    fs::create_dir(repository.path().join("docs")).unwrap();
    fs::write(
        repository.path().join("docs/target file.md"),
        "# Héllo *world*\n\n# Héllo world\n\n<a id=\"manual\"></a>\n",
    )
    .unwrap();
    fs::write(repository.path().join("source.rs"), "one\ntwo\nthree\n").unwrap();
    let index = repository.path().join("docs/index.md");
    fs::write(
        &index,
        "# Index\n\n[target](target%20file.md#héllo-world)\n[duplicate](target%20file.md#héllo-world-1)\n[manual](target%20file.md#manual)\n[line](../source.rs#L2-L3)\n[external](https://example.com)\n\n```markdown\n[not a link](missing.md)\n```\n",
    )
    .unwrap();
    let context = Context::new(repository.path()).unwrap();
    let summary = check_markdown(&context, std::slice::from_ref(&index)).unwrap();
    assert_eq!(summary.documents, 2);
    assert_eq!(summary.local_links, 4);
    assert_eq!(summary.external_not_checked, 1);

    fs::write(&index, "[missing](missing.md)\n").unwrap();
    assert!(check_markdown(&context, std::slice::from_ref(&index)).is_err());
    fs::write(&index, "[anchor](target%20file.md#absent)\n").unwrap();
    assert!(check_markdown(&context, std::slice::from_ref(&index)).is_err());
    fs::write(&index, "[undefined][nowhere]\n").unwrap();
    let error = check_markdown(&context, std::slice::from_ref(&index))
        .unwrap_err()
        .to_string();
    assert!(error.contains("undefined references"), "{error}");
}

#[test]
fn owned_document_discovery_excludes_arbitrary_untracked_markdown() {
    let repository = tempfile::tempdir().unwrap();
    fs::write(repository.path().join("Cargo.toml"), "[workspace]\n").unwrap();
    fs::write(repository.path().join("README.md"), "# Root\n").unwrap();
    crate::process::run(context_command(
        repository.path(),
        "git",
        &["init", "--quiet"],
    ))
    .unwrap();
    crate::process::run(context_command(
        repository.path(),
        "git",
        &["add", "Cargo.toml", "README.md"],
    ))
    .unwrap();
    fs::write(
        repository.path().join("task-notes.md"),
        "[broken](missing)\n",
    )
    .unwrap();
    fs::create_dir_all(repository.path().join("docs")).unwrap();
    fs::write(repository.path().join("docs/new.md"), "# New\n").unwrap();
    fs::create_dir_all(repository.path().join("crate")).unwrap();
    fs::write(repository.path().join("crate/README.md"), "# Owner\n").unwrap();
    let context = Context::new(repository.path()).unwrap();
    let documents = owned_documents(&context, &[]).unwrap();
    assert!(documents.iter().any(|path| path.ends_with("README.md")));
    assert!(documents.iter().any(|path| path.ends_with("docs/new.md")));
    assert!(
        documents
            .iter()
            .any(|path| path.ends_with("crate/README.md"))
    );
    assert!(!documents.iter().any(|path| path.ends_with("task-notes.md")));
}

fn context_command<'a>(root: &Path, program: &'a str, arguments: &[&str]) -> &'a mut Command {
    let mut command = Box::new(Command::new(program));
    command.current_dir(root).args(arguments);
    Box::leak(command)
}

#[test]
fn occupied_output_without_owner_marker_fails_closed() {
    let repository = tempfile::tempdir().unwrap();
    fs::write(repository.path().join("Cargo.toml"), "[workspace]\n").unwrap();
    fs::create_dir_all(repository.path().join("target/docs/gate")).unwrap();
    let context = Context::new(repository.path()).unwrap();
    let error = acquire_output(&context).unwrap_err().to_string();
    assert!(error.contains("ownership marker"), "{error}");
}

#[test]
fn catalog_orchestration_propagates_owner_failures_before_render() {
    let group = CatalogGroup {
        chip: "chip".into(),
        catalogs: vec![PathBuf::from("catalog.toml")],
        programs: vec![PathBuf::from("unknown.toml"), PathBuf::from("exact.toml")],
    };
    for failed in ["catalog", "unknown.toml", "exact.toml"] {
        let mut actions = Vec::new();
        let error = run_catalog_actions(std::slice::from_ref(&group), |_, action| {
            let name = match action {
                CatalogAction::CheckCatalogs => "catalog",
                CatalogAction::CheckProgram(path) => path.to_str().unwrap(),
                CatalogAction::Render { .. } => "render",
            };
            actions.push(name.to_owned());
            if name == failed {
                Err(format!("owner rejected {failed}").into())
            } else {
                Ok(())
            }
        })
        .unwrap_err()
        .to_string();
        assert!(error.contains(failed), "{error}");
        assert!(!actions.iter().any(|action| action == "render"));
    }
}

#[test]
fn explicit_configuration_target_overrides_caller_defaults() {
    let (_repository, _context, mut configuration) = tiny_crate("pub struct Item;\n");
    configuration.target = TARGET.into();
    let mut command = Command::new(OsStr::new("cargo"));
    configuration.apply(&mut command);
    let arguments = command
        .get_args()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert!(
        arguments
            .windows(2)
            .any(|pair| pair == ["--target", TARGET])
    );
}

#[test]
fn planner_follows_transitive_required_features_like_cargo() {
    let repository = tempfile::tempdir().unwrap();
    for path in [
        "fixture/src",
        "examples/example/src",
        "qualification/catalog/chip",
    ] {
        fs::create_dir_all(repository.path().join(path)).unwrap();
    }
    fs::write(
        repository.path().join("Cargo.toml"),
        "[workspace]\nresolver='3'\nmembers=['fixture', 'examples/example']\n",
    )
    .unwrap();
    fs::write(repository.path().join("README.md"), "# Fixture\n").unwrap();
    fs::write(
        repository.path().join("fixture/Cargo.toml"),
        "[package]\nname='required-features-fixture'\nversion='0.0.0'\nedition='2024'\n\
         [package.metadata.open-radio]\nscope='production'\nlayer='facade'\nplatform='portable'\n\
         supported-feature-profiles=['profile']\n\
         [features]\ndefault=['profile']\nprofile=['api']\napi=[]\n\
         [[bin]]\nname='demo'\npath='src/main.rs'\nrequired-features=['api']\n",
    )
    .unwrap();
    fs::write(
        repository.path().join("fixture/src/lib.rs"),
        "pub struct Library;\n",
    )
    .unwrap();
    fs::write(
        repository.path().join("fixture/src/main.rs"),
        "fn main() {}\n",
    )
    .unwrap();
    fs::write(
        repository.path().join("examples/example/Cargo.toml"),
        "[package]\nname='example-fixture'\nversion='0.0.0'\nedition='2024'\n\
         [package.metadata.open-radio]\nscope='development'\nlayer='application'\nplatform='chip'\nchip='esp32s31'\n",
    )
    .unwrap();
    fs::write(
        repository.path().join("examples/example/src/main.rs"),
        "fn main() {}\n",
    )
    .unwrap();
    fs::write(
        repository
            .path()
            .join("qualification/catalog/chip/catalog.toml"),
        "schema = 1\n",
    )
    .unwrap();

    let context = Context::new(repository.path()).unwrap();
    process::run(context.command("git").args(["init", "--quiet"])).unwrap();
    process::run(context.cargo().args(["generate-lockfile", "--offline"])).unwrap();
    process::run(context.command("git").args(["add", "."])).unwrap();

    let plan = build_plan(&context, "x86_64-unknown-linux-gnu").unwrap();
    let demo_jobs = plan
        .jobs
        .iter()
        .filter(|job| {
            matches!(
                &job.configuration.cargo_target,
                common::CargoTargetSelection::Bin(name) if name == "demo"
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(demo_jobs.len(), 4);
    assert_eq!(
        demo_jobs
            .iter()
            .filter(|job| job.configuration.features.is_empty())
            .count(),
        2
    );
    assert_eq!(
        demo_jobs
            .iter()
            .filter(|job| {
                job.configuration.features == ["--no-default-features", "--features", "profile"]
            })
            .count(),
        2
    );
    assert!(!plan.inapplicable.iter().any(|entry| {
        entry.package == "required-features-fixture"
            && entry.target == "demo"
            && entry.action == "rustdoc"
    }));

    let mut package = common::source_packages(&context)
        .unwrap()
        .into_iter()
        .find(|item| item.package.name == "required-features-fixture")
        .unwrap()
        .package;
    package.features.insert(
        "profile".into(),
        vec![
            "dep:api".into(),
            "dependency/api".into(),
            "weak?/api".into(),
        ],
    );
    let profile = [
        "--no-default-features".into(),
        "--features".into(),
        "profile".into(),
    ];
    assert!(!required_features_enabled(&package, &profile, &["api".into()]).unwrap());
    assert!(
        required_features_enabled(&package, &["--all-features".into()], &["api".into()]).unwrap()
    );
    assert!(
        !required_features_enabled(&package, &["--no-default-features".into()], &["api".into()])
            .unwrap()
    );
    package
        .features
        .insert("profile".into(), vec!["api".into()]);
    package
        .features
        .insert("api".into(), vec!["profile".into()]);
    assert!(required_features_enabled(&package, &profile, &["api".into()]).unwrap());
    assert!(
        required_features_enabled(&package, &["--features=profile".into()], &["api".into()])
            .unwrap_err()
            .to_string()
            .contains("unsupported Cargo feature flag")
    );

    let manifest = repository.path().join("fixture/Cargo.toml");
    for flags in [
        Vec::<&str>::new(),
        vec!["--no-default-features", "--features", "profile"],
    ] {
        let mut command = context.cargo();
        command
            .args(["check", "--locked", "--offline", "--manifest-path"])
            .arg(&manifest)
            .args(["--package", "required-features-fixture", "--bin", "demo"])
            .args(flags);
        process::run(&mut command).unwrap();
    }
    let mut command = context.cargo();
    command
        .args(["check", "--locked", "--offline", "--manifest-path"])
        .arg(&manifest)
        .args([
            "--package",
            "required-features-fixture",
            "--bin",
            "demo",
            "--no-default-features",
        ]);
    let output = process::output(&mut command, None).unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("requires the features"), "{stderr}");
    assert!(stderr.contains("api"), "{stderr}");
}

#[test]
fn missing_or_mismatched_toolchain_identity_and_target_fail_closed() {
    assert!(validate_tool_release("rustdoc", "", "1.97.1").is_err());
    let error = validate_tool_release("rustc", "rustc 1.96.0\n", "1.97.1")
        .unwrap_err()
        .to_string();
    assert!(error.contains("does not match pinned toolchain"), "{error}");
    assert!(!declares_target(
        &[toml::Value::String("thumbv7em-none-eabi".into())],
        TARGET
    ));
}

#[test]
fn unavailable_offline_dependency_is_a_real_child_failure() {
    let repository = tempfile::tempdir().unwrap();
    fs::create_dir(repository.path().join("src")).unwrap();
    fs::write(
        repository.path().join("Cargo.toml"),
        "[package]\nname='offline-cache-fixture'\nversion='0.0.0'\nedition='2024'\n\
         [dependencies]\ndependency-that-cannot-exist = '=9999.0.0'\n[workspace]\n",
    )
    .unwrap();
    fs::write(repository.path().join("src/lib.rs"), "pub struct Item;\n").unwrap();
    let context = Context::new(repository.path()).unwrap();
    let mut command = context.cargo();
    command.args(["check", "--offline"]);
    let error = run_checked(&mut command).unwrap_err().to_string();
    assert!(error.contains("failed with"), "{error}");
    assert!(
        error.contains("no matching package named") || error.contains("not found"),
        "{error}"
    );
}
