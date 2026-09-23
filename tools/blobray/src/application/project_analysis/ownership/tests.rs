use super::*;

fn catalog(protected: Vec<PathBuf>) -> OutputCatalog {
    OutputCatalog {
        claims: Vec::new(),
        lookup: BTreeMap::new(),
        work_outputs: BTreeMap::new(),
        unavailable: BTreeMap::new(),
        protected,
    }
}

fn work(key: &str, outputs: Vec<PathBuf>) -> ResolvedWork {
    ResolvedWork::new(
        key.into(),
        "fixture".into(),
        Vec::new(),
        outputs,
        false,
        Some("fixture"),
    )
    .unwrap()
}

#[test]
fn producers_are_exact_declared_files_and_bundle_roots() {
    let dir = tempfile::tempdir().unwrap();
    let mut catalog = catalog(Vec::new());
    for id in ["first", "second"] {
        let key = format!("linked-ir:{id}");
        let root = dir.path().join(id);
        catalog.claim(&key, &root, OutputKind::Bundle).unwrap();
        let file = root.join("facts.json");
        catalog.file(&key, &file).unwrap();
        for path in [&root, &file] {
            assert_eq!(catalog.producer(path).unwrap().unwrap().work, key);
        }
        for path in [dir.path().to_owned(), root.join("undeclared.json")] {
            assert!(catalog.producer(&path).unwrap().is_none());
        }
        assert!(catalog.validate(&work(&key, vec![file])).is_ok());
    }
    assert!(
        catalog
            .producer(&dir.path().join("third"))
            .unwrap()
            .is_none()
    );
}

#[test]
fn file_and_bundle_overlaps_are_rejected_for_any_declaration_order() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("bundle");
    let child = root.join("symbols.json");
    for reverse in [false, true] {
        let mut catalog = catalog(Vec::new());
        let claims = [
            ("linked-ir:fixture", &root, OutputKind::Bundle),
            ("symbol-inventory", &child, OutputKind::File),
        ];
        let [first, second] = if reverse {
            [claims[1], claims[0]]
        } else {
            claims
        };
        catalog.claim(first.0, first.1, first.2).unwrap();
        let error = catalog.claim(second.0, second.1, second.2).unwrap_err();
        assert!(error.to_string().contains("conflicting output ownership"));
    }
    let mut catalog = catalog(Vec::new());
    catalog.file("symbol-inventory", &child).unwrap();
    assert!(catalog.file("interface-discovery", &child).is_err());
    // Duplicate declarations by the same work are also an invalid output set.
    assert!(catalog.file("symbol-inventory", &child).is_err());
}

#[test]
fn source_and_reviewed_inputs_cannot_be_outputs_or_owned_bundle_members() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("reviewed/model.toml");
    std::fs::create_dir_all(input.parent().unwrap()).unwrap();
    std::fs::write(&input, "reviewed evidence").unwrap();
    let mut catalog = catalog(vec![input.clone()]);
    for path in [&input, input.parent().unwrap()] {
        let error = catalog
            .claim("linked-ir:fixture", path, OutputKind::Bundle)
            .unwrap_err();
        assert!(error.to_string().contains("protected analysis input"));
    }
    assert!(
        catalog
            .file("symbol-inventory", &input.join("child"))
            .is_err()
    );
    assert_eq!(std::fs::read_to_string(input).unwrap(), "reviewed evidence");
}

#[test]
fn work_cannot_change_its_output_set_or_order_after_capture() {
    let dir = tempfile::tempdir().unwrap();
    let mut catalog = catalog(Vec::new());
    let files = [dir.path().join("first"), dir.path().join("second")];
    for file in &files {
        catalog.file("event-replays", file).unwrap();
    }
    assert!(
        catalog
            .validate(&work("event-replays", files.to_vec()))
            .is_ok()
    );
    for outputs in [
        vec![],
        vec![files[0].clone()],
        vec![files[1].clone(), files[0].clone()],
    ] {
        assert!(
            catalog
                .validate(&work("event-replays", outputs))
                .unwrap_err()
                .to_string()
                .contains("output set or order")
        );
    }
}

#[test]
fn normalized_paths_keep_collisions_and_reject_regular_file_ancestors() {
    let dir = tempfile::tempdir().unwrap();
    let subdir = dir.path().join("subdir");
    std::fs::create_dir(&subdir).unwrap();
    let file = dir.path().join("file");
    std::fs::write(&file, "original").unwrap();
    let mut catalog = catalog(Vec::new());
    catalog.file("symbol-inventory", &file).unwrap();
    assert!(
        catalog
            .file("interface-discovery", &subdir.join("../file"))
            .is_err()
    );
    assert_eq!(
        catalog
            .producer(&subdir.join("../file"))
            .unwrap()
            .unwrap()
            .work,
        "symbol-inventory"
    );
    assert!(resolve_location(&file.join("../output")).is_err());
    assert!(resolve_location(&file.join("child")).is_err());
}

#[cfg(unix)]
#[test]
fn symlink_resolution_precedes_parent_components_and_rebinding_is_rejected() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real/nested");
    std::fs::create_dir_all(&real).unwrap();
    let link = dir.path().join("link");
    symlink(&real, &link).unwrap();
    let mut catalog = catalog(Vec::new());
    let file = link.join("output");
    catalog.file("symbol-inventory", &file).unwrap();
    assert_eq!(
        catalog
            .producer(&real.join("output"))
            .unwrap()
            .unwrap()
            .work,
        "symbol-inventory"
    );
    assert_eq!(
        resolve_location(&link.join("../sibling")).unwrap(),
        dir.path().join("real/sibling")
    );
    let work = work("symbol-inventory", vec![file]);
    assert!(catalog.validate(&work).is_ok());
    std::fs::remove_file(&link).unwrap();
    symlink(dir.path(), &link).unwrap();
    assert!(
        catalog
            .validate(&work)
            .unwrap_err()
            .to_string()
            .contains("binding changed")
    );
}

#[cfg(unix)]
#[test]
fn existing_hard_links_cannot_alias_other_outputs_or_protected_inputs() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    let alias = dir.path().join("alias");
    std::fs::write(&source, "preserved").unwrap();
    std::fs::hard_link(&source, &alias).unwrap();
    let mut protected = catalog(vec![source.clone()]);
    assert!(protected.file("symbol-inventory", &alias).is_err());
    let mut outputs = catalog(Vec::new());
    outputs.file("symbol-inventory", &source).unwrap();
    assert!(outputs.file("interface-discovery", &alias).is_err());
    std::fs::remove_file(&alias).unwrap();
    outputs.file("interface-discovery", &alias).unwrap();
    std::fs::hard_link(&source, &alias).unwrap();
    assert!(
        outputs
            .validate(&work("symbol-inventory", vec![source]))
            .unwrap_err()
            .to_string()
            .contains("alias changed")
    );
}
