#!/usr/bin/env python3
"""mdBook preprocessor: point links that leave the book at GitHub.

The guides in `docs/` link to documents and code beside their owners with
relative paths, which work on GitHub and in every branch. mdBook publishes only
the book directory, so this preprocessor rewrites a relative link that resolves
outside the book to the same file on GitHub at the commit being built
(`GITHUB_SHA`, else `git rev-parse HEAD`). Links inside the book and all code
blocks and code spans are left unchanged.

Protocol: https://rust-lang.github.io/mdBook/for_developers/preprocessors.html
"""
import json
import os
import re
import subprocess
import sys
import urllib.parse

LINK = re.compile(r"(\]\()([^)\s]+)((?:\s+\"[^\"]*\")?\))")
REFERENCE = re.compile(r"^(\s{0,3}\[[^\]]+\]:\s*)(\S+)", re.MULTILINE)
SCHEME = re.compile(r"^[A-Za-z][A-Za-z0-9+.-]*:")
FENCE = re.compile(r"^\s{0,3}(```|~~~)")
CODE_SPAN = re.compile(r"(`+)(?:.+?)\1")


class Rewriter:
    def __init__(self, context):
        root = context["root"]
        self.book = os.path.realpath(os.path.join(root, context["config"]["book"].get("src", ".")))
        self.repository_root = git(root, "rev-parse", "--show-toplevel")
        self.revision = os.environ.get("GITHUB_SHA") or git(root, "rev-parse", "HEAD")
        html = context["config"].get("output", {}).get("html", {})
        self.repository = html["git-repository-url"].rstrip("/")

    def target(self, chapter, target):
        if target.startswith(("#", "/")) or SCHEME.match(target):
            return target
        path, anchor = (target.split("#", 1) + [""])[:2]
        directory = os.path.dirname(os.path.join(self.book, chapter))
        resolved = os.path.realpath(os.path.join(directory, urllib.parse.unquote(path)))
        if resolved == self.book or resolved.startswith(self.book + os.sep):
            return target
        relative = os.path.relpath(resolved, self.repository_root)
        if relative.startswith(".."):
            raise SystemExit(f"{chapter}: link {target} leaves the repository")
        if not os.path.exists(resolved):
            raise SystemExit(f"{chapter}: link {target} names a missing file")
        kind = "tree" if os.path.isdir(resolved) else "blob"
        url = f"{self.repository}/{kind}/{self.revision}/{urllib.parse.quote(relative)}"
        return url + (f"#{anchor}" if anchor else "")

    def text(self, chapter, text):
        def outside_code(line):
            pieces, last = [], 0
            for span in CODE_SPAN.finditer(line):
                pieces.append(self.links(chapter, line[last : span.start()]))
                pieces.append(span.group(0))
                last = span.end()
            pieces.append(self.links(chapter, line[last:]))
            return "".join(pieces)

        output, fenced = [], None
        for line in text.split("\n"):
            fence = FENCE.match(line)
            if fence and (fenced is None or fence.group(1) == fenced):
                fenced = None if fenced else fence.group(1)
                output.append(line)
            elif fenced:
                output.append(line)
            else:
                output.append(outside_code(line))
        return "\n".join(output)

    def links(self, chapter, text):
        text = LINK.sub(lambda m: m.group(1) + self.target(chapter, m.group(2)) + m.group(3), text)
        return REFERENCE.sub(lambda m: m.group(1) + self.target(chapter, m.group(2)), text)

    def items(self, items):
        for item in items:
            chapter = item.get("Chapter") if isinstance(item, dict) else None
            if chapter is None:
                continue
            if chapter.get("source_path"):
                chapter["content"] = self.text(chapter["source_path"], chapter["content"])
            self.items(chapter.get("sub_items", []))


def git(directory, *arguments):
    return subprocess.run(
        ["git", "-C", directory, *arguments], check=True, capture_output=True, text=True
    ).stdout.strip()


def main():
    if len(sys.argv) > 1 and sys.argv[1] == "supports":
        sys.exit(0)
    context, book = json.load(sys.stdin)
    rewriter = Rewriter(context)
    rewriter.items(book.get("items", book.get("sections", [])))
    json.dump(book, sys.stdout)


if __name__ == "__main__":
    main()
