use super::*;
use std::path::PathBuf;

const ALPHA: &str =
    include_str!("../../../../hil/tests/fixtures/catalog/z-system/alpha-system.toml");
const BETA: &str = include_str!("../../../../hil/tests/fixtures/catalog/a-system/beta-system.toml");
static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
struct Tree(PathBuf);
impl Tree {
    fn new() -> Self {
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("oer-evaluator-catalog-{}-{id}", std::process::id()));
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join("catalog")).unwrap();
        Self(root)
    }
    fn write(&self, name: &str, contents: &str) {
        let path = self.0.join("catalog").join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }
}
impl Drop for Tree {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn nested_catalog_consumes_shared_serialized_documents() {
    let tree = Tree::new();
    tree.write("z-system/alpha-system.toml", ALPHA);
    tree.write("a-system/beta-system.toml", BETA);
    tree.write("README.md", "catalog documentation");
    tree.write("a-system/README.md", "domain documentation");
    let catalog = ScenarioCatalog::load(&tree.0, Path::new("catalog")).unwrap();
    assert_eq!(
        catalog
            .repetitions
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["alpha-system", "beta-system"]
    );
    catalog
        .validate_requirement(&HilRequirement {
            scenario: "alpha-system".into(),
            checks: vec![],
            minimum_repetitions: 3,
        })
        .unwrap();
    assert!(
        catalog
            .validate_requirement(&HilRequirement {
                scenario: "alpha-system".into(),
                checks: vec![],
                minimum_repetitions: 4
            })
            .is_err()
    );
}

#[test]
fn catalog_rejects_ambiguous_or_unsupported_inputs() {
    for (name, contents) in [
        ("other/alpha-system.toml", ALPHA.to_owned()),
        ("wrong-name.toml", BETA.to_owned()),
        ("hidden.txt", "unexpected file".to_owned()),
        ("beta-system.toml", BETA.replace("schema = 5", "schema = 4")),
        (
            "beta-system.toml",
            BETA.replace("repetitions = 3", "repetitions = 0"),
        ),
    ] {
        let tree = Tree::new();
        tree.write("domain/alpha-system.toml", ALPHA);
        tree.write(name, &contents);
        assert!(
            ScenarioCatalog::load(&tree.0, Path::new("catalog")).is_err(),
            "accepted {name}"
        );
    }
    let tree = Tree::new();
    tree.write("README.md", "no scenarios");
    assert!(ScenarioCatalog::load(&tree.0, Path::new("catalog")).is_err());
}

#[cfg(unix)]
#[test]
fn catalog_rejects_symlink_files_directories_and_roots() {
    use std::os::unix::fs::symlink;
    for directory in [false, true] {
        let tree = Tree::new();
        tree.write("domain/alpha-system.toml", ALPHA);
        if directory {
            symlink(tree.0.join("catalog/domain"), tree.0.join("catalog/alias")).unwrap();
        } else {
            symlink(
                tree.0.join("catalog/domain/alpha-system.toml"),
                tree.0.join("catalog/alias.toml"),
            )
            .unwrap();
        }
        assert!(ScenarioCatalog::load(&tree.0, Path::new("catalog")).is_err());
    }
    let tree = Tree::new();
    tree.write("alpha-system.toml", ALPHA);
    fs::rename(tree.0.join("catalog"), tree.0.join("actual")).unwrap();
    symlink(tree.0.join("actual"), tree.0.join("catalog")).unwrap();
    assert!(ScenarioCatalog::load(&tree.0, Path::new("catalog")).is_err());
}

#[cfg(unix)]
#[test]
fn catalog_rejects_symlink_in_supplied_path_ancestors() {
    let tree = Tree::new();
    let outside = Tree::new();
    outside.write("alpha-system.toml", ALPHA);
    std::os::unix::fs::symlink(&outside.0, tree.0.join("alias")).unwrap();
    assert!(ScenarioCatalog::load(&tree.0, Path::new("alias/catalog")).is_err());
}

#[test]
fn a_joint_coexistence_table_is_one_family() {
    let tree = Tree::new();
    tree.write(
        "coexistence/joint.toml",
        "schema = 5\nid = \"joint\"\ndescription = \"joint\"\nrole = \"investigation\"\n\n[coexistence]\nphy = \"ht20\"\nduration_seconds = 12\n",
    );
    let catalog = ScenarioCatalog::load(&tree.0, Path::new("catalog")).unwrap();
    assert!(catalog.repetitions.contains_key("joint"));
    tree.write(
        "coexistence/both.toml",
        "schema = 5\nid = \"both\"\ndescription = \"both\"\nrole = \"investigation\"\n\n[coexistence]\nphy = \"ht20\"\n\n[system]\nkind = \"boot-smoke\"\n",
    );
    assert!(ScenarioCatalog::load(&tree.0, Path::new("catalog")).is_err());
}

#[test]
fn an_unsupported_scenario_stays_an_open_gap_with_its_reason() {
    let tree = Tree::new();
    tree.write(
        "alpha-system.toml",
        &ALPHA
            .replacen("\n[", "\nunsupported = \"no image serves it\"\n[", 1)
            .replace("role = \"investigation\"", "role = \"qualification\""),
    );
    let catalog = ScenarioCatalog::load(&tree.0, Path::new("catalog")).unwrap();
    assert_eq!(
        catalog.unsupported("alpha-system"),
        Some("no image serves it")
    );
    let decision = HilEvidenceIndex::default().decision_for(
        &HilRequirement {
            scenario: "alpha-system".into(),
            checks: Vec::new(),
            minimum_repetitions: 1,
        },
        &catalog,
    );
    assert_eq!(decision.status, EvidenceStatus::Missing);
    let (kind, reason) = decision.next_work().unwrap();
    assert_eq!(kind, crate::model::WorkKind::Implement);
    assert!(reason.contains("no image serves it"));
}

#[test]
fn programs_decide_scenario_roles_and_an_investigation_scenario_never_qualifies() {
    let tree = Tree::new();
    tree.write("alpha-system.toml", ALPHA);
    tree.write(
        "beta-system.toml",
        &BETA.replace("role = \"investigation\"", "role = \"qualification\""),
    );
    let catalog = ScenarioCatalog::load(&tree.0, Path::new("catalog")).unwrap();
    let error = catalog.check_roles(&BTreeSet::new()).unwrap_err();
    assert!(error.to_string().contains("beta-system"), "{error}");
    catalog
        .check_roles(&BTreeSet::from([String::from("beta-system")]))
        .unwrap();
    let index = HilEvidenceIndex::synthetic(&[("alpha-system", 1)]);
    let decision = index.decision_for(
        &HilRequirement {
            scenario: "alpha-system".into(),
            checks: Vec::new(),
            minimum_repetitions: 1,
        },
        &catalog,
    );
    assert!(decision.investigation);
    assert_eq!(
        decision.status,
        EvidenceStatus::Missing,
        "a pass never counts"
    );
    assert!(decision.evidence.is_none());
    let (kind, reason) = decision.next_work().unwrap();
    assert_eq!(kind, crate::model::WorkKind::Implement);
    assert!(reason.contains("investigation scenario"), "{reason}");
}
