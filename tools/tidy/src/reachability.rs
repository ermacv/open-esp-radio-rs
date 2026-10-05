//! Which Rust files a crate root reaches through `mod`, `#[path]`,
//! `#[cfg_attr(…, path = …)]` and `include!`, without compiling.
//!
//! Every declaration counts whatever its `cfg`: a file is reachable when
//! some configuration could compile it.

use std::collections::{BTreeMap, BTreeSet};

use oer_repo::{
    Repo,
    files::{join, parent},
};

use crate::Result;

/// One module-graph event of a source file.
#[derive(Clone, Debug, PartialEq)]
enum Event {
    /// `mod name;` inside the inline modules `inline`, with its path
    /// attributes.
    Module {
        name: String,
        inline: Vec<Frame>,
        paths: Vec<String>,
    },
    /// `include!("…")` or `include_str!("…")`.
    Include { path: String, rust: bool },
}

/// One enclosing inline module: its name or path attribute.
#[derive(Clone, Debug, PartialEq)]
struct Frame {
    directory: String,
    depth: usize,
}

fn is_ident_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_ident(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

struct Lexer<'a> {
    src: &'a [u8],
    at: usize,
}

impl Lexer<'_> {
    fn peek(&self, offset: usize) -> u8 {
        self.src.get(self.at + offset).copied().unwrap_or(0)
    }

    fn skip_space(&mut self) {
        loop {
            while self.peek(0).is_ascii_whitespace() {
                self.at += 1;
            }
            if self.peek(0) == b'/' && self.peek(1) == b'/' {
                self.skip_line_comment();
            } else if self.peek(0) == b'/' && self.peek(1) == b'*' {
                self.skip_block_comment();
            } else {
                return;
            }
        }
    }

    fn skip_line_comment(&mut self) {
        while self.at < self.src.len() && self.src[self.at] != b'\n' {
            self.at += 1;
        }
    }

    fn skip_block_comment(&mut self) {
        let mut level = 0usize;
        while self.at < self.src.len() {
            if self.peek(0) == b'/' && self.peek(1) == b'*' {
                level += 1;
                self.at += 2;
            } else if self.peek(0) == b'*' && self.peek(1) == b'/' {
                level -= 1;
                self.at += 2;
                if level == 0 {
                    return;
                }
            } else {
                self.at += 1;
            }
        }
    }

    fn ident(&mut self) -> Option<&str> {
        if !is_ident_start(self.peek(0)) {
            return None;
        }
        let start = self.at;
        while is_ident(self.peek(0)) {
            self.at += 1;
        }
        std::str::from_utf8(&self.src[start..self.at]).ok()
    }

    fn eat(&mut self, byte: u8) -> bool {
        self.skip_space();
        if self.peek(0) == byte {
            self.at += 1;
            true
        } else {
            false
        }
    }

    /// A plain string literal at the cursor, unescaped only for `\\` and `\"`.
    fn string(&mut self) -> Option<String> {
        self.skip_space();
        if self.peek(0) != b'"' {
            return None;
        }
        self.at += 1;
        let mut value = vec![];
        while self.at < self.src.len() {
            match self.src[self.at] {
                b'\\' => {
                    value.push(self.peek(1));
                    self.at += 2;
                }
                b'"' => {
                    self.at += 1;
                    return String::from_utf8(value).ok();
                }
                byte => {
                    value.push(byte);
                    self.at += 1;
                }
            }
        }
        None
    }

    /// Skips a raw string whose `r` the cursor has just passed.
    fn skip_raw_string(&mut self) {
        let mut hashes = 0;
        while self.peek(0) == b'#' {
            hashes += 1;
            self.at += 1;
        }
        self.at += 1; // opening quote
        while self.at < self.src.len() {
            if self.src[self.at] == b'"' && (1..=hashes).all(|offset| self.peek(offset) == b'#') {
                self.at += 1 + hashes;
                return;
            }
            self.at += 1;
        }
    }

    fn skip_char_or_lifetime(&mut self) {
        self.at += 1;
        if self.peek(0) == b'\\' {
            self.at += 2;
            while self.at < self.src.len() && self.src[self.at] != b'\'' {
                self.at += 1;
            }
            self.at += 1;
            return;
        }
        let width = match self.peek(0) {
            byte if byte < 0x80 => 1,
            byte if byte >= 0xF0 => 4,
            byte if byte >= 0xE0 => 3,
            _ => 2,
        };
        if self.peek(width) == b'\'' {
            self.at += width + 1;
        }
    }

    /// After `#`: a `path` value of `#[path = "…"]` or
    /// `#[cfg_attr(…, path = "…")]`, consuming the attribute.
    fn path_attribute(&mut self) -> Option<String> {
        let start = self.at;
        let found = (|| {
            if !self.eat(b'[') {
                return None;
            }
            self.skip_space();
            match self.ident()? {
                "path" => {
                    let value = self.eat(b'=').then(|| self.string())??;
                    self.eat(b']').then_some(value)
                }
                "cfg_attr" => {
                    if !self.eat(b'(') {
                        return None;
                    }
                    let mut depth = 1usize;
                    let mut value = None;
                    while depth > 0 && self.at < self.src.len() {
                        self.skip_space();
                        match self.peek(0) {
                            b'(' => {
                                depth += 1;
                                self.at += 1;
                            }
                            b')' => {
                                depth -= 1;
                                self.at += 1;
                            }
                            b'"' => {
                                self.string()?;
                            }
                            byte if is_ident_start(byte) => {
                                if self.ident()? == "path" && self.eat(b'=') {
                                    value = Some(self.string()?);
                                }
                            }
                            _ => self.at += 1,
                        }
                    }
                    let value = value?;
                    self.eat(b']').then_some(value)
                }
                _ => None,
            }
        })();
        if found.is_none() {
            self.at = start;
        }
        found
    }
}

/// The module events of `source`.
fn scan(source: &str) -> Vec<Event> {
    let mut lexer = Lexer {
        src: source.as_bytes(),
        at: 0,
    };
    let mut events = vec![];
    let mut inline: Vec<Frame> = vec![];
    let mut depth = 0usize;
    let mut pending: Vec<String> = vec![];
    while lexer.at < lexer.src.len() {
        let byte = lexer.peek(0);
        match byte {
            b'/' if lexer.peek(1) == b'/' => lexer.skip_line_comment(),
            b'/' if lexer.peek(1) == b'*' => lexer.skip_block_comment(),
            b'"' => {
                lexer.string();
            }
            b'\'' => lexer.skip_char_or_lifetime(),
            b'#' => {
                lexer.at += 1;
                if let Some(path) = lexer.path_attribute() {
                    pending.push(path);
                }
            }
            b'{' => {
                depth += 1;
                lexer.at += 1;
            }
            b'}' => {
                if inline.last().is_some_and(|frame| frame.depth == depth) {
                    inline.pop();
                }
                depth = depth.saturating_sub(1);
                lexer.at += 1;
            }
            byte if byte.is_ascii_digit() => {
                while is_ident(lexer.peek(0)) {
                    lexer.at += 1;
                }
            }
            byte if is_ident_start(byte) => {
                let word = lexer.ident().unwrap_or_default().to_owned();
                match word.as_str() {
                    "r" | "br" | "cr" if matches!(lexer.peek(0), b'"' | b'#') => {
                        let mut offset = 0;
                        while lexer.peek(offset) == b'#' {
                            offset += 1;
                        }
                        if lexer.peek(offset) == b'"' {
                            lexer.skip_raw_string();
                        }
                    }
                    "mod" => {
                        let save = lexer.at;
                        lexer.skip_space();
                        if lexer.peek(0) == b'r' && lexer.peek(1) == b'#' {
                            lexer.at += 2;
                        }
                        let Some(name) = lexer.ident().map(str::to_owned) else {
                            lexer.at = save;
                            continue;
                        };
                        lexer.skip_space();
                        match lexer.peek(0) {
                            b';' => {
                                lexer.at += 1;
                                events.push(Event::Module {
                                    name,
                                    inline: inline.clone(),
                                    paths: std::mem::take(&mut pending),
                                });
                            }
                            b'{' => {
                                lexer.at += 1;
                                depth += 1;
                                let directory = pending.pop().unwrap_or(name);
                                pending.clear();
                                inline.push(Frame { directory, depth });
                            }
                            _ => lexer.at = save,
                        }
                    }
                    "include" | "include_str" => {
                        let save = lexer.at;
                        let path = (|| {
                            if !lexer.eat(b'!') || !lexer.eat(b'(') {
                                return None;
                            }
                            let path = lexer.string()?;
                            lexer.eat(b')').then_some(path)
                        })();
                        match path {
                            Some(path) => events.push(Event::Include {
                                path,
                                rust: word == "include",
                            }),
                            None => lexer.at = save,
                        }
                    }
                    _ => {}
                }
            }
            _ => lexer.at += 1,
        }
    }
    events
}

/// Adds every identifier-like word of `text` to `into`.
pub fn words(text: &str, into: &mut BTreeSet<String>) {
    let bytes = text.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        if is_ident(bytes[at]) {
            let from = at;
            while at < bytes.len() && is_ident(bytes[at]) {
                at += 1;
            }
            let word = &text[from..at];
            if is_ident_start(bytes[from]) && !into.contains(word) {
                into.insert(word.to_owned());
            }
        } else {
            at += 1;
        }
    }
}

/// Adds the names a Rust source uses to `into`: its identifiers and the
/// words of its strings and doc comments, whose examples compile as
/// doctests. Plain comments do not count.
pub fn names(source: &str, into: &mut BTreeSet<String>) {
    let mut lexer = Lexer {
        src: source.as_bytes(),
        at: 0,
    };
    let slice = |from: usize, to: usize| source.get(from..to.min(source.len())).unwrap_or("");
    while lexer.at < lexer.src.len() {
        let from = lexer.at;
        match lexer.peek(0) {
            b'/' if lexer.peek(1) == b'/' => {
                let doc = matches!(lexer.peek(2), b'/' | b'!');
                lexer.skip_line_comment();
                if doc {
                    words(slice(from, lexer.at), into);
                }
            }
            b'/' if lexer.peek(1) == b'*' => {
                let doc = matches!(lexer.peek(2), b'*' | b'!');
                lexer.skip_block_comment();
                if doc {
                    words(slice(from, lexer.at), into);
                }
            }
            b'"' => {
                if lexer.string().is_none() {
                    lexer.at = lexer.src.len();
                }
                words(slice(from, lexer.at), into);
            }
            b'\'' => lexer.skip_char_or_lifetime(),
            byte if byte.is_ascii_digit() => {
                while is_ident(lexer.peek(0)) {
                    lexer.at += 1;
                }
            }
            byte if is_ident_start(byte) => {
                lexer.ident();
                let word = slice(from, lexer.at);
                let raw = matches!(word, "r" | "br" | "cr") && {
                    let mut offset = 0;
                    while lexer.peek(offset) == b'#' {
                        offset += 1;
                    }
                    lexer.peek(offset) == b'"'
                };
                if raw {
                    lexer.skip_raw_string();
                    words(slice(from, lexer.at), into);
                } else if !into.contains(word) {
                    into.insert(word.to_owned());
                }
            }
            _ => lexer.at += 1,
        }
    }
}

/// What the module graph of a set of roots reaches.
#[derive(Default)]
pub struct Reach {
    /// Rust files reached.
    pub files: BTreeSet<String>,
    /// Non-Rust files reached through `include_str!`.
    pub texts: BTreeSet<String>,
    /// `(file, declaration)` pairs naming no file.
    pub unresolved: Vec<(String, String)>,
}

/// The module graph walker, caching each file's events.
#[derive(Default)]
pub struct Walker {
    events: BTreeMap<String, Vec<Event>>,
}

impl Walker {
    fn events(&mut self, repo: &Repo, file: &str) -> Result<&[Event]> {
        if !self.events.contains_key(file) {
            let events = scan(&repo.read(file)?);
            self.events.insert(file.to_owned(), events);
        }
        Ok(&self.events[file])
    }

    /// Everything the crate `roots` reach.
    pub fn reach(&mut self, repo: &Repo, roots: &[String]) -> Result<Reach> {
        let mut reach = Reach::default();
        // (file, directory its child modules live in)
        let mut work: Vec<(String, String)> = roots
            .iter()
            .map(|root| (root.clone(), parent(root).to_owned()))
            .collect();
        while let Some((file, module_directory)) = work.pop() {
            if !reach.files.insert(file.clone()) {
                continue;
            }
            let base = parent(&file).to_owned();
            for event in self.events(repo, &file)?.to_vec() {
                match event {
                    Event::Include { path, rust } => {
                        match join(&base, &path).filter(|path| repo.is_file(path)) {
                            Some(target) if rust => work.push((target, module_directory.clone())),
                            Some(target) => {
                                reach.texts.insert(target);
                            }
                            None => reach
                                .unresolved
                                .push((file.clone(), format!("include \"{path}\""))),
                        }
                    }
                    Event::Module {
                        name,
                        inline,
                        paths,
                    } => {
                        let mut directory = module_directory.clone();
                        for frame in &inline {
                            directory = join(&directory, &frame.directory).unwrap_or_default();
                        }
                        if let Some(path) = paths.last() {
                            let from = if inline.is_empty() { &base } else { &directory };
                            match join(from, path).filter(|path| repo.is_file(path)) {
                                Some(target) => {
                                    let owned = parent(&target).to_owned();
                                    work.push((target, owned));
                                }
                                None => reach.unresolved.push((
                                    file.clone(),
                                    format!("#[path = \"{path}\"] mod {name}"),
                                )),
                            }
                            continue;
                        }
                        let flat = join(&directory, &format!("{name}.rs"));
                        let nested = join(&directory, &format!("{name}/mod.rs"));
                        if let Some(flat) = flat.filter(|path| repo.is_file(path)) {
                            let owned = join(&directory, &name).unwrap_or_default();
                            work.push((flat, owned));
                        } else if let Some(nested) = nested.filter(|path| repo.is_file(path)) {
                            let owned = parent(&nested).to_owned();
                            work.push((nested, owned));
                        } else {
                            reach
                                .unresolved
                                .push((file.clone(), format!("mod {name};")));
                        }
                    }
                }
            }
        }
        Ok(reach)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::tree;

    fn reach(files: &[(&str, &str)], roots: &[&str]) -> Reach {
        let dir = tree(files);
        let repo = Repo::from_dir(dir.path()).unwrap();
        let roots: Vec<String> = roots.iter().map(|r| (*r).to_owned()).collect();
        Walker::default().reach(&repo, &roots).unwrap()
    }

    #[test]
    fn modules_follow_rust_directory_ownership() {
        let found = reach(
            &[
                ("src/lib.rs", "mod a;\nmod b;\nmod outer { mod inner; }\n"),
                ("src/a.rs", "mod child;"),
                ("src/a/child.rs", ""),
                ("src/b/mod.rs", "pub(crate) mod c;"),
                ("src/b/c.rs", ""),
                ("src/outer/inner.rs", ""),
                ("src/stray.rs", ""),
            ],
            &["src/lib.rs"],
        );
        assert_eq!(
            found.files.into_iter().collect::<Vec<_>>(),
            [
                "src/a.rs",
                "src/a/child.rs",
                "src/b/c.rs",
                "src/b/mod.rs",
                "src/lib.rs",
                "src/outer/inner.rs"
            ]
        );
        assert!(found.unresolved.is_empty());
    }

    #[test]
    fn path_attributes_includes_and_cfg_attr_paths_reach_their_files() {
        let found = reach(
            &[
                (
                    "src/lib.rs",
                    "#![doc = include_str!(\"../README.md\")]\n#[path = \"x/y.rs\"]\nmod y;\n#[cfg_attr(test, path = \"t.rs\")]\nmod t;\ninclude!(\"gen.rs\");\n",
                ),
                ("README.md", ""),
                ("src/x/y.rs", "mod z;"),
                ("src/x/z.rs", ""),
                ("src/t.rs", ""),
                ("src/gen.rs", ""),
            ],
            &["src/lib.rs"],
        );
        for file in ["src/x/y.rs", "src/x/z.rs", "src/t.rs", "src/gen.rs"] {
            assert!(found.files.contains(file), "{file} unreachable");
        }
        assert!(found.texts.contains("README.md"));
    }

    #[test]
    fn declarations_in_comments_and_strings_do_not_count() {
        let found = reach(
            &[
                (
                    "src/lib.rs",
                    "// mod a;\n/* mod b; /* nested */ mod c; */\nconst S: &str = \"mod d;\";\nconst R: &str = r#\"mod e;\"#;\nfn f<'a>(x: &'a u8) -> char { '{' }\nmod g;\n",
                ),
                ("src/g.rs", ""),
                ("src/a.rs", ""),
            ],
            &["src/lib.rs"],
        );
        assert!(found.files.contains("src/g.rs"));
        assert!(!found.files.contains("src/a.rs"));
        assert!(found.unresolved.is_empty(), "{:?}", found.unresolved);
    }

    #[test]
    fn names_skip_plain_comments_but_keep_code_strings_and_docs() {
        let mut found = BTreeSet::new();
        names(
            "// plain_comment\n/* block_comment */\n/// doc_example::run();\nuse code_name::X;\nconst S: &str = r#\"raw \"quoted\" text\"#;\nconst T: &str = \"http://after_url\"; fn later_name() {}\n",
            &mut found,
        );
        for name in [
            "doc_example",
            "code_name",
            "quoted",
            "after_url",
            "later_name",
        ] {
            assert!(found.contains(name), "{name} missing from {found:?}");
        }
        assert!(!found.contains("plain_comment") && !found.contains("block_comment"));
    }

    #[test]
    fn a_declaration_without_a_file_is_unresolved() {
        let found = reach(&[("src/main.rs", "mod gone;")], &["src/main.rs"]);
        assert_eq!(
            found.unresolved,
            [("src/main.rs".to_owned(), "mod gone;".to_owned())]
        );
    }
}
