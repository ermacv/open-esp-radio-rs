//! Local Markdown links and anchors, independent of Cargo documentation builds.

use super::LinkSummary;
use crate::{Context, Result};
use pulldown_cmark::{BrokenLink, CowStr, Event, Options, Parser, Tag, TagEnd};
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs,
    path::{Component, Path, PathBuf},
};

#[derive(Default)]
struct ParsedMarkdown {
    anchors: BTreeSet<String>,
    links: Vec<String>,
    undefined_references: Vec<String>,
}

fn parse_markdown(text: &str) -> ParsedMarkdown {
    let mut undefined_references = Vec::new();
    let mut callback = |broken: BrokenLink<'_>| {
        undefined_references.push(broken.reference.to_string());
        Some((CowStr::from("#"), CowStr::from("")))
    };
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_HEADING_ATTRIBUTES;
    let parser = Parser::new_with_broken_link_callback(text, options, Some(&mut callback));
    let mut anchors = BTreeSet::new();
    let mut duplicate_headings = BTreeMap::<String, usize>::new();
    let mut links = Vec::new();
    let mut heading: Option<(Option<String>, String)> = None;
    for event in parser {
        match event {
            Event::Start(Tag::Heading { id, .. }) => {
                heading = Some((id.map(|id| id.to_string()), String::new()));
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some((explicit, text)) = heading.take() {
                    let base = explicit.unwrap_or_else(|| heading_slug(&text));
                    let count = duplicate_headings.entry(base.clone()).or_default();
                    let anchor = if *count == 0 {
                        base
                    } else {
                        format!("{base}-{count}")
                    };
                    *count += 1;
                    anchors.insert(anchor);
                }
            }
            Event::Text(text) | Event::Code(text) => {
                if let Some((_, heading)) = &mut heading {
                    heading.push_str(&text);
                }
            }
            Event::Start(Tag::Link { dest_url, .. })
            | Event::Start(Tag::Image { dest_url, .. }) => links.push(dest_url.to_string()),
            Event::Html(html) | Event::InlineHtml(html) => {
                anchors.extend(html_anchors(&html));
            }
            _ => {}
        }
    }
    ParsedMarkdown {
        anchors,
        links,
        undefined_references,
    }
}

fn heading_slug(text: &str) -> String {
    let mut slug = String::new();
    for character in text.chars().flat_map(char::to_lowercase) {
        if character.is_alphanumeric() || matches!(character, '-' | '_') {
            slug.push(character);
        } else if character.is_whitespace() {
            slug.push('-');
        }
    }
    slug
}

fn html_anchors(html: &str) -> Vec<String> {
    let mut anchors = Vec::new();
    let bytes = html.as_bytes();
    let mut cursor = 0;
    while let Some(offset) = html[cursor..].find("<a") {
        let start = cursor + offset + 2;
        let Some(end_offset) = html[start..].find('>') else {
            break;
        };
        let end = start + end_offset;
        if let Ok(attributes) = std::str::from_utf8(&bytes[start..end]) {
            for name in ["id", "name"] {
                if let Some(value) = html_attribute(attributes, name) {
                    anchors.push(value);
                }
            }
        }
        cursor = end + 1;
    }
    anchors
}

fn html_attribute(attributes: &str, wanted: &str) -> Option<String> {
    let mut cursor = 0;
    let bytes = attributes.as_bytes();
    while cursor < bytes.len() {
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        let name_start = cursor;
        while cursor < bytes.len()
            && (bytes[cursor].is_ascii_alphanumeric() || matches!(bytes[cursor], b'-' | b'_'))
        {
            cursor += 1;
        }
        let name = &attributes[name_start..cursor];
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor >= bytes.len() || bytes[cursor] != b'=' {
            cursor += usize::from(cursor < bytes.len());
            continue;
        }
        cursor += 1;
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        let quote = bytes
            .get(cursor)
            .copied()
            .filter(|byte| matches!(byte, b'\'' | b'"'));
        if quote.is_some() {
            cursor += 1;
        }
        let value_start = cursor;
        while cursor < bytes.len()
            && quote.map_or_else(
                || !bytes[cursor].is_ascii_whitespace(),
                |quote| bytes[cursor] != quote,
            )
        {
            cursor += 1;
        }
        let value = &attributes[value_start..cursor];
        if quote.is_some() && cursor < bytes.len() {
            cursor += 1;
        }
        if name.eq_ignore_ascii_case(wanted) {
            return Some(value.to_owned());
        }
    }
    None
}

pub(super) fn check_markdown(ctx: &Context, initial: &[PathBuf]) -> Result<LinkSummary> {
    let root = ctx.root.canonicalize()?;
    let mut queue = initial.iter().cloned().collect::<VecDeque<_>>();
    let mut documents = BTreeMap::<PathBuf, ParsedMarkdown>::new();
    while let Some(path) = queue.pop_front() {
        let path = path.canonicalize()?;
        if documents.contains_key(&path) {
            continue;
        }
        if !path.starts_with(&root) {
            return Err(format!("Markdown document escaped repository: {}", path.display()).into());
        }
        let parsed = parse_markdown(&fs::read_to_string(&path)?);
        if !parsed.undefined_references.is_empty() {
            return Err(format!(
                "Markdown document {} has undefined references: {:?}",
                path.strip_prefix(&root)?.display(),
                parsed.undefined_references
            )
            .into());
        }
        for destination in &parsed.links {
            if let Some((target, _)) = local_destination(&root, &path, destination)?
                && target
                    .extension()
                    .is_some_and(|extension| extension == "md")
                && target.is_file()
                && !documents.contains_key(&target)
            {
                queue.push_back(target);
            }
        }
        documents.insert(path, parsed);
    }

    let mut local_links = 0;
    let mut external_not_checked = 0;
    for (document, parsed) in &documents {
        for destination in &parsed.links {
            let Some((target, fragment)) = local_destination(&root, document, destination)? else {
                external_not_checked += 1;
                continue;
            };
            local_links += 1;
            if !target.try_exists()? {
                return Err(format!(
                    "Markdown link from {} has missing local target {destination:?}",
                    document.strip_prefix(&root)?.display()
                )
                .into());
            }
            let canonical = target.canonicalize()?;
            if !canonical.starts_with(&root) {
                return Err(format!(
                    "Markdown link from {} escapes repository: {destination:?}",
                    document.strip_prefix(&root)?.display()
                )
                .into());
            }
            if let Some(fragment) = fragment.filter(|fragment| !fragment.is_empty()) {
                check_fragment(&root, document, &canonical, &fragment, &documents)?;
            }
        }
    }
    Ok(LinkSummary {
        documents: documents.len(),
        local_links,
        external_not_checked,
        anchors: documents
            .values()
            .map(|document| document.anchors.len())
            .sum(),
    })
}

fn local_destination(
    root: &Path,
    document: &Path,
    destination: &str,
) -> Result<Option<(PathBuf, Option<String>)>> {
    let destination = destination.trim();
    if destination.is_empty() {
        return Ok(Some((document.to_owned(), None)));
    }
    if has_url_scheme(destination) || destination.starts_with("//") {
        return Ok(None);
    }
    let (path, fragment) = destination
        .split_once('#')
        .map_or((destination, None), |(path, fragment)| {
            (path, Some(fragment))
        });
    let path = path.split_once('?').map_or(path, |(path, _)| path);
    let path = percent_decode(path)?;
    let fragment = fragment.map(percent_decode).transpose()?;
    let joined = if path.is_empty() {
        document.to_owned()
    } else if let Some(path) = path.strip_prefix('/') {
        root.join(path)
    } else {
        document
            .parent()
            .ok_or("Markdown document has no parent")?
            .join(path.as_ref())
    };
    let normalized = lexical_normalize(&joined)?;
    if !normalized.starts_with(root) {
        return Err(format!("Markdown link escapes repository: {destination:?}").into());
    }
    Ok(Some((normalized, fragment.map(Cow::into_owned))))
}

fn has_url_scheme(destination: &str) -> bool {
    let Some((scheme, _)) = destination.split_once(':') else {
        return false;
    };
    !scheme.is_empty()
        && scheme.as_bytes()[0].is_ascii_alphabetic()
        && scheme
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
}

fn percent_decode(value: &str) -> Result<Cow<'_, str>> {
    if !value.as_bytes().contains(&b'%') {
        return Ok(Cow::Borrowed(value));
    }
    let mut output = Vec::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = bytes.get(index + 1).and_then(|byte| hex(*byte));
            let low = bytes.get(index + 2).and_then(|byte| hex(*byte));
            let (Some(high), Some(low)) = (high, low) else {
                return Err(format!("invalid percent encoding in Markdown link {value:?}").into());
            };
            output.push((high << 4) | low);
            index += 3;
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    Ok(Cow::Owned(String::from_utf8(output)?))
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn lexical_normalize(path: &Path) -> Result<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(
                        format!("path escapes its filesystem root: {}", path.display()).into(),
                    );
                }
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    Ok(normalized)
}

fn check_fragment(
    root: &Path,
    source: &Path,
    target: &Path,
    fragment: &str,
    documents: &BTreeMap<PathBuf, ParsedMarkdown>,
) -> Result<()> {
    if target.is_dir() {
        return Err(format!(
            "Markdown link from {} uses an anchor on directory {}",
            source.strip_prefix(root)?.display(),
            target.strip_prefix(root)?.display()
        )
        .into());
    }
    if target
        .extension()
        .is_some_and(|extension| extension == "md")
    {
        let parsed = documents.get(target).ok_or_else(|| {
            format!(
                "linked Markdown target was not parsed: {}",
                target.display()
            )
        })?;
        if !parsed.anchors.contains(fragment) {
            return Err(format!(
                "Markdown link from {} has missing anchor #{fragment} in {}",
                source.strip_prefix(root).unwrap_or(source).display(),
                target.strip_prefix(root).unwrap_or(target).display()
            )
            .into());
        }
        return Ok(());
    }
    if matches!(
        target.extension().and_then(|extension| extension.to_str()),
        Some("rs" | "toml")
    ) {
        check_source_line_fragment(target, fragment)?;
        return Ok(());
    }
    if matches!(
        target.extension().and_then(|extension| extension.to_str()),
        Some("html" | "htm")
    ) {
        let anchors = html_anchors(&fs::read_to_string(target)?);
        if anchors.iter().any(|anchor| anchor == fragment) {
            return Ok(());
        }
    }
    Err(format!(
        "Markdown link from {} has unsupported or missing fragment #{fragment} in {}",
        source.strip_prefix(root).unwrap_or(source).display(),
        target.strip_prefix(root).unwrap_or(target).display()
    )
    .into())
}

fn check_source_line_fragment(path: &Path, fragment: &str) -> Result<()> {
    let Some(lines) = fragment.strip_prefix('L') else {
        return Err(
            format!("source link fragment must use GitHub line syntax: #{fragment}").into(),
        );
    };
    let (start, end) = lines
        .split_once("-L")
        .map_or((lines, lines), |(start, end)| (start, end));
    let start: usize = start.parse()?;
    let end: usize = end.parse()?;
    let line_count = fs::read_to_string(path)?.lines().count();
    if start == 0 || end < start || end > line_count {
        return Err(format!(
            "source link fragment #{fragment} is outside {} lines in {}",
            line_count,
            path.display()
        )
        .into());
    }
    Ok(())
}
