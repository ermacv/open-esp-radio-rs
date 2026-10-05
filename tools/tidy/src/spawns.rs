//! Only entry crates run the repository's command lines.
//!
//! A library that spawns `cargo hil` or `cargo xtask` (or builds and runs
//! their packages) hides a dependency on a whole command surface behind a
//! process boundary the layer rules cannot see, and scrapes output no type
//! describes. Libraries call the owning library instead; only packages of
//! the entry host layer (`open-radio.host-layer = "entry"`) parse arguments
//! and may run another command line.
//!
//! The check reads every Rust file a non-entry package's roots reach, outside
//! comments, for the arguments that start such a process: `.arg("hil")`,
//! `.arg("xtask")`, an argument list beginning with either, and the CLI
//! packages' names as string literals.

use crate::Context;
use oer_repo::HostLayer;

/// Argument spellings that start `cargo hil` or `cargo xtask`.
const SPAWNS: &[&str] = &[
    ".arg(\"hil\")",
    ".arg(\"xtask\")",
    ".args([\"hil\"",
    ".args(&[\"hil\"",
    ".args([\"xtask\"",
    ".args(&[\"xtask\"",
    "\"oer-hil-cli\"",
    "\"oer-xtask\"",
];

/// The spawn spelling `line` holds outside a comment, if any.
pub fn spawn(line: &str) -> Option<&'static str> {
    let code = line.split("//").next().unwrap_or(line);
    SPAWNS
        .iter()
        .copied()
        .find(|spelling| code.contains(spelling))
}

/// Every line of a non-entry package that runs a repository command line.
pub fn check(context: &Context<'_>) -> crate::Result<Vec<String>> {
    let mut problems = Vec::new();
    for package in context.model.packages() {
        let entry = context
            .model
            .classification(package)
            .is_ok_and(|class| class.host_layer == Some(HostLayer::Entry));
        if entry {
            continue;
        }
        let Some(reach) = context.reach.get(&package.manifest) else {
            continue;
        };
        for file in &reach.files {
            for (number, line) in context.repo.read(file)?.lines().enumerate() {
                if let Some(spelling) = spawn(line) {
                    problems.push(format!(
                        "{file}:{}: {} runs a repository command line (`{spelling}`); only entry crates do, libraries call the owning library",
                        number + 1,
                        package.name
                    ));
                }
            }
        }
    }
    Ok(problems)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::problems;

    fn manifest(name: &str, host_layer: &str) -> String {
        format!(
            "[package]\nname = \"{name}\"\n[package.metadata.open-radio]\nlayer = \"tool\"\nplatform = \"host\"\nhost-layer = \"{host_layer}\"\n"
        )
    }

    const SPAWNING: &str = "fn f() {\n    // a comment may name .arg(\"hil\")\n    let _ = std::process::Command::new(\"cargo\").arg(\"hil\");\n}\n";

    #[test]
    fn only_entry_crates_spawn_repository_command_lines() {
        let workspace = "[workspace]\nmembers = [\"lib\", \"cli\"]\n";
        let library = manifest("oer-lib", "orchestration");
        let cli = manifest("oer-cli", "entry");
        let found = problems(
            &[
                ("Cargo.toml", workspace),
                ("lib/Cargo.toml", &library),
                ("lib/src/lib.rs", SPAWNING),
                ("cli/Cargo.toml", &cli),
                ("cli/src/main.rs", SPAWNING),
            ],
            |context| check(context).unwrap(),
        );
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(
            found[0].starts_with("lib/src/lib.rs:3: oer-lib runs"),
            "{found:?}"
        );
    }

    #[test]
    fn messages_naming_a_command_are_no_spawn() {
        assert_eq!(
            spawn("    return Err(\"run `cargo hil wait` first\".into());"),
            None
        );
        assert_eq!(spawn("// .args([\"xtask\", \"check\"])"), None);
        assert_eq!(
            spawn("command.args([\"xtask\", \"check\"]);"),
            Some(".args([\"xtask\"")
        );
        assert_eq!(
            spawn("cargo.args([\"run\", \"-p\", \"oer-hil-cli\"]);"),
            Some("\"oer-hil-cli\"")
        );
    }
}
