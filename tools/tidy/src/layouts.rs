//! Code that relies on a foreign type's layout names the release it was
//! reviewed against.
//!
//! An `unsafe impl` (such as `bytemuck::Zeroable`) over another crate's type
//! that rests on how that crate lays the type out at one release carries
//! `// REVIEWED-LAYOUT: <package> <version>` beside its `SAFETY` comment.
//! The lock file of the workspace that compiles the file must still resolve
//! `<package>` to `<version>`: an exact version, or a prefix (at least seven
//! hex digits) of the git revision it is fetched at. A dependency update
//! that moves the package fails here until the layout is reviewed again and
//! the anchor follows it. A guarantee the other crate documents needs no
//! anchor.

use std::collections::BTreeSet;

use oer_repo::lock::{Locked, parse as locked};

use crate::{Context, Result};

const MARKER: &str = "REVIEWED-LAYOUT:";

/// Whether `package` is locked at `reviewed`: its version, or a prefix of
/// its git revision.
fn matches(package: &Locked, reviewed: &str) -> bool {
    if package.version == reviewed {
        return true;
    }
    reviewed.len() >= 7
        && reviewed.bytes().all(|byte| byte.is_ascii_hexdigit())
        && package
            .revision()
            .is_some_and(|revision| revision.starts_with(reviewed))
}

/// The `(package, version)` an anchor on `line` names.
fn anchor(line: &str) -> Option<std::result::Result<(&str, &str), ()>> {
    let comment = line.trim_start().strip_prefix("//")?;
    let rest = comment.trim_start().strip_prefix(MARKER)?;
    let mut words = rest.split_whitespace();
    Some(match (words.next(), words.next(), words.next()) {
        (Some(package), Some(version), None) => Ok((package, version)),
        _ => Err(()),
    })
}

/// Every anchor names a package its workspace still locks at the reviewed
/// release.
pub fn check(context: &Context<'_>) -> Result<Vec<String>> {
    let mut seen = BTreeSet::new();
    let mut problems = vec![];
    for (manifest, reach) in &context.reach {
        let Some(root) = context
            .model
            .manifests
            .packages
            .iter()
            .find(|package| package.manifest == *manifest)
            .and_then(|package| context.model.workspace_of(package))
        else {
            continue;
        };
        let lock = oer_repo::workspaces::lock_of(root);
        for file in &reach.files {
            if !seen.insert((file.clone(), lock.clone())) {
                continue;
            }
            let text = context.repo.read(file)?;
            let mut packages = None;
            for (index, line) in text.lines().enumerate() {
                let at = format!("{file}:{}", index + 1);
                match anchor(line) {
                    None => {}
                    Some(Err(())) => problems.push(format!(
                        "{at}: write `// {MARKER} <package> <version or revision>`"
                    )),
                    Some(Ok((package, reviewed))) => {
                        if packages.is_none() {
                            packages = Some(locked(&context.repo.read(&lock)?, &lock)?);
                        }
                        let locked = packages.as_ref().expect("read above");
                        let candidates: Vec<_> =
                            locked.iter().filter(|p| p.name == package).collect();
                        if !candidates.iter().any(|p| matches(p, reviewed)) {
                            let found: Vec<_> =
                                candidates.iter().map(|p| p.version.as_str()).collect();
                            problems.push(format!(
                                "{at}: the layout was reviewed at {package} {reviewed}, but {lock} locks {}; review it again and update the anchor",
                                if found.is_empty() {
                                    "no such package".to_owned()
                                } else {
                                    found.join(", ")
                                }
                            ));
                        }
                    }
                }
            }
        }
    }
    problems.sort();
    problems.dedup();
    Ok(problems)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::problems;

    const LOCK: &str = r#"
version = 4

[[package]]
name = "p"
version = "0.1.0"

[[package]]
name = "vcell"
version = "0.1.3"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "xarxa-driver"
version = "0.1.0"
source = "git+https://github.com/ermacv/xarxa.git?rev=9e0e3293724c#9e0e3293724c30c5892e6af56150c849f5231eb2"
"#;

    fn found(source: &str) -> Vec<String> {
        problems(
            &[
                ("Cargo.toml", "[package]\nname = \"p\"\n"),
                ("Cargo.lock", LOCK),
                ("src/lib.rs", source),
            ],
            |context| check(context).unwrap(),
        )
    }

    #[test]
    fn anchors_matching_the_lock_by_version_or_revision_pass() {
        assert!(
            found("// REVIEWED-LAYOUT: vcell 0.1.3\n// REVIEWED-LAYOUT: xarxa-driver 9e0e3293\n")
                .is_empty()
        );
    }

    #[test]
    fn a_moved_package_fails_until_the_anchor_follows_it() {
        let found = found("// REVIEWED-LAYOUT: vcell 0.1.2\n");
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].starts_with("src/lib.rs:1: the layout was reviewed at vcell 0.1.2"));
        assert!(found[0].contains("locks 0.1.3"));
        let found = self::found("// REVIEWED-LAYOUT: xarxa-driver 1234567\n");
        assert_eq!(found.len(), 1, "{found:?}");
        let found = self::found("// REVIEWED-LAYOUT: gone 1.0.0\n");
        assert!(found[0].contains("locks no such package"), "{found:?}");
    }

    #[test]
    fn a_malformed_anchor_is_reported() {
        let found = found("// REVIEWED-LAYOUT: vcell\n");
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("write `// REVIEWED-LAYOUT:"));
    }
}
