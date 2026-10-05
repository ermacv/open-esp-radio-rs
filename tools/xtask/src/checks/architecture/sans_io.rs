//! Protocol logic is sans-IO.
//!
//! A `protocol` package is a set of state machines: it takes received
//! frames, results and the time as values and returns the actions to take.
//! It may declare the asynchronous ports a driver implements, but it never
//! waits on one; the drivers that wait belong to the `service` layer. This
//! check rejects every `async` body and `.await` in a protocol package's
//! library sources outside its tests.

use std::path::{Path, PathBuf};

use crate::{Result, checks::common::Classified};

/// The layer whose packages must stay sans-IO.
const SANS_IO_LAYER: oer_repo::Layer = oer_repo::Layer::Protocol;

pub fn check(packages: &[Classified]) -> Result<()> {
    let mut violations = Vec::new();
    for item in packages {
        if item.class.layer != SANS_IO_LAYER {
            continue;
        }
        let source = item
            .manifest
            .parent()
            .ok_or("package manifest has no directory")?
            .join("src");
        for file in library_sources(&source)? {
            let text = std::fs::read_to_string(&file)?;
            for line in waiting_lines(&text) {
                violations.push(format!(
                    "{}:{line}: protocol package {} waits; move the driver to a service package",
                    file.display(),
                    item.package.name
                ));
            }
        }
    }
    if violations.is_empty() {
        Ok(())
    } else {
        Err(violations.join("\n").into())
    }
}

/// Rust files below `directory` except test modules and test support.
fn library_sources(directory: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Ok(files);
    };
    for entry in entries {
        let path = entry?.path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if name == "tests" || name == "test_support" {
            continue;
        }
        if path.is_dir() {
            files.extend(library_sources(&path)?);
        } else if name.ends_with(".rs") && name != "tests.rs" && name != "test_support.rs" {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

/// One-based numbers of the lines that wait, up to the first `#[cfg(test)]`
/// module declared inline. Comments are ignored; an async port declared as
/// `fn … -> impl Future` is not a wait.
fn waiting_lines(source: &str) -> Vec<usize> {
    let mut lines = Vec::new();
    let mut test_gate = false;
    for (index, line) in source.lines().enumerate() {
        let code = line.split("//").next().unwrap_or("").trim();
        if test_gate && code.starts_with("mod ") && code.ends_with('{') {
            break;
        }
        test_gate = code == "#[cfg(test)]";
        let waits = awaits(code)
            || code.contains("async fn ")
            || code.contains("async move")
            || code.contains("async {");
        if waits {
            lines.push(index + 1);
        }
    }
    lines
}

/// Whether `code` contains the `.await` keyword rather than a longer
/// identifier such as `.awaiting_reply()`.
fn awaits(code: &str) -> bool {
    code.match_indices(".await").any(|(index, token)| {
        !code[index + token.len()..]
            .starts_with(|next: char| next.is_ascii_alphanumeric() || next == '_')
    })
}

#[cfg(test)]
mod tests {
    use super::waiting_lines;

    #[test]
    fn declared_ports_are_not_waits_but_bodies_are() {
        let port =
            "pub trait Port {\n    fn send(&mut self) -> impl Future<Output = ()> + '_;\n}\n";
        assert!(waiting_lines(port).is_empty());
        let driver = "pub async fn run(port: &mut impl Port) {\n    port.send().await;\n}\n";
        assert_eq!(waiting_lines(driver), [1, 2]);
        assert_eq!(waiting_lines("let f = async move { 1 };\n"), [1]);
        assert_eq!(waiting_lines("let f = async { 1 };\n"), [1]);
        assert_eq!(waiting_lines("port.send().await?;\n"), [1]);
    }

    #[test]
    fn identifiers_that_start_with_await_are_not_waits() {
        let source = "if session.awaits_confirm() {}\nlet _ = peer.awaiting_token();\n";
        assert!(waiting_lines(source).is_empty());
    }

    #[test]
    fn comments_and_inline_test_modules_are_not_scanned() {
        let source = "// a driver would `.await` here\nfn step() {}\n\
                      #[cfg(test)]\nmod tests {\n    async fn helper() {}\n}\n";
        assert!(waiting_lines(source).is_empty());
        let gated_item = "#[cfg(test)]\nfn only_in_tests() {}\nasync fn driver() {}\n";
        assert_eq!(waiting_lines(gated_item), [3]);
    }
}
