use super::*;

const ROOT: &str = r#"
[workspace]
[patch."https://github.com/upstream/pacs"]
chip = { git = "https://github.com/fork/pacs", rev = "bbbb" }
[patch."https://github.com/fork/stack.git"]
stack-time = { version = "0.5.1", registry = "crates-io" }
[patch.crates-io]
local = { path = "local" }
"#;

fn lock(packages: &[(&str, &str)]) -> String {
    packages
        .iter()
        .map(|(name, source)| {
            format!("[[package]]\nname = {name:?}\nversion = \"0.1.0\"\nsource = {source:?}\n")
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n[[package]]\nname = \"member\"\nversion = \"0.1.0\"\n"
}

fn locks(islands: &[&[(&str, &str)]]) -> Vec<(PathBuf, String)> {
    islands
        .iter()
        .enumerate()
        .map(|(index, packages)| {
            (
                PathBuf::from(format!("island-{index}/Cargo.lock")),
                lock(packages),
            )
        })
        .collect()
}

const FORK_CHIP: (&str, &str) = ("chip", "git+https://github.com/fork/pacs?rev=bbbb#bbbb");

#[test]
fn identical_pins_across_islands_pass() {
    let hal = ("hal", "git+https://github.com/fork/hal?rev=1111#1111");
    let registry = (
        "serde",
        "registry+https://github.com/rust-lang/crates.io-index",
    );
    check(
        ROOT,
        &locks(&[&[hal, FORK_CHIP, registry], &[hal, FORK_CHIP]]),
    )
    .unwrap();
}

#[test]
fn one_repository_may_supply_packages_at_different_commits() {
    let stack = ("net", "git+https://github.com/fork/net.git?rev=aaaa#aaaa");
    let driver = (
        "net-driver",
        "git+https://github.com/fork/net.git?rev=cccc#cccc",
    );
    check(ROOT, &locks(&[&[stack, driver], &[driver]])).unwrap();
}

#[test]
fn island_resolving_another_commit_is_rejected() {
    let old = ("hal", "git+https://github.com/fork/hal?rev=1111#1111");
    let new = ("hal", "git+https://github.com/fork/hal.git?rev=2222#2222");
    let error = check(ROOT, &locks(&[&[old], &[new]]))
        .unwrap_err()
        .to_string();
    assert!(error.contains("hal from https://github.com/fork/hal resolves to several commits"));
    assert!(error.contains("island-0/Cargo.lock") && error.contains("island-1/Cargo.lock"));
}

#[test]
fn island_without_root_patch_is_rejected() {
    let unpatched = ("chip", "git+https://github.com/upstream/pacs?rev=aaaa#aaaa");
    let error = check(ROOT, &locks(&[&[FORK_CHIP], &[unpatched]]))
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("island-1/Cargo.lock: chip resolves from https://github.com/upstream/pacs")
    );
}

#[test]
fn replaced_package_is_matched_by_repository_spelling_and_name() {
    let unpatched = (
        "stack-time",
        "git+https://github.com/Fork/stack?rev=dddd#dddd",
    );
    let sibling = (
        "stack-net",
        "git+https://github.com/fork/stack.git?rev=dddd#dddd",
    );
    let error = check(ROOT, &locks(&[&[unpatched, sibling]]))
        .unwrap_err()
        .to_string();
    assert!(error.contains("stack-time resolves from https://github.com/fork/stack"));
    assert!(!error.contains("stack-net"));
}

#[test]
fn renamed_patch_entry_replaces_its_package() {
    let root = "[patch.\"https://github.com/upstream/pacs\"]\nalias = { package = \"chip\", git = \"https://github.com/fork/pacs\" }\n";
    let unpatched = ("chip", "git+https://github.com/upstream/pacs#aaaa");
    assert!(check(root, &locks(&[&[unpatched]])).is_err());
}
