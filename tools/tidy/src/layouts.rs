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

use crate::{Context, Result, workspaces};

const MARKER: &str = "REVIEWED-LAYOUT:";

/// One locked package: its name, version and source.
struct Locked {
    name: String,
    version: String,
    source: Option<String>,
}

fn locked(text: &str, lock: &str) -> Result<Vec<Locked>> {
    let document: toml::Table = toml::from_str(text).map_err(|error| format!("{lock}: {error}"))?;
    let packages = document
        .get("package")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| format!("{lock}: no [[package]] entries"))?;
    Ok(packages
        .iter()
        .filter_map(|package| {
            Some(Locked {
                name: package.get("name")?.as_str()?.to_owned(),
                version: package.get("version")?.as_str()?.to_owned(),
                source: package
                    .get("source")
                    .and_then(toml::Value::as_str)
                    .map(str::to_owned),
            })
        })
        .collect())
}

/// Whether `package` is locked at `reviewed`: its version, or a prefix of
/// its git revision.
fn matches(package: &Locked, reviewed: &str) -> bool {
    if package.version == reviewed {
        return true;
    }
    reviewed.len() >= 7
        && reviewed.bytes().all(|byte| byte.is_ascii_hexdigit())
        && package
            .source
            .as_deref()
            .and_then(|source| source.rsplit_once('#'))
            .is_some_and(|(_, revision)| revision.starts_with(reviewed))
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
    let owners = workspaces::owners(&context.manifests);
    let mut seen = BTreeSet::new();
    let mut problems = vec![];
    for (manifest, reach) in &context.reach {
        let Some(root) = owners.get(manifest) else {
            continue;
        };
        let lock = format!("{}Cargo.lock", root.trim_end_matches("Cargo.toml"));
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
