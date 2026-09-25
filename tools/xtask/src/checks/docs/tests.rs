use super::*;
use std::process::Command;

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
    fs::write(
        repository.path().join("CONTRIBUTING.md"),
        "# Contributing\n",
    )
    .unwrap();
    fs::create_dir_all(repository.path().join("crate")).unwrap();
    fs::write(repository.path().join("crate/README.md"), "# Owner\n").unwrap();
    let context = Context::new(repository.path()).unwrap();
    let documents = owned_documents(&context, &[]).unwrap();
    assert!(documents.iter().any(|path| path.ends_with("README.md")));
    assert!(documents.iter().any(|path| path.ends_with("docs/new.md")));
    assert!(
        documents
            .iter()
            .any(|path| path.ends_with("CONTRIBUTING.md"))
    );
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
fn catalog_orchestration_propagates_owner_failures_before_render() {
    let group = CatalogGroup {
        chip: "chip".into(),
        catalogs: vec![PathBuf::from("catalog.toml")],
        programs: vec![PathBuf::from("unknown.toml"), PathBuf::from("exact.toml")],
    };
    for failed in ["catalog", "unknown.toml", "exact.toml"] {
        let mut actions = Vec::new();
        let error = run_catalog_actions(
            Path::new("target/docs/static"),
            std::slice::from_ref(&group),
            |_, action| {
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
            },
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains(failed), "{error}");
        assert!(!actions.iter().any(|action| action == "render"));
    }
}
