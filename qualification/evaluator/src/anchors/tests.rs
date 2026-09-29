use super::*;

/// The marker spelled so this file does not anchor anything itself.
fn marker(ids: &str) -> String {
    format!("// {}: {ids}", "CAPABILITY")
}

struct Repository {
    path: PathBuf,
}

impl Repository {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("open-radio-anchors-{}-{name}", std::process::id()));
        if path.exists() {
            fs::remove_dir_all(&path).unwrap();
        }
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("Cargo.toml"), "[workspace]\n").unwrap();
        Self { path }
    }

    fn package(&self, directory: &str, scope: &str, platform: &str) {
        let root = self.path.join(directory);
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{}\"\nversion.workspace = true\n\n\
                 [package.metadata.open-radio]\nscope = \"{scope}\"\nlayer = \"hardware\"\n\
                 platform = \"{platform}\"\n",
                directory.replace('/', "-")
            ),
        )
        .unwrap();
    }

    fn source(&self, path: &str, text: &str) {
        fs::create_dir_all(self.path.join(path).parent().unwrap()).unwrap();
        fs::write(self.path.join(path), text).unwrap();
    }
}

impl Drop for Repository {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.path).unwrap();
    }
}

fn entry(id: &str, kind: EntryKind, expectation: Expectation) -> Entry {
    Entry {
        id: id.into(),
        kind,
        owner: "wifi".into(),
        expectation,
        packages: Vec::new(),
    }
}

fn anchor(id: &str, scope: Scope) -> Anchor {
    Anchor {
        id: id.into(),
        location: Location {
            path: format!("crates/{id}.rs").into(),
            line: 1,
        },
        item: "pub struct Owner".into(),
        package: Package {
            name: "owner".into(),
            scope,
        },
    }
}

fn messages(problems: &[Problem]) -> Vec<String> {
    let mut lines = problems.iter().map(ToString::to_string).collect::<Vec<_>>();
    lines.sort();
    lines
}

#[test]
fn a_scan_attributes_markers_above_items_to_their_package() {
    let repository = Repository::new("scan");
    repository.package("crates/wifi", "production", "chip");
    repository.source(
        "crates/wifi/src/lib.rs",
        &format!(
            "{}\n/// Docs stay between.\n#[derive(\n    Debug,\n)]\npub(crate) struct Station;\n\n\
             {}\nconst fn plan() {{}}\n\n{}\nlet floating = 1;\n\n{}\nfn f() {{}}\n",
            marker("station, reconnect"),
            marker("planner"),
            marker("detached"),
            marker("Bad_Id"),
        ),
    );
    repository.source(
        "stray/lib.rs",
        &format!("{}\nfn stray() {{}}\n", marker("stray")),
    );
    let (anchors, problems) = scan(&repository.path).unwrap();
    let found = anchors
        .iter()
        .map(|a| {
            (
                a.id.as_str(),
                a.location.line,
                a.item.as_str(),
                a.package.scope,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        found,
        [
            (
                "station",
                1,
                "pub(crate) struct Station;",
                Scope::Production
            ),
            (
                "reconnect",
                1,
                "pub(crate) struct Station;",
                Scope::Production
            ),
            ("planner", 8, "const fn plan() {}", Scope::Production),
        ]
    );
    assert_eq!(
        messages(&problems),
        [
            "crates/wifi/src/lib.rs:11: CAPABILITY anchor must sit directly above the Rust item it names",
            "crates/wifi/src/lib.rs:14: malformed CAPABILITY anchor: `Bad_Id` is not a catalog id",
            "stray/lib.rs:1: CAPABILITY anchor outside a package with open-radio scope",
        ]
    );
}

#[test]
fn declared_states_require_matching_anchors() {
    use Expectation::*;
    let ledger = Ledger {
        entries: vec![
            entry("owned", EntryKind::Item, Production),
            entry("unowned", EntryKind::Item, Production),
            entry("tooling-only", EntryKind::Item, Production),
            entry("gatt-caching", EntryKind::Item, Unconstrained),
            entry("probe", EntryKind::Item, Diagnostic),
            entry("missing-probe", EntryKind::Item, Diagnostic),
            entry("nan", EntryKind::Item, Absent),
            entry("claimed-nan", EntryKind::Item, Absent),
            entry("maybe", EntryKind::Capability, Unconstrained),
        ],
        projections: BTreeMap::new(),
    };
    let anchors = [
        anchor("owned", Scope::Production),
        anchor("tooling-only", Scope::Development),
        anchor("probe", Scope::Development),
        anchor("claimed-nan", Scope::Production),
    ];
    assert_eq!(
        messages(&check(&ledger, &anchors)),
        [
            "wifi item `claimed-nan`: absent but anchored at crates/claimed-nan.rs:1",
            "wifi item `missing-probe`: no CAPABILITY anchor in code",
            "wifi item `tooling-only`: anchored only outside production packages (crates/tooling-only.rs:1)",
            "wifi item `unowned`: no CAPABILITY anchor in code",
        ]
    );
}

#[test]
fn a_complete_capability_is_backed_by_itself_or_by_all_its_facts() {
    use Expectation::*;
    let complete = |facts: &[&str]| Complete {
        facts: facts.iter().map(|f| (*f).to_owned()).collect(),
    };
    let ledger = Ledger {
        entries: vec![
            entry("anchored-fact", EntryKind::Fact, Production),
            entry("other-fact", EntryKind::Fact, Production),
            entry("absent-fact", EntryKind::Fact, Absent),
            entry("direct", EntryKind::Capability, complete(&[])),
            entry(
                "through-facts",
                EntryKind::Capability,
                complete(&["anchored-fact"]),
            ),
            entry(
                "half-backed",
                EntryKind::Capability,
                complete(&["anchored-fact", "absent-fact"]),
            ),
            entry("bare", EntryKind::Capability, complete(&[])),
        ],
        projections: BTreeMap::new(),
    };
    let anchors = [
        anchor("anchored-fact", Scope::Production),
        anchor("other-fact", Scope::Production),
        anchor("direct", Scope::Production),
    ];
    let problems = messages(&check(&ledger, &anchors));
    assert_eq!(problems.len(), 2, "{problems:#?}");
    assert!(problems[0].starts_with("wifi capability `bare`: implementation complete"));
    assert!(problems[1].starts_with("wifi capability `half-backed`: implementation complete"));
}

#[test]
fn anchors_name_known_unambiguous_entries_and_projected_items_through_their_fact() {
    use Expectation::*;
    let ledger = Ledger {
        entries: vec![
            entry("shared-owner", EntryKind::Fact, Production),
            entry("clash", EntryKind::Item, Production),
            entry("clash", EntryKind::Capability, Unconstrained),
        ],
        projections: BTreeMap::from([("projection".into(), "shared-owner".into())]),
    };
    let anchors = [
        anchor("shared-owner", Scope::Production),
        anchor("projection", Scope::Production),
        anchor("clash", Scope::Production),
        anchor("typo", Scope::Production),
    ];
    let problems = messages(&check(&ledger, &anchors));
    assert_eq!(
        problems,
        [
            "crates/clash.rs:1: CAPABILITY anchor `clash` names both an inventory item and a capability",
            "crates/projection.rs:1: `projection` projects source fact `shared-owner`; anchor the fact instead",
            "crates/typo.rs:1: CAPABILITY anchor names unknown catalog entry `typo`",
            "wifi item `clash`: no CAPABILITY anchor in code",
        ]
    );
}

#[test]
fn an_edit_lists_the_entries_anchored_in_its_files() {
    let anchors = [
        anchor("edited", Scope::Production),
        anchor("untouched", Scope::Production),
    ];
    let changed = BTreeSet::from([PathBuf::from("crates/edited.rs")]);
    let touched = touched(&anchors, &changed);
    assert_eq!(touched.keys().copied().collect::<Vec<_>>(), ["edited"]);
}

#[test]
fn an_anchor_must_sit_in_a_package_its_item_lists() {
    let mut listed = entry("rx-filter", EntryKind::Item, Expectation::Production);
    listed.packages = vec!["owner".into(), "hal".into()];
    let ledger = Ledger {
        entries: vec![listed.clone()],
        projections: BTreeMap::new(),
    };
    assert!(check(&ledger, &[anchor("rx-filter", Scope::Production)]).is_empty());

    listed.packages = vec!["hal".into()];
    let ledger = Ledger {
        entries: vec![listed],
        projections: BTreeMap::new(),
    };
    assert_eq!(
        messages(&check(&ledger, &[anchor("rx-filter", Scope::Production)])),
        [
            "wifi item `rx-filter`: anchored at crates/rx-filter.rs:1 in `owner`, which its `packages` does not list"
        ]
    );
}
