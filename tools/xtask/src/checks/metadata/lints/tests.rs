use super::*;

const POLICY: &str = "[workspace.lints.rust]\nunsafe_op_in_unsafe_fn = \"deny\"\n";
const INHERITS: &str = "[package]\nname = \"member\"\n[lints]\nworkspace = true\n";

fn island(manifest: &str, contents: &str, members: &[(&str, &str)]) -> Island {
    Island {
        manifest: PathBuf::from(manifest),
        contents: format!("[workspace]\n{contents}"),
        members: members
            .iter()
            .map(|(name, contents)| {
                (
                    (*name).to_owned(),
                    PathBuf::from(format!("{name}/Cargo.toml")),
                    (*contents).to_owned(),
                )
            })
            .collect(),
    }
}

fn root() -> Island {
    island("Cargo.toml", POLICY, &[("oer-root", INHERITS)])
}

#[test]
fn islands_repeating_the_root_policy_pass() {
    check(
        Path::new("Cargo.toml"),
        &[
            root(),
            island("platform/Cargo.toml", POLICY, &[("oer-board", INHERITS)]),
        ],
    )
    .unwrap();
}

#[test]
fn island_with_a_different_or_missing_policy_is_rejected() {
    let weaker = "[workspace.lints.rust]\nunsafe_op_in_unsafe_fn = \"warn\"\n";
    for contents in [weaker, ""] {
        let error = check(
            Path::new("Cargo.toml"),
            &[root(), island("examples/a/Cargo.toml", contents, &[])],
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("examples/a/Cargo.toml: [workspace.lints] differs"));
    }
}

#[test]
fn package_with_local_lints_is_rejected() {
    let local = "[package]\nname = \"oer-local\"\n[lints.rust]\nunexpected_cfgs = \"allow\"\n";
    let error = check(
        Path::new("Cargo.toml"),
        &[
            root(),
            island("platform/Cargo.toml", POLICY, &[("oer-local", local)]),
        ],
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("package oer-local must declare `[lints] workspace = true`"));
}

#[test]
fn blobray_and_generated_bindings_keep_their_own_policy() {
    let own = "[package]\nname = \"x\"\n[lints.rust]\nunsafe_op_in_unsafe_fn = \"allow\"\n";
    check(
        Path::new("Cargo.toml"),
        &[
            island("Cargo.toml", POLICY, &[("oer-esp32s31-pac-raw", own)]),
            island(INDEPENDENT_POLICY, "", &[("blobray-core", own)]),
        ],
    )
    .unwrap();
}
