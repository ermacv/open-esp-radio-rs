//! Agent guidance stays wired and current.
//!
//! Claude Code reads every `CLAUDE.md`, the skills in `.claude/skills/` and
//! the agent profiles in `.claude/agents/`; Codex reads the same files through
//! relative symlinks (the root `CLAUDE.md`, "Agent entry points").
//! The check holds that wiring:
//!
//! - beside every `CLAUDE.md` an `AGENTS.md` is a symlink to `CLAUDE.md`, and
//!   no `AGENTS.md` stands without one;
//! - `.agents/skills` is a symlink to `../.claude/skills`;
//! - every skill directory holds a `SKILL.md` whose frontmatter `name` is the
//!   directory's and whose `description` is not empty;
//! - every agent profile's frontmatter `name` is its file stem, its
//!   `description` is not empty and each of its `skills` exists;
//! - guidance cites code by path and item, never by line number
//!   (`rx.rs:72`): nothing keeps a line number current.

use std::{fs, path::Path};

use oer_repo::{
    Repo,
    files::{parent, within},
};

const SKILLS: &str = ".claude/skills";
const AGENTS: &str = ".claude/agents";
/// The Codex skills link and its target, relative to the link.
const CODEX_SKILLS: (&str, &str) = (".agents/skills", "../.claude/skills");

/// Every problem of the repository's agent guidance.
pub fn check(repo: &Repo) -> Vec<String> {
    let mut problems = Vec::new();
    instruction_links(repo, &mut problems);
    let skills = skills(repo, &mut problems);
    profiles(repo, &skills, &mut problems);
    for file in repo.files().filter(|file| guidance(file)) {
        match repo.read(file) {
            Ok(text) => {
                for (number, line) in text.lines().enumerate() {
                    if let Some(reference) = line_reference(line) {
                        problems.push(format!(
                            "{file}:{}: `{reference}` cites a line number; name the item instead",
                            number + 1
                        ));
                    }
                }
            }
            Err(error) => problems.push(error),
        }
    }
    problems
}

/// Whether `file` is agent guidance: a `CLAUDE.md`, a skill or a profile.
fn guidance(file: &str) -> bool {
    let name = file.rsplit('/').next().unwrap_or(file);
    name == "CLAUDE.md"
        || (within(file, SKILLS) && file.ends_with(".md"))
        || (within(file, AGENTS) && file.ends_with(".md"))
}

/// The `AGENTS.md` link beside each `CLAUDE.md` and the Codex skills link.
fn instruction_links(repo: &Repo, problems: &mut Vec<String>) {
    let instructed: Vec<&str> = repo
        .files()
        .filter(|file| file.rsplit('/').next() == Some("CLAUDE.md"))
        .map(parent)
        .collect();
    for dir in &instructed {
        let link = joined(dir, "AGENTS.md");
        if let Some(problem) = symlink_problem(repo.root(), &link, "CLAUDE.md") {
            problems.push(problem);
        }
    }
    for file in repo.files() {
        if file.rsplit('/').next() == Some("AGENTS.md") && !instructed.contains(&parent(file)) {
            problems.push(format!("{file} stands without a CLAUDE.md beside it"));
        }
    }
    if repo.files().any(|file| within(file, SKILLS)) {
        let (link, target) = CODEX_SKILLS;
        if let Some(problem) = symlink_problem(repo.root(), link, target) {
            problems.push(problem);
        }
    }
}

/// Every skill's name, checking its directory and frontmatter.
fn skills(repo: &Repo, problems: &mut Vec<String>) -> Vec<String> {
    let mut names: Vec<String> = repo
        .below(SKILLS)
        .filter_map(|file| file.strip_prefix(SKILLS)?.strip_prefix('/'))
        .filter_map(|relative| relative.split_once('/').map(|(name, _)| name.to_owned()))
        .collect();
    names.dedup();
    for name in &names {
        let file = format!("{SKILLS}/{name}/SKILL.md");
        if !repo.is_file(&file) {
            problems.push(format!("{SKILLS}/{name}/ holds no SKILL.md"));
            continue;
        }
        frontmatter_problems(repo, &file, name, problems);
    }
    names
}

/// Every agent profile's frontmatter and the skills it loads.
fn profiles(repo: &Repo, skills: &[String], problems: &mut Vec<String>) {
    for file in repo.children(AGENTS).filter(|file| file.ends_with(".md")) {
        let stem = file
            .rsplit('/')
            .next()
            .and_then(|name| name.strip_suffix(".md"))
            .unwrap_or_default();
        let Some(front) = frontmatter_problems(repo, file, stem, problems) else {
            continue;
        };
        for skill in list(&front, "skills") {
            if !skills.contains(&skill) {
                problems.push(format!(
                    "{file}: skill `{skill}` does not exist in {SKILLS}/"
                ));
            }
        }
    }
}

/// Checks `file`'s `name` and `description`; returns its frontmatter.
fn frontmatter_problems(
    repo: &Repo,
    file: &str,
    name: &str,
    problems: &mut Vec<String>,
) -> Option<String> {
    let text = match repo.read(file) {
        Ok(text) => text,
        Err(error) => {
            problems.push(error);
            return None;
        }
    };
    let Some(front) = frontmatter(&text) else {
        problems.push(format!("{file}: no `---` frontmatter"));
        return None;
    };
    match value(front, "name") {
        Some(found) if found == name => {}
        Some(found) => problems.push(format!("{file}: name `{found}` is not `{name}`")),
        None => problems.push(format!("{file}: frontmatter has no name")),
    }
    if value(front, "description").is_none_or(str::is_empty) {
        problems.push(format!("{file}: frontmatter has no description"));
    }
    Some(front.to_owned())
}

/// The text between a leading `---` line and the next one.
fn frontmatter(text: &str) -> Option<&str> {
    let rest = text.strip_prefix("---\n")?;
    let end = rest.find("\n---")?;
    Some(&rest[..end])
}

/// The value of the top-level `key: value` line.
fn value<'a>(front: &'a str, key: &str) -> Option<&'a str> {
    front.lines().find_map(|line| {
        let (found, value) = line.split_once(':')?;
        (found == key).then(|| value.trim())
    })
}

/// The `- item` lines under the top-level `key:` line.
fn list(front: &str, key: &str) -> Vec<String> {
    let mut lines = front.lines().skip_while(|line| *line != format!("{key}:"));
    lines.next();
    lines
        .map_while(|line| line.trim_start().strip_prefix("- "))
        .map(|item| item.trim().to_owned())
        .collect()
}

/// The first `name.ext:NUMBER` code reference in `line`, if any.
pub fn line_reference(line: &str) -> Option<&str> {
    const EXTENSIONS: &[&str] = &[".rs:", ".toml:", ".py:", ".sh:", ".json:", ".yml:"];
    for extension in EXTENSIONS {
        let mut from = 0;
        while let Some(found) = line[from..].find(extension) {
            let colon = from + found + extension.len();
            let digits = line[colon..].bytes().take_while(u8::is_ascii_digit).count();
            let stem = line[..from + found]
                .bytes()
                .rev()
                .take_while(|byte| byte.is_ascii_alphanumeric() || b"_-./".contains(byte))
                .count();
            if digits > 0 && stem > 0 {
                return Some(&line[from + found - stem..colon + digits]);
            }
            from = colon;
        }
    }
    None
}

/// Why `link` (repository-relative) is not a symlink to `target`, if it is not.
fn symlink_problem(root: &Path, link: &str, target: &str) -> Option<String> {
    let path = root.join(link);
    match fs::symlink_metadata(&path) {
        Err(_) => Some(format!("{link} is missing; link it to {target}")),
        Ok(metadata) if !metadata.file_type().is_symlink() => {
            Some(format!("{link} is not a symlink; link it to {target}"))
        }
        Ok(_) => match fs::read_link(&path) {
            Ok(found) if found == Path::new(target) => None,
            Ok(found) => Some(format!("{link} links to {}, not {target}", found.display())),
            Err(error) => Some(format!("cannot read the link {link}: {error}")),
        },
    }
}

fn joined(directory: &str, name: &str) -> String {
    if directory.is_empty() {
        name.to_owned()
    } else {
        format!("{directory}/{name}")
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;
    use crate::testing::tree;

    const SKILL: &str = "---\nname: hil-run\ndescription: Use when running HIL.\n---\n\n# Run\n";
    const PROFILE: &str =
        "---\nname: block-wifi\ndescription: A Wi-Fi session.\nskills:\n  - hil-run\n---\n";

    /// A wired tree: root and `crates/` instructions, one skill, one profile.
    fn wired(extra: &[(&str, &str)]) -> tempfile::TempDir {
        let mut files = vec![
            ("CLAUDE.md", "# Root\n"),
            ("crates/CLAUDE.md", "# crates\n"),
            (".claude/skills/hil-run/SKILL.md", SKILL),
            (".claude/agents/block-wifi.md", PROFILE),
        ];
        files.extend_from_slice(extra);
        let dir = tree(&files);
        symlink("CLAUDE.md", dir.path().join("AGENTS.md")).unwrap();
        symlink("CLAUDE.md", dir.path().join("crates/AGENTS.md")).unwrap();
        fs::create_dir_all(dir.path().join(".agents")).unwrap();
        symlink("../.claude/skills", dir.path().join(".agents/skills")).unwrap();
        dir
    }

    fn found(dir: &tempfile::TempDir) -> Vec<String> {
        check(&Repo::from_dir(dir.path()).unwrap())
    }

    #[test]
    fn wired_guidance_passes() {
        assert_eq!(found(&wired(&[])), Vec::<String>::new());
    }

    #[test]
    fn an_instruction_file_without_its_agents_link_fails() {
        let dir = wired(&[("hil/CLAUDE.md", "# hil\n")]);
        assert_eq!(
            found(&dir),
            ["hil/AGENTS.md is missing; link it to CLAUDE.md"]
        );
    }

    #[test]
    fn a_copied_agents_file_or_one_without_instructions_fails() {
        let dir = wired(&[("hil/AGENTS.md", "# hil\n")]);
        fs::remove_file(dir.path().join("crates/AGENTS.md")).unwrap();
        fs::write(dir.path().join("crates/AGENTS.md"), "# crates\n").unwrap();
        assert_eq!(
            found(&dir),
            [
                "crates/AGENTS.md is not a symlink; link it to CLAUDE.md",
                "hil/AGENTS.md stands without a CLAUDE.md beside it",
            ]
        );
    }

    #[test]
    fn a_wrong_codex_skills_link_fails() {
        let dir = wired(&[]);
        fs::remove_file(dir.path().join(".agents/skills")).unwrap();
        symlink(".claude/skills", dir.path().join(".agents/skills")).unwrap();
        assert_eq!(
            found(&dir),
            [".agents/skills links to .claude/skills, not ../.claude/skills"]
        );
    }

    #[test]
    fn a_skill_named_unlike_its_directory_or_without_description_fails() {
        let dir = wired(&[
            (
                ".claude/skills/push/SKILL.md",
                "---\nname: push-and-ci\ndescription:\n---\n",
            ),
            (".claude/skills/empty/notes.md", "# notes\n"),
        ]);
        assert_eq!(
            found(&dir),
            [
                ".claude/skills/empty/ holds no SKILL.md",
                ".claude/skills/push/SKILL.md: name `push-and-ci` is not `push`",
                ".claude/skills/push/SKILL.md: frontmatter has no description",
            ]
        );
    }

    #[test]
    fn a_profile_loading_a_missing_skill_fails() {
        let dir = wired(&[(
            ".claude/agents/block-ble.md",
            "---\nname: block-ble\ndescription: A BLE session.\nskills:\n  - block-ble\n---\n",
        )]);
        assert_eq!(
            found(&dir),
            [".claude/agents/block-ble.md: skill `block-ble` does not exist in .claude/skills/"]
        );
    }

    #[test]
    fn guidance_citing_a_line_number_fails() {
        let dir = wired(&[(
            "hil/CLAUDE.md",
            "# hil\n\nThe default (`connected_control/power.rs:181`) is off.\n",
        )]);
        symlink("CLAUDE.md", dir.path().join("hil/AGENTS.md")).unwrap();
        assert_eq!(
            found(&dir),
            [
                "hil/CLAUDE.md:3: `connected_control/power.rs:181` cites a line number; name the item instead"
            ]
        );
    }

    #[test]
    fn line_references_need_a_file_and_digits() {
        assert_eq!(line_reference("see `rx.rs:72`"), Some("rx.rs:72"));
        assert_eq!(line_reference("Cargo.toml:12 and"), Some("Cargo.toml:12"));
        assert_eq!(line_reference("`lib.rs`: the root"), None);
        assert_eq!(line_reference("a.rs:x and b.rs:"), None);
        assert_eq!(line_reference("time 10:30"), None);
    }
}
