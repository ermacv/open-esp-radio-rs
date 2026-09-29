"""Extract type definitions and their inherent/trait impls from Rust files."""
import re
import sys

files = sys.argv[1].split(",")
names = sys.argv[2].split(",")
out = sys.argv[3]


def items(text):
    """Top-level items with their leading docs/attributes, as (header, body)."""
    lines = text.splitlines(keepends=True)
    result, i = [], 0
    while i < len(lines):
        start = i
        while i < len(lines) and (lines[i].startswith("///") or lines[i].startswith("#[") or lines[i].startswith("//")):
            i += 1
        if i >= len(lines):
            break
        line = lines[i]
        if re.match(r"^(pub(\(crate\))? )?(struct|enum|impl|const|fn|type|trait)\b", line) and not line.startswith(" "):
            # item to matching close at column 0
            j = i
            if line.rstrip().endswith(";"):
                result.append("".join(lines[start:j + 1]))
                i = j + 1
                continue
            while j < len(lines) and not re.match(r"^[}\)];?\s*$", lines[j]):
                j += 1
            result.append("".join(lines[start:j + 1]))
            i = j + 1
        else:
            i = start + 1 if i == start else i + 1
    return result


chunks = []
for f in files:
    for item in items(open(f).read()):
        head = [l for l in item.splitlines() if not l.startswith(("///", "#[", "//"))][0]
        for n in names:
            if re.search(rf"\b(struct|enum) {n}\b", head) or re.match(rf"^impl(<[^>]*>)? ({n}\b|[A-Za-z:<>'_ ]+ for {n}\b)", head):
                chunks.append(f"// from {f}\n{item}")
                break
open(out, "w").write("\n".join(chunks))
print(len(chunks), "items")
