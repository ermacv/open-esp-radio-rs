//! Commands the Markdown shows for the repository's Cargo aliases, checked
//! against the command trees the tools print for `__command-tree`: every
//! subcommand must exist and every long flag must be accepted by its command
//! or an ancestor.

use std::collections::BTreeMap;

use oer_command_tree::CommandNode;

/// The Cargo aliases whose commands the documentation shows, and where
/// each tool's command tree comes from.
pub(super) const TOOLS: [(&str, TreeSource); 6] = [
    ("xtask", TreeSource::Alias),
    ("hil", TreeSource::Alias),
    ("qualification", TreeSource::Alias),
    ("memory", TreeSource::Alias),
    ("registers", TreeSource::Alias),
    // Blobray is a separate, standalone workspace; its tests keep this file
    // equal to its live tree, so the check need not build it.
    (
        "blobray",
        TreeSource::File("tools/blobray/cli/command-tree.json"),
    ),
];

/// Where the check reads a tool's command tree.
#[derive(Clone, Copy)]
pub(super) enum TreeSource {
    /// `cargo <alias> __command-tree`, which builds the tool first.
    Alias,
    /// A committed file, relative to the repository root.
    File(&'static str),
}

/// The command trees of every tool, keyed by command path.
pub(super) struct Trees {
    nodes: BTreeMap<Vec<String>, CommandNode>,
}

impl Trees {
    pub(super) fn new(nodes: impl IntoIterator<Item = CommandNode>) -> Self {
        Self {
            nodes: nodes
                .into_iter()
                .map(|node| (node.path.clone(), node))
                .collect(),
        }
    }

    /// Why `words`, a command line after `cargo`, does not exist; `None`
    /// when it does, or when a placeholder or forwarded arguments leave the
    /// rest unresolved.
    pub(super) fn check(&self, words: &[String]) -> Option<String> {
        let (tool, arguments) = words.split_first()?;
        let mut path = vec![tool.clone()];
        let mut node = self.nodes.get(&path)?;
        let mut value_pending = false;
        let mut positional = false;
        for word in arguments {
            if placeholder(word) {
                if node.subcommands.is_empty() || positional || value_pending {
                    value_pending = false;
                    continue;
                }
                return None;
            }
            if let Some(flag) = word.strip_prefix("--") {
                let flag = format!("--{}", flag.split('=').next().unwrap_or_default());
                if flag != "--help" && !self.accepts(&path, &flag) {
                    return Some(format!("`cargo {}` has no flag {flag}", path.join(" ")));
                }
                value_pending = !word.contains('=');
                continue;
            }
            if word.starts_with('-') {
                value_pending = false;
                continue;
            }
            if !node.subcommands.is_empty() && !positional {
                if node.subcommands.contains(word) {
                    path.push(word.clone());
                    node = self.nodes.get(&path)?;
                    value_pending = false;
                    continue;
                }
                if value_pending {
                    value_pending = false;
                    continue;
                }
                return Some(format!(
                    "`cargo {}` has no subcommand {word}",
                    path.join(" ")
                ));
            }
            if value_pending {
                value_pending = false;
                continue;
            }
            if node.forwards {
                return None;
            }
            positional = true;
        }
        None
    }

    /// Whether the command at `path` or one of its ancestors takes `flag`.
    fn accepts(&self, path: &[String], flag: &str) -> bool {
        (1..=path.len()).any(|length| {
            self.nodes
                .get(&path[..length])
                .is_some_and(|node| node.flags.iter().any(|known| known == flag))
        })
    }
}

/// Every `cargo <tool> ...` command line in `text`, as the words after
/// `cargo`: continued lines are joined, and a pipe, a list operator, a
/// redirection, a comment, `--` or a closing backtick ends the command.
pub(super) fn invocations(text: &str) -> Vec<Vec<String>> {
    let joined = text.replace("\\\n", " ");
    let mut commands = Vec::new();
    for line in joined.lines() {
        let tokens = line
            .split_whitespace()
            .flat_map(|token| {
                if token.len() > 1 && token.contains('|') && !token.contains("||") {
                    token
                        .split('|')
                        .filter(|part| !part.is_empty())
                        .collect::<Vec<_>>()
                } else {
                    vec![token]
                }
            })
            .collect::<Vec<_>>();
        let mut index = 0;
        while index + 1 < tokens.len() {
            let tool = clean(tokens[index + 1]);
            if clean(tokens[index]) != "cargo" || !TOOLS.iter().any(|(name, _)| *name == tool) {
                index += 1;
                continue;
            }
            let mut words = vec![tool];
            let mut depth = 0_i32;
            index += 2;
            while let Some(&token) = tokens.get(index) {
                if (token == "|" && depth == 0)
                    || ["||", "&&", ";", "--", "#"].contains(&token)
                    || token.starts_with('>')
                    || token.starts_with("2>")
                {
                    break;
                }
                index += 1;
                depth += bracket_depth(token);
                if token == "|" {
                    continue;
                }
                let word = clean(token);
                if !word.is_empty() {
                    words.push(word);
                }
                // A statement or an inline code span ends here.
                if token.ends_with(';') || token.trim_end_matches([',', '.', ':']).ends_with('`') {
                    break;
                }
            }
            commands.push(words);
        }
    }
    commands
}

/// A token without the quotes, brackets and punctuation that frame it in
/// usage lines and prose.
fn clean(token: &str) -> String {
    token
        .trim_matches(|character: char| {
            matches!(
                character,
                '"' | '\'' | '`' | '[' | ']' | '(' | ')' | ',' | ';' | ':'
            )
        })
        .to_owned()
}

fn bracket_depth(token: &str) -> i32 {
    token
        .chars()
        .map(|character| match character {
            '[' | '(' => 1,
            ']' | ')' => -1,
            _ => 0,
        })
        .sum()
}

/// A word that stands for a value to supply: `<scenario>`, `RUN_ID`,
/// `$CHIP`, `{a,b}`, `...`.
fn placeholder(word: &str) -> bool {
    word.contains(['<', '>', '{', '}', '$', '*', '…'])
        || word.contains("...")
        || (word.chars().any(|character| character.is_ascii_uppercase())
            && word.chars().all(|character| {
                character.is_ascii_uppercase() || character.is_ascii_digit() || character == '_'
            }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(path: &[&str], subcommands: &[&str], flags: &[&str], forwards: bool) -> CommandNode {
        let strings = |words: &[&str]| words.iter().map(|word| word.to_string()).collect();
        CommandNode {
            path: strings(path),
            subcommands: strings(subcommands),
            flags: strings(flags),
            forwards,
        }
    }

    fn trees() -> Trees {
        Trees::new([
            node(
                &["xtask"],
                &["check", "vendor-scenario"],
                &["--root"],
                false,
            ),
            node(&["xtask", "check"], &["docs", "phy"], &[], false),
            node(&["xtask", "check", "docs"], &[], &[], false),
            node(&["xtask", "check", "phy"], &[], &["--chip"], false),
            node(&["xtask", "vendor-scenario"], &[], &["--chip"], true),
            node(&["hil"], &["run", "queue"], &["--owner"], false),
            node(&["hil", "run"], &[], &["--repeat"], false),
            node(&["hil", "queue"], &[], &["--json"], false),
        ])
    }

    fn check(line: &str) -> Vec<String> {
        let trees = trees();
        invocations(line)
            .iter()
            .filter_map(|words| trees.check(words))
            .collect()
    }

    #[test]
    fn documented_commands_resolve_through_subcommands_values_and_ancestor_flags() {
        assert!(check("cargo xtask check phy --chip esp32s31").is_empty());
        assert!(check("cargo xtask --root . check docs").is_empty());
        assert!(check("cargo hil --owner wifi run boot-smoke --repeat 3").is_empty());
        assert!(check("$ cargo hil queue --json | jq . && cargo xtask check docs").is_empty());
        assert_eq!(
            check("cargo xtask hil queue --json"),
            ["`cargo xtask` has no subcommand hil"]
        );
        assert!(check("cargo hil run <scenario> [--repeat N]").is_empty());
        assert!(check("cargo xtask vendor-scenario --chip c gain --library lib.a").is_empty());
        assert!(check("cargo hil run x -- --anything").is_empty());
    }

    #[test]
    fn missing_subcommands_and_flags_are_named() {
        assert_eq!(
            check("cargo xtask check pyh"),
            ["`cargo xtask check` has no subcommand pyh"]
        );
        assert_eq!(
            check("cargo hil queue --details"),
            ["`cargo hil queue` has no flag --details"]
        );
        assert_eq!(
            check("cargo hil run --repeats=2"),
            ["`cargo hil run` has no flag --repeats"]
        );
        assert_eq!(
            check("cargo xtask check \\\n  phy --chp x"),
            ["`cargo xtask check phy` has no flag --chp"]
        );
    }

    #[test]
    fn usage_alternatives_and_prose_punctuation_are_words() {
        assert_eq!(
            invocations("run `cargo hil queue [--json|--details]`, then"),
            [vec!["hil", "queue", "--json", "--details"]]
        );
        assert_eq!(
            invocations("cargo hil (--owner A | --owner B) run; ls"),
            [vec!["hil", "--owner", "A", "--owner", "B", "run"]]
        );
    }
}
