//! One opt-in strict deny/test-allow block throughout the repository.
//!
//! The canonical Rust attributes live in `tools/tidy/strict-lints.txt`.
//! Every discovered crate root carrying any part of that block must carry
//! the whole block with the same tokens, including list order. Formatting
//! and comments are ignored. Roots without the block are not enrolled, so
//! migration adds coverage without a package list.

use std::collections::BTreeSet;

use crate::{Context, Result, reachability::root_attributes};

/// The only normative definition of the paired crate attributes.
const CANONICAL: &str = include_str!("../strict-lints.txt");

struct Policy {
    attributes: Vec<Vec<String>>,
    lints: BTreeSet<String>,
}

impl Policy {
    fn parse(source: &str) -> Result<Self> {
        let attributes = root_attributes(source)?;
        let [deny, allow] = attributes.as_slice() else {
            return Err(
                "canonical strict policy must contain deny and cfg_attr(test, allow)".into(),
            );
        };
        let deny_list =
            arguments(deny, "deny").ok_or("canonical strict policy must start with deny")?;
        let conditional = arguments(allow, "cfg_attr")
            .ok_or("canonical strict policy must end with cfg_attr(test, allow)")?;
        let Some([test, comma, rest @ ..]) = conditional.get(..) else {
            return Err("canonical test allowance is empty".into());
        };
        if test != "test" || comma != "," {
            return Err("canonical allowance must be gated by test alone".into());
        }
        let allow_list = arguments(rest, "allow").ok_or("canonical cfg_attr must contain allow")?;
        if trim_comma(deny_list) != trim_comma(allow_list) {
            return Err("canonical deny and test allow lists differ".into());
        }
        let lints = lint_names(deny);
        if lints.is_empty() {
            return Err("canonical strict policy names no clippy lints".into());
        }
        Ok(Self { attributes, lints })
    }

    fn problems(&self, source: &str) -> Result<Vec<String>> {
        let attributes = root_attributes(source)?;
        let enrolled: Vec<_> = attributes
            .iter()
            .filter(|attribute| {
                matches!(
                    attribute.first().map(String::as_str),
                    Some("deny" | "allow" | "forbid" | "cfg_attr")
                ) && !lint_names(attribute).is_disjoint(&self.lints)
            })
            .collect();
        if enrolled.is_empty() {
            return Ok(Vec::new());
        }
        if enrolled
            .iter()
            .map(|attribute| attribute.as_slice())
            .eq(self.attributes.iter().map(Vec::as_slice))
        {
            Ok(Vec::new())
        } else {
            Ok(vec![
                "strict deny/test-allow block differs from tools/tidy/strict-lints.txt".into(),
            ])
        }
    }
}

fn arguments<'a>(tokens: &'a [String], name: &str) -> Option<&'a [String]> {
    let [head, open, middle @ .., close] = tokens else {
        return None;
    };
    (head == name && open == "(" && close == ")").then_some(middle)
}

fn trim_comma(tokens: &[String]) -> &[String] {
    match tokens.split_last() {
        Some((last, rest)) if last == "," => rest,
        _ => tokens,
    }
}

fn lint_names(tokens: &[String]) -> BTreeSet<String> {
    tokens
        .windows(4)
        .filter_map(|parts| match parts {
            [namespace, first, second, name]
                if namespace == "clippy" && first == ":" && second == ":" =>
            {
                Some(format!("clippy::{name}"))
            }
            _ => None,
        })
        .collect()
}

/// Check all targets in all workspaces, without a radio-block or package list.
pub fn check(context: &Context<'_>) -> Result<Vec<String>> {
    let policy = Policy::parse(CANONICAL)?;
    let roots: BTreeSet<_> = context
        .model
        .packages()
        .iter()
        .flat_map(|package| &package.roots)
        .collect();
    let mut problems = Vec::new();
    for root in roots {
        let source = context.repo.read(root)?;
        match policy.problems(&source) {
            Ok(found) => problems.extend(
                found
                    .into_iter()
                    .map(|problem| format!("{root}: {problem}")),
            ),
            Err(error) => problems.push(format!("{root}: {error}")),
        }
    }
    Ok(problems)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::problems;

    fn check_files(files: &[(&str, &str)]) -> Vec<String> {
        problems(files, |context| check(context).unwrap())
    }

    const A: (&str, &str) = ("radio/Cargo.toml", "[package]\nname = \"radio\"\n");
    const B: (&str, &str) = ("tool/Cargo.toml", "[package]\nname = \"tool\"\n");

    #[test]
    fn checks_each_opted_in_target_in_any_package_without_a_list() {
        let wrong = CANONICAL.replace("clippy::panic,", "");
        let found = check_files(&[
            A,
            B,
            ("radio/src/lib.rs", CANONICAL),
            ("tool/src/lib.rs", "#![deny(missing_docs)]"),
            ("tool/src/main.rs", CANONICAL),
            ("tool/examples/strict.rs", &wrong),
        ]);
        assert_eq!(found.len(), 1);
        assert!(found[0].starts_with("tool/examples/strict.rs:"));
    }

    #[test]
    fn missing_or_duplicate_half_and_changed_test_gate_fail() {
        let policy = Policy::parse(CANONICAL).unwrap();
        let split = CANONICAL.find("#![cfg_attr").unwrap();
        for source in [
            CANONICAL[..split].to_owned(),
            CANONICAL[split..].to_owned(),
            CANONICAL.replace("    test,", "    any(test, feature = \"loose\"),"),
            format!("{CANONICAL}{CANONICAL}"),
            CANONICAL.replace("clippy::todo,", "clippy::unknown_lint,"),
        ] {
            assert_eq!(policy.problems(&source).unwrap().len(), 1);
        }
    }

    #[test]
    fn compares_tokens_and_ignores_comments_formatting_and_string_examples() {
        let spaced = CANONICAL.replace("clippy::", "clippy /* policy */ :: ");
        let commented = format!("/* {CANONICAL} */\nconst EXAMPLE: &str = r###\"{CANONICAL}\"###;");
        let found = check_files(&[
            A,
            B,
            ("radio/src/lib.rs", &spaced),
            ("tool/src/lib.rs", &commented),
        ]);
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn rust_shebang_and_utf8_bom_do_not_hide_the_policy() {
        let source = format!("\u{feff}#!/usr/bin/env rust-script\n{CANONICAL}");
        let changed = source.replacen("clippy::expect_used,", "", 1);
        assert!(check_files(&[A, ("radio/src/main.rs", &source)]).is_empty());
        assert_eq!(check_files(&[A, ("radio/src/main.rs", &changed)]).len(), 1);
    }

    #[test]
    fn canonical_deny_and_allow_must_describe_the_same_policy() {
        let first_only = CANONICAL.replacen("clippy::panic,", "", 1);
        assert!(Policy::parse(&first_only).is_err());
        assert!(Policy::parse(&CANONICAL.replace("    test,", "    feature = \"test\",")).is_err());
    }

    #[test]
    fn unterminated_attributes_fail_closed() {
        let found = check_files(&[A, ("radio/src/lib.rs", "#![deny(clippy::panic)")]);
        assert_eq!(found.len(), 1);
        assert!(found[0].contains("unterminated"));
    }
}
